//! The preview window's content: a picture or a card, and the info strip.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use silent::dbus::Direction;
use silent::decode::Event;
use silent::detect::Kind;
use silent::format;
use silent::resume::{self, Key};

use crate::app::{Input, Intent};
use crate::archive::ArchivePane;
use crate::content::{Body, Loaded, Totals};
use crate::lines::{Doc, LinesPane};
use crate::media::{AudioPane, VideoPane};
use crate::pdf::PdfPane;
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
    pane: Pane,
    _pane_changes: Option<Subscription>,
    /// Keeps the decoder or the folder walk feeding this view.
    feed: Option<Task<()>>,
    strip: Strip,
    hide: Option<Task<()>>,
    focus: FocusHandle,
    rounded: bool,
    shift: bool,
    input: async_channel::Sender<Input>,
    traced: bool,
    _appearance: Subscription,
}

/// Kinds with state of their own get a view of their own.
enum Pane {
    None,
    Video(Entity<VideoPane>),
    Audio(Entity<AudioPane>),
    Pdf(Entity<PdfPane>),
    Lines(Entity<LinesPane>),
    Archive(Entity<ArchivePane>),
}

impl Pane {
    fn view(&self) -> Option<AnyView> {
        Some(match self {
            Pane::None => return None,
            Pane::Video(v) => v.clone().into(),
            Pane::Audio(v) => v.clone().into(),
            Pane::Pdf(v) => v.clone().into(),
            Pane::Lines(v) => v.clone().into(),
            Pane::Archive(v) => v.clone().into(),
        })
    }
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
            pane: Pane::None,
            _pane_changes: None,
            feed: None,
            strip: Strip::default(),
            hide: None,
            focus,
            rounded,
            shift: false,
            input,
            traced: false,
            _appearance: appearance,
        };
        preview.attach(window, cx);
        preview
    }

    /// Swaps in new content, as when arrow keys move Nautilus's selection.
    pub fn show(&mut self, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        self.leave(cx);
        self.loaded = loaded;
        self.traced = false;
        self.attach(window, cx);
    }

    /// Remembers where a video or a sound was left, for coming back to it.
    pub fn leave(&self, cx: &App) {
        let (position, duration) = match &self.pane {
            Pane::Video(v) => {
                let v = v.read(cx);
                (v.position(), v.duration())
            }
            Pane::Audio(a) => {
                let a = a.read(cx);
                (a.position(), a.duration())
            }
            _ => return,
        };
        let facts = &self.loaded.facts;
        if let Some(key) = facts
            .path
            .as_deref()
            .and_then(|path| Key::new(path, facts.size, facts.modified))
        {
            resume::remember(&key, position, duration);
        }
    }

    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.feed = None;
        self.totals = None;
        self.pane = Pane::None;
        self._pane_changes = None;
        if let Some(old) = self.image.take() {
            cx.drop_image(old, Some(window));
        }
        let name: SharedString = self.loaded.facts.name.clone().into();
        self.pane = match std::mem::replace(&mut self.loaded.body, Body::Info) {
            Body::Video(video) => Pane::Video(cx.new(|cx| VideoPane::new(video, window, cx))),
            Body::Audio(audio) => Pane::Audio(cx.new(|cx| AudioPane::new(audio, name, window, cx))),
            Body::Pdf(pdf) => Pane::Pdf(cx.new(|_| PdfPane::new(pdf, window))),
            Body::Text(text) => Pane::Lines(cx.new(|_| LinesPane::new(Doc::Text(text)))),
            Body::Hex(hex) => Pane::Lines(cx.new(|_| LinesPane::new(Doc::Hex(hex)))),
            Body::Archive(archive) => {
                Pane::Archive(cx.new(|cx| ArchivePane::new(archive, window, cx)))
            }
            body => {
                self.loaded.body = body;
                Pane::None
            }
        };
        // The strip shows the playing position and the page in view.
        self._pane_changes = match &self.pane {
            Pane::Video(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Audio(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Pdf(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            _ => None,
        };
        self.shift = false;
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
            _ => {}
        }
        self.image = image;
        cx.notify();
    }

    fn ready(&self, cx: &App) -> bool {
        match &self.pane {
            Pane::Video(video) => video.read(cx).ready(),
            Pane::None => !matches!(self.loaded.body, Body::Image(_)) || self.image.is_some(),
            _ => true,
        }
    }

    /// Keys for whatever the window shows: play and seek media, page through
    /// documents.
    fn pane_key(&self, key: &str, window: &Window, cx: &mut App) -> bool {
        match &self.pane {
            Pane::Video(v) => {
                let v = v.read(cx);
                match key {
                    "k" => v.toggle_pause(),
                    "j" => v.seek(false),
                    "l" => v.seek(true),
                    "m" => v.toggle_mute(),
                    _ => return false,
                }
            }
            Pane::Audio(a) => {
                let a = a.read(cx);
                match key {
                    "k" => a.toggle_pause(),
                    "j" => a.seek(false),
                    "l" => a.seek(true),
                    "m" => a.toggle_mute(),
                    _ => return false,
                }
            }
            Pane::Pdf(p) if matches!(key, "pagedown" | "pageup") => {
                p.read(cx).scroll_by_page(window, key == "pagedown");
                p.update(cx, |_, cx| cx.notify());
            }
            Pane::Lines(t) if matches!(key, "pagedown" | "pageup") => {
                t.update(cx, |t, cx| t.scroll_by_page(key == "pagedown", cx));
            }
            _ => return false,
        }
        true
    }

    /// Shift with a key: seek five seconds, step through pages, jump to the
    /// start or the end.
    fn shift_key(&self, key: &str, cx: &mut App) -> bool {
        match (&self.pane, key) {
            (Pane::Video(v), "left" | "right") => {
                v.update(cx, |v, cx| v.step(key == "right", cx));
            }
            (Pane::Audio(a), "left" | "right") => a.read(cx).step(key == "right"),
            (Pane::Pdf(p), "up" | "down") => p.update(cx, |p, cx| p.step_page(key == "down", cx)),
            (Pane::Pdf(p), "home" | "end") => p.update(cx, |p, cx| p.go_to_end(key == "end", cx)),
            (Pane::Lines(t), "home" | "end") => {
                t.update(cx, |t, cx| t.scroll_to_end(key == "end", cx));
            }
            _ => return false,
        }
        true
    }

    /// Ctrl with a key: select all and copy.
    fn control_key(&self, key: &str, cx: &mut App) -> bool {
        match (&self.pane, key) {
            (Pane::Lines(t), "a") => t.update(cx, |t, cx| t.select_all(cx)),
            (Pane::Lines(t), "c") => t.update(cx, |t, cx| t.copy(cx)),
            (Pane::Pdf(p), "c") => p.update(cx, |p, cx| p.copy(cx)),
            _ => return false,
        }
        true
    }

    fn toggle_pause(&self, cx: &App) {
        match &self.pane {
            Pane::Video(v) => v.read(cx).toggle_pause(),
            Pane::Audio(a) => a.read(cx).toggle_pause(),
            _ => {}
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let key = keystroke.key.as_str();
        let handled = if modifiers.control && !(modifiers.shift || modifiers.alt) {
            self.control_key(key, cx)
        } else if only_shift(modifiers) {
            self.shift_key(key, cx)
        } else {
            false
        };
        if handled {
            cx.stop_propagation();
            return;
        }
        // Shift may still be down from looking at the seek bar or the scrubber.
        let closing = matches!(key, "space" | "escape") && only_shift(modifiers);
        if modifiers.modified() && !closing {
            return;
        }
        let intent = match key {
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
            key if self.pane_key(key, window, cx) => None,
            _ => return,
        };
        cx.stop_propagation();
        if let Some(intent) = intent {
            let _ = self.input.try_send(Input::Intent(intent));
        }
    }

    /// Shift alone shows the seek bar or the scrubber, for as long as it is held.
    fn set_shift(&mut self, held: bool, cx: &mut Context<Self>) {
        if self.shift == held {
            return;
        }
        self.shift = held;
        match &self.pane {
            Pane::Video(v) => v.update(cx, |v, cx| v.set_shift(held, cx)),
            Pane::Pdf(p) => p.update(cx, |p, cx| p.set_shift(held, cx)),
            Pane::Lines(t) => t.update(cx, |t, cx| t.set_shift(held, cx)),
            _ => {}
        }
        cx.notify();
    }

    fn pointer_moved(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Modifier changes reach only the window with the keyboard focus.
        self.set_shift(only_shift(event.modifiers), cx);
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

    /// `38.6 MB`, or `38.6 MB, 158.4 MB unpacked` for a compressed file.
    fn sizes(&self) -> String {
        let size = format::size(self.loaded.facts.size);
        match &self.loaded.packed {
            Some(packed) => match packed.unpacked {
                Some(unpacked) => format!("{size}, {} unpacked", format::size(unpacked)),
                None => format!("{size} packed"),
            },
            None => size,
        }
    }

    fn strip_text(&self, cx: &App) -> String {
        let facts = &self.loaded.facts;
        let mut parts = vec![facts.name.clone(), self.sizes()];
        match &self.pane {
            Pane::Video(v) => parts.push(v.read(cx).status()),
            Pane::Audio(a) => parts.push(a.read(cx).status()),
            Pane::Pdf(p) => {
                let p = p.read(cx);
                parts.push(format!("page {} of {}", p.current_page(), p.page_count()));
            }
            Pane::Lines(t) => {
                if facts.kind == Kind::Binary {
                    parts.push(self.loaded.description.clone());
                }
                parts.extend(t.read(cx).summary());
            }
            Pane::Archive(a) => parts.push(a.read(cx).summary()),
            Pane::None => {}
        }
        if !matches!(self.pane, Pane::None) {
            return parts.join(" · ");
        }
        parts.truncate(1);
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
            _ => {
                parts.push(format::size(facts.size));
                parts.push(self.loaded.description.clone());
            }
        }
        parts.join(" · ")
    }

    fn strip(&self, theme: Theme, radius: Pixels, cx: &App) -> AnyElement {
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
                    .child(self.strip_text(cx)),
            );
        let bar = matches!(&self.pane, Pane::Video(v) if v.read(cx).bar_shown());
        if self.strip.epoch == 0 || bar {
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
        if !self.traced && self.ready(cx) {
            self.traced = true;
            window.on_next_frame(|_, _| trace::mark("first frame"));
        }
        let body = match (&self.loaded.body, &self.image) {
            _ if let Some(pane) = self.pane.view() => div()
                .size_full()
                .rounded(radius)
                .overflow_hidden()
                .child(pane)
                .into_any_element(),
            (Body::Image(_), Some(image)) => img(ImageSource::Render(image.clone()))
                .size_full()
                .object_fit(ObjectFit::Contain)
                .rounded(radius)
                .into_any_element(),
            (Body::Image(_), None) => div().size_full().into_any_element(),
            _ => self.card(theme).rounded(radius).into_any_element(),
        };
        let backdrop = match (&self.pane, &self.loaded.body) {
            (Pane::Video(_), _) => rgb(0x000000).into(),
            (Pane::None, Body::Image(_)) => theme.backdrop,
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
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.set_shift(only_shift(event.modifiers), cx);
            }))
            .on_mouse_move(cx.listener(Self::pointer_moved))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.hide = None;
                    this.set_strip(false, cx);
                }
            }))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_pause(cx)))
            .child(body)
            .child(self.strip(theme, radius, cx))
    }
}

fn only_shift(m: Modifiers) -> bool {
    m.shift && !(m.control || m.alt || m.platform)
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
