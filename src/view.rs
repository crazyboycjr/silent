//! The preview window's content: a picture or a card, and the info strip.
//! A click inside content that has keys of its own engages it: the arrows
//! then act on it instead of browsing Nautilus's files, until the window
//! loses the focus or shows another file.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use silent::decode::Event;
use silent::detect::Kind;
use silent::format;
use silent::memory::{self, Key, View};
use silent::vim::{Keys, Motion};

use crate::app::{Input, Intent};
use crate::archive::ArchivePane;
use crate::content::{Body, Loaded, Totals};
use crate::docview::DocPane;
use crate::grid::SheetPane;
use crate::keys::{self, Act, Mods};
use crate::lines::{Doc, LinesPane};
use crate::media::{AudioPane, Loudness, VideoPane};
use crate::overview::{Docking, Family, Mode};
use crate::pdf::PdfPane;
use crate::slides::SlidesPane;
use crate::theme::{Theme, accent, progress};
use crate::trace;

/// Corner radius when a compositor draws the transparent corners.
pub const RADIUS: f32 = 11.0;
/// How long a dock waits for the window to make room before it docks
/// anyway, as in a window manager that will not resize the window.
const DOCK_WAIT: Duration = Duration::from_millis(400);
const STRIP_FADE: Duration = Duration::from_millis(180);
const STRIP_LINGER: Duration = Duration::from_millis(1500);
/// The video's seek bar, which a note in the strip goes above.
const SEEK_BAR: f32 = 44.;

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
    /// Holds the focus while the content is engaged.
    content: FocusHandle,
    rounded: bool,
    input: async_channel::Sender<Input>,
    traced: bool,
    /// What has been typed of a vim command.
    keys: Keys,
    /// The minimap's and the rail's modes, read when first needed.
    minimap: Option<Mode>,
    rail: Option<Mode>,
    /// Volume and mute, carried from one sound or video to the next.
    loudness: Loudness,
    /// Whether the content was engaged as the mouse went down, so that the
    /// click that engages a video does not pause it too.
    engaged_at_press: bool,
    /// An overview docking or leaving its dock, waiting for the window to
    /// take its new width.
    redock: Option<Redock>,
    /// Whether the window's own size has room for the dock, which leaving
    /// fullscreen puts right if M changed it meanwhile.
    sized_docked: bool,
    was_fullscreen: bool,
    _appearance: Subscription,
    _activation: Subscription,
    _bounds: Subscription,
}

struct Redock {
    docked: bool,
    /// The window's width when the change was asked for.
    width: Pixels,
    _wait: Task<()>,
}

/// Kinds with state of their own get a view of their own.
enum Pane {
    None,
    Video(Entity<VideoPane>),
    Audio(Entity<AudioPane>),
    Pdf(Entity<PdfPane>),
    Lines(Entity<LinesPane>),
    Archive(Entity<ArchivePane>),
    Doc(Entity<DocPane>),
    Sheet(Entity<SheetPane>),
    Slides(Entity<SlidesPane>),
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
            Pane::Doc(v) => v.clone().into(),
            Pane::Sheet(v) => v.clone().into(),
            Pane::Slides(v) => v.clone().into(),
        })
    }

    fn docking(&self, cx: &App) -> Option<Docking> {
        Some(match self {
            Pane::Lines(t) => t.read(cx).docking(),
            Pane::Pdf(p) => p.read(cx).docking(),
            Pane::Slides(s) => s.read(cx).docking(),
            Pane::Doc(d) => d.read(cx).docking(),
            _ => return None,
        })
    }

    fn set_docked(&self, docked: bool, cx: &mut App) {
        match self {
            Pane::Lines(t) => t.update(cx, |t, cx| t.set_docked(docked, cx)),
            Pane::Pdf(p) => p.update(cx, |p, cx| p.set_docked(docked, cx)),
            Pane::Slides(s) => s.update(cx, |s, cx| s.set_docked(docked, cx)),
            Pane::Doc(d) => d.update(cx, |d, cx| d.set_docked(docked, cx)),
            _ => {}
        }
    }

    fn kind(&self) -> keys::Kind {
        match self {
            Pane::None => keys::Kind::Still,
            Pane::Video(_) => keys::Kind::Video,
            Pane::Audio(_) => keys::Kind::Audio,
            Pane::Pdf(_) => keys::Kind::Pdf,
            Pane::Lines(_) => keys::Kind::Lines,
            Pane::Archive(_) => keys::Kind::Archive,
            Pane::Doc(_) => keys::Kind::Doc,
            Pane::Sheet(_) => keys::Kind::Sheet,
            Pane::Slides(_) => keys::Kind::Slides,
        }
    }
}

#[derive(Default)]
struct Strip {
    shown: bool,
    /// Bumped on every change, so that each fade is a new animation.
    epoch: usize,
    /// A word from a key, such as the volume, at the right for a moment.
    note: Option<String>,
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
        // Coming back to the window without a click, the arrows browse again.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            this.disengage(window, cx);
        });
        let bounds = cx.observe_window_bounds(window, Self::bounds_changed);
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
            content: cx.focus_handle(),
            rounded,
            input,
            traced: false,
            keys: Keys::default(),
            minimap: None,
            rail: None,
            loudness: Loudness::default(),
            engaged_at_press: false,
            redock: None,
            sized_docked: false,
            was_fullscreen: false,
            _appearance: appearance,
            _activation: activation,
            _bounds: bounds,
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
        self.disengage(window, cx);
    }

    /// Remembers where the file was left, for coming back to it within the
    /// hour, and the volume for the next sound.
    pub fn leave(&mut self, cx: &App) {
        let view = match &self.pane {
            Pane::Video(v) => {
                let v = v.read(cx);
                self.loudness = v.loudness();
                memory::worth_resuming(v.position(), v.duration())
                    .then(|| View::Media(v.position()))
            }
            Pane::Audio(a) => {
                let a = a.read(cx);
                self.loudness = a.loudness();
                memory::worth_resuming(a.position(), a.duration())
                    .then(|| View::Media(a.position()))
            }
            Pane::Pdf(p) => p.read(cx).view(),
            Pane::Slides(s) => s.read(cx).view(),
            Pane::Lines(t) => t.read(cx).view(),
            Pane::Doc(d) => d.read(cx).view(),
            Pane::Sheet(s) => s.read(cx).view(),
            Pane::Archive(a) => a.read(cx).view(cx),
            Pane::None => return,
        };
        let facts = &self.loaded.facts;
        if let Some(key) = facts
            .path
            .as_deref()
            .and_then(|path| Key::new(path, facts.size, facts.modified))
        {
            memory::remember(&key, view);
        }
    }

    fn mode(&mut self, family: Family) -> Mode {
        let slot = match family {
            Family::Minimap => &mut self.minimap,
            Family::Rail => &mut self.rail,
        };
        *slot.get_or_insert_with(|| Mode::load(family))
    }

    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.feed = None;
        self.totals = None;
        self.pane = Pane::None;
        self._pane_changes = None;
        self.keys = Keys::default();
        self.strip.note = None;
        self.redock = None;
        if let Some(old) = self.image.take() {
            cx.drop_image(old, Some(window));
        }
        let name: SharedString = self.loaded.facts.name.clone().into();
        let view = self.loaded.view.take();
        let loudness = self.loudness;
        let docked = self.loaded.dock > 0.;
        self.sized_docked = docked;
        self.pane = match std::mem::replace(&mut self.loaded.body, Body::Info) {
            Body::Video(video) => {
                Pane::Video(cx.new(|cx| VideoPane::new(video, loudness, window, cx)))
            }
            Body::Audio(audio) => {
                Pane::Audio(cx.new(|cx| AudioPane::new(audio, name, loudness, window, cx)))
            }
            Body::Pdf(pdf) => {
                let mode = self.mode(Family::Rail);
                Pane::Pdf(cx.new(|_| PdfPane::new(pdf, mode, docked, view, window)))
            }
            Body::Text(text) => {
                let mode = self.mode(Family::Minimap);
                Pane::Lines(cx.new(|_| LinesPane::new(Doc::Text(text), mode, docked, view)))
            }
            Body::Hex(hex) => {
                let mode = self.mode(Family::Minimap);
                Pane::Lines(cx.new(|_| LinesPane::new(Doc::Hex(hex), mode, docked, view)))
            }
            Body::Document(paper) => {
                let mode = self.mode(Family::Rail);
                Pane::Doc(cx.new(|_| DocPane::new(*paper, mode, docked, view, window)))
            }
            Body::Sheet(sheet) => Pane::Sheet(cx.new(|_| SheetPane::new(*sheet, view))),
            Body::Deck(deck) => {
                let mode = self.mode(Family::Rail);
                Pane::Slides(cx.new(|_| SlidesPane::new(*deck, mode, docked, view, window)))
            }
            Body::Archive(archive) => {
                Pane::Archive(cx.new(|cx| ArchivePane::new(archive, view, window, cx)))
            }
            body => {
                self.loaded.body = body;
                Pane::None
            }
        };
        // The strip shows the playing position, the page in view, the cell.
        self._pane_changes = match &self.pane {
            Pane::Video(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Audio(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Pdf(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Sheet(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Slides(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            _ => None,
        };
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
            Body::Still(still) => image = Some(still.image.clone()),
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

    /// Whether a click has given the content the keys.
    fn engaged(&self, window: &Window, cx: &App) -> bool {
        self.pane.kind().engages() && self.content.contains_focused(window, cx)
    }

    /// The arrows browse files again.
    fn disengage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn intent(&self, intent: Intent) {
        let _ = self.input.try_send(Input::Intent(intent));
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = keystroke.modifiers;
        // Alt and Super belong to the window manager.
        if m.alt || m.platform || m.function {
            return;
        }
        let engaged = self.engaged(window, cx);
        let mods = Mods {
            ctrl: m.control,
            shift: m.shift,
        };
        let act = keys::route(
            self.pane.kind(),
            engaged,
            &keystroke.key,
            mods,
            event.is_held,
            &mut self.keys,
        );
        if act == Act::Pass {
            return;
        }
        cx.stop_propagation();
        // A note answers the key before; this key may bring its own.
        self.strip.note = None;
        let pending = matches!(act, Act::Pending(_));
        self.act(act, window, cx);
        if pending {
            // What is typed stays in the strip until the command is done.
            self.hide = None;
            self.set_strip(true, cx);
        } else if self.strip.shown && self.hide.is_none() {
            self.linger_strip(window, cx);
        }
        cx.notify();
    }

    fn act(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Pass | Act::Nothing | Act::Pending(_) | Act::Cancelled => {}
            Act::Close => self.intent(Intent::Close),
            Act::Select(direction) => self.intent(Intent::Select(direction)),
            Act::Open => self.intent(Intent::Open),
            Act::Rename => self.intent(Intent::Rename),
            Act::Trash => self.intent(Intent::Trash),
            Act::Fullscreen => window.toggle_fullscreen(),
            Act::Move(motion) => self.move_pane(motion, window, cx),
            Act::Sideways(right) => {
                if let Pane::Lines(t) = &self.pane {
                    t.update(cx, |t, cx| t.sideways(right, cx));
                }
            }
            Act::Overview => self.cycle_overview(window, cx),
            Act::SelectAll => {
                if let Pane::Lines(t) = &self.pane {
                    t.update(cx, |t, cx| t.select_all(cx));
                }
            }
            Act::Copy => match &self.pane {
                Pane::Lines(t) => t.update(cx, |t, cx| t.copy(cx)),
                Pane::Pdf(p) => p.update(cx, |p, cx| p.copy(cx)),
                _ => {}
            },
            Act::Sheet(forward) => {
                if let Pane::Sheet(s) = &self.pane {
                    s.update(cx, |s, cx| s.step_sheet(forward, cx));
                }
            }
            Act::Cursor(columns, rows) => {
                if let Pane::Sheet(s) = &self.pane {
                    s.update(cx, |s, cx| s.move_cursor(columns, rows, cx));
                }
            }
            Act::Pause => {
                self.toggle_pause(cx);
                self.note(None, window, cx);
            }
            Act::Mute => {
                match &self.pane {
                    Pane::Video(v) => v.read(cx).toggle_mute(),
                    Pane::Audio(a) => a.read(cx).toggle_mute(),
                    _ => {}
                }
                self.note(None, window, cx);
            }
            Act::Seek {
                forward,
                by,
                accurate,
            } => match &self.pane {
                Pane::Video(v) => v.update(cx, |v, cx| v.seek(forward, by, accurate, cx)),
                Pane::Audio(a) => {
                    a.read(cx).seek(forward, by, accurate);
                    self.note(None, window, cx);
                }
                _ => {}
            },
            Act::Volume(louder) => {
                let percent = match &self.pane {
                    Pane::Video(v) => v.read(cx).louder(louder),
                    Pane::Audio(a) => a.read(cx).louder(louder),
                    _ => return,
                };
                self.note(Some(format!("volume {percent}%")), window, cx);
            }
        }
    }

    fn move_pane(&mut self, motion: Motion, window: &mut Window, cx: &mut Context<Self>) {
        match &self.pane {
            Pane::Lines(t) => t.update(cx, |t, cx| t.motion(motion, cx)),
            Pane::Doc(d) => d.update(cx, |d, cx| d.motion(motion, window, cx)),
            Pane::Pdf(p) => p.update(cx, |p, cx| p.motion(motion, cx)),
            Pane::Slides(s) => s.update(cx, |s, cx| s.motion(motion, cx)),
            Pane::Sheet(s) => s.update(cx, |s, cx| s.motion(motion, cx)),
            _ => {}
        }
    }

    /// M: the minimap, the rail or the miniature goes to its next mode,
    /// which is kept for the next file and the next preview, and the window
    /// makes room for it or gives the room back.
    fn cycle_overview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (family, what) = match &self.pane {
            Pane::Lines(_) => (Family::Minimap, "Minimap"),
            Pane::Pdf(_) | Pane::Slides(_) => (Family::Rail, "Thumbnails"),
            Pane::Doc(_) => (Family::Rail, "Overview"),
            _ => return,
        };
        let mode = self.mode(family).next(family);
        match family {
            Family::Minimap => self.minimap = Some(mode),
            Family::Rail => self.rail = Some(mode),
        }
        mode.save(family);
        match &self.pane {
            Pane::Lines(t) => t.update(cx, |t, cx| t.set_mode(mode, cx)),
            Pane::Pdf(p) => p.update(cx, |p, cx| p.set_mode(mode, cx)),
            Pane::Slides(s) => s.update(cx, |s, cx| s.set_mode(mode, cx)),
            Pane::Doc(d) => d.update(cx, |d, cx| d.set_mode(mode, cx)),
            _ => {}
        }
        self.redock(window, cx);
        self.note(Some(mode.describe(what)), window, cx);
    }

    /// Docks the overview or takes it away, as its mode now asks: the
    /// window grows or shrinks at its right first, so that the content
    /// keeps its size and place, and the dock follows once it has.
    fn redock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.redock = None;
        let Some(docking) = self.pane.docking(cx) else {
            return;
        };
        if docking.wanted == docking.docked {
            return;
        }
        if window.is_fullscreen() {
            self.pane.set_docked(docking.wanted, cx);
            return;
        }
        let by = if docking.wanted {
            docking.width
        } else {
            -docking.width
        };
        self.intent(Intent::Redock(by));
        let wait = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DOCK_WAIT).await;
            let _ = this.update(cx, |this, cx| this.settle(cx));
        });
        self.redock = Some(Redock {
            docked: docking.wanted,
            width: window.viewport_size().width,
            _wait: wait,
        });
    }

    fn settle(&mut self, cx: &mut Context<Self>) {
        if let Some(redock) = self.redock.take() {
            self.pane.set_docked(redock.docked, cx);
            self.sized_docked = redock.docked;
        }
    }

    fn bounds_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = window.viewport_size().width;
        if self.redock.as_ref().is_some_and(|r| r.width != width) {
            self.settle(cx);
        }
        // Back from fullscreen in a window sized before M changed the dock.
        let fullscreen = window.is_fullscreen();
        if self.was_fullscreen
            && !fullscreen
            && let Some(docking) = self.pane.docking(cx)
            && docking.docked != self.sized_docked
        {
            let by = if docking.docked {
                docking.width
            } else {
                -docking.width
            };
            self.intent(Intent::Redock(by));
            self.sized_docked = docking.docked;
        }
        self.was_fullscreen = fullscreen;
    }

    fn toggle_pause(&self, cx: &App) {
        match &self.pane {
            Pane::Video(v) => v.read(cx).toggle_pause(),
            Pane::Audio(a) => a.read(cx).toggle_pause(),
            _ => {}
        }
    }

    fn pointer_moved(&mut self, _: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.strip.shown {
            // A note faded out with the strip; it is not shown again.
            self.strip.note = None;
        }
        self.set_strip(true, cx);
        self.linger_strip(window, cx);
    }

    /// Shows the strip for a moment, with `note` at its right.
    fn note(&mut self, note: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.strip.note = note;
        self.set_strip(true, cx);
        self.linger_strip(window, cx);
    }

    /// Hides the strip 1.5 s from now, unless a vim command is unfinished.
    fn linger_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(STRIP_LINGER).await;
            let _ = this.update(cx, |this, cx| {
                if this.keys.pending().is_none() {
                    this.set_strip(false, cx);
                }
            });
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
            Pane::Doc(_) => parts.push(self.loaded.description.clone()),
            Pane::Sheet(s) => parts.push(s.read(cx).summary()),
            Pane::Slides(s) => {
                let s = s.read(cx);
                parts.push(format!(
                    "slide {} of {}",
                    s.current_slide(),
                    s.slide_count()
                ));
            }
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
            Body::Still(_) => {
                parts.push(format::size(facts.size));
                parts.push(self.loaded.description.clone());
                parts.push("its preview picture".into());
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

    /// Where the scrollbar has the view, while it is held or pointed at.
    fn scrub_label(&self, cx: &App) -> Option<String> {
        match &self.pane {
            Pane::Lines(t) => t.read(cx).scrub_label(),
            Pane::Doc(d) => d.read(cx).scrub_label(),
            _ => None,
        }
    }

    fn strip(&self, theme: Theme, radius: Pixels, cx: &App) -> AnyElement {
        let bar = matches!(&self.pane, Pane::Video(v) if v.read(cx).bar_shown());
        // Above a workbook's sheet tabs, which are clicked where it would be,
        // and with a note above the seek bar.
        let lift = match &self.pane {
            Pane::Sheet(s) if s.read(cx).has_tabs() => px(crate::grid::TABS),
            Pane::Video(_) if bar => px(SEEK_BAR),
            _ => px(0.),
        };
        // What is typed of a vim command, or a word from a key.
        let aside = self
            .keys
            .pending()
            .or_else(|| self.strip.note.clone())
            .or_else(|| self.scrub_label(cx));
        let strip = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(lift)
            .px(px(14.))
            .py(px(7.))
            .when(lift == px(0.), |strip| strip.rounded_b(radius))
            .bg(theme.strip)
            .text_color(theme.strip_text)
            .text_size(px(13.))
            .flex()
            .flex_row()
            .gap(px(16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_middle()
                    .child(self.strip_text(cx)),
            )
            .children(aside.map(|aside| {
                div()
                    .flex_none()
                    .font_family("JetBrains Mono")
                    .text_color(progress())
                    .child(aside)
            }));
        let hidden = bar && self.strip.note.is_none() && self.keys.pending().is_none();
        if self.strip.epoch == 0 || hidden {
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
        let engaged = self.engaged(window, cx);
        let body = match (&self.loaded.body, &self.image) {
            _ if let Some(pane) = self.pane.view() => div()
                .id("content")
                .size_full()
                .rounded(radius)
                .overflow_hidden()
                .when(self.pane.kind().engages(), |body| {
                    body.track_focus(&self.content)
                })
                .child(pane)
                .into_any_element(),
            (Body::Image(_) | Body::Still(_), Some(image)) => {
                img(ImageSource::Render(image.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .rounded(radius)
                    .into_any_element()
            }
            (Body::Image(_), None) => div().size_full().into_any_element(),
            _ => self.card(theme).rounded(radius).into_any_element(),
        };
        let backdrop = match (&self.pane, &self.loaded.body) {
            (Pane::Video(_), _) => rgb(0x000000).into(),
            (Pane::None, Body::Image(_) | Body::Still(_)) => theme.backdrop,
            _ => theme.surface,
        };
        // Engaged content wears a thin warm ring, inside the window's edge.
        let ring = engaged.then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .rounded(radius)
                .border_2()
                .border_color(progress().opacity(0.55))
        });
        div()
            .id("preview")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .font_family("Noto Sans")
            .bg(backdrop)
            .rounded(radius)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_move(cx.listener(Self::pointer_moved))
            // Where the pointer is, not whether a key came last: by default
            // a key ends the hover, and the strip it showed would go with it.
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.hide = None;
                    this.set_strip(false, cx);
                }
            }))
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.engaged_at_press = this.engaged(window, cx);
            }))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                // The click that engages a video or a sound only engages it.
                if this.engaged_at_press {
                    this.toggle_pause(cx);
                }
            }))
            .child(body)
            .children(ring)
            .child(self.strip(theme, radius, cx))
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
