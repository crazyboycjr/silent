//! Where video and audio were left, so that coming back to the same file
//! within the hour carries on from there. The only state silent keeps: one
//! small file in `$XDG_RUNTIME_DIR/silent`, a tmpfs that is private to the
//! user and empty after a reboot.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a position is remembered.
const KEEP: Duration = Duration::from_secs(3600);
/// Positions this close to the start are not worth coming back to.
const TOO_EARLY: Duration = Duration::from_secs(2);

/// A file as it was when its position was taken: the same path with another
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

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    key: Key,
    saved: u64,
    position: Duration,
}

fn store() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(runtime).join("silent").join("positions"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Where `key` was left within the last hour.
pub fn recall(key: &Key) -> Option<Duration> {
    let text = fs::read_to_string(store()?).ok()?;
    find(&parse(&text), key, now())
}

/// Records where `key` was left, or forgets it when it was barely begun or
/// nearly over, so that it starts from the beginning next time.
pub fn remember(key: &Key, position: Duration, duration: Option<Duration>) {
    let Some(path) = store() else {
        return;
    };
    let text = fs::read_to_string(&path).unwrap_or_default();
    let entry = keep(position, duration).then(|| Entry {
        key: key.clone(),
        saved: now(),
        position,
    });
    let updated = update(parse(&text), key, entry, now());
    if updated.is_empty() && text.is_empty() {
        return;
    }
    let Some(dir) = path.parent() else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let temporary = dir.join(format!("positions.{}", std::process::id()));
    if fs::write(&temporary, format(&updated)).is_ok() {
        let _ = fs::rename(&temporary, &path);
    }
}

/// Whether a position is worth coming back to: not at the very start, and
/// not in the last few seconds, which for short clips means the last tenth.
fn keep(position: Duration, duration: Option<Duration>) -> bool {
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

fn find(entries: &[Entry], key: &Key, now: u64) -> Option<Duration> {
    entries
        .iter()
        .find(|e| &e.key == key && now.saturating_sub(e.saved) < KEEP.as_secs())
        .map(|e| e.position)
}

fn update(entries: Vec<Entry>, key: &Key, entry: Option<Entry>, now: u64) -> Vec<Entry> {
    let mut kept: Vec<Entry> = entries
        .into_iter()
        .filter(|e| e.key.path != key.path && now.saturating_sub(e.saved) < KEEP.as_secs())
        .collect();
    kept.extend(entry);
    kept
}

/// One entry per line: saved, size, modified, position in ms, and the path last.
fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let saved = fields.next()?.parse().ok()?;
            let size = fields.next()?.parse().ok()?;
            let modified = fields.next()?.parse().ok()?;
            let position = Duration::from_millis(fields.next()?.parse().ok()?);
            let path = PathBuf::from(fields.next()?);
            Some(Entry {
                key: Key {
                    path,
                    size,
                    modified,
                },
                saved,
                position,
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
                e.position.as_millis()
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

    fn entry(path: &str, saved: u64, secs: u64) -> Entry {
        Entry {
            key: key(path),
            saved,
            position: Duration::from_secs(secs),
        }
    }

    #[test]
    fn entries_round_trip_through_the_file() {
        let entries = vec![
            entry("/a/movie.mkv", 100, 42),
            entry("/b/song\twith tab.flac", 200, 7),
        ];
        assert_eq!(parse(&format(&entries)), entries);
    }

    #[test]
    fn positions_last_an_hour_and_only_for_the_same_file() {
        let entries = vec![entry("/a/movie.mkv", 1000, 42)];
        let at = |now| find(&entries, &key("/a/movie.mkv"), now);
        assert_eq!(at(1000 + 3599), Some(Duration::from_secs(42)));
        assert_eq!(at(1000 + 3600), None);
        let mut changed = key("/a/movie.mkv");
        changed.size += 1;
        assert_eq!(find(&entries, &changed, 1001), None);
    }

    #[test]
    fn updates_replace_the_file_and_drop_stale_entries() {
        let entries = vec![
            entry("/a/movie.mkv", 1000, 42),
            entry("/old.mkv", 10, 5),
            entry("/other.mkv", 4000, 9),
        ];
        let updated = update(
            entries,
            &key("/a/movie.mkv"),
            Some(entry("/a/movie.mkv", 4500, 50)),
            4500,
        );
        assert_eq!(
            updated,
            vec![
                entry("/other.mkv", 4000, 9),
                entry("/a/movie.mkv", 4500, 50)
            ]
        );
    }

    #[test]
    fn the_start_and_the_end_are_not_kept() {
        let minutes = |m: u64| Some(Duration::from_secs(m * 60));
        assert!(!keep(Duration::from_secs(1), minutes(10)));
        assert!(keep(Duration::from_secs(30), minutes(10)));
        assert!(!keep(Duration::from_secs(596), minutes(10)));
        assert!(keep(Duration::from_secs(594), minutes(10)));
        // A ten-second clip: the last second counts as the end.
        let clip = Some(Duration::from_secs(10));
        assert!(keep(Duration::from_millis(8500), clip));
        assert!(!keep(Duration::from_millis(9200), clip));
        assert!(keep(Duration::from_secs(30), None));
    }
}
