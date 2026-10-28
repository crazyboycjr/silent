//! The preview window: the content, a picture or a card, and over its
//! bottom the strip of facts, which a video's or a sound's playbar joins.
//! The strip shows while the pointer moves, always, or never, as i cycles.
//! A click inside content that has keys of its own engages it: the arrows
//! then act on it instead of browsing Nautilus's files, until the window
//! loses the focus or shows another file. A file inside an archive is
//! taken out and shown in the same window, over the archive; Space takes
//! it off again, and Esc closes them all.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use silent::decode::Event;
use silent::detect::Kind;
use silent::extract::{self, Extracted, Places, Refusal};
use silent::format;
use silent::geometry::Screen;
use silent::markup::Markup;
use silent::memory::{self, Key, View};
use silent::scrim::Scrim;
use silent::vim::{Keys, Motion};

use crate::app::{Input, Intent};
use crate::archive::{self, ArchivePane};
use crate::content::{self, Body, Loaded, Totals};
use crate::docview::{self, DocPane};
use crate::grid::{self, SheetPane};
use crate::help;
use crate::keys::{self, Act, Mods};
use crate::lines::{Doc, LinesPane};
use crate::media::{self, AudioPane, Loudness, Player, VideoPane};
use crate::overview::{Docking, Family, Mode};
use crate::pdf::PdfPane;
use crate::rendered::RenderedPane;
use crate::slides::SlidesPane;
use crate::strip::{self, Mode as StripMode};
use crate::theme::{Theme, accent};
use crate::trace;
use crate::zoom::Zoom;

/// Corner radius when a compositor draws the transparent corners.
pub const RADIUS: f32 = 11.0;
const STRIP_FADE: Duration = Duration::from_millis(180);
/// How long the strip stays once the pointer rests; a playbar goes sooner.
const STRIP_LINGER: Duration = Duration::from_millis(1500);
const PLAYBAR_LINGER: Duration = Duration::from_millis(1000);
/// How long a word from a key shows while the strip is off.
const NOTICE: Duration = Duration::from_millis(1000);
/// How long a dock waits for the window to make room before it docks
/// anyway, as in a window manager that will not resize the window.
const DOCK_WAIT: Duration = Duration::from_millis(400);
/// An entry taken out of an archive shows its progress after this long.
const SLOW: Duration = Duration::from_millis(150);

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
    /// Whether the first frame of the content is marked, or has none to
    /// mark: the blank a withdrawn window holds is never on the screen.
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
    /// The monitor and scale, for files taken out of archives.
    screen: Screen,
    /// The archives left for a file inside them, the outermost first.
    levels: Vec<Level>,
    /// The file shown, when it was taken out of an archive.
    inside: Option<Extracted>,
    /// An entry being taken out.
    opening: Option<Opening>,
    /// The file shown rendered or as its source, on its way after `s`.
    switching: Option<Task<()>>,
    /// The card of keys over the content.
    help: bool,
    /// Nautilus's own preview, whose arrows, F2 and Delete act on its files.
    nautilus: bool,
    /// The text size, 1 at 100%, and the wheel's turn with Ctrl held that
    /// has not made a step yet.
    zoom: f32,
    wheel: f32,
    _archive: Option<Subscription>,
    _appearance: Subscription,
    _activation: Subscription,
    _bounds: Subscription,
}

/// An archive left for a file inside it, kept as it was for Space.
struct Level {
    loaded: Loaded,
    pane: Pane,
    /// The file this archive was itself taken out to, when it was inside
    /// another.
    file: Option<Extracted>,
}

/// An entry being taken out of the archive in view.
struct Opening {
    name: String,
    size: u64,
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    since: Instant,
    _work: Task<()>,
    _tick: Task<()>,
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
    Rendered(Entity<RenderedPane>),
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
            Pane::Rendered(v) => v.clone().into(),
            Pane::Archive(v) => v.clone().into(),
            Pane::Doc(v) => v.clone().into(),
            Pane::Sheet(v) => v.clone().into(),
            Pane::Slides(v) => v.clone().into(),
        })
    }

    fn kind(&self) -> keys::Kind {
        match self {
            Pane::None => keys::Kind::Still,
            Pane::Video(_) => keys::Kind::Video,
            Pane::Audio(_) => keys::Kind::Audio,
            Pane::Pdf(_) => keys::Kind::Pdf,
            Pane::Lines(_) => keys::Kind::Lines,
            Pane::Rendered(_) => keys::Kind::Rendered,
            Pane::Archive(_) => keys::Kind::Archive,
            Pane::Doc(_) => keys::Kind::Doc,
            Pane::Sheet(_) => keys::Kind::Sheet,
            Pane::Slides(_) => keys::Kind::Slides,
        }
    }

    fn media(&self) -> bool {
        matches!(self, Pane::Video(_) | Pane::Audio(_))
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

    /// Gives the pictures the pane drew back to the window's GPU atlas, as
    /// the pane goes: the window outlives it, and with it the atlas.
    fn drop_pictures(&self, window: &mut Window, cx: &mut App) {
        match self {
            Pane::Video(v) => v.update(cx, |v, cx| v.drop_pictures(window, cx)),
            Pane::Audio(a) => a.update(cx, |a, cx| a.drop_pictures(window, cx)),
            Pane::Pdf(p) => p.update(cx, |p, cx| p.drop_pictures(window, cx)),
            Pane::Doc(d) => d.update(cx, |d, cx| d.drop_pictures(window, cx)),
            Pane::Slides(s) => s.update(cx, |s, cx| s.drop_pictures(window, cx)),
            Pane::Rendered(r) => r.update(cx, |r, cx| r.drop_pictures(window, cx)),
            Pane::None | Pane::Lines(_) | Pane::Archive(_) | Pane::Sheet(_) => {}
        }
    }
}

#[derive(Default)]
struct Strip {
    mode: StripMode,
    /// Shown while the strip is in its auto mode.
    shown: bool,
    /// Bumped on every change, so that each fade is a new animation.
    epoch: usize,
    /// A word from a key, such as the volume, at the right for a moment.
    note: Option<String>,
    /// The pointer is on the strip, which keeps it.
    hovered: bool,
    /// The veil worked out for a picture, by the picture and the window's
    /// size.
    veil: Option<(ImageId, Size<Pixels>, Scrim)>,
    /// While the strip is off, a word from a key for a second; the last
    /// one stays to fade out.
    notice: Option<String>,
    notice_shown: bool,
    notice_epoch: usize,
    notice_timer: Option<Task<()>>,
}

impl Preview {
    pub fn new(
        loaded: Loaded,
        screen: Screen,
        rounded: bool,
        input: async_channel::Sender<Input>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let appearance = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        // Leaving the window, the arrows browse again; the click that brings
        // the keyboard back may engage the content again.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.disengage(window, cx);
            }
        });
        let bounds = cx.observe_window_bounds(window, Self::bounds_changed);
        let traced = loaded.is_blank();
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
            traced,
            keys: Keys::default(),
            minimap: None,
            rail: None,
            loudness: Loudness::default(),
            engaged_at_press: false,
            redock: None,
            sized_docked: false,
            was_fullscreen: false,
            screen,
            levels: Vec::new(),
            inside: None,
            opening: None,
            switching: None,
            help: false,
            nautilus: false,
            zoom: crate::zoom::level(),
            wheel: 0.,
            _archive: None,
            _appearance: appearance,
            _activation: activation,
            _bounds: bounds,
        };
        preview.attach(window, cx);
        trace::mark("preview built");
        preview
    }

    /// Swaps in new content, as when arrow keys move Nautilus's selection.
    pub fn show(&mut self, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        self.leave(cx);
        self.loaded = loaded;
        self.traced = self.loaded.is_blank();
        window.set_window_title(&self.loaded.facts.name);
        self.attach(window, cx);
        self.disengage(window, cx);
    }

    /// Shows `loaded` in the window shown anew, on `screen`, as a new window
    /// would show it: the strip waits for the pointer to move.
    pub fn reopen(
        &mut self,
        loaded: Loaded,
        screen: Screen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.screen = screen;
        self.strip = Strip::default();
        self.hide = None;
        self.was_fullscreen = false;
        self.help = false;
        self.show(loaded, window, cx);
    }

    pub fn set_nautilus(&mut self, nautilus: bool) {
        self.nautilus = nautilus;
    }

    /// Lets the file go, remembering where it was left, before the window
    /// is withdrawn to wait for the next: sound and video stop, and what
    /// was taken out of archives goes.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show(Loaded::blank(self.screen), window, cx);
    }

    /// Remembers where the file was left, for coming back to it within the
    /// hour, and the volume for the next sound. Inside an archive, that is
    /// how the outermost archive was left; what was taken out goes.
    pub fn leave(&mut self, cx: &App) {
        self.keep_loudness(cx);
        let (loaded, pane) = match self.levels.first() {
            Some(outermost) => (&outermost.loaded, &outermost.pane),
            None => (&self.loaded, &self.pane),
        };
        let facts = &loaded.facts;
        if !matches!(pane, Pane::None)
            && let Some(key) = facts
                .path
                .as_deref()
                .and_then(|path| Key::new(path, facts.size, facts.modified))
        {
            // Markdown and HTML left as their source open so again.
            let source = loaded.markup.is_some() && matches!(pane, Pane::Lines(_));
            let view =
                view_of(pane, cx).or_else(|| source.then_some(View::Lines { top: 0., side: 0. }));
            memory::remember(&key, view);
        }
        self.switching = None;
        if let Some(opening) = self.opening.take() {
            opening.cancel.store(true, Ordering::Relaxed);
        }
        self.levels.clear();
        self.inside = None;
    }

    /// The volume and mute of the sound or video shown, for the next.
    fn keep_loudness(&mut self, cx: &App) {
        match &self.pane {
            Pane::Video(v) => self.loudness = v.read(cx).loudness(),
            Pane::Audio(a) => self.loudness = a.read(cx).loudness(),
            _ => {}
        }
    }

    /// Takes the files the archive in view asks to look inside.
    fn watch_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._archive = match &self.pane {
            Pane::Archive(a) => Some(cx.subscribe_in(
                a,
                window,
                |this, _, archive::Open(entry, size), window, cx| {
                    this.take_out(entry.clone(), *size, window, cx);
                },
            )),
            _ => None,
        };
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
        self.pane.drop_pictures(window, cx);
        self.pane = Pane::None;
        self._pane_changes = None;
        self.keys = Keys::default();
        self.strip.note = None;
        self.strip.veil = None;
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
                let turns = self.loaded.turns;
                Pane::Video(cx.new(|cx| VideoPane::new(video, loudness, turns, window, cx)))
            }
            Body::Audio(audio) => {
                Pane::Audio(cx.new(|cx| AudioPane::new(audio, name, loudness, window, cx)))
            }
            Body::Pdf(pdf) => {
                let mode = self.mode(Family::Rail);
                Pane::Pdf(cx.new(|_| PdfPane::new(pdf, mode, docked, view, window)))
            }
            Body::Text(text) => {
                let (mode, zoom) = (self.mode(Family::Minimap), self.zoom);
                let doc = Doc::Text(text);
                Pane::Lines(cx.new(|_| LinesPane::new(doc, mode, docked, view, zoom)))
            }
            Body::Hex(hex) => {
                let (mode, zoom) = (self.mode(Family::Minimap), self.zoom);
                let doc = Doc::Hex(hex);
                Pane::Lines(cx.new(|_| LinesPane::new(doc, mode, docked, view, zoom)))
            }
            Body::Rendered(doc) => {
                Pane::Rendered(cx.new(|cx| RenderedPane::new(*doc, view, window, cx)))
            }
            Body::Document(paper) => {
                let (mode, zoom) = (self.mode(Family::Rail), self.zoom);
                Pane::Doc(cx.new(|_| DocPane::new(*paper, mode, docked, view, zoom, window)))
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
        self.strip.mode = StripMode::load(self.family());
        // The strip shows the playing position, the page in view, the cell.
        self._pane_changes = match &self.pane {
            Pane::Video(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Audio(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Pdf(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Sheet(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            Pane::Slides(v) => Some(cx.observe(v, |_, _, cx| cx.notify())),
            // A text parsed in the background is in: a window held back for
            // it can show.
            Pane::Rendered(v) => Some(cx.observe(v, |this, _, cx| {
                if !this.traced && !this.parsing(cx) {
                    this.intent(Intent::Ready);
                }
                cx.notify();
            })),
            _ => None,
        };
        self.watch_archive(window, cx);
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

    /// The family of files whose strip mode i sets together.
    fn family(&self) -> &'static str {
        match &self.pane {
            Pane::Video(_) | Pane::Audio(_) => "media",
            Pane::Lines(_) | Pane::Rendered(_) => "text",
            Pane::Pdf(_) | Pane::Slides(_) | Pane::Doc(_) => "pages",
            Pane::Sheet(_) => "sheet",
            Pane::Archive(_) => "archive",
            Pane::None => match self.loaded.body {
                Body::Image(_) | Body::Still(_) => "picture",
                _ => "card",
            },
        }
    }

    /// What the card of keys is for.
    fn help_shown(&self, cx: &App) -> help::Shown {
        use help::Subject;
        let subject = match &self.pane {
            Pane::Lines(t) if t.read(cx).is_hex() => Subject::Hexdump,
            Pane::Lines(_) => self.loaded.markup.map_or(Subject::Text, Subject::Source),
            Pane::Rendered(_) => {
                Subject::Rendered(self.loaded.markup.unwrap_or(Markup::Markdown))
            }
            Pane::Pdf(_) => Subject::Pdf,
            Pane::Slides(_) => Subject::Slides,
            Pane::Doc(_) => Subject::Document,
            Pane::Sheet(s) => Subject::Sheet {
                tabs: s.read(cx).has_tabs(),
            },
            Pane::Archive(_) => Subject::Archive,
            Pane::Video(_) => Subject::Video,
            Pane::Audio(_) => Subject::Audio,
            Pane::None => match self.loaded.body {
                Body::Image(_) => Subject::Picture,
                Body::Folder(_) => Subject::Folder,
                _ => Subject::Card,
            },
        };
        help::Shown {
            subject,
            inside: !self.levels.is_empty(),
            nautilus: self.nautilus,
        }
    }

    /// Whether a long Markdown or HTML text is still being parsed, in the
    /// background, before it can be drawn.
    pub fn parsing(&self, cx: &App) -> bool {
        matches!(&self.pane, Pane::Rendered(r) if !r.read(cx).ready())
    }

    fn ready(&self, cx: &App) -> bool {
        match &self.pane {
            Pane::Video(video) => video.read(cx).ready(),
            Pane::Rendered(r) => r.read(cx).ready(),
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
        // Esc takes the help away first, as it drops a half-typed command.
        if self.help && keystroke.key == "escape" {
            self.help = false;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // Space stops an entry being taken out, and the archive stays.
        if keystroke.key == "space"
            && !m.control
            && let Some(opening) = self.opening.take()
        {
            opening.cancel.store(true, Ordering::Relaxed);
            cx.stop_propagation();
            cx.notify();
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
            match self.strip.mode {
                StripMode::Off => cx.notify(),
                _ => {
                    self.hide = None;
                    self.set_strip(true, cx);
                }
            }
        } else if self.strip.shown && self.hide.is_none() {
            self.linger_strip(window, cx);
        }
        cx.notify();
    }

    fn act(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Pass | Act::Nothing | Act::Pending(_) | Act::Cancelled => {}
            Act::Close => self.intent(Intent::Close),
            Act::Off => match keys::off(self.levels.len()) {
                keys::Off::Back => self.back(window, cx),
                keys::Off::Close => self.intent(Intent::Close),
            },
            Act::Select(direction) => self.intent(Intent::Select(direction)),
            // These act on the file in Nautilus, the outermost archive.
            Act::Open | Act::Rename | Act::Trash if self.inside.is_some() => {}
            Act::Open => self.intent(Intent::Open),
            Act::Rename => self.intent(Intent::Rename),
            Act::Trash => self.intent(Intent::Trash),
            Act::Inside(otherwise) => {
                if !self.look_inside(window, cx) {
                    self.act(*otherwise, window, cx);
                }
            }
            Act::Fullscreen => window.toggle_fullscreen(),
            Act::Move(motion) => self.move_pane(motion, window, cx),
            Act::Sideways(right) => {
                if let Pane::Lines(t) = &self.pane {
                    t.update(cx, |t, cx| t.sideways(right, cx));
                }
            }
            Act::Overview => self.cycle_overview(window, cx),
            Act::Strip => self.cycle_strip(window, cx),
            Act::Wrap => self.toggle_wrap(window, cx),
            Act::Source => self.toggle_source(window, cx),
            Act::Help => self.help = !self.help,
            Act::Turn(clockwise) => self.turn(clockwise, window, cx),
            Act::Zoom(zoom) => self.zoom_text(zoom, None, window, cx),
            Act::SelectAll => match &self.pane {
                Pane::Lines(t) => t.update(cx, |t, cx| t.select_all(cx)),
                Pane::Rendered(r) => r.update(cx, |r, cx| r.select_all(cx)),
                _ => {}
            },
            Act::Copy => match &self.pane {
                Pane::Lines(t) => t.update(cx, |t, cx| t.copy(cx)),
                Pane::Rendered(r) => r.update(cx, |r, cx| r.copy(cx)),
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
                self.note_playing(window, cx);
            }
            Act::Mute => {
                match &self.pane {
                    Pane::Video(v) => v.read(cx).toggle_mute(),
                    Pane::Audio(a) => a.read(cx).toggle_mute(),
                    _ => {}
                }
                self.note_playing(window, cx);
            }
            Act::Seek {
                forward,
                by,
                accurate,
            } => {
                match &self.pane {
                    Pane::Video(v) => v.update(cx, |v, cx| v.seek(forward, by, accurate, cx)),
                    Pane::Audio(a) => a.update(cx, |a, _| a.seek(forward, by, accurate)),
                    _ => return,
                }
                self.note_playing(window, cx);
            }
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
            Pane::Rendered(r) => r.update(cx, |r, cx| r.motion(motion, cx)),
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
            Pane::Doc(_) => (Family::Rail, "Minimap"),
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

    /// i: the strip goes to its next mode, kept for files of the family.
    fn cycle_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.strip.mode.next();
        self.strip.mode = mode;
        mode.save(self.family());
        self.hide = None;
        self.strip.shown = mode == StripMode::Auto;
        self.strip.epoch += 1;
        self.note(Some(mode.describe()), window, cx);
    }

    /// w: text wraps at the window's width, which stays as it is, or
    /// scrolls sideways again, and the next text does the same; the strip
    /// says which, or that a hexdump keeps its width.
    fn toggle_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Pane::Lines(t) = &self.pane else {
            return;
        };
        let word = match t.update(cx, |t, cx| t.toggle_wrap(window, cx)) {
            Some(true) => "Wrap",
            Some(false) => "No wrap",
            None => "Hexdumps do not wrap",
        };
        self.note(Some(word.into()), window, cx);
    }

    /// s: Markdown and HTML rendered, or as their source, in the window as
    /// it is; the file opens the same way again within the hour.
    fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loaded.markup.is_none() || self.switching.is_some() {
            return;
        }
        let rendered = !matches!(self.pane, Pane::Rendered(_));
        let (uri, screen) = (self.loaded.facts.uri.clone(), self.screen);
        let name = self.loaded.facts.name.clone();
        let job = cx
            .background_executor()
            .spawn(async move { content::load_as(&uri, screen, true, Some(rendered), None) });
        self.switching = Some(cx.spawn_in(window, async move |this, cx| {
            let mut loaded = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.switching = None;
                if rendered && !matches!(loaded.body, Body::Rendered(_)) {
                    return this.note(Some("Too long to render".into()), window, cx);
                }
                // The name a file taken out of an archive is shown by.
                loaded.facts.name = name;
                loaded.dock = 0.;
                loaded.view = None;
                this.loaded = loaded;
                this.attach(window, cx);
                this.redock(window, cx);
                let word = if rendered { "Rendered" } else { "Source" };
                this.note(Some(word.into()), window, cx);
            });
        }));
    }

    /// r: a quarter turn of a picture, every page of a PDF or a video,
    /// clockwise or back. The window takes the turned shape about its
    /// centre, and the file opens turned again within the hour.
    fn turn(&mut self, clockwise: bool, window: &mut Window, cx: &mut Context<Self>) {
        let turns = (self.loaded.turns + if clockwise { 1 } else { 3 }) % 4;
        let word = match turns {
            0 => "Upright".to_owned(),
            turns => format!("Turned {}°", u32::from(turns) * 90),
        };
        if let Pane::Video(video) = &self.pane {
            let shown = video.update(cx, |video, cx| video.set_turns(turns, window, cx));
            self.loaded.turns = turns;
            self.remember_turns();
            if !window.is_fullscreen() {
                let fit = self.screen.fit_media(shown);
                self.intent(Intent::Fit((fit.width, fit.height)));
            }
            return self.note(Some(word), window, cx);
        }
        let turnable = matches!(self.pane, Pane::Pdf(_))
            || matches!(
                (&self.pane, &self.loaded.body),
                (Pane::None, Body::Image(_))
            );
        if !turnable || self.switching.is_some() {
            return;
        }
        // Read again, turned: pictures are turned as they are decoded, and
        // a PDF's pages as they are drawn.
        let (uri, screen) = (self.loaded.facts.uri.clone(), self.screen);
        let name = self.loaded.facts.name.clone();
        let view = view_of(&self.pane, cx);
        let job = cx
            .background_executor()
            .spawn(async move { content::load_as(&uri, screen, true, None, Some(turns)) });
        self.switching = Some(cx.spawn_in(window, async move |this, cx| {
            let mut loaded = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.switching = None;
                loaded.facts.name = name;
                loaded.view = view;
                let size = loaded.size;
                this.loaded = loaded;
                this.remember_turns();
                this.attach(window, cx);
                if !window.is_fullscreen() {
                    this.intent(Intent::Fit(size));
                }
                this.note(Some(word), window, cx);
            });
        }));
    }

    /// The text bigger, smaller or at 100%, for this file and the next;
    /// what is under the pointer at `at`, or else at the top, stays put.
    fn zoom_text(
        &mut self,
        zoom: Zoom,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pane.kind().zooms() {
            return;
        }
        let level = crate::zoom::change(self.zoom, zoom);
        self.zoom = level;
        match &self.pane {
            Pane::Lines(t) => t.update(cx, |t, cx| t.set_zoom(level, at, window, cx)),
            Pane::Rendered(r) => r.update(cx, |r, cx| r.set_zoom(at, cx)),
            Pane::Doc(d) => d.update(cx, |d, cx| d.set_zoom(level, at, cx)),
            _ => {}
        }
        self.note(Some(crate::zoom::describe(level)), window, cx);
    }

    /// Ctrl with the wheel: a step of the text size for every notch, about
    /// the pointer.
    fn wheel_zoom(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // gpui counts a notch as three lines.
        self.wheel += match event.delta {
            ScrollDelta::Lines(delta) => delta.y / 3.,
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / 60.,
        };
        while self.wheel.abs() >= 1. {
            let zoom = if self.wheel > 0. { Zoom::In } else { Zoom::Out };
            self.wheel -= self.wheel.signum();
            self.zoom_text(zoom, Some(event.position), window, cx);
        }
    }

    /// Keeps the turns of the file shown for an hour, unless it was taken
    /// out of an archive.
    fn remember_turns(&self) {
        let facts = &self.loaded.facts;
        if self.inside.is_none()
            && let Some(key) = facts
                .path
                .as_deref()
                .and_then(|path| Key::new(path, facts.size, facts.modified))
        {
            memory::turn(&key, self.loaded.turns);
        }
    }

    /// Looks inside the selected entry of the archive in view, or opens or
    /// closes the selected folder; false when nothing is selected.
    fn look_inside(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Pane::Archive(archive) = &self.pane else {
            return false;
        };
        match archive.read(cx).selected(cx) {
            Some((entry, Some(size))) => self.take_out(entry, size, window, cx),
            Some((_, None)) => ArchivePane::toggle(window, cx),
            None => return false,
        }
        true
    }

    /// Takes `entry`, `size` bytes, out of the archive in view, in the
    /// background, and shows it once it is out.
    fn take_out(&mut self, entry: String, size: u64, window: &mut Window, cx: &mut Context<Self>) {
        let name = entry.rsplit('/').next().unwrap_or(&entry).to_owned();
        if size > extract::MOST {
            let too_large = format!("{name} is {}, too large to open here", format::size(size));
            self.note(Some(too_large), window, cx);
            return;
        }
        let Some(archive) = self.loaded.facts.path.clone() else {
            return;
        };
        if let Some(earlier) = self.opening.take() {
            earlier.cancel.store(true, Ordering::Relaxed);
        }
        let (progress, cancel) = (
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let shown_as = format!(
            "{} › {}",
            self.loaded.facts.name,
            extract::normalize(&entry)
        );
        let screen = self.screen;
        let (written, stop) = (progress.clone(), cancel.clone());
        let job = cx.background_executor().spawn(async move {
            let file =
                extract::extract(&archive, &entry, size, &written, &stop, &Places::standard())?;
            let uri = gio::glib::filename_to_uri(&file.path, None)
                .map_err(|e| Refusal::Failed(e.to_string()))?;
            let mut loaded = content::load(&uri, screen, true);
            loaded.facts.name = shown_as;
            Ok((file, loaded))
        });
        let work = cx.spawn_in(window, async move |this, cx| {
            let taken = job.await;
            let _ = this.update_in(cx, |this, window, cx| this.taken_out(taken, window, cx));
        });
        // Redrawn while it takes, for the progress.
        let tick = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let going = this.update(cx, |this, cx| {
                    cx.notify();
                    this.opening.is_some()
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        });
        self.opening = Some(Opening {
            name,
            size,
            progress,
            cancel,
            since: Instant::now(),
            _work: work,
            _tick: tick,
        });
    }

    fn taken_out(
        &mut self,
        taken: Result<(Extracted, Loaded), Refusal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Space, Esc or another file came first.
        let Some(opening) = self.opening.take() else {
            return;
        };
        let name = opening.name;
        let word = match taken {
            Ok((file, loaded)) => return self.enter(file, loaded, window, cx),
            Err(Refusal::Cancelled) => return,
            Err(Refusal::Encrypted) => format!("{name} is encrypted"),
            Err(Refusal::TooLarge(size)) => {
                format!("{name} is {}, too large to open here", format::size(size))
            }
            Err(Refusal::Missing) => format!("{name} is not in the archive"),
            Err(Refusal::Failed(why)) => {
                eprintln!("silent: cannot take {name} out: {why}");
                format!("{name} cannot be read")
            }
        };
        self.note(Some(word), window, cx);
    }

    /// Shows a file taken out of the archive in view, over it until Space.
    fn enter(
        &mut self,
        file: Extracted,
        loaded: Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let size = loaded.size;
        let pane = std::mem::replace(&mut self.pane, Pane::None);
        let archive = std::mem::replace(&mut self.loaded, loaded);
        self.levels.push(Level {
            loaded: archive,
            pane,
            file: self.inside.replace(file),
        });
        self.attach(window, cx);
        self.disengage(window, cx);
        if !window.is_fullscreen() {
            self.intent(Intent::Fit(size));
        }
    }

    /// Space: back to the archive the file was taken out of, as it was
    /// left, and the file goes.
    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(level) = self.levels.pop() else {
            return;
        };
        self.keep_loudness(cx);
        self.feed = None;
        self.totals = None;
        self._pane_changes = None;
        if let Some(old) = self.image.take() {
            cx.drop_image(old, Some(window));
        }
        self.pane.drop_pictures(window, cx);
        self.pane = level.pane;
        self.loaded = level.loaded;
        self.inside = level.file;
        self.keys = Keys::default();
        self.strip.note = None;
        self.strip.veil = None;
        self.redock = None;
        self.strip.mode = StripMode::load(self.family());
        self.watch_archive(window, cx);
        // The tree had the keys when the file was opened, and has them again.
        if let Pane::Archive(a) = &self.pane {
            a.update(cx, |a, cx| a.engage(window, cx));
        }
        if !window.is_fullscreen() {
            self.intent(Intent::Fit(self.loaded.size));
        }
        cx.notify();
    }

    /// An entry being taken out slowly: its name and how far it has come,
    /// over the archive.
    fn opening(&self, theme: Theme) -> Option<Div> {
        let opening = self.opening.as_ref().filter(|o| o.since.elapsed() > SLOW)?;
        let done = opening.progress.load(Ordering::Relaxed);
        let fraction = if opening.size > 0 {
            (done as f32 / opening.size as f32).clamp(0., 1.)
        } else {
            0.
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(380.))
                        .px(px(20.))
                        .py(px(16.))
                        .rounded(px(10.))
                        .bg(theme.surface)
                        .border_1()
                        .border_color(theme.rule)
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .text_size(px(13.))
                        .text_color(theme.text)
                        .child(div().truncate().child(format!("Opening {}", opening.name)))
                        .child(
                            div()
                                .h(px(4.))
                                .rounded(px(2.))
                                .bg(theme.muted.opacity(0.25))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(fraction))
                                        .rounded(px(2.))
                                        .bg(crate::theme::progress()),
                                ),
                        )
                        .child(div().text_color(theme.muted).child(format!(
                            "{} of {} · Space stops it",
                            format::size(done),
                            format::size(opening.size)
                        ))),
                ),
        )
    }

    fn toggle_pause(&self, cx: &mut App) {
        match &self.pane {
            Pane::Video(v) => v.update(cx, |v, _| v.toggle_pause()),
            Pane::Audio(a) => a.update(cx, |a, _| a.toggle_pause()),
            _ => {}
        }
    }

    fn pointer_moved(&mut self, _: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.strip.mode != StripMode::Auto {
            return;
        }
        if !self.strip.shown {
            // A note faded out with the strip; it is not shown again.
            self.strip.note = None;
        }
        self.set_strip(true, cx);
        self.linger_strip(window, cx);
    }

    /// Shows `note` at the strip's right for a moment, and the strip with
    /// it; while the strip is off, the note alone.
    fn note(&mut self, note: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        match self.strip.mode {
            StripMode::Off => self.notice(note, window, cx),
            StripMode::Auto | StripMode::On => {
                self.strip.note = note;
                self.set_strip(true, cx);
                self.linger_strip(window, cx);
            }
        }
    }

    /// A seek, a pause or the mute: the playbar shows them, or while the
    /// strip is off, a word with the time does.
    fn note_playing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let word = (self.strip.mode == StripMode::Off).then(|| {
            let (position, duration, flags): (_, _, Vec<&str>) = match &self.pane {
                Pane::Video(v) => {
                    let v = v.read(cx);
                    (v.position(), v.duration(), v.flags().collect())
                }
                Pane::Audio(a) => {
                    let a = a.read(cx);
                    (a.position(), a.duration(), a.flags().collect())
                }
                _ => return String::new(),
            };
            let mut word = format::duration(position);
            if let Some(duration) = duration {
                word = format!("{word} / {}", format::duration(duration));
            }
            for flag in flags {
                word = format!("{word} · {flag}");
            }
            word
        });
        self.note(word, window, cx);
    }

    /// While the strip is off: `word` for a second over the bottom right.
    fn notice(&mut self, word: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(word) = word.filter(|w| !w.is_empty()) else {
            return;
        };
        self.strip.notice = Some(word);
        if !self.strip.notice_shown {
            self.strip.notice_shown = true;
            self.strip.notice_epoch += 1;
        }
        self.strip.notice_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(NOTICE).await;
            let _ = this.update(cx, |this, cx| {
                this.strip.notice_shown = false;
                this.strip.notice_epoch += 1;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Hides the strip a moment from now, unless a vim command is
    /// unfinished or the pointer is on the strip then; a strip that is
    /// always shown only drops its note.
    fn linger_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let linger = if self.pane.media() {
            PLAYBAR_LINGER
        } else {
            STRIP_LINGER
        };
        self.hide = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(linger).await;
            let _ = this.update(cx, |this, cx| {
                if this.keys.pending().is_some() || this.strip.hovered {
                    return;
                }
                this.hide = None;
                match this.strip.mode {
                    StripMode::Auto => this.set_strip(false, cx),
                    _ => {
                        this.strip.note = None;
                        cx.notify();
                    }
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
            // The times are on the playbar.
            Pane::Video(v) => parts.extend(v.read(cx).flags().map(str::to_owned)),
            Pane::Audio(a) => parts.extend(a.read(cx).flags().map(str::to_owned)),
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
            Pane::Doc(_) | Pane::Rendered(_) => parts.push(self.loaded.description.clone()),
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
        let turned = (self.loaded.turns != 0)
            .then(|| format!("turned {}°", u32::from(self.loaded.turns) * 90));
        if !matches!(self.pane, Pane::None) {
            parts.extend(turned);
            return parts.join(" · ");
        }
        parts.truncate(1);
        match &self.loaded.body {
            Body::Image(picture) => {
                parts.push(format::size(facts.size));
                parts.push(format!("{} × {}", picture.natural.0, picture.natural.1));
                parts.extend(turned);
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

    /// The strip's veil over the bottom `band` of the window.
    fn scrim(&mut self, band: f32, theme: Theme, window: &Window, cx: &mut App) -> Scrim {
        let area = window.viewport_size();
        let colours = match &self.pane {
            Pane::Video(v) => return v.update(cx, |v, _| v.scrim(area, band)),
            Pane::Audio(_)
            | Pane::Lines(_)
            | Pane::Rendered(_)
            | Pane::Sheet(_)
            | Pane::Archive(_) => vec![theme.surface, theme.text],
            Pane::Doc(_) => vec![rgb(docview::PAPER).into(), rgb(docview::INK).into()],
            // Pages are drawn white whatever their ink, between the
            // window's own ground.
            Pane::Pdf(_) | Pane::Slides(_) => {
                vec![rgb(0xffffff).into(), rgb(0x000000).into(), theme.backdrop]
            }
            Pane::None => match (&self.loaded.body, &self.image) {
                (Body::Image(_) | Body::Still(_), Some(image)) => {
                    if let Some((id, size, scrim)) = self.strip.veil
                        && id == image.id
                        && size == area
                    {
                        return scrim;
                    }
                    let ground = strip::rgb_of(theme.backdrop);
                    let under = strip::under_image(image, area, band, ground);
                    let scrim = strip::follow(&under, None);
                    self.strip.veil = Some((image.id, area, scrim));
                    return scrim;
                }
                (Body::Image(_) | Body::Still(_), None) => vec![theme.backdrop],
                _ => vec![theme.surface, theme.text],
            },
        };
        strip::over_colours(&colours)
    }

    /// The strip over the bottom of the window, with a video's or a
    /// sound's playbar; while it is off, only a notice from a key.
    fn strip(
        &mut self,
        theme: Theme,
        radius: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mode = self.strip.mode;
        // Above a workbook's sheet tabs, which are clicked where it would be.
        let lift = match &self.pane {
            Pane::Sheet(s) if s.read(cx).has_tabs() => px(grid::TABS),
            _ => px(0.),
        };
        if mode == StripMode::Off {
            return self.notice_line(lift, theme, window, cx);
        }
        // Before the pointer has moved there is nothing to fade.
        if mode == StripMode::Auto && self.strip.epoch == 0 {
            return None;
        }
        let shown = mode == StripMode::On || self.strip.shown;
        let (fade, band) = if self.pane.media() {
            (strip::PLAYBAR_FADE, strip::TRACK + strip::ROW)
        } else {
            (strip::FADE, strip::ROW)
        };
        let scrim = self.scrim(band, theme, window, cx);
        let ink = strip::hsla(scrim.ink(), 1.);
        let aside = self
            .keys
            .pending()
            .or_else(|| self.strip.note.clone())
            .or_else(|| self.scrub_label(cx))
            .map(|aside| div().flex_none().child(aside));
        let text = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis_middle()
            .child(self.strip_text(cx));
        let playbar = match &self.pane {
            Pane::Video(v) => Some(v.update(cx, |v, cx| media::playbar(v, shown, ink, cx))),
            Pane::Audio(a) => Some(a.update(cx, |a, cx| media::playbar(a, shown, ink, cx))),
            _ => None,
        };
        let strip = div()
            .id("strip")
            .absolute()
            .left_0()
            .right_0()
            .bottom(lift)
            .h(px(fade + band))
            .bg(strip::background(scrim, fade / (fade + band)))
            .when(lift == px(0.), |strip| strip.rounded_b(radius))
            .flex()
            .flex_col()
            .justify_end()
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                this.strip.hovered = *hovered;
                if !*hovered && this.strip.shown {
                    this.linger_strip(window, cx);
                }
            }));
        let strip = match playbar {
            Some(bar) => strip
                // A shown playbar is a control: clicks on it do not reach
                // the video. Occluding the video instead would also take
                // the window's hover, which keeps the playbar.
                .when(shown, |strip| {
                    strip.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                })
                .child(div().px(px(strip::INSET)).child(bar.track))
                .child(
                    strip::line(ink)
                        .child(div().flex_none().child(bar.times))
                        .child(text)
                        .children(aside)
                        .child(bar.volume),
                ),
            None => strip.child(strip::line(ink).child(text).children(aside)),
        };
        Some(if mode == StripMode::On {
            strip.into_any_element()
        } else {
            strip
                .with_animation(
                    ("strip", self.strip.epoch),
                    Animation::new(STRIP_FADE).with_easing(ease_in_out),
                    move |strip, t| strip.opacity(if shown { t } else { 1. - t }),
                )
                .into_any_element()
        })
    }

    /// A word from a key while the strip is off, on a veil of its own, where
    /// the strip's line would put it; what is typed of a vim command and
    /// where the scrollbar has the view show for as long as they last.
    fn notice_line(
        &mut self,
        lift: Pixels,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let live = self.keys.pending().or_else(|| self.scrub_label(cx));
        let shown = live.is_some() || self.strip.notice_shown;
        let word = live.or_else(|| self.strip.notice.clone())?;
        let scrim = self.scrim(strip::ROW, theme, window, cx);
        let notice = strip::line(strip::hsla(scrim.ink(), 1.))
            .absolute()
            .left_0()
            .right_0()
            .bottom(lift)
            .justify_end()
            .child(strip::chip(word, strip::hsla(scrim.veil(), scrim.alpha)));
        Some(
            notice
                .with_animation(
                    ("notice", self.strip.notice_epoch),
                    Animation::new(STRIP_FADE).with_easing(ease_in_out),
                    move |notice, t| notice.opacity(if shown { t } else { 1. - t }),
                )
                .into_any_element(),
        )
    }

    /// What the window shows behind the content.
    fn ground(&self, theme: Theme) -> Hsla {
        match (&self.pane, &self.loaded.body) {
            (Pane::Video(_), _) => rgb(0x000000).into(),
            (Pane::Pdf(_) | Pane::Slides(_), _) | (Pane::None, Body::Image(_) | Body::Still(_)) => {
                theme.backdrop
            }
            _ => theme.surface,
        }
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
        // Rendered text is sized in rems, as a page in a browser; silent's
        // own words are in pixels.
        let rem = match self.pane {
            Pane::Rendered(_) => self.zoom,
            _ => 1.,
        };
        window.set_rem_size(px(16. * rem));
        // Ctrl with the wheel sizes the text before the content scrolls.
        let view = cx.entity().downgrade();
        let wheel = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !event.modifiers.control {
                        return;
                    }
                    let _ = view.update(cx, |this, cx| {
                        if this.pane.kind().zooms() {
                            this.wheel_zoom(event, window, cx);
                            cx.stop_propagation();
                        }
                    });
                });
            },
        )
        .absolute()
        .size_full();
        if !self.traced && self.ready(cx) {
            self.traced = true;
            window.on_next_frame(|_, _| trace::mark("first frame"));
        }
        let engaged = self.engaged(window, cx);
        let body = match (&self.loaded.body, &self.image) {
            _ if let Some(pane) = self.pane.view() => pane.into_any_element(),
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
        let content = div()
            .id("content")
            .size_full()
            .rounded(radius)
            .overflow_hidden()
            .when(self.pane.kind().engages(), |content| {
                content.track_focus(&self.content)
            })
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.engaged_at_press = this.engaged(window, cx);
            }))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                // The click that engages a video or a sound only engages it.
                if this.engaged_at_press {
                    this.toggle_pause(cx);
                }
            }))
            .child(body);
        let strip = self.strip(theme, radius, window, cx);
        // Engaged content wears a thin warm ring, inside the window's edge.
        let ring = engaged.then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .rounded(radius)
                .border_2()
                .border_color(crate::theme::progress().opacity(0.55))
        });
        div()
            .id("preview")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .font_family("Noto Sans")
            .bg(self.ground(theme))
            .rounded(radius)
            .on_key_down(cx.listener(Self::key))
            // A preview opened from a terminal leaves it the keyboard, until
            // a click here.
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, _| {
                if !window.is_window_active() {
                    this.intent(Intent::Focus);
                }
            }))
            .on_mouse_move(cx.listener(Self::pointer_moved))
            // Where the pointer is, not whether a key came last: by default
            // a key ends the hover, and the strip it showed would go with it.
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.strip.mode == StripMode::Auto {
                    this.hide = None;
                    this.set_strip(false, cx);
                }
            }))
            .child(content)
            .child(wheel)
            .children(strip)
            .children(self.opening(theme))
            .children(self.help.then(|| {
                let dock = match self.pane.docking(cx) {
                    Some(docking) if docking.docked => px(docking.width),
                    _ => px(0.),
                };
                help_overlay(self.help_shown(cx), theme, radius, dock, window, cx)
            }))
            .children(ring)
    }
}

/// Where a file was left in `pane`, to come back to.
fn view_of(pane: &Pane, cx: &App) -> Option<View> {
    match pane {
        Pane::Video(v) => {
            let v = v.read(cx);
            memory::worth_resuming(v.position(), v.duration()).then(|| View::Media(v.position()))
        }
        Pane::Audio(a) => {
            let a = a.read(cx);
            memory::worth_resuming(a.position(), a.duration()).then(|| View::Media(a.position()))
        }
        Pane::Pdf(p) => p.read(cx).view(),
        Pane::Slides(s) => s.read(cx).view(),
        Pane::Lines(t) => t.read(cx).view(),
        Pane::Rendered(r) => r.read(cx).view(cx),
        Pane::Doc(d) => d.read(cx).view(),
        Pane::Sheet(s) => s.read(cx).view(),
        Pane::Archive(a) => a.read(cx).view(cx),
        Pane::None => None,
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

/// The card of keys over the content, on a veil that keeps the content
/// back, and clear of a dock `dock` wide; a click, Esc or ? takes it away.
fn help_overlay(
    shown: help::Shown,
    theme: Theme,
    radius: Pixels,
    dock: Pixels,
    window: &Window,
    cx: &mut Context<Preview>,
) -> Stateful<Div> {
    let viewport = window.viewport_size();
    let room = size((viewport.width - dock).max(px(1.)), viewport.height);
    div()
        .id("help")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .pr(dock)
        .rounded(radius)
        .bg(theme.backdrop.opacity(0.85))
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.help = false;
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(help::card(shown, theme, room, window))
}
