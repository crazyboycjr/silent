//! PowerPoint slides in a vertical scroll like PDF pages: each slide drawn
//! from its shapes, pictures and tables, read in the background around what
//! is in view.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;
use silent::office::Align;
use silent::office::pptx::{self, Anchor, Fill, Item, Kind, Text};

use crate::content::{Deck, Slide, render_slide};
use crate::docview::{family, styled};
use crate::paged::{GAP, Pages};
use crate::scrub::{self, Scrubbed, Scrubber};
use crate::theme::Theme;

/// Slides kept read on each side of those in view.
const MARGIN: usize = 1;

pub struct SlidesPane {
    path: PathBuf,
    deck: Arc<pptx::Deck>,
    pages: Pages,
    shown: HashMap<usize, Rc<Slide>>,
    pending: HashSet<usize>,
    /// The device width slides were last read for.
    width: u32,
    scrubber: Scrubber,
}

impl SlidesPane {
    pub fn new(deck: Deck, window: &Window) -> Self {
        let width = (f32::from(window.viewport_size().width) * window.scale_factor()) as u32;
        let mut shown = HashMap::new();
        if let Some(first) = deck.first {
            shown.insert(0, Rc::new(first));
        }
        let sizes = vec![deck.deck.size; deck.deck.slides.len()];
        Self {
            path: deck.path,
            deck: Arc::new(deck.deck),
            pages: Pages::new(sizes),
            shown,
            pending: HashSet::new(),
            width,
            scrubber: Scrubber::default(),
        }
    }

    pub fn slide_count(&self) -> usize {
        self.pages.count()
    }

    pub fn current_slide(&self) -> usize {
        self.pages.at_top() + 1
    }

    pub fn scroll_by_page(&self, window: &Window, down: bool) {
        self.pages.scroll_by_page(window, down);
    }

    pub fn step_slide(&mut self, down: bool, cx: &mut Context<Self>) {
        self.pages.step(down);
        cx.notify();
    }

    pub fn go_to_end(&mut self, end: bool, cx: &mut Context<Self>) {
        self.pages.go_to(if end { self.pages.count() } else { 0 });
        cx.notify();
    }

    pub fn set_shift(&mut self, held: bool, cx: &mut Context<Self>) {
        if self.scrubber.held != held {
            self.scrubber.held = held;
            cx.notify();
        }
    }

    fn read_around_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width);
        let device = (width * window.scale_factor()) as u32;
        if device.abs_diff(self.width) > 1 {
            // Pictures were shrunk for the old width; read them again, sharp.
            self.width = device;
            self.shown.clear();
        }
        let Some(view) = self.pages.in_view(width, f32::from(viewport.height)) else {
            return;
        };
        let keep = view.start.saturating_sub(MARGIN)..(view.end + MARGIN).min(self.pages.count());
        self.shown.retain(|i, _| keep.contains(i));
        for i in keep {
            if self.shown.contains_key(&i) || !self.pending.insert(i) {
                continue;
            }
            let (path, deck, wanted) = (self.path.clone(), self.deck.clone(), self.width);
            cx.spawn_in(window, async move |this, cx| {
                let slide = cx
                    .background_executor()
                    .spawn(async move { render_slide(&path, &deck, i, wanted) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.pending.remove(&i);
                    if let Some(slide) = slide
                        && this.width == wanted
                    {
                        this.shown.insert(i, Rc::new(slide));
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}

impl Scrubbed for SlidesPane {
    fn scrubber(&mut self) -> &mut Scrubber {
        &mut self.scrubber
    }

    fn scrub_to(&mut self, fraction: f32, _: &mut Window, cx: &mut Context<Self>) {
        self.pages
            .go_to((fraction * self.pages.count() as f32) as usize);
        cx.notify();
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

impl Render for SlidesPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.read_around_view(window, cx);
        let theme = Theme::of(window.appearance());
        let width = f32::from(window.viewport_size().width);
        let scale = width / self.deck.size.0.max(1.);
        let slides = self.pages.sizes.iter().enumerate().map(|(i, &(w, h))| {
            let page = div()
                .relative()
                .w_full()
                .h(px(h * width / w))
                .flex_none()
                .overflow_hidden();
            let Some(shown) = self.shown.get(&i) else {
                return page.bg(rgb(0xffffff));
            };
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
        });
        let scrubber = self.scrubber.shown().then(|| {
            let label = format!("slide {} of {}", self.current_slide(), self.slide_count());
            scrub::track(&self.scrubber, self.pages.fraction(), label, theme, cx)
        });
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("slides")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.pages.scroll)
                    .on_scroll_wheel(cx.listener(|_, _: &ScrollWheelEvent, _, cx| cx.notify()))
                    .bg(theme.backdrop)
                    .child(div().flex().flex_col().gap(px(GAP)).children(slides)),
            )
            .children(scrubber)
    }
}
