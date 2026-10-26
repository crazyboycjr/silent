//! Loading what a preview shows, off the UI thread.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::RenderImage;
use gpui_kit::component::highlighter::{Language, SyntaxHighlighter};
use gpui_video_player::{Video, VideoOptions};
use gstreamer as gst;
use gstreamer::prelude::*;
use silent::decode::{Decoder, Event};
use silent::detect::{self, Facts, Kind};
use silent::geometry::Screen;
use silent::memory::{self, Key, View};
use silent::office::{self, doc, docx, odf, ppt, pptx};
use silent::{hexdump, pretty, unpack};
use smallvec::SmallVec;
use unicode_width::UnicodeWidthChar;

use crate::overview::Mode;
use crate::paged::RAIL;
use crate::picture::{self, Prepared};

/// Card sizes in logical pixels, above the row; they grow with the desktop
/// scale.
const CARD: (f64, f64) = (600.0, 340.0);
const AUDIO_CARD: (f64, f64) = (640.0, 300.0);
/// Text beyond this is not read; the head is shown.
pub const TEXT_LIMIT: usize = 2 << 20;
/// Columns a tab stretches to.
pub const TAB: usize = 4;

pub struct Loaded {
    pub facts: Facts,
    /// What the type is called, as in Nautilus's properties.
    pub description: String,
    /// Window size in root-window pixels.
    pub size: (i32, i32),
    /// For a compressed file, what is known about the data inside.
    pub packed: Option<Packed>,
    pub body: Body,
    /// Where the file was left, if it was within the hour.
    pub view: Option<View>,
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
    Hex(Hex),
    Archive(Archive),
    Document(Box<office::Document>),
    Sheet(Box<Sheet>),
    Deck(Box<Deck>),
    Info,
}

pub struct Still {
    pub image: Arc<RenderImage>,
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
    /// The slides' width in the window, in root-window pixels.
    pub width: u32,
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
    /// Page sizes in points.
    pub pages: Vec<(f32, f32)>,
    /// The page shown first, by its index, rendered at the pages' width.
    pub first: Option<(usize, Arc<RenderImage>)>,
    /// The pages' width in the window, in root-window pixels.
    pub width: f32,
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
    let facts = match detect::probe(uri) {
        Ok(facts) => facts,
        Err(error) => return missing(uri, &error.to_string(), screen),
    };
    let description = gio::content_type_get_description(&facts.content_type).into();
    let path = facts.path.clone();
    let view = path
        .as_deref()
        .and_then(|p| Key::new(p, facts.size, facts.modified))
        .and_then(|key| memory::recall(&key));
    let mut loaded = Loaded {
        size: card(screen, CARD),
        packed: None,
        body: Body::Info,
        facts,
        description,
        view,
    };
    let resume = match loaded.view {
        Some(View::Media(position)) => Some(position),
        _ => None,
    };
    let page = match loaded.view {
        Some(View::Pages { page, .. }) => page,
        _ => 0,
    };
    let (size, body) = match (loaded.facts.kind, path) {
        (Kind::Image, _) => match image(&loaded.facts.uri, screen, wait_for_pixels) {
            Some((size, picture)) => (size, Body::Image(picture)),
            None => return loaded,
        },
        (Kind::Folder, path) => (loaded.size, Body::Folder(folder(path))),
        (Kind::Video, _) => match video(&loaded.facts.uri, resume) {
            Some(video) => {
                let (w, h) = video.size();
                let size = media(screen, (w.max(1) as u32, h.max(1) as u32));
                (size, Body::Video(video))
            }
            None => return loaded,
        },
        (Kind::Audio, Some(path)) => match audio(&loaded.facts.uri, &path, resume) {
            Some(audio) => (card(screen, AUDIO_CARD), Body::Audio(audio)),
            None => return loaded,
        },
        (Kind::Pdf, Some(path)) => match pdf(&path, screen, page) {
            Some((size, pdf)) => (size, Body::Pdf(pdf)),
            None => return loaded,
        },
        (Kind::Text, Some(path)) => match read_head(&path) {
            Some((bytes, truncated)) => {
                let text = text(bytes, truncated, &path, &loaded.facts.content_type);
                (text_size(&text, screen), Body::Text(Box::new(text)))
            }
            None => return loaded,
        },
        (Kind::Binary, Some(path)) => match mapped(&path) {
            Some(hex) => (hex_size(&hex, screen), Body::Hex(hex)),
            None => return loaded,
        },
        (Kind::Archive, Some(path)) => match archive(&path) {
            Some(archive) => (archive_size(&archive, screen), Body::Archive(archive)),
            None => return loaded,
        },
        (Kind::Compressed, Some(path)) => match compressed(&path, &loaded.facts.name, screen) {
            Some((size, body, packed)) => {
                loaded.packed = packed;
                (size, body)
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
    loaded.body = body;
    loaded
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
        packed: None,
        body: Body::Info,
        view: None,
    }
}

/// The window for a card of `size` logical pixels.
fn card(screen: Screen, (width, height): (f64, f64)) -> (i32, i32) {
    screen.window(screen.card(width, height, 0.0), 0.0)
}

/// The window for a picture or a video of `natural` pixels.
fn media(screen: Screen, natural: (u32, u32)) -> (i32, i32) {
    let fit = screen.fit_media(natural);
    screen.window((fit.width, fit.height), 0.0)
}

/// The column beside a document of `pages` pages, in logical pixels: the
/// rail's, or the scrollbar's once M has turned the rail off; none beside
/// a single page, which fits.
fn rail_column(pages: usize) -> f64 {
    if pages > 1 {
        f64::from(Mode::load("rail").column(RAIL))
    } else {
        0.0
    }
}

fn image(uri: &str, screen: Screen, wait_for_pixels: bool) -> Option<((i32, i32), Picture)> {
    let frames = picture::decode(uri, screen);
    let natural = match frames.events.recv_blocking().ok()? {
        Event::Size { width, height } => (width, height),
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
) -> Option<((i32, i32), Body)> {
    let iwork = ["apple", "iwork"].iter().any(|k| content_type.contains(k));
    let open = content_type.contains("opendocument");
    // The binary formats of Office 97 to 2003.
    let ole = office::is_ole_file(path);
    if iwork {
        let bytes = office::iwork_preview(path)?;
        let longest = screen.monitor.width.max(screen.monitor.height) as u32;
        let (image, natural) = decode_still(&bytes, longest)?;
        return Some((media(screen, natural), Body::Still(Still { image })));
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
            Some((size, Body::Sheet(Box::new(sheet))))
        }
        Kind::Presentation if !open && !ole => {
            let deck = pptx::open(path)?;
            let column = rail_column(deck.slides.len());
            let slide = screen.fit_page((deck.size.0 as f64, deck.size.1 as f64), column);
            let index = match view {
                Some(View::Pages { page, .. }) => (*page).min(deck.slides.len().saturating_sub(1)),
                _ => 0,
            };
            let width = slide.0 as u32;
            let first = render_slide(path, &deck, index, width).map(|slide| (index, slide));
            let deck = Deck {
                path: path.to_owned(),
                deck,
                first,
                width,
            };
            Some((screen.window(slide, column), Body::Deck(Box::new(deck))))
        }
        _ => {
            let document = match kind {
                Kind::Presentation if ole => ppt::read(path)?,
                Kind::Presentation => odf::read_slides(path)?,
                _ if ole => doc::read(path)?,
                _ if open => odf::read_text(path)?,
                _ => docx::read(path)?,
            };
            let (w, h) = document.page;
            let page = screen.fit_page((w as f64, h as f64), 0.0);
            Some((screen.window(page, 0.0), Body::Document(Box::new(document))))
        }
    }
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
fn pdf(path: &Path, screen: Screen, page: usize) -> Option<((i32, i32), Pdf)> {
    let bytes = std::fs::read(path).ok()?;
    let document = gpui_pdf::parse(Arc::new(bytes)).ok()?;
    let pages = gpui_pdf::page_dims(&document);
    let &(width, height) = pages.first()?;
    let column = rail_column(pages.len());
    let shown = screen.fit_page((width as f64, height as f64), column);
    let page = page.min(pages.len() - 1);
    let width = shown.0 as f32;
    let first = gpui_pdf::render_page(&document, page, width / pages[page].0)
        .ok()
        .map(|image| (page, image));
    Some((
        screen.window(shown, column),
        Pdf {
            document,
            pages,
            first,
            width,
        },
    ))
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
        let mut highlighter = SyntaxHighlighter::new(language);
        highlighter.update(None, &ropey::Rope::from_str(&text), None);
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

/// How many columns `line` takes, tabs stretched to the next stop.
pub fn columns(line: &str) -> usize {
    line.chars().fold(0, |column, c| match c {
        '\t' => (column / TAB + 1) * TAB,
        c => column + c.width().unwrap_or(0),
    })
}

fn text_size(text: &Text, screen: Screen) -> (i32, i32) {
    let rows = text.lines.len().clamp(8, 44) as f64;
    lines_window(text.lines.len(), (860.0, rows * 21.0 + 40.0), screen)
}

/// The window for `lines` lines of text or a hexdump in `size` logical
/// pixels, with the minimap's column beside them when they need scrolling.
fn lines_window(lines: usize, (width, height): (f64, f64), screen: Screen) -> (i32, i32) {
    let shown = screen.card(width, height, 0.0).1 as f64 / screen.scale;
    let column = if crate::lines::scrolls(lines, shown as f32) {
        let minimap = crate::lines::minimap_width(screen.scale as f32);
        f64::from(Mode::load("minimap").column(minimap))
    } else {
        0.0
    };
    screen.window(screen.card(width, height, column), column)
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

fn hex_size(hex: &Hex, screen: Screen) -> (i32, i32) {
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
) -> Option<((i32, i32), Body, Option<Packed>)> {
    let head = unpack::head(path, TEXT_LIMIT + 1).ok()?;
    if head.bytes.get(257..262) == Some(b"ustar") {
        let archive = archive(path)?;
        return Some((archive_size(&archive, screen), Body::Archive(archive), None));
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
        return Some((text_size(&text, screen), Body::Text(Box::new(text)), packed));
    }
    let hex = Hex {
        bytes: Bytes::Owned(bytes),
        truncated,
    };
    Some((hex_size(&hex, screen), Body::Hex(hex), packed))
}

fn archive(path: &Path) -> Option<Archive> {
    let file = File::open(path).ok()?;
    let listed = compress_tools::list_archive_entries_with_encoding(file, decode_name).ok()?;
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

fn decode_name(bytes: &[u8]) -> compress_tools::Result<String> {
    Ok(match std::str::from_utf8(bytes) {
        Ok(name) => name.to_owned(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    })
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
