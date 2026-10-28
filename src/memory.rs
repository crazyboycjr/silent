//! Where each file was left, so that coming back to it within the hour shows
//! the same place: video and sound carry on, documents open at the page or
//! line, sheets on the sheet and cell, archives with the same folders open.
//! Beside it, the few choices made with keys, such as whether the minimap
//! shows. All of it lives in `$XDG_RUNTIME_DIR/silent`, a tmpfs that is
//! private to the user and empty after a reboot.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a view is remembered.
const KEEP: Duration = Duration::from_secs(3600);
/// Positions this close to the start are not worth coming back to.
const TOO_EARLY: Duration = Duration::from_secs(2);
/// Files remembered at most; the oldest go first.
const MOST: usize = 256;
/// Open folders remembered per archive.
const OPEN_MOST: usize = 512;

/// A file as it was when its view was taken: the same path with another
/// size or modification time is another file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub path: PathBuf,
    pub size: u64,
    pub modified: u128,
}

impl Key {
    pub fn new(path: &Path, size: u64, modified: Option<SystemTime>) -> Option<Self> {
        Some(Self {
            path: fs::canonicalize(path).ok()?,
            size,
            modified: modified?.duration_since(UNIX_EPOCH).ok()?.as_nanos(),
        })
    }
}

/// Where a preview was left, by the kind of view.
#[derive(Clone, Debug, PartialEq)]
pub enum View {
    /// Video and sound: the playing position.
    Media(Duration),
    /// Text, code and hexdumps: the first line in view, with the fraction of
    /// it scrolled past, and the columns scrolled sideways. For Markdown and
    /// HTML, also that `s` showed the source.
    Lines { top: f64, side: f32 },
    /// Markdown and HTML rendered: the block at the top and the pixels
    /// scrolled into it.
    Rendered { block: usize, offset: f32 },
    /// A page laid out by Blitz: how far down it the view starts, as a
    /// share of its height, which another text size or width keeps close.
    Page { within: f32 },
    /// PDFs and slides: the page at the top, how far down it the view
    /// starts, from 0 to 1, and where across the pages the view's middle
    /// is, which counts once they are zoomed wider than the window.
    Pages {
        page: usize,
        within: f32,
        across: f32,
    },
    /// A zoomed picture: the point of it in the middle of the view, as
    /// shares of its width and height.
    Picture { x: f32, y: f32 },
    /// Word documents: the block at the top and the pixels scrolled into it.
    Blocks { block: usize, offset: f32 },
    /// Spreadsheets: the sheet, the cell the cursor is on, and the scroll.
    Sheet {
        sheet: usize,
        cell: Option<(u32, u32)>,
        top: f64,
        side: f32,
    },
    /// Archives: the pixels the list is scrolled, the selected entry and
    /// the open folders, by their paths in the archive.
    Tree {
        top: usize,
        selected: Option<String>,
        open: Vec<String>,
    },
}

impl View {
    /// One field without tabs or line breaks. A bare number is a playing
    /// position, as the first version of this file wrote it.
    fn encode(&self) -> String {
        match self {
            View::Media(position) => position.as_millis().to_string(),
            View::Lines { top, side } => format!("lines {top} {side}"),
            View::Pages {
                page,
                within,
                across,
            } => format!("pages {page} {within} {across}"),
            View::Picture { x, y } => format!("picture {x} {y}"),
            View::Blocks { block, offset } => format!("blocks {block} {offset}"),
            View::Rendered { block, offset } => format!("rendered {block} {offset}"),
            View::Page { within } => format!("page {within}"),
            View::Sheet {
                sheet,
                cell,
                top,
                side,
            } => {
                let cell = cell.map_or("-".to_owned(), |(r, c)| format!("{r},{c}"));
                format!("sheet {sheet} {cell} {top} {side}")
            }
            View::Tree {
                top,
                selected,
                open,
            } => {
                let mut out = format!("tree {top}");
                if let Some(selected) = selected {
                    out.push_str(" *");
                    out.push_str(&escape(selected));
                }
                for folder in open.iter().take(OPEN_MOST) {
                    out.push(' ');
                    out.push_str(&escape(folder));
                }
                out
            }
        }
    }

    fn decode(field: &str) -> Option<Self> {
        if let Ok(ms) = field.parse::<u64>() {
            return Some(View::Media(Duration::from_millis(ms)));
        }
        let mut words = field.split(' ');
        let kind = words.next()?;
        let mut next = || words.next().ok_or(());
        let view = (|| -> Result<View, ()> {
            let number = |w: &str| w.parse::<f64>().map_err(drop);
            Ok(match kind {
                "lines" => View::Lines {
                    top: number(next()?)?,
                    side: number(next()?)? as f32,
                },
                // Written before pages zoomed without where across.
                "pages" => View::Pages {
                    page: next()?.parse().map_err(drop)?,
                    within: number(next()?)? as f32,
                    across: next().map_or(Ok(0.5), number)? as f32,
                },
                "picture" => View::Picture {
                    x: number(next()?)? as f32,
                    y: number(next()?)? as f32,
                },
                "blocks" => View::Blocks {
                    block: next()?.parse().map_err(drop)?,
                    offset: number(next()?)? as f32,
                },
                "rendered" => View::Rendered {
                    block: next()?.parse().map_err(drop)?,
                    offset: number(next()?)? as f32,
                },
                "page" => View::Page {
                    within: number(next()?)? as f32,
                },
                "sheet" => View::Sheet {
                    sheet: next()?.parse().map_err(drop)?,
                    cell: match next()? {
                        "-" => None,
                        cell => {
                            let (r, c) = cell.split_once(',').ok_or(())?;
                            Some((r.parse().map_err(drop)?, c.parse().map_err(drop)?))
                        }
                    },
                    top: number(next()?)?,
                    side: number(next()?)? as f32,
                },
                "tree" => {
                    let top = next()?.parse().map_err(drop)?;
                    let mut selected = None;
                    let mut open = Vec::new();
                    while let Ok(word) = next() {
                        match word.strip_prefix('*') {
                            Some(id) => selected = Some(unescape(id)?),
                            None => open.push(unescape(word)?),
                        }
                    }
                    View::Tree {
                        top,
                        selected,
                        open,
                    }
                }
                _ => return Err(()),
            })
        })();
        view.ok()
    }
}

/// `%`, the separators and a leading `*` written as `%XX`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '%' | ' ' | '\t' | '\n' | '\r' | '*' => out.push_str(&format!("%{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> Result<String, ()> {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next().ok_or(())?, bytes.next().ok_or(())?];
            let hex = std::str::from_utf8(&hex).map_err(drop)?;
            out.push(u8::from_str_radix(hex, 16).map_err(drop)?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).map_err(drop)
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    key: Key,
    saved: u64,
    view: Option<View>,
    /// Quarter turns clockwise r gave a picture, a PDF or a video.
    turns: u8,
    /// The zoom of a PDF, slides or a picture, in percent of its fit.
    zoom: u16,
}

/// The view field: `zoomed N` and `turned N` before the view when the file
/// was zoomed or turned.
fn encode(view: &Option<View>, turns: u8, zoom: u16) -> String {
    let mut words = Vec::new();
    if zoom != 100 {
        words.push(format!("zoomed {zoom}"));
    }
    if turns != 0 {
        words.push(format!("turned {turns}"));
    }
    words.extend(view.as_ref().map(View::encode));
    words.join(" ")
}

fn decode(field: &str) -> Option<(Option<View>, u8, u16)> {
    let (zoom, field) = match field.strip_prefix("zoomed ") {
        Some(rest) => {
            let (zoom, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            let zoom = zoom
                .parse::<u16>()
                .ok()
                .filter(|z| (10..=1000).contains(z))?;
            (zoom, rest)
        }
        None => (100, field),
    };
    let (turns, field) = match field.strip_prefix("turned ") {
        Some(rest) => {
            let (turns, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            (turns.parse::<u8>().ok().filter(|t| *t < 4)?, rest)
        }
        None => (0, field),
    };
    let view = if field.is_empty() {
        None
    } else {
        Some(View::decode(field)?)
    };
    (view.is_some() || turns != 0 || zoom != 100).then_some((view, turns, zoom))
}

fn dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(runtime).join("silent"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How `key` was left within the last hour.
pub fn recall(key: &Key) -> Option<View> {
    let text = fs::read_to_string(dir()?.join("positions")).ok()?;
    find(&parse(&text), key, now())?.view.clone()
}

/// The quarter turns `key` was given within the last hour.
pub fn turns(key: &Key) -> u8 {
    let text = dir().and_then(|dir| fs::read_to_string(dir.join("positions")).ok());
    text.and_then(|text| find(&parse(&text), key, now()).map(|e| e.turns))
        .unwrap_or(0)
}

/// The zoom `key`, a PDF, slides or a picture, was left at within the last
/// hour, 1 at its fit.
pub fn zoom(key: &Key) -> f32 {
    let text = dir().and_then(|dir| fs::read_to_string(dir.join("positions")).ok());
    text.and_then(|text| find(&parse(&text), key, now()).map(|e| f32::from(e.zoom) / 100.))
        .unwrap_or(1.)
}

/// Records how `key` was left, and at what zoom, or forgets it: a view that
/// is where a new preview starts anyway is not worth a line. Its turns stay.
pub fn remember(key: &Key, view: Option<View>, zoom: f32) {
    store(key, |entry| {
        entry.view = view;
        entry.zoom = (zoom * 100.).round().clamp(10., 1000.) as u16;
    });
}

/// Records the quarter turns `key` was given; its view stays.
pub fn turn(key: &Key, turns: u8) {
    store(key, |entry| entry.turns = turns % 4);
}

fn store(key: &Key, change: impl FnOnce(&mut Entry)) {
    let Some(dir) = dir() else {
        return;
    };
    let text = fs::read_to_string(dir.join("positions")).unwrap_or_default();
    let before = parse(&text);
    let mut entry = find(&before, key, now()).cloned().unwrap_or(Entry {
        key: key.clone(),
        saved: 0,
        view: None,
        turns: 0,
        zoom: 100,
    });
    change(&mut entry);
    entry.saved = now();
    let entry = (entry.view.is_some() || entry.turns != 0 || entry.zoom != 100).then_some(entry);
    let updated = update(before.clone(), key, entry, now());
    if updated != before {
        write(&dir, "positions", &format(&updated));
    }
}

/// Whether a playing position is worth coming back to: not at the very
/// start, and not in the last few seconds, which for short clips means the
/// last tenth.
pub fn worth_resuming(position: Duration, duration: Option<Duration>) -> bool {
    if position < TOO_EARLY {
        return false;
    }
    match duration.filter(|d| !d.is_zero()) {
        Some(duration) => {
            let tail = (duration / 10).clamp(Duration::from_secs(1), Duration::from_secs(5));
            position + tail < duration
        }
        None => true,
    }
}

/// A choice made with a key, kept until the next reboot.
pub fn setting(name: &str) -> Option<String> {
    let text = fs::read_to_string(dir()?.join("settings")).ok()?;
    settings(&text)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, value)| value)
}

pub fn set_setting(name: &str, value: &str) {
    let Some(dir) = dir() else {
        return;
    };
    let text = fs::read_to_string(dir.join("settings")).unwrap_or_default();
    let mut all: Vec<(String, String)> = settings(&text)
        .into_iter()
        .filter(|(n, _)| n != name)
        .collect();
    all.push((name.to_owned(), value.to_owned()));
    let out: String = all.iter().map(|(n, v)| format!("{n} {v}\n")).collect();
    write(&dir, "settings", &out);
}

fn settings(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(' ')?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// Replaces `name` in `dir` whole, so that a reader never sees half of it.
fn write(dir: &Path, name: &str, text: &str) {
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let temporary = dir.join(format!("{name}.{}", std::process::id()));
    if fs::write(&temporary, text).is_ok() {
        let _ = fs::rename(&temporary, dir.join(name));
    }
}

fn find<'a>(entries: &'a [Entry], key: &Key, now: u64) -> Option<&'a Entry> {
    entries
        .iter()
        .find(|e| &e.key == key && now.saturating_sub(e.saved) < KEEP.as_secs())
}

fn update(entries: Vec<Entry>, key: &Key, entry: Option<Entry>, now: u64) -> Vec<Entry> {
    let mut kept: Vec<Entry> = entries
        .into_iter()
        .filter(|e| e.key.path != key.path && now.saturating_sub(e.saved) < KEEP.as_secs())
        .collect();
    kept.extend(entry);
    let extra = kept.len().saturating_sub(MOST);
    kept.drain(..extra);
    kept
}

/// One entry per line: saved, size, modified, the view, and the path last.
fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let saved = fields.next()?.parse().ok()?;
            let size = fields.next()?.parse().ok()?;
            let modified = fields.next()?.parse().ok()?;
            let (view, turns, zoom) = decode(fields.next()?)?;
            let path = PathBuf::from(fields.next()?);
            Some(Entry {
                key: Key {
                    path,
                    size,
                    modified,
                },
                saved,
                view,
                turns,
                zoom,
            })
        })
        .collect()
}

fn format(entries: &[Entry]) -> String {
    entries
        .iter()
        .filter_map(|e| {
            let path = e.key.path.to_str().filter(|p| !p.contains('\n'))?;
            Some(format!(
                "{}\t{}\t{}\t{}\t{path}\n",
                e.saved,
                e.key.size,
                e.key.modified,
                encode(&e.view, e.turns, e.zoom)
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str) -> Key {
        Key {
            path: path.into(),
            size: 1000,
            modified: 1_729_000_000_123_456_789,
        }
    }

    fn entry(path: &str, saved: u64, view: View) -> Entry {
        Entry {
            key: key(path),
            saved,
            view: Some(view),
            turns: 0,
            zoom: 100,
        }
    }

    fn pages(page: usize, within: f32) -> View {
        View::Pages {
            page,
            within,
            across: 0.5,
        }
    }

    #[test]
    fn turns_ride_along_with_the_view_or_alone() {
        let turned = Entry {
            turns: 3,
            ..entry("/p/photo.jpg", 100, pages(2, 0.5))
        };
        let alone = Entry {
            view: None,
            turns: 1,
            ..entry("/p/clip.mp4", 200, media(1))
        };
        let entries = vec![turned, alone];
        assert_eq!(parse(&format(&entries)), entries);
        assert!(format(&entries).contains("\tturned 1\t/p/clip.mp4"));
        // Only quarter turns.
        assert_eq!(parse("1\t2\t3\tturned 4\t/p/x.jpg\n"), vec![]);
    }

    #[test]
    fn a_zoom_rides_along_with_the_turns_and_the_view_or_alone() {
        let zoomed = Entry {
            zoom: 150,
            turns: 1,
            ..entry(
                "/p/scan.pdf",
                100,
                View::Pages {
                    page: 3,
                    within: 0.25,
                    across: 0.75,
                },
            )
        };
        let picture = Entry {
            zoom: 400,
            ..entry("/p/photo.jpg", 200, View::Picture { x: 0.25, y: 0.5 })
        };
        let alone = Entry {
            view: None,
            zoom: 60,
            ..entry("/p/deck.pptx", 300, media(1))
        };
        let entries = vec![zoomed, picture, alone];
        let text = format(&entries);
        assert_eq!(parse(&text), entries);
        assert!(text.contains("\tzoomed 150 turned 1 pages 3 0.25 0.75\t/p/scan.pdf"));
        assert!(text.contains("\tzoomed 60\t/p/deck.pptx"));
        // Pages written before they zoomed are in the middle across.
        let old = parse("1\t2\t3\tturned 2 pages 4 0.5\t/p/old.pdf\n");
        assert_eq!(old[0].view, Some(pages(4, 0.5)));
        assert_eq!((old[0].turns, old[0].zoom), (2, 100));
    }

    fn media(secs: u64) -> View {
        View::Media(Duration::from_secs(secs))
    }

    #[test]
    fn every_view_round_trips_through_the_file() {
        let entries = vec![
            entry("/a/movie.mkv", 100, media(42)),
            entry("/b/song\twith tab.flac", 200, media(7)),
            entry(
                "/c/big.rs",
                300,
                View::Lines {
                    top: 1520.25,
                    side: 12.5,
                },
            ),
            entry("/d/paper.pdf", 400, pages(11, 0.375)),
            entry(
                "/e/report.docx",
                500,
                View::Blocks {
                    block: 40,
                    offset: 18.5,
                },
            ),
            entry(
                "/e/README.md",
                550,
                View::Rendered {
                    block: 12,
                    offset: 31.5,
                },
            ),
            entry("/e/class.SocketClient.html", 575, View::Page { within: 0.25 }),
            entry(
                "/f/sales.xlsx",
                600,
                View::Sheet {
                    sheet: 2,
                    cell: Some((120, 3)),
                    top: 101.,
                    side: 240.,
                },
            ),
            entry(
                "/f/budget.ods",
                650,
                View::Sheet {
                    sheet: 0,
                    cell: None,
                    top: 0.,
                    side: 0.,
                },
            ),
            entry(
                "/g/project.zip",
                700,
                View::Tree {
                    top: 3,
                    selected: Some("*star/with space%".into()),
                    open: vec!["project".into(), "project/新 建\t文件夹".into()],
                },
            ),
            entry(
                "/g/empty.zip",
                800,
                View::Tree {
                    top: 0,
                    selected: None,
                    open: vec![],
                },
            ),
        ];
        assert_eq!(parse(&format(&entries)), entries);
    }

    #[test]
    fn positions_from_the_first_version_still_read() {
        let line = "100\t1000\t1729000000123456789\t42000\t/a/movie.mkv\n";
        assert_eq!(parse(line), vec![entry("/a/movie.mkv", 100, media(42))]);
        // A line nothing can read is skipped, not the file.
        let text = format!("garbage\n100\t1\t2\tpages x 0.5\t/p.pdf\n{line}");
        assert_eq!(parse(&text).len(), 1);
    }

    #[test]
    fn views_last_an_hour_and_only_for_the_same_file() {
        let entries = vec![entry("/a/movie.mkv", 1000, media(42))];
        let at = |now| find(&entries, &key("/a/movie.mkv"), now).and_then(|e| e.view.clone());
        assert_eq!(at(1000 + 3599), Some(media(42)));
        assert_eq!(at(1000 + 3600), None);
        let mut changed = key("/a/movie.mkv");
        changed.size += 1;
        assert_eq!(find(&entries, &changed, 1001), None);
    }

    #[test]
    fn updates_replace_the_file_and_drop_stale_entries() {
        let entries = vec![
            entry("/a/movie.mkv", 1000, media(42)),
            entry("/old.mkv", 10, media(5)),
            entry("/other.mkv", 4000, media(9)),
        ];
        let updated = update(
            entries.clone(),
            &key("/a/movie.mkv"),
            Some(entry("/a/movie.mkv", 4500, media(50))),
            4500,
        );
        assert_eq!(
            updated,
            vec![
                entry("/other.mkv", 4000, media(9)),
                entry("/a/movie.mkv", 4500, media(50))
            ]
        );
        // Forgetting a file drops its line.
        let forgotten = update(entries, &key("/a/movie.mkv"), None, 4500);
        assert_eq!(forgotten, vec![entry("/other.mkv", 4000, media(9))]);
    }

    #[test]
    fn the_newest_files_are_kept() {
        let many: Vec<Entry> = (0..MOST as u64 + 10)
            .map(|i| entry(&format!("/f{i}"), 1000 + i, media(5)))
            .collect();
        let updated = update(
            many,
            &key("/new"),
            Some(entry("/new", 2000, media(5))),
            2000,
        );
        assert_eq!(updated.len(), MOST);
        assert_eq!(updated.last().unwrap().key.path, PathBuf::from("/new"));
        assert_eq!(updated[0].key.path, PathBuf::from("/f11"));
    }

    #[test]
    fn the_start_and_the_end_are_not_resumed() {
        let minutes = |m: u64| Some(Duration::from_secs(m * 60));
        assert!(!worth_resuming(Duration::from_secs(1), minutes(10)));
        assert!(worth_resuming(Duration::from_secs(30), minutes(10)));
        assert!(!worth_resuming(Duration::from_secs(596), minutes(10)));
        assert!(worth_resuming(Duration::from_secs(594), minutes(10)));
        // A ten-second clip: the last second counts as the end.
        let clip = Some(Duration::from_secs(10));
        assert!(worth_resuming(Duration::from_millis(8500), clip));
        assert!(!worth_resuming(Duration::from_millis(9200), clip));
        assert!(worth_resuming(Duration::from_secs(30), None));
    }

    #[test]
    fn settings_read_back_by_name() {
        let text = "minimap off\nrail on\n";
        assert_eq!(
            settings(text),
            vec![
                ("minimap".into(), "off".into()),
                ("rail".into(), "on".into())
            ]
        );
    }
}
