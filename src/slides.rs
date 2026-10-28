//! PowerPoint slides in a scroll like PDF pages: each slide drawn from its
//! shapes, pictures and tables, read in the background around what is in
//! view, with a rail of the slides drawn small docked at the right. Ctrl
//! with + and - zooms them, each deck at a zoom of its own: the shapes and
//! the text are drawn at the new size at once, and once the zoom rests the
//! slides whose pictures are far from it are read again for it.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use silent::memory::View;
use silent::office::pptx::{self, Anchor, Fill, Item, Kind, Text};
use silent::office::{Align, family};
use silent::vim::Motion;

use crate::content::{Deck, Slide, render_slide};
use crate::docview::styled;
use crate::overview::{self, Docking, Geometry, Mode, Overview, Overviewed, Shown};
use crate::paged::{GAP, Pages, RAIL, THUMB};
use crate::pdf::thumb_frame;
use crate::theme::Theme;

/// Slides kept read on each side of those in view.
const MARGIN: usize = 1;
/// Slides kept read for the rail on each side of those on it, and read at once.
const THUMBS_KEPT: usize = 12;
const THUMBS_AT_ONCE: usize = 3;
/// How long the zoom rests before slides are read again for it.
const REST: Duration = Duration::from_millis(100);
/// The widths, against the width a slide was read for, its pictures look
/// right at: they are decoded at twice their size.
const SHARP: std::ops::RangeInclusive<f32> = 0.5..=1.6;

pub struct SlidesPane {
    path: PathBuf,
    deck: Arc<pptx::Deck>,
    pages: Pages,
    /// Slides read, each with the device width it was read for.
    shown: HashMap<usize, (u32, Rc<Slide>)>,
    pending: HashSet<usize>,
    /// When the zoom last changed, and the wake-up once it has rested.
    zoomed: Option<Instant>,
    rest: Option<Task<()>>,
    /// The slides' area when last drawn: the window, less the rail's dock.
    viewport: Size<Pixels>,
    overview: Overview,
    /// Slides read small, for the rail.
    thumbs: HashMap<usize, Rc<Slide>>,
    thumbs_pending: HashSet<usize>,
    restore: Option<View>,
}

/// Lets the window's atlas go of a slide's pictures and background.
fn let_go(slide: &Slide, window: &mut Window, cx: &mut App) {
    for image in slide.pictures.values().chain(&slide.background) {
        cx.drop_image(image.clone(), Some(window));
    }
}

impl SlidesPane {
    /// Lets the window's atlas go of the slides' pictures and backgrounds.
    pub fn drop_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let slides = self.shown.drain().map(|(_, (_, slide))| slide);
        for slide in slides.chain(self.thumbs.drain().map(|(_, slide)| slide)) {
            let_go(&slide, window, cx);
        }
    }

    pub fn new(
        deck: Deck,
        mode: Mode,
        docked: bool,
        view: Option<View>,
        zoom: f32,
        window: &Window,
    ) -> Self {
        let viewport = overview::beside(window, if docked { RAIL } else { 0. });
        let mut shown = HashMap::new();
        if let Some((index, first)) = deck.first {
            let read = (f32::from(viewport.width) * window.scale_factor()).round() as u32;
            shown.insert(index, (read, Rc::new(first)));
        }
        Self {
            path: deck.path,
            pages: Pages::new(vec![deck.deck.size; deck.deck.slides.len()], zoom),
            deck: Arc::new(deck.deck),
            shown,
            pending: HashSet::new(),
            zoomed: None,
            rest: None,
            viewport,
            overview: Overview::new(mode, docked),
            thumbs: HashMap::new(),
            thumbs_pending: HashSet::new(),
            restore: view,
        }
    }

    /// The zoom, 1 when a slide's width fills the view.
    pub fn zoom(&self) -> f32 {
        self.pages.zoom
    }

    /// The slides at `zoom`, the place under the pointer at `at`, or else
    /// the middle of the view's top, kept where it is.
    pub fn set_zoom(
        &mut self,
        zoom: f32,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.pages.scroll.bounds();
        let view = size(
            f32::from(self.viewport.width),
            f32::from(self.viewport.height),
        );
        let at = match at {
            Some(at) => point(
                f32::from(at.x - bounds.left()).clamp(0., view.width),
                f32::from(at.y - bounds.top()).clamp(0., view.height),
            ),
            None => point(view.width / 2., 0.),
        };
        self.pages.zoom_to(zoom, at, view);
        self.zoomed = Some(Instant::now());
        self.rest = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(REST).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        }));
        cx.notify();
    }

    /// The engaged ← and →: sideways on slides zoomed wider than the
    /// window, then to the previous or next slide.
    pub fn across(&mut self, right: bool, n: usize, cx: &mut Context<Self>) {
        self.pages.across(right, n);
        overview::reveal(self, cx);
    }

    pub fn slide_count(&self) -> usize {
        self.pages.count()
    }

    pub fn current_slide(&self) -> usize {
        self.pages.at_top() + 1
    }

    pub fn view(&self) -> Option<View> {
        self.pages.view()
    }

    pub fn motion(&mut self, motion: Motion, cx: &mut Context<Self>) {
        self.pages.motion(motion, f32::from(self.viewport.height));
        overview::reveal(self, cx);
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.overview.mode = mode;
        overview::reveal(self, cx);
    }

    pub fn docking(&self) -> Docking {
        Docking {
            width: RAIL,
            docked: self.overview.docked,
            wanted: self.overview.mode == Mode::On && self.pages.count() > 1,
        }
    }

    pub fn set_docked(&mut self, docked: bool, cx: &mut Context<Self>) {
        self.overview.docked = docked;
        cx.notify();
    }

    fn dock(&self) -> f32 {
        if self.overview.docked { RAIL } else { 0. }
    }

    /// Reads the slides around the view, and again those whose pictures
    /// were decoded for a width far from the slides' now, as for another
    /// zoom or fullscreen, once the zoom rests; a slide stays shown until
    /// the one read again replaces it.
    fn read_around_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = self.viewport;
        let column = self.pages.column_in(f32::from(viewport.width));
        let device = (column * window.scale_factor()).round().max(1.) as u32;
        let Some(view) = self.pages.in_view(column, f32::from(viewport.height)) else {
            return;
        };
        let keep = view.start.saturating_sub(MARGIN)..(view.end + MARGIN).min(self.pages.count());
        let far: Vec<usize> = (self.shown.keys().copied())
            .filter(|i| !keep.contains(i))
            .collect();
        for i in far {
            if let Some((_, slide)) = self.shown.remove(&i) {
                let_go(&slide, window, cx);
            }
        }
        let resting = self.zoomed.is_none_or(|at| at.elapsed() >= REST);
        for i in keep {
            let stale = match self.shown.get(&i) {
                Some((read, _)) => resting && !SHARP.contains(&(device as f32 / *read as f32)),
                None => true,
            };
            if !stale || !self.pending.insert(i) {
                continue;
            }
            let (path, deck) = (self.path.clone(), self.deck.clone());
            cx.spawn_in(window, async move |this, cx| {
                let slide = cx
                    .background_executor()
                    .spawn(async move { render_slide(&path, &deck, i, device) })
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.pending.remove(&i);
                    // One that cannot be read is not asked for again at once.
                    let Some(slide) = slide else {
                        return;
                    };
                    crate::trace::mark(&format!("slide read for {device}"));
                    if let Some((_, old)) = this.shown.insert(i, (device, Rc::new(slide))) {
                        let_go(&old, window, cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Reads the slides `on_rail` for the rail, a few at a time, and lets
    /// those far from it go. Slides read for the view serve too.
    fn thumbs_around(
        &mut self,
        on_rail: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keep = on_rail.start.saturating_sub(THUMBS_KEPT)..on_rail.end + THUMBS_KEPT;
        self.thumbs.retain(|i, _| keep.contains(i));
        let device = (THUMB * window.scale_factor()) as u32;
        let wanted = on_rail.start.saturating_sub(1)..(on_rail.end + 1).min(self.pages.count());
        for i in wanted {
            if self.thumbs_pending.len() >= THUMBS_AT_ONCE {
                break;
            }
            if self.thumbs.contains_key(&i)
                || self.shown.contains_key(&i)
                || !self.thumbs_pending.insert(i)
            {
                continue;
            }
            let (path, deck) = (self.path.clone(), self.deck.clone());
            cx.spawn_in(window, async move |this, cx| {
                let slide = cx
                    .background_executor()
                    .spawn(async move { render_slide(&path, &deck, i, device) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.thumbs_pending.remove(&i);
                    if let Some(slide) = slide {
                        this.thumbs.insert(i, Rc::new(slide));
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn rail(&mut self, g: Geometry, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let column = self.pages.column_in(f32::from(self.viewport.width));
        let (scale, shift) = (g.scale as f32, g.shift() as f32);
        let span = shift / scale..(shift + g.track as f32) / scale;
        let Some(on_rail) = self.pages.within(column, span) else {
            return div().into_any_element();
        };
        self.thumbs_around(on_rail.clone(), window, cx);
        let theme = Theme::of(window.appearance());
        // Points to pixels on the rail.
        let points = THUMB / self.deck.size.0.max(1.);
        let thumbs: Vec<AnyElement> = self
            .pages
            .layout_at(column)
            .enumerate()
            .skip(on_rail.start)
            .take(on_rail.len())
            .map(|(i, (top, height))| {
                let frame = thumb_frame(top * scale - shift, height * scale, theme);
                let read = self.shown.get(&i).map(|(_, slide)| slide);
                match self.thumbs.get(&i).or(read) {
                    Some(slide) => frame.child(slide_view(slide, points)).into_any_element(),
                    None => frame.into_any_element(),
                }
            })
            .collect();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(thumbs)
            .into_any_element()
    }
}

/// Text in a shape or a table cell, `scale` pixels to the point.
fn text(t: &Text, scale: f32) -> Div {
    let [left, top, right, bottom] = t.insets.map(|i| px(i * scale));
    let column = div()
        .size_full()
        .flex()
        .flex_col()
        .pl(left)
        .pt(top)
        .pr(right)
        .pb(bottom);
    let column = match t.anchor {
        Anchor::Top => column.justify_start(),
        Anchor::Middle => column.justify_center(),
        Anchor::Bottom => column.justify_end(),
    };
    column.children(t.paragraphs.iter().map(|p| {
        let size = p.spans.iter().filter_map(|s| s.size).fold(p.size, f32::max) * scale;
        let font = family(p.spans.iter().find_map(|s| s.font.as_deref()));
        let ink = p
            .spans
            .first()
            .and_then(|s| s.color)
            .map_or(rgb(0x000000).into(), |c| rgb(c).into());
        let align = match p.align {
            Align::Center => TextAlign::Center,
            Align::End => TextAlign::Right,
            _ => TextAlign::Left,
        };
        let mut row = div()
            .flex()
            .flex_row()
            .font_family(font)
            .text_size(px(size))
            .line_height(px(size * 1.15))
            .min_h(px(size * 1.15))
            .text_color(ink)
            .pl(px(f32::from(p.level) * 27. * scale));
        if let Some(bullet) = &p.bullet {
            row = row.child(div().flex_none().w(px(size * 1.1)).child(bullet.clone()));
        }
        row.child(
            div()
                .flex_1()
                .min_w_0()
                .text_align(align)
                .child(styled(&p.spans, ink)),
        )
    }))
}

fn item(item: &Item, picture: Option<&Arc<RenderImage>>, scale: f32) -> Option<AnyElement> {
    let r = item.rect;
    let frame = div()
        .absolute()
        .left(px(r.x * scale))
        .top(px(r.y * scale))
        .w(px(r.w * scale))
        .h(px(r.h * scale));
    Some(match &item.kind {
        Kind::Picture(_) => frame
            .child(
                img(ImageSource::Render(picture?.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Fill),
            )
            .into_any_element(),
        Kind::Shape {
            fill,
            line,
            ellipse,
            text: body,
        } => {
            let mut shape = frame;
            if let Some(fill) = fill {
                shape = shape.bg(rgb(*fill));
            }
            if let Some(line) = line {
                shape = shape.border_1().border_color(rgb(*line));
            }
            if *ellipse {
                shape = shape.rounded_full();
            }
            shape
                .children(body.as_ref().map(|t| text(t, scale)))
                .into_any_element()
        }
        Kind::Table { columns, rows } => {
            let rule: Hsla = rgb(0x8a8a8a).into();
            frame
                .flex()
                .flex_col()
                .border_t_1()
                .border_l_1()
                .border_color(rule)
                .children(rows.iter().map(|(height, cells)| {
                    div().flex().flex_row().min_h(px(height * scale)).children(
                        cells.iter().enumerate().map(|(c, cell)| {
                            let width = columns
                                .get(c)
                                .copied()
                                .unwrap_or(r.w / cells.len().max(1) as f32);
                            div()
                                .w(px(width * scale))
                                .border_r_1()
                                .border_b_1()
                                .border_color(rule)
                                .child(text(cell, scale))
                        }),
                    )
                }))
                .into_any_element()
        }
    })
}

/// A slide filling its frame, `scale` pixels to the point.
fn slide_view(shown: &Slide, scale: f32) -> Div {
    let page = div().absolute().size_full().overflow_hidden();
    let page = match (&shown.slide.background, &shown.background) {
        (_, Some(image)) => page.child(
            img(ImageSource::Render(image.clone()))
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Cover),
        ),
        (Fill::Color(c), None) => page.bg(rgb(*c)),
        _ => page.bg(rgb(0xffffff)),
    };
    page.children(
        shown
            .slide
            .items
            .iter()
            .enumerate()
            .filter_map(|(ix, it)| item(it, shown.pictures.get(&ix), scale)),
    )
}

impl Overviewed for SlidesPane {
    fn overview(&mut self) -> &mut Overview {
        &mut self.overview
    }

    fn geometry(&self, track: f64) -> Geometry {
        let column = self.pages.column_in(f32::from(self.viewport.width)).max(1.);
        Geometry::new(
            f64::from(self.pages.height_at(column)),
            f64::from(self.viewport.height),
            f64::from(self.pages.view_top()),
            f64::from(THUMB / column),
            track,
        )
    }

    fn scroll_to(&mut self, top: f64, _: &mut Window, cx: &mut Context<Self>) {
        self.pages.scroll_to(top as f32);
        cx.notify();
    }
}

impl Render for SlidesPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport = overview::beside(window, self.dock());
        if let Some(view) = self.restore.take() {
            self.pages.restore(&view, f32::from(self.viewport.width));
        }
        self.read_around_view(window, cx);
        let theme = Theme::of(window.appearance());
        let width = f32::from(self.viewport.width);
        let column = self.pages.column_in(width);
        let scale = column / self.deck.size.0.max(1.);
        let slides: Vec<_> = self
            .pages
            .sizes
            .iter()
            .enumerate()
            .map(|(i, &(w, h))| {
                let page = div()
                    .relative()
                    .w(px(column))
                    .h(px(h * column / w.max(1.)))
                    .flex_none()
                    .bg(rgb(0xffffff));
                match self.shown.get(&i) {
                    Some((_, shown)) => page.child(slide_view(shown, scale)),
                    None => page,
                }
            })
            .collect();
        let height = f32::from(self.viewport.height);
        let (dock, bar) = match self.overview.shown(self.geometry(1.).scrollable()) {
            Shown::Docked => {
                let g = self.geometry(f64::from(height));
                let content = self.rail(g, window, cx);
                let panel = overview::panel(&self.overview, g, RAIL, content, theme, cx);
                let dock = overview::dock(RAIL, theme.backdrop, None, Some(panel)).on_scroll_wheel(
                    cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        this.pages.wheel(event, window);
                        cx.notify();
                    }),
                );
                (Some(dock), None)
            }
            Shown::Bar => {
                let g = self.geometry(overview::bar_track(height));
                (
                    None,
                    Some(overview::scrollbar(&self.overview, g, theme, cx)),
                )
            }
            Shown::Overlay | Shown::Nothing => (None, None),
        };
        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(theme.backdrop)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    .child(
                        div()
                            .id("slides")
                            .size_full()
                            // Sideways only while the slides are wider than
                            // the view.
                            .map(|slides| {
                                if column > width + 0.5 {
                                    slides.overflow_scroll()
                                } else {
                                    slides.overflow_y_scroll()
                                }
                            })
                            .track_scroll(&self.pages.scroll)
                            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                                overview::reveal(this, cx);
                            }))
                            // As wide as the slides, or as the view, where
                            // narrower slides stand in the middle.
                            .child(
                                div()
                                    .w(px(column.max(width)))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(GAP))
                                    .children(slides),
                            ),
                    )
                    .children(bar),
            )
            .children(dock)
    }
}
