//! PDF pages in a vertical scroll, rendered by gpui-pdf (hayro) around
//! what is in view.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::*;

use crate::content::Pdf;
use crate::theme::Theme;

const GAP: f32 = 10.;
/// Pages kept rendered on each side of those in view.
const MARGIN: usize = 2;

pub struct PdfPane {
    document: Arc<gpui_pdf::Document>,
    pages: Vec<(f32, f32)>,
    rendered: HashMap<usize, Arc<RenderImage>>,
    pending: HashSet<usize>,
    scroll: ScrollHandle,
    /// The device width pages were last rendered at.
    width: f32,
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
            pages: pdf.pages,
            rendered,
            pending: HashSet::new(),
            scroll: ScrollHandle::new(),
            width,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The page at the top of the view, counted from one.
    pub fn current_page(&self, window: &Window) -> usize {
        let top = -f32::from(self.scroll.offset().y);
        let width = f32::from(window.viewport_size().width);
        let mut y = 0.;
        for (i, &(w, h)) in self.pages.iter().enumerate() {
            y += h * width / w + GAP;
            if y > top + 1. {
                return i + 1;
            }
        }
        self.pages.len()
    }

    pub fn scroll_by_page(&self, window: &Window, down: bool) {
        let height = f32::from(window.viewport_size().height) * 0.9;
        let mut offset = self.scroll.offset();
        offset.y += px(if down { -height } else { height });
        offset.y = offset.y.min(px(0.));
        self.scroll.set_offset(offset);
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
        let top = -f32::from(self.scroll.offset().y);
        let bottom = top + f32::from(viewport.height);
        let mut y = 0.;
        let (mut first, mut last) = (usize::MAX, 0);
        for (i, &(w, h)) in self.pages.iter().enumerate() {
            let next = y + h * width / w;
            if next >= top && y <= bottom {
                first = first.min(i);
                last = i;
            }
            y = next + GAP;
        }
        if first == usize::MAX {
            return;
        }
        let keep = first.saturating_sub(MARGIN)..(last + MARGIN + 1).min(self.pages.len());
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
            let scale = device_width / self.pages[i].0;
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

impl Render for PdfPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_around_view(window, cx);
        let theme = Theme::of(window.appearance());
        let width = f32::from(window.viewport_size().width);
        let pages = self.pages.iter().enumerate().map(|(i, &(w, h))| {
            let page = div()
                .w_full()
                .h(px(h * width / w))
                .flex_none()
                .bg(rgb(0xffffff));
            match self.rendered.get(&i) {
                Some(image) => page.child(img(ImageSource::Render(image.clone())).size_full()),
                None => page,
            }
        });
        div()
            .id("pages")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|_, _: &ScrollWheelEvent, _, cx| cx.notify()))
            .bg(theme.backdrop)
            .child(div().flex().flex_col().gap(px(GAP)).children(pages))
    }
}
