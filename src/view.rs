//! The preview window's content: a picture or a card, and the info strip.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use silent::dbus::Direction;
use silent::decode::Event;
use silent::detect::Kind;
use silent::format;

use crate::app::{Input, Intent};
use crate::content::{Body, Loaded, Totals};
use crate::theme::{Theme, accent};
use crate::trace;

/// Corner radius when a compositor draws the transparent corners.
pub const RADIUS: f32 = 11.0;
const STRIP_FADE: Duration = Duration::from_millis(180);
const STRIP_LINGER: Duration = Duration::from_millis(1500);

pub struct Preview {
    loaded: Loaded,
    image: Option<Arc<RenderImage>>,
    totals: Option<Totals>,
    /// Keeps the decoder or the folder walk feeding this view.
    feed: Option<Task<()>>,
    strip: Strip,
    hide: Option<Task<()>>,
    focus: FocusHandle,
    rounded: bool,
    input: async_channel::Sender<Input>,
    traced: bool,
    _appearance: Subscription,
}

#[derive(Default)]
struct Strip {
    shown: bool,
    /// Bumped on every change, so that each fade is a new animation.
    epoch: usize,
}

impl Preview {
    pub fn new(
        loaded: Loaded,
        rounded: bool,
        input: async_channel::Sender<Input>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let appearance = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        let mut preview = Self {
            loaded,
            image: None,
            totals: None,
            feed: None,
            strip: Strip::default(),
            hide: None,
            focus,
            rounded,
            input,
            traced: false,
            _appearance: appearance,
        };
        preview.attach(window, cx);
        preview
    }

    /// Swaps in new content, as when arrow keys move Nautilus's selection.
    pub fn show(&mut self, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        self.loaded = loaded;
        self.traced = false;
        self.attach(window, cx);
    }

    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.feed = None;
        self.totals = None;
        if let Some(old) = self.image.take() {
            cx.drop_image(old, Some(window));
        }
        let mut image = None;
        match &mut self.loaded.body {
            Body::Image(picture) => {
                image = picture.first.take().map(|p| p.image);
                let events = picture.frames.events.clone();
                if image.is_none()
                    && let Ok(Event::Frame(prepared)) = events.try_recv()
                {
                    image = Some(prepared.image);
                }
                self.feed = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = events.recv().await {
                        let Event::Frame(prepared) = event else {
                            continue;
                        };
                        let shown = this.update_in(cx, |this, window, cx| {
                            if let Some(old) = this.image.replace(prepared.image) {
                                cx.drop_image(old, Some(window));
                            }
                            cx.notify();
                        });
                        if shown.is_err() {
                            break;
                        }
                    }
                }));
            }
            Body::Folder(folder) => {
                let totals = folder.totals.clone();
                self.feed = Some(cx.spawn(async move |this, cx| {
                    while let Ok(update) = totals.recv().await {
                        let shown = this.update(cx, |this, cx| {
                            this.totals = Some(update);
                            cx.notify();
                        });
                        if shown.is_err() {
                            break;
                        }
                    }
                }));
            }
            Body::Info => {}
        }
        self.image = image;
        cx.notify();
    }

    fn ready(&self) -> bool {
        !matches!(self.loaded.body, Body::Image(_)) || self.image.is_some()
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() {
            return;
        }
        let intent = match keystroke.key.as_str() {
            "space" | "escape" if !event.is_held => Some(Intent::Close),
            "up" => Some(Intent::Select(Direction::Up)),
            "down" => Some(Intent::Select(Direction::Down)),
            "left" => Some(Intent::Select(Direction::Left)),
            "right" => Some(Intent::Select(Direction::Right)),
            "enter" if !event.is_held => Some(Intent::Open),
            "f2" if !event.is_held => Some(Intent::Rename),
            "delete" if !event.is_held => Some(Intent::Trash),
            "f" if !event.is_held => {
                window.toggle_fullscreen();
                None
            }
            _ => return,
        };
        cx.stop_propagation();
        if let Some(intent) = intent {
            let _ = self.input.try_send(Input::Intent(intent));
        }
    }

    fn pointer_moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_strip(true, cx);
        self.hide = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(STRIP_LINGER).await;
            let _ = this.update(cx, |this, cx| this.set_strip(false, cx));
        }));
    }

    fn set_strip(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.strip.shown != shown {
            self.strip.shown = shown;
            self.strip.epoch += 1;
            cx.notify();
        }
    }

    fn strip_text(&self) -> String {
        let facts = &self.loaded.facts;
        let mut parts = vec![facts.name.clone()];
        match &self.loaded.body {
            Body::Image(picture) => {
                parts.push(format::size(facts.size));
                parts.push(format!("{} × {}", picture.natural.0, picture.natural.1));
            }
            Body::Folder(folder) => {
                if let Some(items) = folder.items {
                    parts.push(format::count(items, "item", "items"));
                }
                if let Some(totals) = self.totals {
                    parts.push(format::size(totals.bytes));
                }
            }
            Body::Info => {
                parts.push(format::size(facts.size));
                parts.push(self.loaded.description.clone());
            }
        }
        parts.join(" · ")
    }

    fn strip(&self, theme: Theme, radius: Pixels) -> AnyElement {
        let strip = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .px(px(14.))
            .py(px(7.))
            .rounded_b(radius)
            .bg(theme.strip)
            .text_color(theme.strip_text)
            .text_size(px(13.))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_middle()
                    .child(self.strip_text()),
            );
        if self.strip.epoch == 0 {
            return strip.opacity(0.).into_any_element();
        }
        let shown = self.strip.shown;
        strip
            .with_animation(
                ("strip", self.strip.epoch),
                Animation::new(STRIP_FADE).with_easing(ease_in_out),
                move |strip, t| strip.opacity(if shown { t } else { 1. - t }),
            )
            .into_any_element()
    }

    fn card(&self, theme: Theme) -> Div {
        let facts = &self.loaded.facts;
        let mut rows: Vec<(&str, String)> = Vec::new();
        let glyph = match &self.loaded.body {
            Body::Folder(folder) => {
                if let Some(items) = folder.items {
                    rows.push(("Contains", format::count(items, "item", "items")));
                }
                rows.push((
                    "Total size",
                    match self.totals {
                        Some(t) if t.done => format::size(t.bytes),
                        Some(t) => format!("{} so far…", format::size(t.bytes)),
                        None => "Counting…".into(),
                    },
                ));
                folder_glyph().into_any_element()
            }
            _ => {
                rows.push(("Size", format::size(facts.size)));
                file_glyph(facts.kind, &facts.name).into_any_element()
            }
        };
        if let Some(modified) = facts.modified {
            rows.push(("Modified", format::date(modified)));
        }
        let table = div()
            .mt(px(14.))
            .flex()
            .flex_col()
            .gap(px(5.))
            .children(rows.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(96.))
                            .flex_none()
                            .text_color(theme.muted)
                            .child(label),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text)
                            .child(value),
                    )
            }));
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(30.))
            .px(px(40.))
            .py(px(32.))
            .bg(theme.surface)
            .text_size(px(13.5))
            .child(glyph)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(19.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .line_clamp(2)
                            .child(facts.name.clone()),
                    )
                    .child(
                        div()
                            .mt(px(3.))
                            .text_color(theme.muted)
                            .truncate()
                            .child(self.loaded.description.clone()),
                    )
                    .child(table),
            )
    }
}

impl Focusable for Preview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window.appearance());
        let radius = if self.rounded && !window.is_fullscreen() {
            px(RADIUS)
        } else {
            px(0.)
        };
        if !self.traced && self.ready() {
            self.traced = true;
            window.on_next_frame(|_, _| trace::mark("first frame"));
        }
        let body = match (&self.loaded.body, &self.image) {
            (Body::Image(_), Some(image)) => img(ImageSource::Render(image.clone()))
                .size_full()
                .object_fit(ObjectFit::Contain)
                .rounded(radius)
                .into_any_element(),
            (Body::Image(_), None) => div().size_full().into_any_element(),
            _ => self.card(theme).rounded(radius).into_any_element(),
        };
        let backdrop = match self.loaded.body {
            Body::Image(_) => theme.backdrop,
            _ => theme.surface,
        };
        div()
            .id("preview")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .font_family("Noto Sans")
            .bg(backdrop)
            .rounded(radius)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_move(
                cx.listener(|this, _: &MouseMoveEvent, window, cx| this.pointer_moved(window, cx)),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.hide = None;
                    this.set_strip(false, cx);
                }
            }))
            .child(body)
            .child(self.strip(theme, radius))
    }
}

/// A page with the file's extension on it, in the colour of its kind.
fn file_glyph(kind: Kind, name: &str) -> Div {
    let color = accent(kind);
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| !ext.is_empty() && ext.len() <= 5)
        .unwrap_or("")
        .to_uppercase();
    div()
        .flex_none()
        .w(px(84.))
        .h(px(106.))
        .rounded(px(9.))
        .bg(color.opacity(0.14))
        .border_1()
        .border_color(color.opacity(0.45))
        .flex()
        .items_end()
        .justify_center()
        .pb(px(15.))
        .text_size(px(15.))
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(extension)
}

fn folder_glyph() -> Div {
    let color = accent(Kind::Folder);
    div()
        .flex_none()
        .relative()
        .w(px(104.))
        .h(px(84.))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(44.))
                .h(px(20.))
                .rounded_t(px(6.))
                .bg(color.opacity(0.75)),
        )
        .child(
            div()
                .absolute()
                .top(px(10.))
                .left_0()
                .w_full()
                .h(px(74.))
                .rounded(px(9.))
                .bg(color),
        )
}
