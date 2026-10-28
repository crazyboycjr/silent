//! PDF pages in a scroll, drawn by hayro around what is in view, with a
//! rail of thumbnails docked at the right. Text on a page can be selected
//! by dragging. Ctrl with + and - zooms the pages, each file at a zoom of
//! its own: the pages as drawn show larger or smaller at once, and are
//! drawn again at their new size once the zoom rests. Zoomed wider than the
//! window, whole pages stay drawn at the window's width, and over each the
//! part in view is drawn at the zoom's, so that a page is never drawn
//! larger than about two windows.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};
use silent::memory::View;
use silent::vim::Motion;
use smallvec::SmallVec;

use crate::content::Pdf;
use crate::overview::{self, Docking, Geometry, Mode, Overview, Overviewed, Shown};
use crate::paged::{GAP, Pages, RAIL, THUMB};
use crate::pdftext::{Area, PageText};
use crate::theme::Theme;

/// Pages kept drawn on each side of those in view.
const MARGIN: usize = 2;
/// Thumbnails kept on each side of those on the rail, and drawn at once.
const THUMBS_KEPT: usize = 16;
const THUMBS_AT_ONCE: usize = 4;
/// How long the zoom rests before the pages are drawn at its size.
const REST: Duration = Duration::from_millis(100);
/// How far past the view the part of a zoomed page drawn reaches, in views.
const BEYOND: (f32, f32) = (0.25, 0.4);
/// A whole page: left, top, right and bottom, as shares of it.
const WHOLE: [f32; 4] = [0., 0., 1., 1.];

pub struct PdfPane {
    document: Arc<gpui_pdf::Document>,
    pages: Pages,
    /// Whole pages around the view, drawn at the window's width at most.
    rendered: HashMap<usize, Raster>,
    /// The part of each page in view drawn at the zoom's size, while the
    /// pages are wider than the window.
    tiles: HashMap<usize, Raster>,
    /// Pages being drawn, whole or in part.
    pending: HashSet<(usize, bool)>,
    /// When the zoom last changed, and the wake-up once it has rested.
    zoomed: Option<Instant>,
    rest: Option<Task<()>>,
    /// The pages' area when last drawn: the window, less the rail's dock.
    viewport: Size<Pixels>,
    /// Text layers read so far; `None` while one is being read.
    texts: HashMap<usize, Option<PageText>>,
    selecting: Option<Drag>,
    overview: Overview,
    thumbs: HashMap<usize, Arc<RenderImage>>,
    thumbs_pending: HashSet<usize>,
    /// Where the file was left, gone to on the first frame.
    restore: Option<View>,
    /// Quarter turns clockwise of every page, which r gave.
    turns: u8,
}

/// A page, or a part of it, as drawn.
pub struct Raster {
    pub image: Arc<RenderImage>,
    /// Device pixels a point.
    scale: f32,
    /// The part of the page drawn, as shares of its width and height as it
    /// is shown: left, top, right, bottom.
    area: [f32; 4],
}

/// A selection by dragging over one page.
struct Drag {
    page: usize,
    /// Where on the page, from 0 to 1 across and down.
    from: (f32, f32),
    to: (f32, f32),
    moved: bool,
    held: bool,
}

impl PdfPane {
    /// Lets the window's atlas go of the pages and thumbnails drawn.
    pub fn drop_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pages = self.rendered.drain().chain(self.tiles.drain());
        for (_, raster) in pages {
            cx.drop_image(raster.image, Some(window));
        }
        for (_, image) in self.thumbs.drain() {
            cx.drop_image(image, Some(window));
        }
    }

    pub fn new(
        pdf: Pdf,
        mode: Mode,
        docked: bool,
        view: Option<View>,
        zoom: f32,
        window: &Window,
    ) -> Self {
        let mut rendered = HashMap::new();
        if let Some((page, raster)) = pdf.first {
            rendered.insert(page, raster);
        }
        let viewport = overview::beside(window, if docked { RAIL } else { 0. });
        let width = f32::from(viewport.width);
        Self {
            document: pdf.document,
            pages: Pages::new(pdf.pages, zoom, width, window.scale_factor()),
            rendered,
            tiles: HashMap::new(),
            pending: HashSet::new(),
            zoomed: None,
            rest: None,
            viewport,
            texts: HashMap::new(),
            selecting: None,
            overview: Overview::new(mode, docked),
            thumbs: HashMap::new(),
            thumbs_pending: HashSet::new(),
            restore: view,
            turns: pdf.turns,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.count()
    }

    /// The page at the top of the view, counted from one.
    pub fn current_page(&self) -> usize {
        self.pages.at_top() + 1
    }

    pub fn view(&self) -> Option<View> {
        self.pages.view()
    }

    /// The zoom, 1 when a page's width fills the view.
    pub fn zoom(&self) -> f32 {
        self.pages.zoom
    }

    /// The pages at `zoom`, shown at once at their new size from what is
    /// drawn, the place under the pointer at `at`, or else the middle of
    /// the view's top, kept where it is; drawn again once the zoom rests.
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

    pub fn motion(&mut self, motion: Motion, cx: &mut Context<Self>) {
        self.pages.motion(motion, f32::from(self.viewport.height));
        overview::reveal(self, cx);
    }

    /// The engaged ← and →: sideways on pages zoomed wider than the window,
    /// then to the previous or next page.
    pub fn across(&mut self, right: bool, n: usize, cx: &mut Context<Self>) {
        self.pages.across(right, n);
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

    /// The page under a window position, and where on it, from 0 to 1.
    fn point_on_page(&self, position: Point<Pixels>) -> Option<(usize, (f32, f32))> {
        let bounds = self.pages.scroll.bounds();
        let width = f32::from(bounds.size.width);
        let column = self.pages.column_in(width).max(1.);
        let y = f32::from(position.y - bounds.top()) + self.pages.view_top();
        let x = f32::from(position.x - bounds.left()) + self.pages.view_left();
        let x = (x - self.pages.inset(width)) / column;
        let (page, (top, height)) = self
            .pages
            .layout()
            .enumerate()
            .find(|(_, (top, height))| y < top + height + GAP / 2.)
            .or_else(|| self.pages.layout().enumerate().last())?;
        Some((page, (x.clamp(0., 1.), ((y - top) / height).clamp(0., 1.))))
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The text of a page is laid out as the page stands, not turned.
        if self.turns != 0 {
            return;
        }
        let Some((page, point)) = self.point_on_page(event.position) else {
            return;
        };
        self.selecting = Some(Drag {
            page,
            from: point,
            to: point,
            moved: false,
            held: true,
        });
        if let Entry::Vacant(slot) = self.texts.entry(page) {
            slot.insert(None);
            let document = self.document.clone();
            cx.spawn_in(window, async move |this, cx| {
                let text = cx
                    .background_executor()
                    .spawn(async move { PageText::read(&document, page) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.texts.insert(page, text);
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selecting.as_ref().is_some_and(|d| d.held) {
            return;
        }
        let bounds = self.pages.scroll.bounds();
        let beyond = if position.y < bounds.top() {
            position.y - bounds.top()
        } else if position.y > bounds.bottom() {
            position.y - bounds.bottom()
        } else {
            px(0.)
        };
        if beyond != px(0.) {
            self.pages
                .scroll_to(self.pages.view_top() + f32::from(beyond) * 0.5);
        }
        let Some(drag) = &self.selecting else {
            return;
        };
        let page = drag.page;
        // The selection stays on the page it began on.
        let point = match self.point_on_page(position) {
            Some((p, point)) if p == page => point,
            Some((p, _)) if p > page => (1., 1.),
            Some(_) => (0., 0.),
            None => return,
        };
        if let Some(drag) = &mut self.selecting {
            let width = self.pages.column();
            drag.moved |= (point.0 - drag.from.0).abs() * width > 3.
                || (point.1 - drag.from.1).abs() * width > 3.;
            drag.to = point;
        }
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        match &mut self.selecting {
            Some(drag) if drag.held => drag.held = false,
            _ => return,
        }
        if let Some((text, _)) = self.selection() {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    /// The selected text and its boxes on the page.
    fn selection(&self) -> Option<(String, Vec<Area>)> {
        let drag = self.selecting.as_ref().filter(|d| d.moved)?;
        let text = self.texts.get(&drag.page)?.as_ref()?;
        text.select(drag.from, drag.to)
    }

    pub fn copy(&self, cx: &mut Context<Self>) {
        if let Some((text, _)) = self.selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Draws the pages around the view at their size, whole, and the part
    /// of each in view at the zoom's size while they are wider than the
    /// window; lets those far from it go. What is drawn already stays shown
    /// until what replaces it is in, and while the zoom moves, only pages
    /// not drawn at all are drawn.
    fn render_around_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (width, height) = (
            f32::from(self.viewport.width),
            f32::from(self.viewport.height),
        );
        let device = window.scale_factor();
        let column = self.pages.column_in(width);
        let Some(view) = self.pages.in_view(column, height) else {
            return;
        };
        let keep = view.start.saturating_sub(MARGIN)..(view.end + MARGIN).min(self.pages.count());
        let far: Vec<usize> = self
            .rendered
            .keys()
            .copied()
            .filter(|i| !keep.contains(i))
            .collect();
        for i in far {
            if let Some(raster) = self.rendered.remove(&i) {
                cx.drop_image(raster.image, Some(window));
            }
        }
        let away: Vec<usize> = self
            .tiles
            .keys()
            .copied()
            .filter(|i| !view.contains(i))
            .collect();
        for i in away {
            if let Some(tile) = self.tiles.remove(&i) {
                cx.drop_image(tile.image, Some(window));
            }
        }
        let resting = self.zoomed.is_none_or(|at| at.elapsed() >= REST);
        let layout: Vec<(f32, f32)> = self.pages.layout_at(column).collect();
        for i in keep {
            let points = self.pages.sizes[i].0.max(1.);
            // The zoom's scale, and the window's, which whole pages keep to.
            let full = column * device / points;
            let whole = full.min(width * device / points);
            let stale = match self.rendered.get(&i) {
                Some(drawn) => resting && !near(drawn.scale, whole),
                None => true,
            };
            if stale {
                self.draw(i, whole, None, window, cx);
            }
            if !view.contains(&i) || full <= whole * 1.01 {
                if let Some(tile) = self.tiles.remove(&i) {
                    cx.drop_image(tile.image, Some(window));
                }
                continue;
            }
            let (top, tall) = layout[i];
            let left = self.pages.inset(width);
            let seen = [
                (self.pages.view_left() - left) / column,
                (self.pages.view_top() - top) / tall,
                (self.pages.view_left() + width - left) / column,
                (self.pages.view_top() + height - top) / tall,
            ]
            .map(|share| share.clamp(0., 1.));
            if seen[2] <= seen[0] || seen[3] <= seen[1] {
                continue;
            }
            let fresh = self
                .tiles
                .get(&i)
                .is_some_and(|tile| near(tile.scale, full) && covers(tile.area, seen));
            if !fresh && resting {
                let (x, y) = (BEYOND.0 * width / column, BEYOND.1 * height / tall);
                let wanted = [seen[0] - x, seen[1] - y, seen[2] + x, seen[3] + y];
                self.draw(i, full, Some(wanted.map(|s| s.clamp(0., 1.))), window, cx);
            }
        }
    }

    /// Draws page `page` at `scale` in the background, the part `area` of
    /// it or else the whole, unless it is being drawn already.
    fn draw(
        &mut self,
        page: usize,
        scale: f32,
        area: Option<[f32; 4]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let part = area.is_some();
        if !self.pending.insert((page, part)) {
            return;
        }
        let (document, turns) = (self.document.clone(), self.turns);
        cx.spawn_in(window, async move |this, cx| {
            let area = area.unwrap_or(WHOLE);
            let drawn = cx
                .background_executor()
                .spawn(async move { rasterize(&document, page, scale, area, turns) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.pending.remove(&(page, part));
                // A page that cannot be drawn is not asked for again at
                // once; one the zoom has moved past is drawn again next.
                let Some(raster) = drawn else {
                    return;
                };
                let what = if part { "part of a page" } else { "page" };
                crate::trace::mark(&format!("{what} drawn at {:.2}", raster.scale));
                let slot = if part {
                    &mut this.tiles
                } else {
                    &mut this.rendered
                };
                if let Some(old) = slot.insert(page, raster) {
                    cx.drop_image(old.image, Some(window));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Draws the thumbnails of the pages `shown` on the rail, a few at a
    /// time, and lets those far from it go.
    fn thumbs_around(&mut self, shown: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let keep = shown.start.saturating_sub(THUMBS_KEPT)..shown.end + THUMBS_KEPT;
        let far: Vec<usize> = self
            .thumbs
            .keys()
            .copied()
            .filter(|i| !keep.contains(i))
            .collect();
        for i in far {
            if let Some(image) = self.thumbs.remove(&i) {
                cx.drop_image(image, Some(window));
            }
        }
        let device = THUMB * window.scale_factor();
        let wanted = shown.start.saturating_sub(1)..(shown.end + 1).min(self.pages.count());
        for i in wanted {
            if self.thumbs_pending.len() >= THUMBS_AT_ONCE {
                break;
            }
            if self.thumbs.contains_key(&i) || !self.thumbs_pending.insert(i) {
                continue;
            }
            let document = self.document.clone();
            let scale = device / self.pages.sizes[i].0.max(1.);
            let turns = self.turns;
            cx.spawn_in(window, async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { rasterize(&document, i, scale, WHOLE, turns) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.thumbs_pending.remove(&i);
                    if let Some(raster) = image {
                        this.thumbs.insert(i, raster.image);
                    }
                    // The next ones, if the rail still shows them.
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// The thumbnails on the rail, from the geometry's shift down.
    fn rail(&mut self, g: Geometry, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let column = self.pages.column_in(f32::from(self.viewport.width));
        let (scale, shift) = (g.scale as f32, g.shift() as f32);
        let span = shift / scale..(shift + g.track as f32) / scale;
        let Some(shown) = self.pages.within(column, span) else {
            return div().into_any_element();
        };
        self.thumbs_around(shown.clone(), window, cx);
        let theme = Theme::of(window.appearance());
        let thumbs: Vec<AnyElement> = self
            .pages
            .layout_at(column)
            .enumerate()
            .skip(shown.start)
            .take(shown.len())
            .map(|(i, (top, height))| {
                let page = thumb_frame(top * scale - shift, height * scale, theme);
                match self.thumbs.get(&i) {
                    Some(image) => page
                        .child(img(ImageSource::Render(image.clone())).size_full())
                        .into_any_element(),
                    None => page.into_any_element(),
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

/// Whether two scales are as good as one.
fn near(a: f32, b: f32) -> bool {
    (a / b.max(1e-6) - 1.).abs() < 0.02
}

/// Whether the part `outer` of a page holds all of `inner`.
fn covers(outer: [f32; 4], inner: [f32; 4]) -> bool {
    const SLACK: f32 = 1e-4;
    outer[0] <= inner[0] + SLACK
        && outer[1] <= inner[1] + SLACK
        && outer[2] >= inner[2] - SLACK
        && outer[3] >= inner[3] - SLACK
}

/// A part of a page, as shares of it, once the page is turned `turns`
/// quarters clockwise.
fn quarter([left, top, right, bottom]: [f32; 4], turns: u8) -> [f32; 4] {
    match turns % 4 {
        1 => [1. - bottom, left, 1. - top, right],
        2 => [1. - right, 1. - bottom, 1. - left, 1. - top],
        3 => [top, 1. - right, bottom, 1. - left],
        _ => [left, top, right, bottom],
    }
}

/// Page `index` drawn at `scale` device pixels a point on white, the part
/// `area` of it as it is shown, turned `turns` quarters clockwise, with
/// the part it covers to the pixel.
pub fn rasterize(
    document: &gpui_pdf::Document,
    index: usize,
    scale: f32,
    area: [f32; 4],
    turns: u8,
) -> Option<Raster> {
    let page = document.pages().get(index)?;
    let (width, height) = page.render_dimensions();
    let (wide, tall) = (width * scale, height * scale);
    // The part on the page as it stands, to whole pixels; a part that
    // reaches an edge reaches it exactly.
    let [x0, y0, x1, y1] = quarter(area, (4 - turns % 4) % 4);
    let edge = |share: f32, length: f32, round: fn(f32) -> f32| {
        if share >= 1. {
            length.round().max(1.)
        } else {
            round(share * length)
        }
    };
    let (left, top) = ((x0 * wide).floor(), (y0 * tall).floor());
    let (right, bottom) = (edge(x1, wide, f32::ceil), edge(y1, tall, f32::ceil));
    let (w, h) = ((right - left).max(1.), (bottom - top).max(1.));
    if w > f32::from(u16::MAX) || h > f32::from(u16::MAX) {
        return None;
    }
    let settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        width: Some(w as u16),
        height: Some(h as u16),
        bg_color: WHITE,
        origin: (left, top),
    };
    let pixmap = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &settings,
    );
    // Premultiplied RGBA over white, as gpui's straight BGRA.
    let mut bgra = pixmap.data_as_u8_slice().to_vec();
    for p in bgra.as_chunks_mut::<4>().0 {
        let add = 255 - p[3];
        *p = [
            p[2].saturating_add(add),
            p[1].saturating_add(add),
            p[0].saturating_add(add),
            255,
        ];
    }
    let (columns, rows) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    let buffer = image::RgbaImage::from_raw(columns, rows, bgra)?;
    let image = Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(buffer),
        1,
    )));
    let covered = [
        left / wide,
        top / tall,
        if x1 >= 1. { 1. } else { right / wide },
        if y1 >= 1. { 1. } else { bottom / tall },
    ];
    Some(Raster {
        image: crate::picture::turn(image, turns),
        scale,
        area: quarter(covered, turns),
    })
}

/// A page, or a part of it, where it lies on a page `width` by `height`.
fn placed(raster: &Raster, width: f32, height: f32) -> Img {
    let [left, top, right, bottom] = raster.area;
    img(ImageSource::Render(raster.image.clone()))
        .absolute()
        .left(px(left * width))
        .top(px(top * height))
        .w(px((right - left) * width))
        .h(px((bottom - top) * height))
}

/// A page on the rail, `top` pixels down the track and `height` tall,
/// a little apart from its neighbours.
pub fn thumb_frame(top: f32, height: f32, theme: Theme) -> Div {
    div()
        .absolute()
        .left(px((RAIL - THUMB) / 2.))
        .top(px(top + 2.))
        .w(px(THUMB))
        .h(px((height - 4.).max(2.)))
        .overflow_hidden()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(theme.muted.opacity(0.35))
}

impl Overviewed for PdfPane {
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

impl Render for PdfPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.viewport = overview::beside(window, self.dock());
        let (width, height) = (self.viewport.width, self.viewport.height);
        self.pages.lay_out(
            size(f32::from(width), f32::from(height)),
            window.scale_factor(),
        );
        if let Some(view) = self.restore.take() {
            self.pages.restore(&view, f32::from(width));
        }
        self.render_around_view(window, cx);
        let theme = Theme::of(window.appearance());
        let width = f32::from(self.viewport.width);
        let column = self.pages.column_in(width);
        let selected = self
            .selecting
            .as_ref()
            .map(|d| d.page)
            .zip(self.selection());
        let pages = self.pages.sizes.iter().enumerate().map(|(i, &(w, h))| {
            let height = h * column / w.max(1.);
            let mut page = div()
                .relative()
                .w(px(column))
                .h(px(height))
                .flex_none()
                .bg(rgb(0xffffff));
            if let Some(raster) = self.rendered.get(&i) {
                page = page.child(placed(raster, column, height));
            }
            if let Some(tile) = self.tiles.get(&i) {
                page = page.child(placed(tile, column, height));
            }
            if let Some((page_ix, (_, areas))) = &selected
                && *page_ix == i
            {
                page = page.children(areas.iter().map(|r| {
                    div()
                        .absolute()
                        .left(px(r.x * column))
                        .top(px(r.y * height))
                        .w(px(r.w * column))
                        .h(px(r.h * height))
                        .bg(theme.selection)
                }));
            }
            page
        });
        let pages: Vec<_> = pages.collect();
        let view = cx.entity().downgrade();
        // Listened to on every frame: a quick click can be released before
        // the frame after its press is drawn.
        let follow = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let moved = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = moved.update(cx, |this, cx| this.drag(event.position, cx));
                    }
                });
                let released = view.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        let _ = released.update(cx, |this, cx| this.release(cx));
                    }
                });
            },
        )
        .absolute();
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
                            .id("pages")
                            .size_full()
                            // Sideways only while the pages are wider than
                            // the view: until then Shift and the wheel scroll
                            // down, as they did.
                            .map(|pages| {
                                if column > width + 0.5 {
                                    pages.overflow_scroll()
                                } else {
                                    pages.overflow_y_scroll()
                                }
                            })
                            .track_scroll(&self.pages.scroll)
                            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                                overview::reveal(this, cx);
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                    this.press(event, window, cx)
                                }),
                            )
                            // As wide as the pages, or as the view, where
                            // narrower pages stand in the middle.
                            .child(
                                div()
                                    .w(px(column.max(width)))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(GAP))
                                    .children(pages),
                            ),
                    )
                    .child(follow)
                    .children(bar),
            )
            .children(dock)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` attribute would come with it.
    use std::sync::Arc;

    use super::{Raster, WHOLE, covers, near, quarter, rasterize};

    /// A page of 200 by 100 points: a red box, a blue one and a black line.
    fn sample() -> Arc<gpui_pdf::Document> {
        let content =
            "1 0 0 rg 20 20 60 40 re f 0 0 1 rg 120 50 50 30 re f 0 g 2 w 10 90 m 190 10 l S";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
        }
        let xref = pdf.len();
        pdf.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        gpui_pdf::parse(Arc::new(pdf.into_bytes())).expect("a PDF")
    }

    fn pixels(raster: &Raster) -> (u32, u32, Vec<u8>) {
        let size = raster.image.size(0);
        let bytes = raster.image.as_bytes(0).unwrap().to_vec();
        (size.width.0 as u32, size.height.0 as u32, bytes)
    }

    #[test]
    fn a_part_of_a_page_is_drawn_as_the_whole_page_draws_it() {
        let document = sample();
        let whole = rasterize(&document, 0, 2., WHOLE, 0).unwrap();
        let (width, height, all) = pixels(&whole);
        assert_eq!((width, height), (400, 200));
        assert_eq!(whole.area, WHOLE);
        // The middle right: from (100, 50) to (350, 150) at twice the size.
        let part = rasterize(&document, 0, 2., [0.25, 0.25, 0.875, 0.75], 0).unwrap();
        let (w, h, some) = pixels(&part);
        assert_eq!((w, h), (250, 100));
        assert_eq!(part.area, [0.25, 0.25, 0.875, 0.75]);
        let mut most = 0;
        for y in 0..h {
            for x in 0..w {
                let a = ((y + 50) * width + x + 100) as usize * 4;
                let b = (y * w + x) as usize * 4;
                for c in 0..4 {
                    most = most.max(all[a + c].abs_diff(some[b + c]));
                }
            }
        }
        assert!(most <= 2, "{most}");
        // The red box, 40 to 160 across and 80 to 160 down, is there, in
        // BGRA.
        let red = (50 * w + 10) as usize * 4;
        assert_eq!(&some[red..red + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn a_part_of_a_turned_page_is_found_on_the_page_as_it_stands() {
        for turns in 0..4 {
            let area = [0.1, 0.2, 0.6, 0.9];
            let back = quarter(quarter(area, turns), (4 - turns) % 4);
            assert!(
                back.iter().zip(area).all(|(a, b)| (a - b).abs() < 1e-6),
                "{back:?}"
            );
        }
        // Turned a quarter clockwise, the page's left edge is at the top.
        assert_eq!(quarter([0., 0., 0.5, 1.], 1), [0., 0., 1., 0.5]);
        let document = sample();
        let part = rasterize(&document, 0, 1., [0., 0., 1., 0.5], 1).unwrap();
        // 100 by 200 turned; its top half is the page's left half, turned.
        let (w, h, _) = pixels(&part);
        assert_eq!((w, h), (100, 100));
        assert_eq!(part.area, [0., 0., 1., 0.5]);
    }

    #[test]
    fn a_part_covers_another_within_it() {
        assert!(covers([0., 0.2, 1., 0.8], [0.1, 0.3, 0.9, 0.7]));
        assert!(!covers([0., 0.2, 1., 0.8], [0.1, 0.1, 0.9, 0.7]));
        assert!(near(2., 2.01) && !near(2., 2.2));
    }
}
