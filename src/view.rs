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
use silent::decode::{Event, Wanted};
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
use crate::framing::Framing;
use crate::grid::{self, SheetPane};
use crate::help;
use crate::keys::{self, Act, Mods};
use crate::lines::{Doc, LinesPane};
use crate::media::{self, AudioPane, Loudness, Player, VideoPane};
use crate::overview::{Docking, Family, Mode};
use crate::page::PagePane;
use crate::pdf::PdfPane;
use crate::rendered::RenderedPane;
use crate::slides::SlidesPane;
use crate::strip::{self, Mode as StripMode};
use crate::theme::{Theme, accent};
use crate::trace;
use crate::zoom::Zoom;

/// Corner radius when a compositor draws the transparent corners.
pub const RADIUS: f32 = 11.0;
/// Whether a compositor draws them, as silent found when it started.
static ROUNDED: AtomicBool = AtomicBool::new(false);
/// Whether paper fills the window, whose corners then stay square.
static PAPER: AtomicBool = AtomicBool::new(false);
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
/// How long a picture's zoom rests before it is decoded again for it.
const ZOOM_REST: Duration = Duration::from_millis(150);
/// The most a picture is decoded to for a zoom: pixels in all, and on a
/// side, which every GPU's textures hold.
const MOST_PIXELS: f32 = 32e6;
const MOST_SIDE: f32 = 8192.;

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
    input: async_channel::Sender<Input>,
    /// Whether the first frame of the content is marked, or has none to
    /// mark: the blank a withdrawn window holds is never on the screen.
    traced: bool,
    /// The ground the window was given last, as 0xRRGGBB.
    ground: Option<u32>,
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
    /// A picture's zoom and the part of it in view; the picture decoded
    /// again for that zoom, and for what size; whether it is an animation,
    /// which keeps its frames instead; and the wait for the zoom to rest.
    framing: Framing,
    sharp: Option<Arc<RenderImage>>,
    sharp_for: Option<(u32, u32)>,
    sharpening: Option<Task<()>>,
    animated: bool,
    zoom_rest: Option<Task<()>>,
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
    Page(Entity<PagePane>),
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
            Pane::Page(v) => v.clone().into(),
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
            Pane::Rendered(_) | Pane::Page(_) => keys::Kind::Rendered,
            Pane::Archive(_) => keys::Kind::Archive,
            Pane::Doc(_) => keys::Kind::Doc,
            Pane::Sheet(_) => keys::Kind::Sheet,
            Pane::Slides(_) => keys::Kind::Slides,
        }
    }

    fn media(&self) -> bool {
        matches!(self, Pane::Video(_) | Pane::Audio(_))
    }

    /// Pages to read, which fill the window to its edges as paper does and
    /// keep its corners square.
    fn paper(&self) -> bool {
        matches!(self, Pane::Pdf(_) | Pane::Doc(_))
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
            Pane::Page(p) => p.update(cx, |p, cx| p.drop_pictures(window, cx)),
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
    /// The veil worked out for a picture, by the picture, the window's size
    /// and where the picture is drawn in it.
    veil: Option<(ImageId, Size<Pixels>, Bounds<Pixels>, Scrim)>,
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
        ROUNDED.store(rounded, Ordering::Relaxed);
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
            input,
            traced,
            ground: None,
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
            framing: Framing::new(1., (0.5, 0.5)),
            sharp: None,
            sharp_for: None,
            sharpening: None,
            animated: false,
            zoom_rest: None,
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
        let picture = self.levels.is_empty() && self.picture_shape().is_some();
        if (picture || !matches!(pane, Pane::None))
            && let Some(key) = facts
                .path
                .as_deref()
                .and_then(|path| Key::new(path, facts.size, facts.modified))
        {
            // Markdown and HTML left as their source open so again.
            let source = loaded.markup.is_some() && matches!(pane, Pane::Lines(_));
            let view =
                view_of(pane, cx).or_else(|| source.then_some(View::Lines { top: 0., side: 0. }));
            // PDFs, slides and pictures keep a zoom each.
            let (view, zoom) = match pane {
                Pane::Pdf(p) => (view, p.read(cx).zoom()),
                Pane::Slides(s) => (view, s.read(cx).zoom()),
                Pane::None if self.framing.level != 1. => {
                    let (x, y) = self.framing.centre;
                    (Some(View::Picture { x, y }), self.framing.level)
                }
                _ => (view, 1.),
            };
            memory::remember(&key, view, zoom);
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
        for old in [self.image.take(), self.sharp.take()].into_iter().flatten() {
            cx.drop_image(old, Some(window));
        }
        let name: SharedString = self.loaded.facts.name.clone().into();
        let view = self.loaded.view.take();
        let loudness = self.loudness;
        let docked = self.loaded.dock > 0.;
        self.sized_docked = docked;
        // A picture opens at fit, unless it was left zoomed within the hour.
        let centre = match view {
            Some(View::Picture { x, y }) => (x, y),
            _ => (0.5, 0.5),
        };
        self.framing = Framing::new(self.loaded.zoom, centre);
        (self.sharp_for, self.sharpening, self.zoom_rest) = (None, None, None);
        self.animated = false;
        let zoom = self.loaded.zoom;
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
                Pane::Pdf(cx.new(|_| PdfPane::new(pdf, mode, docked, view, zoom, window)))
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
            Body::Page(page) => Pane::Page(cx.new(|_| PagePane::new(*page, window))),
            Body::Document(paper) => {
                let (mode, zoom) = (self.mode(Family::Rail), self.zoom);
                Pane::Doc(cx.new(|_| DocPane::new(*paper, mode, docked, view, zoom, window)))
            }
            Body::Sheet(sheet) => {
                let zoom = self.zoom;
                Pane::Sheet(cx.new(|_| SheetPane::new(*sheet, view, zoom)))
            }
            Body::Deck(deck) => {
                let mode = self.mode(Family::Rail);
                Pane::Slides(cx.new(|_| SlidesPane::new(*deck, mode, docked, view, zoom, window)))
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
                let mut first = picture.first.take();
                let events = picture.frames.events.clone();
                if first.is_none()
                    && let Ok(Event::Frame(prepared)) = events.try_recv()
                {
                    first = Some(prepared);
                }
                if let Some(first) = first {
                    self.animated = first.animated;
                    image = Some(first.image);
                }
                self.feed = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = events.recv().await {
                        let Event::Frame(prepared) = event else {
                            continue;
                        };
                        let shown = this.update_in(cx, |this, window, cx| {
                            this.animated |= prepared.animated;
                            let first = this.image.is_none();
                            if let Some(old) = this.image.replace(prepared.image) {
                                cx.drop_image(old, Some(window));
                            }
                            // Left zoomed, it is decoded again for the zoom.
                            if first && this.framing.level != 1. {
                                this.rest_then_sharpen(window, cx);
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
        if self.image.is_some() && self.framing.level != 1. {
            self.rest_then_sharpen(window, cx);
        }
        if let Some(note) = self.loaded.note.take() {
            self.note(Some(note), window, cx);
        }
        cx.notify();
    }

    /// The family of files whose strip mode i sets together.
    fn family(&self) -> &'static str {
        match &self.pane {
            Pane::Video(_) | Pane::Audio(_) => "media",
            Pane::Lines(_) | Pane::Rendered(_) | Pane::Page(_) => "text",
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
            Pane::Page(_) => Subject::Rendered(Markup::Html),
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
                Body::Still(_) => Subject::Still,
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

    /// What keys take the view for: a picture shown, as far as they go, is
    /// one that zooms.
    fn kind(&self) -> keys::Kind {
        match self.pane {
            Pane::None if self.picture_shape().is_some() => keys::Kind::Picture,
            _ => self.pane.kind(),
        }
    }

    /// The width and height of the picture shown, upright and as r turned
    /// it, once it has its first frame.
    fn picture_shape(&self) -> Option<(f32, f32)> {
        if !matches!(self.pane, Pane::None) {
            return None;
        }
        let (width, height) = match &self.loaded.body {
            Body::Image(picture) if self.image.is_some() => picture.natural,
            Body::Still(still) => {
                let size = still.image.size(0);
                (size.width.0.max(1) as u32, size.height.0.max(1) as u32)
            }
            _ => return None,
        };
        Some((width as f32, height as f32))
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
            self.kind(),
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
            Act::Across(right, n) => match &self.pane {
                Pane::Pdf(p) => p.update(cx, |p, cx| p.across(right, n, cx)),
                Pane::Slides(s) => s.update(cx, |s, cx| s.across(right, n, cx)),
                _ => {}
            },
            Act::Overview => self.cycle_overview(window, cx),
            Act::Strip => self.cycle_strip(window, cx),
            Act::Wrap => self.toggle_wrap(window, cx),
            Act::Source => self.toggle_source(window, cx),
            Act::Help => self.help = !self.help,
            Act::Turn(clockwise) => self.turn(clockwise, window, cx),
            Act::Zoom(zoom) => self.step_zoom(zoom, None, window, cx),
            Act::SelectAll => match &self.pane {
                Pane::Lines(t) => t.update(cx, |t, cx| t.select_all(cx)),
                Pane::Rendered(r) => r.update(cx, |r, cx| r.select_all(cx)),
                Pane::Page(p) => p.update(cx, |p, cx| p.select_all(cx)),
                _ => {}
            },
            Act::Copy => match &self.pane {
                Pane::Lines(t) => t.update(cx, |t, cx| t.copy(cx)),
                Pane::Rendered(r) => r.update(cx, |r, cx| r.copy(cx)),
                Pane::Page(p) => p.update(cx, |p, cx| p.copy(cx)),
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
            Pane::Page(p) => p.update(cx, |p, cx| p.motion(motion, cx)),
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
        // A picture fits the new size, at its zoom; decoded for it once the
        // size rests, as for fullscreen.
        if self.picture_shape().is_some() {
            self.rest_then_sharpen(window, cx);
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
        let rendered = !matches!(self.pane, Pane::Rendered(_) | Pane::Page(_));
        let word = if rendered { "Rendered" } else { "Source" };
        self.read_again(rendered, word, window, cx);
    }

    /// The file read again, rendered or as its source, into the window as
    /// it is, and `word` on the info line.
    fn read_again(
        &mut self,
        rendered: bool,
        word: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (uri, screen) = (self.loaded.facts.uri.clone(), self.screen);
        let name = self.loaded.facts.name.clone();
        let job = cx
            .background_executor()
            .spawn(async move { content::load_as(&uri, screen, true, Some(rendered), None) });
        self.switching = Some(cx.spawn_in(window, async move |this, cx| {
            let mut loaded = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.switching = None;
                if rendered && !matches!(loaded.body, Body::Rendered(_) | Body::Page(_)) {
                    return this.note(Some("Too long to render".into()), window, cx);
                }
                // The name a file taken out of an archive is shown by.
                loaded.facts.name = name;
                loaded.dock = 0.;
                loaded.view = None;
                this.loaded = loaded;
                this.attach(window, cx);
                this.redock(window, cx);
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
        // a PDF's pages as they are drawn. A zoom stays, a picture's about
        // its middle.
        let (uri, screen) = (self.loaded.facts.uri.clone(), self.screen);
        let name = self.loaded.facts.name.clone();
        let view = view_of(&self.pane, cx);
        let zoom = match &self.pane {
            Pane::Pdf(p) => p.read(cx).zoom(),
            _ => self.framing.level,
        };
        let job = cx
            .background_executor()
            .spawn(async move { content::load_as(&uri, screen, true, None, Some(turns)) });
        self.switching = Some(cx.spawn_in(window, async move |this, cx| {
            let mut loaded = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.switching = None;
                loaded.facts.name = name;
                loaded.view = view;
                loaded.zoom = zoom;
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

    /// Ctrl with + and -, 0 or the wheel: the text bigger or smaller, or
    /// pages and pictures zoomed; what is under the pointer at `at`, or
    /// else at the top, or a picture's middle, stays put.
    fn step_zoom(
        &mut self,
        zoom: Zoom,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = self.kind();
        if kind.sizes_text() {
            self.zoom_text(zoom, at, window, cx);
        } else if kind.zooms() {
            self.zoom_view(zoom, at, window, cx);
        }
    }

    /// The text bigger, smaller or at 100%, for this file and the next.
    fn zoom_text(
        &mut self,
        zoom: Zoom,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let level = crate::zoom::change(self.zoom, zoom);
        trace::mark(&format!("zoom to {}%", (level * 100.).round()));
        if level != self.zoom {
            self.zoom = level;
            match &self.pane {
                Pane::Lines(t) => t.update(cx, |t, cx| t.set_zoom(level, at, window, cx)),
                Pane::Rendered(r) => r.update(cx, |r, cx| r.set_zoom(at, cx)),
                Pane::Page(p) => p.update(cx, |p, cx| p.set_zoom(level, at, cx)),
                Pane::Doc(d) => d.update(cx, |d, cx| d.set_zoom(level, at, cx)),
                Pane::Sheet(s) => s.update(cx, |s, cx| s.set_zoom(level, at, cx)),
                _ => {}
            }
        }
        self.note(Some(crate::zoom::describe(level)), window, cx);
    }

    /// A PDF, slides or a picture zoomed in, out or to fit the window, at
    /// a zoom of its own, which the next file does not take.
    fn zoom_view(
        &mut self,
        zoom: Zoom,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = match &self.pane {
            Pane::Pdf(p) => p.read(cx).zoom(),
            Pane::Slides(s) => s.read(cx).zoom(),
            _ => self.framing.level,
        };
        let level = crate::zoom::step(before, zoom);
        trace::mark(&format!("zoom to {}%", (level * 100.).round()));
        if level != before {
            match &self.pane {
                Pane::Pdf(p) => p.update(cx, |p, cx| p.set_zoom(level, at, window, cx)),
                Pane::Slides(s) => s.update(cx, |s, cx| s.set_zoom(level, at, window, cx)),
                _ => self.zoom_picture(level, at, window, cx),
            }
        }
        self.note(Some(crate::zoom::describe_zoom(level)), window, cx);
    }

    /// The picture at `level`, the point under the pointer at `at`, or else
    /// in the middle, kept where it is; decoded again for it once it rests.
    fn zoom_picture(
        &mut self,
        level: f32,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shape) = self.picture_shape() else {
            return;
        };
        self.framing.zoom(level, at, shape, window.viewport_size());
        self.rest_then_sharpen(window, cx);
        cx.notify();
    }

    /// Decodes the picture again for the size it is drawn at, once the
    /// zoom or the window's size has rested a moment.
    fn rest_then_sharpen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_rest = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(ZOOM_REST).await;
            let _ = this.update_in(cx, |this, window, cx| this.sharpen(window, cx));
        }));
    }

    /// The decode of the picture to draw where it is drawn `wide` device
    /// pixels: of the first decode and the one for the zoom, the smaller
    /// that is as wide as that, or else the wider.
    fn shown_picture(&self, wide: f32) -> Option<Arc<RenderImage>> {
        let base = self.image.as_ref()?;
        let Some(sharp) = &self.sharp else {
            return Some(base.clone());
        };
        let width = |image: &Arc<RenderImage>| image.size(0).width.0 as f32;
        let (b, s) = (width(base), width(sharp));
        let enough = |w: f32| w >= wide * 0.97;
        let sharp_one = match (enough(b), enough(s)) {
            (true, true) => s <= b,
            (false, false) => s >= b,
            (base_enough, _) => !base_enough,
        };
        Some(if sharp_one {
            sharp.clone()
        } else {
            base.clone()
        })
    }

    /// Decodes the picture again for the size it is drawn at, unless what
    /// is decoded already serves: sharper when zoomed past the size of its
    /// first decode, as far as the picture's own pixels go, and smaller when
    /// zoomed out, so that the GPU, which shrinks without mipmaps, need not
    /// shrink it much. A huge JPEG is decoded at the fraction of its size
    /// that covers the zoom; an animation keeps its frames as they come.
    fn sharpen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Body::Image(picture) = &self.loaded.body else {
            return;
        };
        let Some(shape) = self.picture_shape() else {
            return;
        };
        if self.animated {
            return;
        }
        let rect = self.framing.rect(shape, window.viewport_size());
        let device = window.scale_factor();
        let wide = f32::from(rect.size.width) * device;
        let tall = f32::from(rect.size.height) * device;
        // Pixel art grows a whole number of times; anything else no larger
        // than its own pixels.
        let pixel_art = picture.natural.0.max(picture.natural.1) <= 64;
        let most = if pixel_art { f32::MAX } else { shape.0 };
        let serves = |image: &Arc<RenderImage>| {
            let w = image.size(0).width.0 as f32;
            w >= wide.min(most) * 0.97 && w <= wide * 1.6
        };
        let (Some(shown), Some(base)) = (self.shown_picture(wide), self.image.clone()) else {
            return;
        };
        if serves(&shown) {
            return;
        }
        // Back near the fit, the first decode serves again.
        if serves(&base) {
            if let Some(sharp) = self.sharp.take() {
                cx.drop_image(sharp, Some(window));
            }
            self.sharp_for = None;
            cx.notify();
            return;
        }
        let scale = [
            most / wide,
            (MOST_PIXELS / (wide * tall)).sqrt(),
            MOST_SIDE / wide.max(tall),
            1.,
        ]
        .into_iter()
        .fold(f32::MAX, f32::min);
        let target = ((wide * scale).round() as u32, (tall * scale).round() as u32);
        if self.sharp_for == Some(target) {
            return;
        }
        self.sharp_for = Some(target);
        let (uri, turns) = (self.loaded.facts.uri.clone(), self.loaded.turns);
        let wanted = Wanted::Within(target.0.max(1), target.1.max(1));
        self.sharpening = Some(cx.spawn_in(window, async move |this, cx| {
            let decoder = crate::picture::decode(&uri, wanted, turns);
            let prepared = loop {
                match decoder.events.recv().await {
                    Ok(Event::Size { .. }) => continue,
                    Ok(Event::Frame(prepared)) => break prepared,
                    Ok(Event::Failed(_)) | Err(_) => return,
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                trace::mark("picture sharpened");
                if let Some(old) = this.sharp.replace(prepared.image) {
                    cx.drop_image(old, Some(window));
                }
                cx.notify();
            });
        }));
    }

    /// Ctrl with the wheel: a step of the text size or the zoom for every
    /// notch, about the pointer.
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
            self.step_zoom(zoom, Some(event.position), window, cx);
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
        for old in [self.image.take(), self.sharp.take()].into_iter().flatten() {
            cx.drop_image(old, Some(window));
        }
        (self.sharp_for, self.sharpening, self.zoom_rest) = (None, None, None);
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
            Pane::Doc(_) | Pane::Rendered(_) | Pane::Page(_) => {
                parts.push(self.loaded.description.clone())
            }
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
            | Pane::Page(_)
            | Pane::Sheet(_)
            | Pane::Archive(_) => vec![theme.surface, theme.text],
            Pane::Doc(_) => vec![rgb(docview::PAPER).into(), rgb(docview::INK).into()],
            // Pages are drawn white whatever their ink, between the
            // window's own ground.
            Pane::Pdf(_) | Pane::Slides(_) => {
                vec![rgb(0xffffff).into(), rgb(0x000000).into(), theme.backdrop]
            }
            Pane::None => match (&self.loaded.body, self.picture_shape()) {
                (Body::Image(_) | Body::Still(_), Some(shape)) => {
                    let drawn = self.framing.rect(shape, area);
                    let wide = f32::from(drawn.size.width) * window.scale_factor();
                    let Some(image) = self.shown_picture(wide) else {
                        return strip::over_colours(&[theme.backdrop]);
                    };
                    if let Some((id, size, at, scrim)) = self.strip.veil
                        && id == image.id
                        && size == area
                        && at == drawn
                    {
                        return scrim;
                    }
                    let ground = strip::rgb_of(theme.backdrop);
                    let under = strip::under_picture(&image, drawn, area, band, ground);
                    // As a drag or a zoom moves the picture, the veil
                    // changes gently, as over a video.
                    let before = self.strip.veil.filter(|veil| veil.0 == image.id);
                    let scrim = strip::follow(&under, before.map(|veil| veil.3));
                    self.strip.veil = Some((image.id, area, drawn, scrim));
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

    /// The picture where its zoom puts it: whole in the window at first,
    /// and once it is larger than the window, a drag moves it.
    fn picture(&self, radius: Pixels, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let area = window.viewport_size();
        let Some(shape) = self.picture_shape() else {
            return div().size_full().into_any_element();
        };
        let rect = self.framing.rect(shape, area);
        let wide = f32::from(rect.size.width) * window.scale_factor();
        let Some(image) = self.shown_picture(wide) else {
            return div().size_full().into_any_element();
        };
        let (movable, held) = (self.framing.movable(shape, area), self.framing.held());
        let view = cx.entity().downgrade();
        // Listened to on every frame of a drag, which may leave the window.
        let follow = (movable || held).then(|| {
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let moved = view.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            let _ = moved.update(cx, |this, cx| {
                                this.drag_picture(event.position, window, cx)
                            });
                        }
                    });
                    let released = view.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                            let _ = released.update(cx, |this, cx| {
                                if this.framing.held() {
                                    this.framing.release();
                                    cx.notify();
                                }
                            });
                        }
                    });
                },
            )
            .absolute()
            .size_full()
        });
        div()
            .id("picture")
            .size_full()
            .relative()
            .when(movable, |picture| {
                let hand = if held {
                    CursorStyle::ClosedHand
                } else {
                    CursorStyle::OpenHand
                };
                picture.cursor(hand).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.framing.hold(event.position);
                        cx.notify();
                    }),
                )
            })
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| paint_picture(&image, rect, radius, window),
                )
                .absolute()
                .size_full(),
            )
            .children(follow)
            .into_any_element()
    }

    /// The pointer moves while a drag holds the picture.
    fn drag_picture(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shape) = self.picture_shape() else {
            return;
        };
        if self.framing.drag(at, shape, window.viewport_size()) {
            cx.notify();
        }
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
        log::debug!(target: "silent::frames", "render");
        let theme = Theme::of(window.appearance());
        // Pages are laid out in the desktop's theme before their window draws.
        crate::web::set_dark(matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ));
        // Before the pane renders: the first frame of paper is square, and
        // the first frame after it round.
        PAPER.store(self.pane.paper(), Ordering::Relaxed);
        let radius = radius(window);
        // Rendered text is sized in rems, as a page in a browser; silent's
        // own words are in pixels.
        let rem = match self.pane {
            Pane::Rendered(_) => self.zoom,
            _ => 1.,
        };
        window.set_rem_size(px(16. * rem));
        // Ctrl with the wheel sizes the text before the content scrolls: this
        // is painted before the content, so that it hears the wheel first,
        // before a rendered page's text view takes it.
        let view = cx.entity().downgrade();
        let wheel = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !event.modifiers.control {
                        return;
                    }
                    let _ = view.update(cx, |this, cx| {
                        if this.kind().zooms() {
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
        // The X server fills the strip a growing window gains with the
        // window's own background until silent's frame for the new size is
        // on the screen: the content's ground, not black.
        let ground = Rgba::from(self.ground(theme));
        let ground = [ground.r, ground.g, ground.b]
            .into_iter()
            .fold(0, |rgb, c| rgb << 8 | (c * 255.).round() as u32);
        if self.ground != Some(ground) {
            self.ground = Some(ground);
            self.intent(Intent::Ground(ground));
        }
        let engaged = self.engaged(window, cx);
        let body = match (&self.loaded.body, &self.image) {
            _ if let Some(pane) = self.pane.view() => pane.into_any_element(),
            (Body::Image(_) | Body::Still(_), Some(_)) => self.picture(radius, window, cx),
            (Body::Image(_), None) => div().size_full().into_any_element(),
            _ => self.card(theme).rounded(radius).into_any_element(),
        };
        let content = div()
            .id("content")
            .size_full()
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
            .child(wheel)
            .child(content)
            // The frames trace: the content drawn since "render".
            .child(
                canvas(
                    |_, _, _| {},
                    |_, _, _, _| log::debug!(target: "silent::frames", "painted"),
                )
                .absolute(),
            )
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
        Pane::Page(p) => p.read(cx).view(),
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

/// The window's corner radius: round while a compositor draws the corners
/// clear, square when fullscreen or over paper.
pub fn radius(window: &Window) -> Pixels {
    let square = PAPER.load(Ordering::Relaxed) || window.is_fullscreen();
    if ROUNDED.load(Ordering::Relaxed) && !square {
        px(RADIUS)
    } else {
        px(0.)
    }
}

/// Paints a square `quad` as far as it is in view, inside the window's
/// round corners, its border only on the sides in view.
pub fn paint_inside(quad: PaintQuad, window: &mut Window) {
    let (shown, corners) = inside(quad.bounds, px(0.), window);
    if shown.is_empty() {
        return;
    }
    let border = |cut: bool, width: Pixels| if cut { px(0.) } else { width };
    let widths = quad.border_widths;
    let border_widths = Edges {
        top: border(shown.top() > quad.bounds.top(), widths.top),
        right: border(shown.right() < quad.bounds.right(), widths.right),
        bottom: border(shown.bottom() < quad.bounds.bottom(), widths.bottom),
        left: border(shown.left() > quad.bounds.left(), widths.left),
    };
    window.paint_quad(PaintQuad {
        bounds: shown,
        corner_radii: corners,
        border_widths,
        ..quad
    });
}

/// `color` filling its parent, as far as it is in view, inside the window's
/// round corners.
pub fn filled(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint_inside(fill(bounds, color), window),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Paints `image` where `place` puts it, as far as it is in view: inside
/// the window's round corners, its own corners `own` round.
pub fn paint_picture(
    image: &Arc<RenderImage>,
    place: Bounds<Pixels>,
    own: Pixels,
    window: &mut Window,
) {
    let (shown, corners) = inside(place, own, window);
    // Whole, for the content mask to cut, unless a corner rounds it: as it
    // was drawn before, and out of view, so that the atlas has it before it
    // scrolls into view.
    let (bounds, corners) = if shown.is_empty() || corners == Corners::default() {
        (place, Corners::default())
    } else {
        (shown, corners)
    };
    let _ = window.paint_image(bounds, place, corners, image.clone(), 0, false);
}

/// Paints with `paint` what lies over `bounds` and cannot be rounded, as
/// text cannot, clear of the window's round corners: what reaches into them
/// is painted again for each row of device pixels there, each as far as
/// the corner leaves room.
pub fn clear_of_corners(
    bounds: Bounds<Pixels>,
    window: &mut Window,
    mut paint: impl FnMut(&mut Window),
) {
    let r = f32::from(radius(window));
    let bounds = bounds.intersect(&window.content_mask().bounds);
    let area = window.viewport_size();
    let (w, h) = (f32::from(area.width), f32::from(area.height));
    let sides = f32::from(bounds.left()) < r || f32::from(bounds.right()) > w - r;
    let ends = f32::from(bounds.top()) < r || f32::from(bounds.bottom()) > h - r;
    if bounds.is_empty() || !(sides && ends) {
        return paint(window);
    }
    // In device pixels: gpui widens a mask to whole ones, so each mask lies
    // a hair inside the pixels it is for, and every pixel is painted once.
    let scale = window.scale_factor();
    let (wide, tall, rows) = ((w * scale).round(), (h * scale).round(), (r * scale).ceil());
    let mut paint_in = |side: f32, top: f32, bottom: f32, window: &mut Window| {
        let hair = 0.01;
        let (left, right) = ((side + hair) / scale, (wide - side - hair) / scale);
        let mask = Bounds::from_corners(
            point(px(left), px((top + hair) / scale)),
            point(px(right), px((bottom - hair) / scale)),
        );
        if mask.intersects(&bounds) {
            window.with_content_mask(Some(ContentMask { bounds: mask }), |window| paint(window));
        }
    };
    paint_in(0., rows, tall - rows, window);
    for row in 0..rows as u32 {
        let (row, d) = (row as f32, row as f32 / scale);
        let side = ((r - (r * r - (r - d) * (r - d)).sqrt()) * scale).ceil();
        paint_in(side, row, row + 1., window);
        paint_in(side, tall - row - 1., tall - row, window);
    }
}

fn inside(
    bounds: Bounds<Pixels>,
    own: Pixels,
    window: &Window,
) -> (Bounds<Pixels>, Corners<Pixels>) {
    let shown = bounds.intersect(&window.content_mask().bounds);
    within(shown, bounds, window.viewport_size(), radius(window), own)
}

/// `shown`, the part of `bounds` in view in a window `area` large with
/// corners `radius` round, kept inside those corners: rounded as they are
/// where it meets them, less as it stands further in, and where it is too
/// thin to be that round, drawn in along its length until it fits. Its own
/// corners in view are at least `own` round.
fn within(
    shown: Bounds<Pixels>,
    bounds: Bounds<Pixels>,
    area: Size<Pixels>,
    radius: Pixels,
    own: Pixels,
) -> (Bounds<Pixels>, Corners<Pixels>) {
    let r = f32::from(radius);
    let half = f32::from(shown.size.width.min(shown.size.height)) / 2.;
    let wide = shown.size.width >= shown.size.height;
    let (left, top) = (f32::from(shown.left()), f32::from(shown.top()));
    let right = f32::from(area.width - shown.right());
    let bottom = f32::from(area.height - shown.bottom());
    // A corner `x` and `y` in from the window's edges: its radius, and how
    // far in from the window's edge along its length it has to start.
    let corner = |x: f32, y: f32| {
        let round = r - x.max(y);
        if round <= half {
            return (round.max(0.), 0.);
        }
        // As round as it can be, its centre on the circle the window's
        // corner leaves room for.
        let room = r - half;
        let across = if wide { y } else { x }.min(room);
        (half, room - (room * room - (room - across).powi(2)).sqrt())
    };
    let (tl, tr) = (corner(left, top), corner(right, top));
    let (br, bl) = (corner(right, bottom), corner(left, bottom));
    let (mut start, mut end) = (shown.origin, shown.bottom_right());
    if wide {
        start.x = px(left.max(tl.1).max(bl.1));
        end.x = area.width - px(right.max(tr.1).max(br.1));
    } else {
        start.y = px(top.max(tl.1).max(tr.1));
        end.y = area.height - px(bottom.max(bl.1).max(br.1));
    }
    let (own_left, own_top) = (shown.left() == bounds.left(), shown.top() == bounds.top());
    let (own_right, own_bottom) = (
        shown.right() == bounds.right(),
        shown.bottom() == bounds.bottom(),
    );
    let round = |(radius, _): (f32, f32), uncut: bool| {
        if uncut {
            px(radius).max(own)
        } else {
            px(radius)
        }
    };
    let corners = Corners {
        top_left: round(tl, own_left && own_top),
        top_right: round(tr, own_right && own_top),
        bottom_right: round(br, own_right && own_bottom),
        bottom_left: round(bl, own_left && own_bottom),
    };
    (Bounds::from_corners(start, end.max(&start)), corners)
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Bounds, Corners, Pixels, point, px, size};

    use super::within;

    const AREA: (f32, f32) = (400., 300.);

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Bounds<Pixels> {
        Bounds::from_corners(point(px(x0), px(y0)), point(px(x1), px(y1)))
    }

    fn keep(shown: Bounds<Pixels>, own: f32) -> (Bounds<Pixels>, Corners<Pixels>) {
        within(shown, shown, size(px(AREA.0), px(AREA.1)), px(11.), px(own))
    }

    fn radii(c: Corners<Pixels>) -> [f32; 4] {
        [c.top_left, c.top_right, c.bottom_right, c.bottom_left].map(f32::from)
    }

    #[test]
    fn a_band_along_an_edge_is_as_round_as_the_window_there() {
        let (shown, corners) = keep(rect(0., 0., 400., 24.), 0.);
        assert_eq!(shown, rect(0., 0., 400., 24.));
        assert_eq!(radii(corners), [11., 11., 0., 0.]);
        // A little in from the top: rounder corners would leave a gap.
        assert_eq!(radii(keep(rect(0., 5., 400., 30.), 0.).1), [6., 6., 0., 0.]);
        // Away from the corners, square.
        assert_eq!(radii(keep(rect(0., 40., 400., 60.), 0.).1), [0.; 4]);
        assert_eq!(radii(keep(rect(30., 0., 370., 24.), 0.).1), [0.; 4]);
    }

    #[test]
    fn a_line_too_thin_to_round_stops_short_of_the_corners() {
        // A hairline 2 px above the bottom edge, across the window.
        let (shown, corners) = keep(rect(0., 297., 400., 298.), 0.);
        let (start, end) = (f32::from(shown.left()), f32::from(shown.right()));
        assert!((start - 4.34).abs() < 0.01 && (end - (400. - 4.34)).abs() < 0.01);
        assert_eq!(radii(corners), [0., 0., 0.5, 0.5]);
        // Its rounded end touches the window's corner from inside.
        let centre: (f32, f32) = (start + 0.5, 297.5);
        let reach = ((11. - centre.0).powi(2) + (289. - centre.1).powi(2)).sqrt() + 0.5;
        assert!((reach - 11.).abs() < 0.01);
        // Down the right edge, it starts below the corner.
        let (shown, _) = keep(rect(395., 0., 396., 300.), 0.);
        assert!(f32::from(shown.top()) > 1. && f32::from(shown.bottom()) < 299.);
    }

    #[test]
    fn a_picture_keeps_its_own_round_corners_and_is_cut_round() {
        // Inside the window, its own; filling it, the window's.
        assert_eq!(radii(keep(rect(50., 40., 350., 260.), 11.).1), [11.; 4]);
        assert_eq!(radii(keep(rect(0., 0., 400., 300.), 11.).1), [11.; 4]);
        // Zoomed past the window's sides: cut square where the view cuts it
        // away from the corners.
        let picture = rect(-100., 40., 500., 260.);
        let shown = rect(0., 40., 400., 260.);
        let size = size(px(AREA.0), px(AREA.1));
        let (_, corners) = within(shown, picture, size, px(11.), px(11.));
        assert_eq!(radii(corners), [0.; 4]);
        // Past every side, the window's corners.
        let picture = rect(-100., -100., 500., 400.);
        let (_, corners) = within(rect(0., 0., 400., 300.), picture, size, px(11.), px(11.));
        assert_eq!(radii(corners), [11.; 4]);
    }

    #[test]
    fn square_windows_leave_everything_square() {
        let size = size(px(AREA.0), px(AREA.1));
        for shown in [rect(0., 0., 400., 24.), rect(0., 299., 400., 300.)] {
            let (kept, corners) = within(shown, shown, size, px(0.), px(0.));
            assert_eq!(kept, shown);
            assert_eq!(radii(corners), [0.; 4]);
        }
    }
}
