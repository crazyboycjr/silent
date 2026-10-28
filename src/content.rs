//! Loading what a preview shows, off the UI thread.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::component::highlighter::{Language, SyntaxHighlighter};
use gpui_kit::{Image, ImageFormat, ImageSource, RenderImage};
use gpui_video_player::{Video, VideoOptions};
use gstreamer as gst;
use gstreamer::prelude::*;
use silent::decode::{Decoder, Event};
use silent::detect::{self, Facts, Kind};
use silent::geometry::Screen;
use silent::markup::{self, Markup};
use silent::memory::{self, Key, View};
use silent::office::miniature::Miniature;
use silent::office::{self, doc, docx, odf, ppt, pptx};
use silent::{extract, hexdump, pretty, unpack};
use smallvec::SmallVec;
use tree_sitter::{InputEdit, Point};
use unicode_width::UnicodeWidthChar;

use crate::overview::{Family, Mode};
use crate::paged::RAIL;
use crate::picture::{self, Prepared};

/// Card sizes in logical pixels; they grow with the desktop scale.
const CARD: (f64, f64) = (600.0, 340.0);
const AUDIO_CARD: (f64, f64) = (640.0, 300.0);
/// Text beyond this is not read; the head is shown.
pub const TEXT_LIMIT: usize = 2 << 20;
/// Markdown and HTML past this open as their source, unless `s` asks: the
/// text view parses and lays out every block of a document before its first
/// frame, some 0.4 ms a kilobyte.
const RENDER_LIMIT: usize = 256 << 10;
/// The column rendered Markdown and HTML are read in, in logical pixels.
const COLUMN: f64 = 860.0;
/// The pixels a rendered document's pictures may take, decoded, in all.
const PICTURE_BUDGET: u64 = 48 << 20;
/// Columns a tab stretches to.
pub const TAB: usize = 4;

pub struct Loaded {
    pub facts: Facts,
    /// What the type is called, as in Nautilus's properties.
    pub description: String,
    /// Window size in root-window pixels.
    pub size: (i32, i32),
    /// The overview docked at the window's right, in logical pixels; none
    /// when it is zero.
    pub dock: f32,
    /// For a compressed file, what is known about the data inside.
    pub packed: Option<Packed>,
    pub body: Body,
    /// Where the file was left, if it was within the hour.
    pub view: Option<View>,
    /// The markup a Markdown or HTML file is in, which `s` shows rendered
    /// or as its source.
    pub markup: Option<Markup>,
    /// Quarter turns clockwise r gave a picture, a PDF or a video.
    pub turns: u8,
}

pub struct Packed {
    pub unpacked: Option<u64>,
}

pub enum Body {
    Image(Picture),
    /// A picture decoded here, such as the preview an iWork file carries.
    Still(Still),
    Folder(Folder),
    Video(Video),
    Audio(Audio),
    Pdf(Pdf),
    Text(Box<Text>),
    Rendered(Box<Rendered>),
    Hex(Hex),
    Archive(Archive),
    Document(Box<Paper>),
    Sheet(Box<Sheet>),
    Deck(Box<Deck>),
    Info,
}

pub struct Still {
    pub image: Arc<RenderImage>,
}

/// A word-processing document, and its column drawn small.
pub struct Paper {
    pub document: office::Document,
    pub miniature: Miniature,
}

pub struct Sheet {
    pub path: PathBuf,
    pub names: Vec<String>,
    /// The sheet to open on.
    pub current: usize,
    pub grid: office::sheet::Grid,
}

pub struct Deck {
    pub path: PathBuf,
    pub deck: office::pptx::Deck,
    /// The slide shown first, by its index, read before the window opens.
    pub first: Option<(usize, Slide)>,
}

/// A slide with its pictures decoded.
pub struct Slide {
    pub slide: office::pptx::Slide,
    /// Pictures by the index of their item.
    pub pictures: HashMap<usize, Arc<RenderImage>>,
    pub background: Option<Arc<RenderImage>>,
}

pub struct Picture {
    pub natural: (u32, u32),
    pub first: Option<Prepared>,
    /// Further frames of an animation, or the first frame when it was not
    /// waited for.
    pub frames: Decoder<Prepared>,
}

pub struct Folder {
    pub items: Option<u64>,
    pub totals: async_channel::Receiver<Totals>,
}

/// What a walk through a folder has found so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Totals {
    pub bytes: u64,
    pub files: u64,
    pub done: bool,
}

pub struct Audio {
    pub playbin: gst::Element,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub cover: Option<Arc<RenderImage>>,
}

pub struct Pdf {
    pub document: Arc<gpui_pdf::Document>,
    /// Page sizes in points, as the pages are shown, turned.
    pub pages: Vec<(f32, f32)>,
    /// The page shown first, by its index, rendered at the pages' width.
    pub first: Option<(usize, Arc<RenderImage>)>,
    /// Quarter turns clockwise of every page.
    pub turns: u8,
}

/// Markdown or HTML to render.
pub struct Rendered {
    pub text: String,
    pub markup: Markup,
    /// The pictures the text draws, by their URLs as written: those beside
    /// the file, decoded; the rest are their words in `text`.
    pub pictures: HashMap<String, Drawn>,
    /// Where the file is, for its relative links.
    pub dir: PathBuf,
}

/// A picture in a rendered document.
pub enum Drawn {
    Raster(Arc<RenderImage>),
    /// For gpui to draw.
    Svg(Arc<Image>),
}

impl Drawn {
    pub fn source(&self) -> ImageSource {
        match self {
            Drawn::Raster(image) => image.clone().into(),
            Drawn::Svg(image) => image.clone().into(),
        }
    }
}

pub struct Text {
    pub text: String,
    /// Byte range of each line, without its line break.
    pub lines: Vec<Range<usize>>,
    pub highlighter: Option<SyntaxHighlighter>,
    pub language: Option<&'static str>,
    /// Only the head of the file was read.
    pub truncated: bool,
    /// Columns of the widest line, tabs and wide characters counted.
    pub widest: usize,
    /// Laid out from minified JSON.
    pub pretty: bool,
}

/// Bytes shown as `hexdump -C` shows them.
pub struct Hex {
    pub bytes: Bytes,
    /// Only the head of the data is here.
    pub truncated: bool,
}

pub enum Bytes {
    /// A file mapped into memory, so that only the pages in view are read.
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Mapped(map) => map,
            Bytes::Owned(bytes) => bytes,
        }
    }
}

pub struct Archive {
    pub entries: Vec<Entry>,
    pub files: u64,
    pub bytes: u64,
}

pub struct Entry {
    pub path: String,
    pub size: u64,
    pub dir: bool,
}

/// Loads `uri`. With `wait_for_pixels` an image arrives with its first frame,
/// so that a window already showing something swaps straight to the new
/// picture; without it the image comes as soon as its size is known, so a
/// new window can open while it decodes.
pub fn load(uri: &str, screen: Screen, wait_for_pixels: bool) -> Loaded {
    load_as(uri, screen, wait_for_pixels, None, None)
}

/// [`load`], with Markdown and HTML rendered or shown as their source when
/// `rendered` says, else as they were left, else rendered; and a picture, a
/// PDF or a video turned by `turns` quarters, else as it was left.
pub fn load_as(
    uri: &str,
    screen: Screen,
    wait_for_pixels: bool,
    rendered: Option<bool>,
    turns: Option<u8>,
) -> Loaded {
    let facts = match detect::probe(uri) {
        Ok(facts) => facts,
        Err(error) => return missing(uri, &error.to_string(), screen),
    };
    let description = gio::content_type_get_description(&facts.content_type).into();
    let path = facts.path.clone();
    let key = path
        .as_deref()
        .and_then(|p| Key::new(p, facts.size, facts.modified));
    let view = key.as_ref().and_then(memory::recall);
    let turns = turns.unwrap_or_else(|| key.as_ref().map_or(0, memory::turns));
    let mut loaded = Loaded {
        size: card(screen, CARD),
        dock: 0.,
        packed: None,
        body: Body::Info,
        facts,
        description,
        view,
        markup: None,
        turns,
    };
    let resume = match loaded.view {
        Some(View::Media(position)) => Some(position),
        _ => None,
    };
    let page = match loaded.view {
        Some(View::Pages { page, .. }) => page,
        _ => 0,
    };
    let (size, dock, body) = match (loaded.facts.kind, path) {
        (Kind::Image, _) => match image(&loaded.facts.uri, screen, wait_for_pixels, turns) {
            Some((size, picture)) => (size, 0., Body::Image(picture)),
            None => return loaded,
        },
        (Kind::Folder, path) => (loaded.size, 0., Body::Folder(folder(path))),
        (Kind::Video, _) => match video(&loaded.facts.uri, resume) {
            Some(video) => {
                let (w, h) = video.size();
                let natural = (w.max(1) as u32, h.max(1) as u32);
                let size = media(screen, picture::turned(natural, turns));
                (size, 0., Body::Video(video))
            }
            None => return loaded,
        },
        (Kind::Audio, Some(path)) => match audio(&loaded.facts.uri, &path, resume) {
            Some(audio) => (card(screen, AUDIO_CARD), 0., Body::Audio(audio)),
            None => return loaded,
        },
        (Kind::Pdf, Some(path)) => match pdf(&path, screen, page, turns) {
            Some((size, dock, pdf)) => (size, dock, Body::Pdf(pdf)),
            None => return loaded,
        },
        (Kind::Text, Some(path)) => match read_head(&path) {
            Some((bytes, truncated)) => {
                loaded.markup = Markup::of(&path);
                let left_as_source = matches!(loaded.view, Some(View::Lines { .. }));
                let short = bytes.len() <= RENDER_LIMIT;
                match loaded
                    .markup
                    .filter(|_| !truncated && rendered.unwrap_or(short && !left_as_source))
                {
                    Some(markup) => {
                        let rendered = render(bytes, markup, &path, screen);
                        (
                            rendered_size(&rendered, screen),
                            0.,
                            Body::Rendered(Box::new(rendered)),
                        )
                    }
                    None => {
                        let text = text(bytes, truncated, &path, &loaded.facts.content_type);
                        let (size, dock) = text_size(&text, screen);
                        (size, dock, Body::Text(Box::new(text)))
                    }
                }
            }
            None => return loaded,
        },
        (Kind::Binary, Some(path)) => match mapped(&path) {
            Some(hex) => {
                let (size, dock) = hex_size(&hex, screen);
                (size, dock, Body::Hex(hex))
            }
            None => return loaded,
        },
        (Kind::Archive, Some(path)) => match archive(&path) {
            Some(archive) => (archive_size(&archive, screen), 0., Body::Archive(archive)),
            None => return loaded,
        },
        (Kind::Compressed, Some(path)) => match compressed(&path, &loaded.facts.name, screen) {
            Some((size, dock, body, packed)) => {
                loaded.packed = packed;
                (size, dock, body)
            }
            None => return loaded,
        },
        (kind @ (Kind::Document | Kind::Spreadsheet | Kind::Presentation), Some(path)) => {
            match document(
                kind,
                &path,
                &loaded.facts.content_type,
                screen,
                &loaded.view,
            ) {
                Some(shown) => shown,
                None => return loaded,
            }
        }
        _ => return loaded,
    };
    loaded.size = size;
    loaded.dock = dock as f32;
    loaded.body = body;
    loaded
}

impl Loaded {
    /// Nothing yet: what a window made ahead of its first file holds, at
    /// the size of a long text file's, the commonest preview.
    pub fn blank(screen: Screen) -> Self {
        let rows = 44.0;
        let (size, dock) = lines_window(usize::MAX, (860.0, rows * 21.0 + 40.0), screen);
        Loaded {
            facts: Facts {
                uri: String::new(),
                path: None,
                name: String::new(),
                size: 0,
                modified: None,
                content_type: String::new(),
                kind: Kind::Other,
            },
            description: String::new(),
            size,
            dock: dock as f32,
            packed: None,
            body: Body::Info,
            view: None,
            markup: None,
            turns: 0,
        }
    }

    /// Whether this is the blank of a window waiting for its next file.
    pub fn is_blank(&self) -> bool {
        self.facts.uri.is_empty()
    }
}

fn missing(uri: &str, error: &str, screen: Screen) -> Loaded {
    let name = uri.rsplit('/').next().unwrap_or(uri).to_owned();
    Loaded {
        facts: Facts {
            uri: uri.to_owned(),
            path: None,
            name,
            size: 0,
            modified: None,
            content_type: "application/octet-stream".into(),
            kind: Kind::Other,
        },
        description: error.to_owned(),
        size: card(screen, CARD),
        dock: 0.,
        packed: None,
        body: Body::Info,
        view: None,
        markup: None,
        turns: 0,
    }
}

/// The window for a card of `size` logical pixels.
fn card(screen: Screen, (width, height): (f64, f64)) -> (i32, i32) {
    screen.card(width, height, 0.0)
}

/// The window for a picture or a video of `natural` pixels.
fn media(screen: Screen, natural: (u32, u32)) -> (i32, i32) {
    let fit = screen.fit_media(natural);
    (fit.width, fit.height)
}

/// The rail docked beside `pages` pages, in logical pixels, unless M has
/// taken it away; a single page needs none.
fn rail_dock(pages: usize) -> f64 {
    if pages > 1 && Mode::load(Family::Rail) == Mode::On {
        f64::from(RAIL)
    } else {
        0.0
    }
}

/// A picture, its size and shape as r turned it.
fn image(
    uri: &str,
    screen: Screen,
    wait_for_pixels: bool,
    turns: u8,
) -> Option<((i32, i32), Picture)> {
    let frames = picture::decode(uri, screen, turns);
    let natural = match frames.events.recv_blocking().ok()? {
        Event::Size { width, height } => picture::turned((width, height), turns),
        _ => return None,
    };
    let first = if wait_for_pixels {
        match frames.events.recv_blocking().ok()? {
            Event::Frame(prepared) => Some(prepared),
            _ => return None,
        }
    } else {
        None
    };
    Some((
        media(screen, natural),
        Picture {
            natural,
            first,
            frames,
        },
    ))
}

/// Opening a video prerolls its pipeline, which takes a few hundred
/// milliseconds; that wait, and the seek to where it was left, happen here,
/// not on the UI thread.
fn video(uri: &str, resume: Option<Duration>) -> Option<Video> {
    let url = url::Url::parse(uri).ok()?;
    let options = VideoOptions {
        frame_buffer_capacity: Some(0),
        looping: Some(false),
        // Unset counts as 0, a rate GStreamer rejects.
        speed: Some(1.0),
    };
    let video = Video::new_with_options(&url, options).ok()?;
    if let Some(position) = resume {
        // The keyframe before the position: a little earlier, at once.
        let pipeline = video.pipeline();
        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_BEFORE;
        if pipeline.seek_simple(flags, clock(position)).is_ok() {
            let _ = pipeline.state(gst::ClockTime::from_mseconds(500));
            // The frame from before the seek is not shown.
            video.take_frame_ready();
        }
    }
    Some(video)
}

fn audio(uri: &str, path: &Path, resume: Option<Duration>) -> Option<Audio> {
    use lofty::prelude::*;
    gst::init().ok()?;
    let mute_video = gst::ElementFactory::make("fakesink").build().ok()?;
    let playbin = gst::ElementFactory::make("playbin")
        .property("uri", uri)
        .property("video-sink", &mute_video)
        .build()
        .ok()?;
    if let Some(position) = resume {
        let _ = playbin.set_state(gst::State::Paused);
        let _ = playbin.state(gst::ClockTime::from_mseconds(500));
        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE;
        let _ = playbin.seek_simple(flags, clock(position));
    }
    let tagged = lofty::read_from_path(path).ok();
    let tag = tagged
        .as_ref()
        .and_then(|t| t.primary_tag().or_else(|| t.first_tag()));
    let text = |value: Option<std::borrow::Cow<'_, str>>| {
        value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
    };
    let cover = tag
        .and_then(|tag| tag.pictures().first())
        .and_then(|picture| cover(picture.data()));
    Some(Audio {
        title: tag.and_then(|t| text(t.title())),
        artist: tag.and_then(|t| text(t.artist())),
        album: tag.and_then(|t| text(t.album())),
        duration: tagged.as_ref().map(|t| t.properties().duration()),
        cover,
        playbin,
    })
}

pub fn clock(position: Duration) -> gst::ClockTime {
    gst::ClockTime::from_nseconds(position.as_nanos() as u64)
}

/// Cover art is small and comes from inside the file, so it is decoded here
/// rather than in a sandboxed loader.
fn cover(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    decode_still(bytes, u32::MAX).map(|(image, _)| image)
}

/// A JPEG or PNG from inside a file, shrunk to at most `longest` pixels on
/// its long side, and its own size.
pub fn decode_still(bytes: &[u8], longest: u32) -> Option<(Arc<RenderImage>, (u32, u32))> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let natural = (decoded.width(), decoded.height());
    let decoded = if natural.0.max(natural.1) > longest {
        decoded.thumbnail(longest, longest)
    } else {
        decoded
    };
    let mut rgba = decoded.into_rgba8();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let image = RenderImage::new(SmallVec::from_elem(image::Frame::new(rgba), 1));
    Some((Arc::new(image), natural))
}

/// Office and iWork files. iWork's own formats are not read: the preview
/// picture they carry is shown, as Quick Look does.
fn document(
    kind: Kind,
    path: &Path,
    content_type: &str,
    screen: Screen,
    view: &Option<View>,
) -> Option<((i32, i32), f64, Body)> {
    let iwork = ["apple", "iwork"].iter().any(|k| content_type.contains(k));
    let open = content_type.contains("opendocument");
    // The binary formats of Office 97 to 2003.
    let ole = office::is_ole_file(path);
    if iwork {
        let bytes = office::iwork_preview(path)?;
        let longest = screen.monitor.width.max(screen.monitor.height) as u32;
        let (image, natural) = decode_still(&bytes, longest)?;
        return Some((media(screen, natural), 0., Body::Still(Still { image })));
    }
    match kind {
        Kind::Spreadsheet => {
            let (names, mut current, mut grid) = office::sheet::open(path)?;
            // Back on the sheet it was left on.
            if let Some(View::Sheet { sheet, .. }) = view
                && *sheet != current
                && let Some(name) = names.get(*sheet)
                && let Some(left) = office::sheet::read(path, name)
            {
                current = *sheet;
                grid = left;
            }
            let width: f64 = grid
                .widths
                .iter()
                .map(|w| (*w).clamp(4, 40) as f64 * 7.4 + 18.)
                .sum();
            let rows = grid.cells.len().clamp(8, 28) as f64;
            let size = card(screen, ((width + 60.).clamp(560., 1400.), rows * 24. + 70.));
            let sheet = Sheet {
                path: path.to_owned(),
                names,
                current,
                grid,
            };
            Some((size, 0., Body::Sheet(Box::new(sheet))))
        }
        Kind::Presentation if !open && !ole => {
            let deck = pptx::open(path)?;
            let dock = rail_dock(deck.slides.len());
            let slide = screen.fit_page((deck.size.0 as f64, deck.size.1 as f64), dock);
            let index = match view {
                Some(View::Pages { page, .. }) => (*page).min(deck.slides.len().saturating_sub(1)),
                _ => 0,
            };
            let first =
                render_slide(path, &deck, index, slide.0 as u32).map(|slide| (index, slide));
            let deck = Deck {
                path: path.to_owned(),
                deck,
                first,
            };
            Some((screen.window(slide, dock), dock, Body::Deck(Box::new(deck))))
        }
        _ => {
            let document = match kind {
                Kind::Presentation if ole => ppt::read(path)?,
                Kind::Presentation => odf::read_slides(path)?,
                _ if ole => doc::read(path)?,
                _ if open => odf::read_text(path)?,
                _ => docx::read(path)?,
            };
            let miniature = Miniature::of(&document);
            let page = (f64::from(document.page.0), f64::from(document.page.1));
            let docked = screen.fit_page(page, f64::from(RAIL));
            let long = Mode::load(Family::Rail) == Mode::On
                && f64::from(miniature.height) > shown_points(&document, docked, screen);
            let (dock, size) = if long {
                (f64::from(RAIL), docked)
            } else {
                (0.0, screen.fit_page(page, 0.0))
            };
            let paper = Paper {
                document,
                miniature,
            };
            Some((
                screen.window(size, dock),
                dock,
                Body::Document(Box::new(paper)),
            ))
        }
    }
}

/// How much of a document's column, in points, a view of `size` root-window
/// pixels shows at once: the page's width fills the view, below a margin
/// at the top.
fn shown_points(document: &office::Document, size: (i32, i32), screen: Screen) -> f64 {
    let (width, height) = (
        f64::from(size.0) / screen.scale,
        f64::from(size.1) / screen.scale,
    );
    let scale = width / f64::from(document.page.0.max(1.));
    let margin = (f64::from(document.margins.0) * scale).clamp(16., width / 4.);
    (height - margin.min(48.)) / scale
}

/// Reads a slide and decodes its pictures for a slide `width` pixels wide.
pub fn render_slide(path: &Path, deck: &pptx::Deck, index: usize, width: u32) -> Option<Slide> {
    let slide = pptx::slide(path, deck, index)?;
    let scale = width as f32 / deck.size.0.max(1.);
    let mut pictures = HashMap::new();
    for (i, item) in slide.items.iter().enumerate() {
        if let pptx::Kind::Picture(data) = &item.kind {
            let longest = (item.rect.w.max(item.rect.h) * scale).ceil() as u32;
            if let Some((image, _)) = decode_still(data, longest.max(16) * 2) {
                pictures.insert(i, image);
            }
        }
    }
    let background = match &slide.background {
        pptx::Fill::Image(data) => decode_still(data, width.max(16)).map(|(image, _)| image),
        _ => None,
    };
    Some(Slide {
        slide,
        pictures,
        background,
    })
}

/// A PDF, with page `page` rendered: the first, or the one it was left on.
/// Every page turned `turns` quarters: a scan made on its side.
fn pdf(path: &Path, screen: Screen, page: usize, turns: u8) -> Option<((i32, i32), f64, Pdf)> {
    let bytes = std::fs::read(path).ok()?;
    let document = gpui_pdf::parse(Arc::new(bytes)).ok()?;
    let mut pages = gpui_pdf::page_dims(&document);
    if turns % 2 == 1 {
        pages = pages.into_iter().map(|(w, h)| (h, w)).collect();
    }
    let &(width, height) = pages.first()?;
    let dock = rail_dock(pages.len());
    let shown = screen.fit_page((width as f64, height as f64), dock);
    let page = page.min(pages.len() - 1);
    let first = gpui_pdf::render_page(&document, page, shown.0 as f32 / pages[page].0)
        .ok()
        .map(|image| (page, picture::turn(image, turns)));
    Some((
        screen.window(shown, dock),
        dock,
        Pdf {
            document,
            pages,
            first,
            turns,
        },
    ))
}

/// Markdown or HTML to render, with the pictures beside the file decoded at
/// no more than the column's width, and the others, and those that cannot
/// be read, put as their words: silent fetches nothing.
fn render(bytes: Vec<u8>, markup: Markup, path: &Path, screen: Screen) -> Rendered {
    let text = match markup {
        Markup::Html => markup::without_head(&decode_text(&bytes)),
        Markup::Markdown => decode_text(&bytes),
    };
    let dir = path.parent().map(Path::to_owned).unwrap_or_default();
    let found = markup::pictures(&text, markup);
    let width = screen.card(COLUMN, 0.0, 0.0).0.max(1) as u32;
    let mut budget = PICTURE_BUDGET;
    let mut pictures = HashMap::new();
    for picture in &found {
        if pictures.contains_key(&picture.url) {
            continue;
        }
        if let Some(source) = picture_source(&picture.url, &dir, width, &mut budget) {
            pictures.insert(picture.url.clone(), source);
        }
    }
    let text = markup::rewrite(&text, &found, |p| pictures.contains_key(&p.url));
    crate::trace::mark("rendered pictures");
    Rendered {
        text,
        markup,
        pictures,
        dir,
    }
}

/// A picture beside the file or carried in a `data:` URL: raster pictures
/// decoded here, the first frame of an animation, SVG for gpui to draw.
fn picture_source(url: &str, dir: &Path, width: u32, budget: &mut u64) -> Option<Drawn> {
    let bytes = match url.strip_prefix("data:") {
        Some(data) => data_url(data)?,
        None => std::fs::read(markup::local_path(url, dir)?).ok()?,
    };
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]).to_ascii_lowercase();
    if head.contains("<svg") || (head.trim_start().starts_with("<?xml") && head.contains("svg")) {
        return Some(Drawn::Svg(Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            bytes,
        ))));
    }
    let decoded = image::load_from_memory(&bytes).ok()?;
    let decoded = if decoded.width() > width {
        decoded.thumbnail(width, u32::MAX)
    } else {
        decoded
    };
    *budget = budget.checked_sub(u64::from(decoded.width()) * u64::from(decoded.height()))?;
    let mut bgra = decoded.into_rgba8();
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let image = RenderImage::new(SmallVec::from_elem(image::Frame::new(bgra), 1));
    Some(Drawn::Raster(Arc::new(image)))
}

/// The bytes of a `data:` URL, past its `data:`.
fn data_url(data: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let (meta, payload) = data.split_once(',')?;
    if meta.ends_with(";base64") {
        let payload: String = payload
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()
    } else {
        Some(percent_encoding::percent_decode_str(payload).collect())
    }
}

/// A rendered document's window: the column, as tall as its source's lines
/// would make it, which pictures and headings outgrow.
fn rendered_size(rendered: &Rendered, screen: Screen) -> (i32, i32) {
    let rows = rendered
        .text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    let rows = (rows + rendered.pictures.len() * 6).clamp(12, 44) as f64;
    let zoom = f64::from(crate::zoom::level());
    card(screen, (COLUMN * zoom, (rows * 22.0 + 80.0) * zoom))
}

/// The first [`TEXT_LIMIT`] bytes, and whether there were more.
fn read_head(path: &Path) -> Option<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(TEXT_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let truncated = bytes.len() > TEXT_LIMIT;
    bytes.truncate(TEXT_LIMIT);
    Some((bytes, truncated))
}

/// `name` gives the language; for a compressed file it is the inner name.
fn text(bytes: Vec<u8>, mut truncated: bool, name: &Path, content_type: &str) -> Text {
    let mut text = decode_text(&bytes);
    drop(bytes);
    if truncated && let Some(end) = text.rfind('\n') {
        text.truncate(end);
    }
    let language = language(name, content_type);
    let pretty = language == Some("json") && pretty::minified(&text);
    if pretty {
        let (laid_out, cut) = pretty::json(&text, TEXT_LIMIT);
        text = laid_out;
        truncated |= cut;
    }
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, _) in text.match_indices('\n') {
        let end = if i > start && text.as_bytes()[i - 1] == b'\r' {
            i - 1
        } else {
            i
        };
        lines.push(start..end);
        start = i + 1;
    }
    if start < text.len() || lines.is_empty() {
        lines.push(start..text.len());
    }
    let widest = lines
        .iter()
        .map(|line| columns(&text[line.clone()]))
        .max()
        .unwrap_or(0);
    let highlighter = language.map(|language| {
        let highlighter = highlight(language, &text);
        crate::trace::mark("highlighted");
        highlighter
    });
    Text {
        text,
        lines,
        highlighter,
        language,
        truncated,
        widest,
        pretty,
    }
}

/// A highlighter made ahead for each language seen, and for the commonest,
/// with the text it was run over: making one compiles its language's
/// queries, some 10 ms, and the first run over a text compiles those of the
/// languages inside it, such as Markdown in Rust's doc comments and Rust in
/// its macros, 10 ms more. One run over a text like the next keeps both.
struct Ready {
    highlighter: SyntaxHighlighter,
    /// Where the text it ran over ends.
    end: (usize, Point),
}

static READY: Mutex<Vec<Ready>> = Mutex::new(Vec::new());

/// The languages a resident previewer makes highlighters for as it starts,
/// with a text to run them over first.
const COMMON: [(&str, &str); 8] = [
    (
        "rust",
        "/// A `doc` comment.\nfn f() {\n    println!(\"{}\", vec![1]);\n}\n",
    ),
    ("python", ""),
    ("javascript", ""),
    ("json", ""),
    ("markdown", "# Title\n\nSome *text* with `code`.\n"),
    ("bash", ""),
    ("c", ""),
    ("toml", ""),
];

/// How much of a file the next highlighter for its language runs over.
const PRIMER: usize = 64 << 10;

/// `text` highlighted in `language`, by the highlighter made ahead when
/// there is one; the next is made in the background, run over this text.
fn highlight(language: &'static str, text: &str) -> SyntaxHighlighter {
    let ready = {
        let mut ready = READY.lock().unwrap();
        ready
            .iter()
            .position(|r| r.highlighter.language().as_ref() == language)
            .map(|i| ready.swap_remove(i))
    };
    let mut head = &text[..text.len().min(PRIMER)];
    while !text.is_char_boundary(head.len()) {
        head = &head[..head.len() - 1];
    }
    prepare_highlighter(language, head.to_owned());
    let rope = ropey::Rope::from_str(text);
    match ready {
        // The text it ran over is replaced whole: what it knows of the
        // languages inside stays.
        Some(Ready {
            mut highlighter,
            end,
        }) => {
            let edit = InputEdit {
                start_byte: 0,
                old_end_byte: end.0,
                new_end_byte: text.len(),
                start_position: Point::new(0, 0),
                old_end_position: end.1,
                new_end_position: end_of(text),
            };
            highlighter.update(Some(edit), &rope, None);
            highlighter
        }
        None => {
            let mut highlighter = SyntaxHighlighter::new(language);
            highlighter.update(None, &rope, None);
            highlighter
        }
    }
}

fn prepare_highlighter(language: &'static str, primer: String) {
    let _ = std::thread::Builder::new()
        .name("highlighter".into())
        .spawn(move || {
            let mut highlighter = SyntaxHighlighter::new(language);
            highlighter.update(None, &ropey::Rope::from_str(&primer), None);
            let end = (primer.len(), end_of(&primer));
            READY.lock().unwrap().push(Ready { highlighter, end });
        });
}

/// Where `text` ends, as tree-sitter counts: rows, and bytes into the last.
fn end_of(text: &str) -> Point {
    let row = text.matches('\n').count();
    let column = text.len() - text.rfind('\n').map_or(0, |i| i + 1);
    Point::new(row, column)
}

/// Prepares, in the background and once, what the next previews of a
/// resident previewer would wait for: highlighters for the commonest
/// languages, and GStreamer with the plugins playback takes, some 60 ms. It
/// would slow a preview being opened, so it waits for none to be.
pub fn prepare() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for (language, primer) in COMMON {
            if Language::from_str(language) != Language::Plain {
                prepare_highlighter(language, primer.to_owned());
            }
        }
        let _ = std::thread::Builder::new().name("media".into()).spawn(|| {
            if gst::init().is_ok() {
                let _ = gst::ElementFactory::make("playbin").build();
            }
        });
    });
}

/// How many columns `line` takes, tabs stretched to the next stop.
pub fn columns(line: &str) -> usize {
    line.chars().fold(0, |column, c| match c {
        '\t' => (column / TAB + 1) * TAB,
        c => column + c.width().unwrap_or(0),
    })
}

/// Wrapped text is as tall as the rows its lines wrap into at the window's
/// width. The window holds as many characters at any text size.
fn text_size(text: &Text, screen: Screen) -> ((i32, i32), f64) {
    let zoom = crate::zoom::level();
    let width = f64::from(screen.card(860.0 * f64::from(zoom), 0.0, 0.0).0) / screen.scale;
    let rows = crate::lines::rows_for(text, width as f32, zoom);
    lines_window(rows, (860.0, rows.clamp(8, 44) as f64 * 21.0 + 40.0), screen)
}

/// The window for `lines` lines of text or a hexdump that take `size`
/// logical pixels at 100%, and the minimap docked beside them once M has
/// pinned it and they need scrolling.
fn lines_window(lines: usize, size: (f64, f64), screen: Screen) -> ((i32, i32), f64) {
    let zoom = crate::zoom::level();
    let (width, height) = (size.0 * f64::from(zoom), size.1 * f64::from(zoom));
    let shown = screen.card(width, height, 0.0);
    let tall = f64::from(shown.1) / screen.scale;
    let dock = if Mode::load(Family::Minimap) == Mode::On
        && crate::lines::scrolls(lines, tall as f32, zoom)
    {
        f64::from(crate::lines::minimap_width(screen.scale as f32))
    } else {
        return (shown, 0.0);
    };
    (screen.window(screen.card(width, height, dock), dock), dock)
}

/// UTF-8 when it is valid, the encoding a byte-order mark names, and
/// otherwise GB18030, which covers the GBK text Chinese Windows writes.
fn decode_text(bytes: &[u8]) -> String {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        return encoding
            .decode_without_bom_handling(&bytes[bom..])
            .0
            .into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(e) if e.error_len().is_none() => String::from_utf8_lossy(bytes).into_owned(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

/// The highlighter's language for a file, by extension and then by type.
fn language(path: &Path, content_type: &str) -> Option<&'static str> {
    let by_name = |name: &str| match Language::from_str(name) {
        Language::Plain => None,
        language => Some(language.name()),
    };
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    extension
        .as_deref()
        .and_then(|ext| match ext {
            "h" => Some("c"),
            "hpp" | "hh" | "cc" | "cxx" => Some("cpp"),
            "mjs" | "cjs" | "jsx" => Some("javascript"),
            "zsh" | "bash" => Some("bash"),
            "mk" => Some("make"),
            "patch" => Some("diff"),
            "htm" | "xhtml" => Some("html"),
            "jsonl" | "ndjson" | "geojson" => Some("json"),
            ext => by_name(ext),
        })
        .or(match file_name {
            "Makefile" | "makefile" | "GNUmakefile" => Some("make"),
            "CMakeLists.txt" => Some("cmake"),
            "PKGBUILD" | ".bashrc" | ".zshrc" | ".profile" => Some("bash"),
            _ => None,
        })
        .or(match content_type {
            "application/x-shellscript" | "text/x-shellscript" => Some("bash"),
            "text/x-python" | "text/x-python3" => Some("python"),
            "application/json" => Some("json"),
            _ => None,
        })
}

/// Maps the file, so that a dump of gigabytes opens at once.
fn mapped(path: &Path) -> Option<Hex> {
    let file = File::open(path).ok()?;
    // Safety: the map is only read. A file truncated while it is shown would
    // fault the read, as it would for any program that maps files.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some(Hex {
        bytes: Bytes::Mapped(map),
        truncated: false,
    })
}

fn hex_size(hex: &Hex, screen: Screen) -> ((i32, i32), f64) {
    let lines = hexdump::line_count(hex.bytes.len() as u64);
    let rows = lines.clamp(8, 44) as f64;
    let digits = hexdump::offset_digits(hex.bytes.len() as u64) as f64;
    let size = (600.0 + digits * 7.6, rows * 21.0 + 40.0);
    lines_window(lines as usize, size, screen)
}

/// What is inside a compressed file: text, a tar's listing, or the bytes.
fn compressed(
    path: &Path,
    name: &str,
    screen: Screen,
) -> Option<((i32, i32), f64, Body, Option<Packed>)> {
    let head = unpack::head(path, TEXT_LIMIT + 1).ok()?;
    if head.bytes.get(257..262) == Some(b"ustar") {
        let archive = archive(path)?;
        return Some((
            archive_size(&archive, screen),
            0.,
            Body::Archive(archive),
            None,
        ));
    }
    let unpacked = if head.complete {
        Some(head.bytes.len() as u64)
    } else {
        unpack::recorded_size(path).filter(|&size| size > head.bytes.len() as u64)
    };
    let packed = Some(Packed { unpacked });
    let inner = unpack::inner_name(name);
    let sample = &head.bytes[..head.bytes.len().min(4096)];
    let (content_type, _) = gio::content_type_guess(Some(inner), sample);
    let is_text = detect::classify(&content_type, sample) == Kind::Text;
    let truncated = !head.complete || head.bytes.len() > TEXT_LIMIT;
    let mut bytes = head.bytes;
    bytes.truncate(TEXT_LIMIT);
    if is_text {
        let text = text(bytes, truncated, Path::new(inner), &content_type);
        let (size, dock) = text_size(&text, screen);
        return Some((size, dock, Body::Text(Box::new(text)), packed));
    }
    let hex = Hex {
        bytes: Bytes::Owned(bytes),
        truncated,
    };
    let (size, dock) = hex_size(&hex, screen);
    Some((size, dock, Body::Hex(hex), packed))
}

fn archive(path: &Path) -> Option<Archive> {
    let file = File::open(path).ok()?;
    let listed =
        compress_tools::list_archive_entries_with_encoding(file, extract::decode_name).ok()?;
    let mut archive = Archive {
        entries: Vec::with_capacity(listed.len()),
        files: 0,
        bytes: 0,
    };
    for entry in listed {
        let dir = entry.path.ends_with('/');
        if !dir {
            archive.files += 1;
            archive.bytes += entry.size;
        }
        archive.entries.push(Entry {
            path: entry.path.trim_end_matches('/').to_owned(),
            size: entry.size,
            dir,
        });
    }
    Some(archive)
}

fn archive_size(archive: &Archive, screen: Screen) -> (i32, i32) {
    let rows = archive.entries.len().clamp(6, 26) as f64;
    card(screen, (720.0, rows * 30.0 + 96.0))
}

fn folder(path: Option<PathBuf>) -> Folder {
    let (sender, totals) = async_channel::unbounded();
    let items = path
        .as_ref()
        .and_then(|p| std::fs::read_dir(p).ok())
        .map(|entries| entries.count() as u64);
    if let Some(path) = path {
        std::thread::Builder::new()
            .name("folder".into())
            .spawn(move || walk(path, &sender))
            .expect("spawning the folder walker");
    }
    Folder { items, totals }
}

/// Adds up a folder's files without following links, reporting every
/// tenth of a second until done or until nobody listens.
fn walk(root: PathBuf, sender: &async_channel::Sender<Totals>) {
    let mut totals = Totals::default();
    let mut pending = vec![root];
    let mut reported = Instant::now();
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                totals.files += 1;
                totals.bytes += metadata.len();
            }
        }
        if reported.elapsed() > Duration::from_millis(100) {
            if sender.send_blocking(totals).is_err() {
                return;
            }
            reported = Instant::now();
        }
    }
    totals.done = true;
    let _ = sender.send_blocking(totals);
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};

    use super::{COMMON, Ready, end_of, highlight, prepare_highlighter};

    #[test]
    fn a_text_ends_where_tree_sitter_counts() {
        assert_eq!(end_of(""), tree_sitter::Point::new(0, 0));
        assert_eq!(end_of("ab"), tree_sitter::Point::new(0, 2));
        assert_eq!(end_of("ab\n"), tree_sitter::Point::new(1, 0));
        assert_eq!(end_of("ab\ncdé"), tree_sitter::Point::new(1, 4));
    }

    #[test]
    fn a_highlighter_made_ahead_colours_as_a_new_one_does() {
        let text = include_str!("geometry.rs");
        let theme = HighlightTheme::default_dark();
        let fresh = {
            let mut fresh = SyntaxHighlighter::new("rust");
            fresh.update(None, &ropey::Rope::from_str(text), None);
            fresh.styles(&(0..text.len()), &*theme)
        };
        // One made ahead over the warm-up's sample, then one over this file.
        let (_, primer) = COMMON
            .iter()
            .find(|(language, _)| *language == "rust")
            .unwrap();
        for primer in [primer.to_string(), text[..text.len() / 2].to_string()] {
            prepare_highlighter("rust", primer);
            while !super::READY
                .lock()
                .unwrap()
                .iter()
                .any(|r: &Ready| r.highlighter.language().as_ref() == "rust")
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let made_ahead = highlight("rust", text);
            assert_eq!(made_ahead.styles(&(0..text.len()), &*theme), fresh);
        }
    }
}
