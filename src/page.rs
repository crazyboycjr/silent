//! A web page in the window: Blitz's layout (web.rs) painted on the CPU in
//! strips of device rows around what is in view, as the PDF pane renders
//! its pages, and scrolled by silent: the keys, the wheel and the scrollbar
//! move it. A page with boxes fixed to the window is painted a window at a
//! time instead, so that they stay put. Text is selected by dragging and
//! copied with Ctrl+C; a link opens through the desktop or, into the page,
//! scrolls to its place; a summary opens or closes its details.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use silent::markup::Markup;
use silent::memory::View;
use silent::vim::Motion;

use crate::overview::{self, Geometry, Mode, Overview, Overviewed, Shown};
use crate::theme::Theme;
use crate::web::{self, Click, Doc, Page, STRIP};

/// What a line of the keys scrolls, in logical pixels, as in the text view.
const LINE: f32 = 40.;
/// What a line of the wheel scrolls.
const WHEEL_LINE: f32 = 20.;
/// Strips kept painted on each side of those in view.
const MARGIN: u32 = 2;
/// Strips painted in one frame, the nearest first; the rest follow.
const AT_ONCE: usize = 3;
/// How far the pointer moves before a press is a drag, in logical pixels.
const SLOP: f32 = 3.;
/// How soon after the page was laid out for a size it is laid out for another: a window
/// being resized shows the last layout in between, cut off or with paper beside it.
const RELAYOUT: Duration = Duration::from_millis(50);

pub struct PagePane {
    doc: Doc,
    markup: Markup,
    dir: Arc<PathBuf>,
    /// The scroll, in CSS pixels from the page's top.
    top: f64,
    /// The page's height, in CSS pixels.
    height: f64,
    /// What the layout was made for: the pane in device pixels, the
    /// desktop's scale, the text size and the theme.
    size: (u32, u32),
    scale: f64,
    zoom: f32,
    dark: bool,
    fixed: bool,
    /// When the layout was last made for a new size.
    laid_out_at: Option<Instant>,
    strips: HashMap<u32, Arc<RenderImage>>,
    /// The window's picture, for a page with fixed boxes, and the device
    /// row of the page it starts at.
    window: Option<(u32, Arc<RenderImage>)>,
    /// Pictures the atlas can let go of, at the next frame.
    dropped: Vec<Arc<RenderImage>>,
    /// The pane's bounds as last laid out.
    area: Rc<Cell<Bounds<Pixels>>>,
    pressed: Option<Press>,
    /// The pointer is on a link or a summary.
    pointing: bool,
    overview: Overview,
    /// A block held under the pointer while the text changes size.
    held: Option<Held>,
    /// A frame was drawn: what is around the view can be painted ahead.
    drawn: bool,
}

struct Press {
    at: Point<Pixels>,
    clicks: usize,
    moved: bool,
}

/// A block, how far down it, and where in the pane that place stays.
struct Held {
    node: blitz_dom::NodeId,
    within: f64,
    /// Logical pixels from the pane's top.
    y: f32,
}

impl PagePane {
    pub fn new(page: Page, window: &Window) -> Self {
        let scale = f64::from(window.scale_factor());
        let mut pane = Self {
            height: web::height(&page.doc),
            doc: page.doc,
            markup: page.markup,
            dir: Arc::new(page.dir),
            top: page.top,
            size: (page.size.0.max(1) as u32, page.size.1.max(1) as u32),
            scale,
            zoom: page.zoom,
            dark: page.dark,
            fixed: page.fixed,
            laid_out_at: None,
            strips: page.strips.into_iter().collect(),
            window: None,
            dropped: Vec::new(),
            area: Rc::new(Cell::new(Bounds::default())),
            pressed: None,
            pointing: false,
            overview: Overview::new(Mode::Off, false),
            held: None,
            drawn: false,
        };
        if let Some(image) = page.window {
            pane.window = Some((pane.device(pane.top), image));
        }
        pane
    }

    pub fn markup(&self) -> Markup {
        self.markup
    }

    /// Lets the window's atlas go of the pictures drawn.
    pub fn drop_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strips = self.strips.drain().map(|(_, image)| image);
        let window_picture = self.window.take().map(|(_, image)| image);
        for image in strips.chain(window_picture).chain(self.dropped.drain(..)) {
            cx.drop_image(image, Some(window));
        }
    }

    /// CSS pixels as device pixels.
    fn device(&self, css: f64) -> u32 {
        (css * self.scale * f64::from(self.zoom)).round().max(0.0) as u32
    }

    /// The pane's height in CSS pixels.
    fn view_height(&self) -> f64 {
        f64::from(self.size.1) / (self.scale * f64::from(self.zoom))
    }

    fn most(&self) -> f64 {
        (self.height - self.view_height()).max(0.0)
    }

    fn clamp(&mut self) {
        self.top = self.top.clamp(0.0, self.most());
    }

    /// Everything painted is stale: the layout or the selection changed.
    fn repaint(&mut self) {
        self.dropped
            .extend(self.strips.drain().map(|(_, image)| image));
        self.dropped
            .extend(self.window.take().map(|(_, image)| image));
    }

    /// The page point under window point `at`, in CSS pixels.
    fn page_point(&self, at: Point<Pixels>) -> (f32, f32) {
        let area = self.area.get();
        let zoom = self.zoom;
        let x = f32::from(at.x - area.origin.x) / zoom;
        let y = f32::from(at.y - area.origin.y) / zoom + self.top as f32;
        (x, y)
    }

    /// The layout follows the pane's size, the desktop's theme and the
    /// text size, with the block at the top kept where it is.
    fn sync(&mut self, window: &Window) {
        let viewport = window.viewport_size();
        let scale = f64::from(window.scale_factor());
        let size = (
            (f64::from(viewport.width) * scale).round().max(1.0) as u32,
            (f64::from(viewport.height) * scale).round().max(1.0) as u32,
        );
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        if size == self.size && scale == self.scale && dark == self.dark {
            return;
        }
        let soon = self.laid_out_at.is_some_and(|at| at.elapsed() < RELAYOUT);
        if scale == self.scale && dark == self.dark && soon {
            window.request_animation_frame();
            return;
        }
        self.laid_out_at = Some(Instant::now());
        let relayout = size.0 != self.size.0 || scale != self.scale || dark != self.dark;
        if relayout && self.held.is_none() {
            self.held = self.place_at_y(0.);
        }
        self.size = size;
        self.scale = scale;
        self.dark = dark;
        self.doc.set_viewport(web::viewport(
            (size.0 as i32, size.1 as i32),
            scale,
            self.zoom,
            dark,
        ));
        self.relayout();
    }

    /// The block at `y` logical pixels down the pane, held there.
    fn place_at_y(&self, y: f32) -> Option<Held> {
        let page_y = y / self.zoom + self.top as f32;
        let (node, within) = web::place_at(&self.doc, 8., page_y)?;
        Some(Held { node, within, y })
    }

    /// Lays the page out again and brings the held block back to its place.
    fn relayout(&mut self) {
        self.doc.resolve(0.0);
        self.height = web::height(&self.doc);
        if let Some(held) = self.held.take()
            && let Some((top, height)) = web::span_of(&self.doc, held.node)
        {
            self.top = top + height * held.within - f64::from(held.y / self.zoom);
        }
        self.clamp();
        self.repaint();
    }

    /// The text at `level`, 1 at 100%: the page laid out again with the
    /// place under the pointer at `at`, or else the top, kept still.
    pub fn set_zoom(&mut self, level: f32, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let area = self.area.get();
        let y = at.map_or(0., |at| f32::from(at.y - area.origin.y)).max(0.);
        self.held = self.place_at_y(y);
        self.zoom = level;
        self.doc.zoom_to(level);
        self.relayout();
        cx.notify();
    }

    pub fn motion(&mut self, motion: Motion, cx: &mut Context<Self>) {
        let view = self.view_height();
        let line = f64::from(LINE / self.zoom);
        let by = |k: usize| k as f64;
        self.top = match motion {
            Motion::Down(k) | Motion::ScrollDown(k) => self.top + line * by(k),
            Motion::Up(k) | Motion::ScrollUp(k) => self.top - line * by(k),
            Motion::HalfDown(k) => self.top + view / 2.0 * by(k),
            Motion::HalfUp(k) => self.top - view / 2.0 * by(k),
            Motion::PageDown(k) => self.top + view * 0.9 * by(k),
            Motion::PageUp(k) => self.top - view * 0.9 * by(k),
            Motion::Top | Motion::Line(_) => 0.0,
            Motion::Bottom => self.most(),
            Motion::Left(_) | Motion::Right(_) | Motion::Home => self.top,
        };
        self.clamp();
        overview::reveal(self, cx);
        cx.notify();
    }

    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(WHEEL_LINE));
        let before = self.top;
        self.top -= f64::from(f32::from(delta.y) / self.zoom);
        self.clamp();
        if self.top != before {
            overview::reveal(self, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn press(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let (x, y) = self.page_point(event.position);
        self.doc.set_viewport_scroll(blitz_dom::Point {
            x: 0.0,
            y: self.top,
        });
        let selected = self.doc.get_text_selection_ranges();
        match event.click_count {
            _ if event.modifiers.shift && self.doc.has_text_selection() => {
                self.doc.extend_text_selection_to_point(x, y);
            }
            1 => match self.doc.find_text_position(x, y) {
                Some((root, at)) => self.doc.set_text_selection(root, at, root, at),
                None => self.doc.clear_text_selection(),
            },
            2 => {
                web::select_at(&mut self.doc, x, y, false);
            }
            _ => {
                web::select_at(&mut self.doc, x, y, true);
            }
        }
        self.pressed = Some(Press {
            at: event.position,
            clicks: event.click_count,
            moved: false,
        });
        // A click that selects nothing, as on a link, paints nothing again.
        if self.doc.get_text_selection_ranges() != selected {
            self.repaint();
        }
        cx.notify();
    }

    fn moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(press) = &mut self.pressed else {
            return self.hover(event.position, cx);
        };
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (dx, dy) = (event.position.x - press.at.x, event.position.y - press.at.y);
        let far = f32::from(dx).hypot(f32::from(dy)) > SLOP;
        if !press.moved && !far {
            return;
        }
        press.moved = true;
        // Past the top or bottom edge the page follows, faster the further.
        let area = self.area.get();
        let beyond = if event.position.y < area.top() {
            event.position.y - area.top()
        } else if event.position.y > area.bottom() {
            event.position.y - area.bottom()
        } else {
            px(0.)
        };
        if beyond != px(0.) {
            self.top += f64::from(f32::from(beyond) / self.zoom) * 0.5;
            self.clamp();
        }
        let (x, y) = self.page_point(event.position);
        self.doc.set_viewport_scroll(blitz_dom::Point {
            x: 0.0,
            y: self.top,
        });
        if self.doc.extend_text_selection_to_point(x, y) || beyond != px(0.) {
            self.repaint();
            cx.notify();
        }
    }

    /// The hand over links and summaries.
    fn hover(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.area.get().contains(&at) {
            return;
        }
        let (x, y) = self.page_point(at);
        let pointing = web::click_at(&self.doc, x, y).is_some();
        if pointing != self.pointing {
            self.pointing = pointing;
            cx.notify();
        }
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        let Some(press) = self.pressed.take() else {
            return;
        };
        if press.moved || press.clicks > 1 {
            if let Some(text) = web::selected_text(&self.doc) {
                cx.write_to_primary(ClipboardItem::new_string(text));
            }
            return;
        }
        let (x, y) = self.page_point(press.at);
        match web::click_at(&self.doc, x, y) {
            Some(Click::Link(href)) => self.follow(&href, cx),
            Some(Click::Details(details)) => {
                self.doc.toggle_details_open(details);
                self.relayout();
                cx.notify();
            }
            None => {}
        }
    }

    /// A link: into the page, its place scrolls to the top; else the
    /// desktop opens it, a relative one beside the file.
    fn follow(&mut self, href: &str, cx: &mut Context<Self>) {
        if let Some(top) = web::fragment_top(&self.doc, href) {
            self.top = top;
            self.clamp();
            overview::reveal(self, cx);
            cx.notify();
            return;
        }
        crate::rendered::open(&SharedString::from(href.to_owned()), &self.dir, cx);
    }

    pub fn copy(&self, cx: &mut Context<Self>) {
        if let Some(text) = web::selected_text(&self.doc) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        web::select_all(&mut self.doc);
        if let Some(text) = web::selected_text(&self.doc) {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
        self.repaint();
        cx.notify();
    }

    /// Where the page was left, as a share of its height, so that another
    /// text size or width finds about the same place.
    pub fn view(&self) -> Option<View> {
        (self.top > 0.5 && self.height > 0.0).then(|| View::Page {
            within: (self.top / self.height) as f32,
        })
    }

    /// Paints what the view needs: the strips in view first, then those
    /// around them, a few a frame; or the window, for fixed boxes.
    fn paint_around_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for image in self.dropped.drain(..) {
            cx.drop_image(image, Some(window));
        }
        let paper = web::paper(self.dark);
        let top = self.device(self.top);
        let (width, height) = self.size;
        let css = |device: u32| f64::from(device) / (self.scale * f64::from(self.zoom));
        if self.fixed {
            if self.window.as_ref().is_none_or(|(at, _)| *at != top) {
                let image = web::paint(&mut self.doc, css(top), width, height, paper);
                if let Some((_, old)) = self.window.replace((top, image)) {
                    cx.drop_image(old, Some(window));
                }
            }
            return;
        }
        let last = self.device(self.height).div_ceil(STRIP).max(1);
        let shown = (top / STRIP).min(last - 1)..(top + height).div_ceil(STRIP).min(last);
        let kept = shown.start.saturating_sub(MARGIN)..(shown.end + MARGIN).min(last);
        let far: Vec<u32> = self
            .strips
            .keys()
            .copied()
            .filter(|i| !kept.contains(i))
            .collect();
        for i in far {
            if let Some(image) = self.strips.remove(&i) {
                cx.drop_image(image, Some(window));
            }
        }
        // What is in view is painted now; the strips around it, which the
        // view may scroll to next, a few a frame from the frame after the
        // first, below before above.
        let missing: Vec<u32> = shown
            .clone()
            .filter(|i| !self.strips.contains_key(i))
            .collect();
        let around: Vec<u32> = (shown.end..kept.end)
            .chain((kept.start..shown.start).rev())
            .filter(|i| !self.strips.contains_key(i))
            .collect();
        let ahead = if self.drawn && missing.is_empty() {
            AT_ONCE
        } else {
            0
        };
        for &i in missing.iter().chain(around.iter().take(ahead)) {
            let image = web::paint(&mut self.doc, css(i * STRIP), width, STRIP, paper);
            self.strips.insert(i, image);
        }
        if around.len() > ahead {
            window.request_animation_frame();
        }
        self.drawn = true;
        self.doc.set_viewport_scroll(blitz_dom::Point {
            x: 0.0,
            y: self.top,
        });
    }
}

impl Overviewed for PagePane {
    fn overview(&mut self) -> &mut Overview {
        &mut self.overview
    }

    fn geometry(&self, track: f64) -> Geometry {
        Geometry::new(self.height, self.view_height(), self.top, 1.0, track)
    }

    fn scroll_to(&mut self, top: f64, _: &mut Window, cx: &mut Context<Self>) {
        self.top = top;
        self.clamp();
        cx.notify();
    }
}

impl Render for PagePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window);
        self.paint_around_view(window, cx);
        let theme = Theme::of(window.appearance());
        let scale = self.scale as f32;
        let top = self.device(self.top) as f32;
        let width = px(self.size.0 as f32 / scale);
        let pictures: Vec<AnyElement> = match &self.window {
            Some((_, image)) => vec![
                img(ImageSource::Render(image.clone()))
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(width)
                    .h(px(self.size.1 as f32 / scale))
                    .object_fit(ObjectFit::Fill)
                    .into_any_element(),
            ],
            None => self
                .strips
                .iter()
                .map(|(&i, image)| {
                    img(ImageSource::Render(image.clone()))
                        .absolute()
                        .top(px((i as f32 * STRIP as f32 - top) / scale))
                        .left_0()
                        .w(width)
                        .h(px(STRIP as f32 / scale))
                        .object_fit(ObjectFit::Fill)
                        .into_any_element()
                })
                .collect(),
        };
        let area = self.area.clone();
        let view = cx.entity().downgrade();
        // Listened to on every frame: a quick click can be released before
        // the frame after its press is drawn.
        let follow = canvas(
            move |bounds, _, _| area.set(bounds),
            move |_, _, window, _| {
                let moved = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = moved.update(cx, |this, cx| this.moved(event, cx));
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
        .absolute()
        .size_full();
        let height = f32::from(window.viewport_size().height);
        let bar = (self.overview.shown(self.geometry(1.).scrollable()) == Shown::Bar).then(|| {
            let g = self.geometry(overview::bar_track(height));
            overview::scrollbar(&self.overview, g, theme, cx)
        });
        div()
            .id("page")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(theme.surface)
            .cursor(if self.pointing {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            })
            .on_scroll_wheel(
                cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.wheel(event, cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| this.press(event, cx)),
            )
            .children(pictures)
            .child(follow)
            .children(bar)
    }
}
