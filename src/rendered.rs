//! Markdown and HTML rendered by gpui-base's text view, in silent's warm
//! colours: the pictures beside the file decoded ahead, every other picture
//! its words, links opened by the desktop when clicked, nothing fetched. The
//! view scrolls a list of the document's blocks, which the keys move.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::base::{TextView, TextViewState, TextViewStyle};
use gpui_kit::*;
use silent::markup::Markup;
use silent::memory::View;
use silent::vim::Motion;

use crate::content::{Drawn, Rendered};
use crate::theme::Theme;
use crate::zoom::{self, Held};

/// What a line of the keys scrolls.
const LINE: f32 = 40.;
/// The margin around the column.
const MARGIN: f32 = 36.;
/// Texts up to this many bytes the text view parses before its first frame,
/// longer ones in the background (gpui-base's `MAX_SYNC_FULL_REPLACE_BYTES`).
const PARSED_AT_ONCE: usize = 4 << 10;

pub struct RenderedPane {
    state: Entity<TextViewState>,
    pictures: Arc<HashMap<String, Drawn>>,
    dir: Arc<PathBuf>,
    /// Where the file was left, gone to once the blocks are laid out.
    restore: Option<(usize, f32)>,
    parsed: bool,
    /// The place under the pointer as the text size changed.
    held: Option<Held>,
}

impl RenderedPane {
    pub fn new(
        doc: Rendered,
        view: Option<View>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::theme::components(window, cx);
        let state = cx.new(|cx| {
            match doc.markup {
                Markup::Markdown => TextViewState::markdown(&doc.text, cx),
                Markup::Html => TextViewState::html(&doc.text, cx),
            }
            .remeasure_gradually()
        });
        // The first word from the text view is a long text parsed.
        cx.observe(&state, |this: &mut Self, _, cx| {
            if !this.parsed {
                crate::trace::mark("parsed");
                this.parsed = true;
            }
            cx.notify();
        })
        .detach();
        Self {
            state,
            pictures: Arc::new(doc.pictures),
            dir: Arc::new(doc.dir),
            restore: match view {
                Some(View::Rendered { block, offset }) => Some((block, offset)),
                _ => None,
            },
            parsed: doc.text.len() <= PARSED_AT_ONCE,
            held: None,
        }
    }

    /// The text at another size, which the window's rem size sets, with the
    /// place under the pointer at `at`, or else at the top, kept where it
    /// is. The text view lays out the blocks in view at once and measures
    /// the others over the next frames.
    pub fn set_zoom(&mut self, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let list = self.list(cx);
        let y = at.map_or_else(|| list.viewport_bounds().top(), |at| at.y);
        self.held = zoom::hold(&list, y);
        cx.notify();
    }

    /// Lets the window's atlas and gpui's cache go of the pictures drawn.
    pub fn drop_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for picture in self.pictures.values() {
            match picture {
                Drawn::Raster(image) => cx.drop_image(image.clone(), Some(window)),
                Drawn::Svg(image) => {
                    if let Some(drawn) = image.clone().get_render_image(window, cx) {
                        cx.drop_image(drawn, Some(window));
                    }
                    image.clone().remove_asset(cx);
                }
            }
        }
    }

    /// Whether the text is parsed into blocks, which a long one is in the
    /// background.
    pub fn ready(&self) -> bool {
        self.parsed
    }

    fn list(&self, cx: &App) -> ListState {
        self.state.read(cx).list_state().clone()
    }

    /// The block at the top and how far into it, when the view is not at
    /// the start, where a rendered document opens anyway.
    pub fn view(&self, cx: &App) -> Option<View> {
        let top = self.list(cx).logical_scroll_top();
        let offset = f32::from(top.offset_in_item);
        (top.item_ix > 0 || offset > 0.5).then_some(View::Rendered {
            block: top.item_ix,
            offset,
        })
    }

    pub fn motion(&mut self, motion: Motion, cx: &mut Context<Self>) {
        let list = self.list(cx);
        let height = f32::from(list.viewport_bounds().size.height);
        let by = |pixels: f32, k: usize| list.scroll_by(px(pixels * k as f32));
        match motion {
            Motion::Down(k) | Motion::ScrollDown(k) => by(LINE, k),
            Motion::Up(k) | Motion::ScrollUp(k) => by(-LINE, k),
            Motion::HalfDown(k) => by(height / 2., k),
            Motion::HalfUp(k) => by(-height / 2., k),
            Motion::PageDown(k) => by(height * 0.9, k),
            Motion::PageUp(k) => by(-height * 0.9, k),
            Motion::Top => list.scroll_to(ListOffset::default()),
            Motion::Bottom => list.scroll_to_end(),
            Motion::Line(k) => list.scroll_to(ListOffset {
                item_ix: k.saturating_sub(1).min(list.item_count().saturating_sub(1)),
                offset_in_item: px(0.),
            }),
            Motion::Left(_) | Motion::Right(_) | Motion::Home => {}
        }
        cx.notify();
    }

    pub fn copy(&self, cx: &mut Context<Self>) {
        let text = self.state.read(cx).selected_text();
        if !text.trim().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| state.select_all(cx));
    }
}

/// The text view's colours, from silent's: warm paper, links in the blue of
/// Word documents' cards, code on the window's darker ground.
fn style(theme: Theme, dark: bool) -> TextViewStyle {
    let code: Hsla = theme.backdrop;
    let link: Hsla = if dark { rgb(0x8eb3dd) } else { rgb(0x3f6a98) }.into();
    TextViewStyle::default()
        .with_foreground(theme.text)
        .with_muted_foreground(theme.muted)
        .with_link(link)
        .with_selection(theme.selection)
        .with_code_background(code)
        .with_border(theme.rule)
        .with_inline_code(HighlightStyle {
            background_color: Some(code),
            ..Default::default()
        })
        .with_table(StyleRefinement::default().bg(theme.surface))
        .with_table_head(StyleRefinement::default().bg(code))
        .with_code_block(
            StyleRefinement::default().font_family(crate::lines::LinesPane::font().family),
        )
        .with_dark(dark)
}

/// A click on a link: the desktop opens it, a relative one beside the file.
pub(crate) fn open(url: &SharedString, dir: &std::path::Path, cx: &mut App) {
    let url = url.as_ref();
    if url.starts_with('#') {
        return;
    }
    if silent::markup::remote(url) {
        return cx.open_url(url);
    }
    if let Some(path) = silent::markup::local_path(url, dir)
        && let Ok(uri) = gio::glib::filename_to_uri(&path, None)
    {
        cx.open_url(&uri);
    }
}

impl Render for RenderedPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self.list(cx);
        if let Some(held) = &mut self.held {
            if zoom::keep(&list, held) {
                self.held = None;
            } else {
                window.request_animation_frame();
            }
        }
        if let Some((block, offset)) = self.restore {
            let list = self.list(cx);
            // The blocks of a long text are parsed in the background.
            if list.item_count() > block {
                self.restore = None;
                list.scroll_to(ListOffset {
                    item_ix: block,
                    offset_in_item: px(offset),
                });
            }
        }
        let theme = Theme::of(window.appearance());
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let (pictures, dir) = (self.pictures.clone(), self.dir.clone());
        // Nothing found for a URL is nothing drawn: the pictures not decoded
        // were put as their words before the text was parsed.
        let nothing = Arc::<std::path::Path>::from(std::path::Path::new(""));
        div()
            .size_full()
            .bg(theme.surface)
            .rounded(crate::view::radius(window))
            .text_color(theme.text)
            .child(
                TextView::new(&self.state)
                    .scrollable(true)
                    .selectable(true)
                    .style(style(theme, dark))
                    .pl(px(MARGIN))
                    .pr(px(MARGIN))
                    .image_source(move |uri| match pictures.get(uri.as_ref()) {
                        Some(picture) => picture.source(),
                        None => nothing.clone().into(),
                    })
                    .on_link_click(move |url, _, _, cx| open(url, &dir, cx)),
            )
    }
}
