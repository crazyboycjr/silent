//! Loading what a preview shows, off the UI thread.

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
use silent::decode::{Decoder, Event};
use silent::detect::{self, Facts, Kind};
use silent::geometry::Screen;
use smallvec::SmallVec;

use crate::picture::{self, Prepared};

/// Card sizes in logical pixels; they grow with the desktop scale.
const CARD: (f64, f64) = (600.0, 340.0);
const AUDIO_CARD: (f64, f64) = (640.0, 300.0);
/// Text beyond this is not read; the head is shown.
const TEXT_LIMIT: usize = 2 << 20;

pub struct Loaded {
    pub facts: Facts,
    /// What the type is called, as in Nautilus's properties.
    pub description: String,
    /// Window size in root-window pixels.
    pub size: (i32, i32),
    pub body: Body,
}

pub enum Body {
    Image(Picture),
    Folder(Folder),
    Video(Video),
    Audio(Audio),
    Pdf(Pdf),
    Text(Box<Text>),
    Archive(Archive),
    Info,
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
    /// The first page, rendered at the window's width.
    pub first: Option<Arc<RenderImage>>,
}

pub struct Text {
    pub text: String,
    /// Byte range of each line, without its line break.
    pub lines: Vec<Range<usize>>,
    pub highlighter: Option<SyntaxHighlighter>,
    pub language: Option<&'static str>,
    /// Only the head of the file was read.
    pub truncated: bool,
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
    let mut loaded = Loaded {
        size: screen.card(CARD.0, CARD.1),
        body: Body::Info,
        facts,
        description,
    };
    let path = loaded.facts.path.clone();
    let (size, body) = match (loaded.facts.kind, path) {
        (Kind::Image, _) => match image(&loaded.facts.uri, screen, wait_for_pixels) {
            Some((size, picture)) => (size, Body::Image(picture)),
            None => return loaded,
        },
        (Kind::Folder, path) => (loaded.size, Body::Folder(folder(path))),
        (Kind::Video, _) => match video(&loaded.facts.uri) {
            Some(video) => {
                let (w, h) = video.size();
                let fit = screen.fit_media((w.max(1) as u32, h.max(1) as u32));
                ((fit.width, fit.height), Body::Video(video))
            }
            None => return loaded,
        },
        (Kind::Audio, Some(path)) => match audio(&loaded.facts.uri, &path) {
            Some(audio) => (screen.card(AUDIO_CARD.0, AUDIO_CARD.1), Body::Audio(audio)),
            None => return loaded,
        },
        (Kind::Pdf, Some(path)) => match pdf(&path, screen) {
            Some((size, pdf)) => (size, Body::Pdf(pdf)),
            None => return loaded,
        },
        (Kind::Text, Some(path)) => match text(&path, &loaded.facts.content_type) {
            Some(text) => {
                let rows = text.lines.len().clamp(8, 44) as f64;
                (
                    screen.card(860.0, rows * 21.0 + 40.0),
                    Body::Text(Box::new(text)),
                )
            }
            None => return loaded,
        },
        (Kind::Archive, Some(path)) => match archive(&path) {
            Some(archive) => {
                let rows = archive.entries.len().clamp(6, 26) as f64;
                (
                    screen.card(720.0, rows * 30.0 + 96.0),
                    Body::Archive(archive),
                )
            }
            None => return loaded,
        },
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
        size: screen.card(CARD.0, CARD.1),
        body: Body::Info,
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
    let fit = screen.fit_media(natural);
    Some((
        (fit.width, fit.height),
        Picture {
            natural,
            first,
            frames,
        },
    ))
}

/// Opening a video prerolls its pipeline, which takes a few hundred
/// milliseconds; that wait happens here, not on the UI thread.
fn video(uri: &str) -> Option<Video> {
    let url = url::Url::parse(uri).ok()?;
    let options = VideoOptions {
        frame_buffer_capacity: Some(0),
        looping: Some(false),
        // Unset counts as 0, a rate GStreamer rejects.
        speed: Some(1.0),
    };
    Video::new_with_options(&url, options).ok()
}

fn audio(uri: &str, path: &Path) -> Option<Audio> {
    use lofty::prelude::*;
    gst::init().ok()?;
    let mute_video = gst::ElementFactory::make("fakesink").build().ok()?;
    let playbin = gst::ElementFactory::make("playbin")
        .property("uri", uri)
        .property("video-sink", &mute_video)
        .build()
        .ok()?;
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

/// Cover art is small and comes from inside the file, so it is decoded here
/// rather than in a sandboxed loader.
fn cover(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(rgba),
        1,
    ))))
}

fn pdf(path: &Path, screen: Screen) -> Option<((i32, i32), Pdf)> {
    let bytes = std::fs::read(path).ok()?;
    let document = gpui_pdf::parse(Arc::new(bytes)).ok()?;
    let pages = gpui_pdf::page_dims(&document);
    let &(width, height) = pages.first()?;
    let size = screen.fit_page((width as f64, height as f64));
    let first = gpui_pdf::render_page(&document, 0, size.0 as f32 / width).ok();
    Some((
        size,
        Pdf {
            document,
            pages,
            first,
        },
    ))
}

fn text(path: &Path, content_type: &str) -> Option<Text> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(TEXT_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let truncated = bytes.len() > TEXT_LIMIT;
    let mut text = decode_text(&bytes[..bytes.len().min(TEXT_LIMIT)]);
    if truncated && let Some(end) = text.rfind('\n') {
        text.truncate(end);
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
    let language = language(path, content_type);
    let highlighter = language.map(|language| {
        let mut highlighter = SyntaxHighlighter::new(language);
        highlighter.update(None, &ropey::Rope::from_str(&text), None);
        highlighter
    });
    Some(Text {
        text,
        lines,
        highlighter,
        language,
        truncated,
    })
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
