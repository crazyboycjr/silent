//! PDF pages in a vertical scroll, rendered by gpui-pdf (hayro) around
//! what is in view. Text on a page can be selected by dragging.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::*;

use crate::content::Pdf;
use crate::paged::{GAP, Pages};
use crate::pdftext::{Area, PageText};
use crate::scrub::{self, Scrubbed, Scrubber};
use crate::theme::Theme;

/// Pages kept rendered on each side of those in view.
const MARGIN: usize = 2;

pub struct PdfPane {
    document: Arc<gpui_pdf::Document>,
    pages: Pages,
    rendered: HashMap<usize, Arc<RenderImage>>,
    pending: HashSet<usize>,
    /// The device width pages were last rendered at.
    width: f32,
    scrubber: Scrubber,
    /// Text layers read so far; `None` while one is being read.
    texts: HashMap<usize, Option<PageText>>,
    selecting: Option<Drag>,
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
    pub fn new(pdf: Pdf, window: &Window) -> Self {
        let width = window.viewport_size().width.to_f64() as f32 * window.scale_factor();
        let mut rendered = HashMap::new();
        if let Some(first) = pdf.first {
            rendered.insert(0, first);
        }
        Self {
            document: pdf.document,
            pages: Pages::new(pdf.pages),
            rendered,
            pending: HashSet::new(),
            width,
            scrubber: Scrubber::default(),
            texts: HashMap::new(),
            selecting: None,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.count()
    }

    /// The page at the top of the view, counted from one.
    pub fn current_page(&self) -> usize {
        self.pages.at_top() + 1
    }

    pub fn scroll_by_page(&self, window: &Window, down: bool) {
        self.pages.scroll_by_page(window, down);
    }

    pub fn step_page(&mut self, down: bool, cx: &mut Context<Self>) {
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

    /// The page under a window position, and where on it, from 0 to 1.
    fn point_on_page(&self, position: Point<Pixels>) -> Option<(usize, (f32, f32))> {
        let bounds = self.pages.scroll.bounds();
        let width = f32::from(bounds.size.width);
        let y = f32::from(position.y - bounds.top()) + self.pages.view_top();
        let x = f32::from(position.x - bounds.left()) / width.max(1.);
        let (page, (top, height)) = self
            .pages
            .layout()
            .enumerate()
            .find(|(_, (top, height))| y < top + height + GAP / 2.)
            .or_else(|| self.pages.layout().enumerate().last())?;
        Some((page, (x.clamp(0., 1.), ((y - top) / height).clamp(0., 1.))))
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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
            let width = f32::from(bounds.size.width);
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

    fn render_around_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width);
        let device_width = width * window.scale_factor();
        if (device_width - self.width).abs() > 1. {
            // The window changed size, as for fullscreen: render again, sharp.
            self.width = device_width;
            for (_, image) in self.rendered.drain() {
                cx.drop_image(image, Some(window));
            }
        }
        let Some(view) = self.pages.in_view(width, f32::from(viewport.height)) else {
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
            if let Some(image) = self.rendered.remove(&i) {
                cx.drop_image(image, Some(window));
            }
        }
        for i in keep {
            if self.rendered.contains_key(&i) || !self.pending.insert(i) {
                continue;
            }
            let document = self.document.clone();
            let scale = device_width / self.pages.sizes[i].0;
            let wanted = self.width;
            cx.spawn_in(window, async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { gpui_pdf::render_page(&document, i, scale) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.pending.remove(&i);
                    if let Ok(image) = image
                        && this.width == wanted
                    {
                        this.rendered.insert(i, image);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}

impl Scrubbed for PdfPane {
    fn scrubber(&mut self) -> &mut Scrubber {
        &mut self.scrubber
    }

    fn scrub_to(&mut self, fraction: f32, _: &mut Window, cx: &mut Context<Self>) {
        self.pages
            .go_to((fraction * self.pages.count() as f32) as usize);
        cx.notify();
    }
}

impl Render for PdfPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_around_view(window, cx);
        let theme = Theme::of(window.appearance());
        let width = f32::from(window.viewport_size().width);
        let selected = self
            .selecting
            .as_ref()
            .map(|d| d.page)
            .zip(self.selection());
        let pages = self.pages.sizes.iter().enumerate().map(|(i, &(w, h))| {
            let height = h * width / w;
            let mut page = div()
                .relative()
                .w_full()
                .h(px(height))
                .flex_none()
                .bg(rgb(0xffffff));
            if let Some(image) = self.rendered.get(&i) {
                page = page.child(img(ImageSource::Render(image.clone())).size_full());
            }
            if let Some((page_ix, (_, areas))) = &selected
                && *page_ix == i
            {
                page = page.children(areas.iter().map(|r| {
                    div()
                        .absolute()
                        .left(px(r.x * width))
                        .top(px(r.y * height))
                        .w(px(r.w * width))
                        .h(px(r.h * height))
                        .bg(theme.selection)
                }));
            }
            page
        });
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
        let scrubber = self.scrubber.shown().then(|| {
            let label = format!("page {} of {}", self.current_page(), self.page_count());
            scrub::track(&self.scrubber, self.pages.fraction(), label, theme, cx)
        });
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("pages")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.pages.scroll)
                    .on_scroll_wheel(cx.listener(|_, _: &ScrollWheelEvent, _, cx| cx.notify()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.press(event, window, cx)
                        }),
                    )
                    .bg(theme.backdrop)
                    .child(div().flex().flex_col().gap(px(GAP)).children(pages)),
            )
            .child(follow)
            .children(scrubber)
    }
}
