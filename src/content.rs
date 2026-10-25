//! Loading what a preview shows, off the UI thread.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use silent::decode::{Decoder, Event};
use silent::detect::{self, Facts, Kind};
use silent::geometry::Screen;

use crate::picture::{self, Prepared};

/// Card sizes in logical pixels; they grow with the desktop scale.
const CARD: (f64, f64) = (600.0, 340.0);

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
    match loaded.facts.kind {
        Kind::Image => {
            if let Some((size, picture)) = image(&loaded.facts.uri, screen, wait_for_pixels) {
                loaded.size = size;
                loaded.body = Body::Image(picture);
            }
        }
        Kind::Folder => loaded.body = Body::Folder(folder(loaded.facts.path.clone())),
        _ => {}
    }
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
