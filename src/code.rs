//! Text and code: a list of lines that renders only what is in view,
//! highlighted by gpui-component's tree-sitter highlighter.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::*;

use crate::content::Text;
use crate::theme::Theme;

pub struct TextPane {
    text: Rc<Text>,
    list: ListState,
}

impl TextPane {
    pub fn new(text: Text) -> Self {
        let list = ListState::new(text.lines.len(), ListAlignment::Top, px(600.));
        Self {
            text: Rc::new(text),
            list,
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.lines.len()
    }

    pub fn truncated(&self) -> bool {
        self.text.truncated
    }

    pub fn language(&self) -> Option<&'static str> {
        self.text.language
    }

    pub fn scroll_by_page(&self, window: &Window, down: bool) {
        let height = window.viewport_size().height * 0.9;
        self.list.scroll_by(if down { height } else { -height });
    }
}

impl Render for TextPane {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let appearance = window.appearance();
        let theme = Theme::of(appearance);
        let highlight: Arc<HighlightTheme> = match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => {
                HighlightTheme::default_dark()
            }
            _ => HighlightTheme::default_light(),
        };
        let text = self.text.clone();
        let digits = text.lines.len().to_string().len() as f32;
        let gutter = px(digits * 8.4 + 22.);
        let rows = list(self.list.clone(), move |ix, _, _| {
            let range = text.lines[ix].clone();
            let line: SharedString = text.text[range.clone()].to_owned().into();
            let styled = match &text.highlighter {
                Some(highlighter) if !range.is_empty() => {
                    let styles = highlighter
                        .styles(&range, highlight.as_ref())
                        .into_iter()
                        .filter(|(r, _)| r.start >= range.start && r.end <= range.end)
                        .map(|(r, style)| (r.start - range.start..r.end - range.start, style));
                    StyledText::new(line).with_highlights(styles)
                }
                _ => StyledText::new(line),
            };
            div()
                .flex()
                .flex_row()
                .child(
                    div()
                        .flex_none()
                        .w(gutter)
                        .pr(px(14.))
                        .flex()
                        .justify_end()
                        .text_color(theme.muted.opacity(0.7))
                        .child((ix + 1).to_string()),
                )
                .child(div().flex_1().min_w_0().pr(px(18.)).child(styled))
                .into_any_element()
        })
        .size_full();
        div()
            .size_full()
            .pt(px(14.))
            .bg(theme.surface)
            .text_color(theme.text)
            .font_family("JetBrains Mono")
            .font_features(FontFeatures::disable_ligatures())
            .text_size(px(12.5))
            .line_height(px(20.))
            .child(rows)
    }
}
