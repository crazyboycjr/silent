//! Files taken out of archives to be previewed. An entry is streamed out
//! of libarchive into a folder of its own, under the runtime directory (a
//! tmpfs) while it is small and under the cache directory once it is not,
//! and the folder goes when the preview of it closes. Each process keeps
//! its folders under its id, so the next start can sweep away those of a
//! process that crashed.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use compress_tools::{ArchiveContents, ArchiveIteratorBuilder};

/// Entries up to this size go to the runtime directory's tmpfs.
pub const SMALL: u64 = 64 << 20;
/// Entries larger than this are not taken out.
pub const MOST: u64 = 4 << 30;

/// A file taken out of an archive; its folder is removed when it drops.
#[derive(Debug)]
pub struct Extracted {
    pub path: PathBuf,
    folder: PathBuf,
}

impl Drop for Extracted {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// Why an entry was not taken out.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    Encrypted,
    TooLarge(u64),
    Missing,
    Cancelled,
    Failed(String),
}

/// Where taken-out files go: small ones, and large ones.
#[derive(Clone, Debug)]
pub struct Places {
    pub small: PathBuf,
    pub large: PathBuf,
    /// The size past which a file moves from `small` to `large`.
    pub limit: u64,
}

impl Places {
    /// `$XDG_RUNTIME_DIR/silent/extract` and `$XDG_CACHE_HOME/silent/extract`.
    pub fn standard() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"));
        let runtime =
            std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| cache.clone(), PathBuf::from);
        Self {
            small: runtime.join("silent/extract"),
            large: cache.join("silent/extract"),
            limit: SMALL,
        }
    }

    fn mine(&self, large: bool) -> PathBuf {
        let base = if large { &self.large } else { &self.small };
        base.join(std::process::id().to_string())
    }
}

/// An archive's names in UTF-8, or else in GB18030, which covers the GBK
/// that Chinese Windows writes into zip files.
pub fn decode_name(bytes: &[u8]) -> compress_tools::Result<String> {
    Ok(match std::str::from_utf8(bytes) {
        Ok(name) => name.to_owned(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    })
}

/// A path in an archive without empty parts, as the tree shows it.
pub fn normalize(path: &str) -> String {
    path.split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// Takes `entry` out of the archive at `archive`, which lists it as `size`
/// bytes: the bytes written so far go to `progress`, and setting `cancel`
/// stops it.
pub fn extract(
    archive: &Path,
    entry: &str,
    size: u64,
    progress: &AtomicU64,
    cancel: &AtomicBool,
    places: &Places,
) -> Result<Extracted, Refusal> {
    if size > MOST {
        return Err(Refusal::TooLarge(size));
    }
    let failed = |error: &dyn std::fmt::Display| Refusal::Failed(error.to_string());
    let file = File::open(archive).map_err(|e| failed(&e))?;
    let wanted = normalize(entry);
    let name = wanted.rsplit('/').next().unwrap_or("entry").to_owned();
    let mut contents = ArchiveIteratorBuilder::new(file)
        .decoder(decode_name)
        .filter(move |path, _| normalize(path) == wanted)
        .build()
        .map_err(|e| refusal(&e))?;
    let mut out: Option<Spill> = None;
    for content in &mut contents {
        match content {
            ArchiveContents::StartOfEntry(_, stat) => {
                let size = (stat.st_size.max(0) as u64).max(size);
                if size > MOST {
                    return Err(Refusal::TooLarge(size));
                }
                out = Some(Spill::new(places, &name, size).map_err(|e| failed(&e))?);
            }
            ArchiveContents::DataChunk(bytes) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Refusal::Cancelled);
                }
                let Some(out) = &mut out else { continue };
                out.write_all(&bytes).map_err(|e| failed(&e))?;
                if out.written > MOST {
                    return Err(Refusal::TooLarge(out.written));
                }
                progress.store(out.written, Ordering::Relaxed);
            }
            ArchiveContents::EndOfEntry => break,
            ArchiveContents::Err(error) => return Err(refusal(&error)),
        }
    }
    out.map(Spill::finish).ok_or(Refusal::Missing)
}

/// libarchive says why it stopped only in words.
fn refusal(error: &compress_tools::Error) -> Refusal {
    let words = error.to_string();
    let lower = words.to_lowercase();
    if ["passphrase", "encrypt", "decrypt"]
        .iter()
        .any(|w| lower.contains(w))
    {
        Refusal::Encrypted
    } else {
        Refusal::Failed(words)
    }
}

/// Numbers each process's folders.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A file being written in the small place, moved to the large one once it
/// outgrows it. Until it is finished it goes, folder and all, if it drops.
struct Spill {
    places: Places,
    name: String,
    out: Extracted,
    file: File,
    written: u64,
    large: bool,
}

impl Spill {
    fn new(places: &Places, name: &str, size: u64) -> io::Result<Self> {
        let large = size > places.limit;
        let (out, file) = open(places, large, name)?;
        Ok(Self {
            places: places.clone(),
            name: name.to_owned(),
            out,
            file,
            written: 0,
            large,
        })
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if !self.large && self.written + bytes.len() as u64 > self.places.limit {
            // Its listed size was wrong or missing: on to the large place.
            let (out, mut file) = open(&self.places, true, &self.name)?;
            self.file.flush()?;
            io::copy(&mut File::open(&self.out.path)?, &mut file)?;
            (self.out, self.file, self.large) = (out, file, true);
        }
        self.file.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn finish(self) -> Extracted {
        self.out
    }
}

/// `name` in a new folder of this process's, opened for writing.
fn open(places: &Places, large: bool, name: &str) -> io::Result<(Extracted, File)> {
    let folder = places
        .mine(large)
        .join(NEXT.fetch_add(1, Ordering::Relaxed).to_string());
    fs::create_dir_all(&folder)?;
    let path = folder.join(name);
    let file = File::create(&path)?;
    Ok((Extracted { path, folder }, file))
}

/// Removes everything this process took out.
pub fn clear(places: &Places) {
    for large in [false, true] {
        let _ = fs::remove_dir_all(places.mine(large));
    }
}

/// Removes what processes no longer running left behind, as after a crash.
pub fn sweep(places: &Places) {
    for base in [&places.small, &places.large] {
        let Ok(entries) = fs::read_dir(base) else {
            continue;
        };
        for entry in entries.flatten() {
            let pid = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok());
            if pid.is_some_and(|pid| !alive(pid)) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|comm| comm.trim() == "silent")
        || pid == std::process::id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A note.txt of `hello`, encrypted with ZipCrypto by `zip -P`.
    const SECRET: &[u8] = &[
        0x50, 0x4b, 0x03, 0x04, 0x0a, 0x00, 0x09, 0x00, 0x00, 0x00, 0x28, 0xa2, 0x59, 0x5d, 0x86,
        0xa6, 0x10, 0x36, 0x11, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x08, 0x00, 0x1c, 0x00,
        0x6e, 0x6f, 0x74, 0x65, 0x2e, 0x74, 0x78, 0x74, 0x55, 0x54, 0x09, 0x00, 0x03, 0x3b, 0xc6,
        0xde, 0x6a, 0x3b, 0xc6, 0xde, 0x6a, 0x75, 0x78, 0x0b, 0x00, 0x01, 0x04, 0xe8, 0x03, 0x00,
        0x00, 0x04, 0x64, 0x00, 0x00, 0x00, 0xd9, 0x2c, 0xd9, 0xf7, 0xa7, 0xec, 0xea, 0xce, 0x6d,
        0x40, 0x8d, 0x08, 0x63, 0x24, 0x5f, 0x60, 0x82, 0x50, 0x4b, 0x07, 0x08, 0x86, 0xa6, 0x10,
        0x36, 0x11, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x50, 0x4b, 0x01, 0x02, 0x1e, 0x03,
        0x0a, 0x00, 0x09, 0x00, 0x00, 0x00, 0x28, 0xa2, 0x59, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x11,
        0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x08, 0x00, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0xa4, 0x81, 0x00, 0x00, 0x00, 0x00, 0x6e, 0x6f, 0x74, 0x65, 0x2e,
        0x74, 0x78, 0x74, 0x55, 0x54, 0x05, 0x00, 0x03, 0x3b, 0xc6, 0xde, 0x6a, 0x75, 0x78, 0x0b,
        0x00, 0x01, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x04, 0x64, 0x00, 0x00, 0x00, 0x50, 0x4b, 0x05,
        0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x4e, 0x00, 0x00, 0x00, 0x63, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ];

    /// A scratch folder of the test's own: tests run side by side.
    fn scratch(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("silent-extract-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn places(dir: &Path, limit: u64) -> Places {
        Places {
            small: dir.join("runtime/silent/extract"),
            large: dir.join("cache/silent/extract"),
            limit,
        }
    }

    /// A zip of `files`, deflated.
    fn zip(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("test.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn take(archive: &Path, entry: &str, size: u64, places: &Places) -> Result<Extracted, Refusal> {
        extract(
            archive,
            entry,
            size,
            &AtomicU64::new(0),
            &AtomicBool::new(false),
            places,
        )
    }

    fn read(path: &Path) -> Vec<u8> {
        let mut bytes = Vec::new();
        File::open(path).unwrap().read_to_end(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn an_entry_comes_out_whole_into_a_folder_of_its_own() {
        let dir = scratch("whole");
        let archive = zip(
            &dir,
            &[("src/main.rs", b"fn main() {}\n"), ("README", b"read me")],
        );
        let places = places(&dir, SMALL);
        let taken = take(&archive, "src/main.rs", 13, &places).unwrap();
        assert_eq!(read(&taken.path), b"fn main() {}\n");
        assert_eq!(taken.path.file_name().unwrap(), "main.rs");
        assert!(taken.path.starts_with(&places.small));
        assert!(taken.path.starts_with(places.mine(false)));
        // Its folder goes with it.
        let folder = taken.path.parent().unwrap().to_owned();
        drop(taken);
        assert!(!folder.exists());
        // An entry that is not there.
        assert_eq!(
            take(&archive, "src/lib.rs", 1, &places).unwrap_err(),
            Refusal::Missing
        );
    }

    #[test]
    fn large_entries_go_to_the_cache_and_growing_ones_move_there() {
        let dir = scratch("large");
        let big = vec![7u8; 5000];
        let archive = zip(&dir, &[("big.bin", &big)]);
        let places = places(&dir, 1000);
        // Listed as large: straight to the large place.
        let taken = take(&archive, "big.bin", 5000, &places).unwrap();
        assert!(taken.path.starts_with(&places.large));
        assert_eq!(read(&taken.path), big);
        drop(taken);
        // Listed as small but larger: moved there whole.
        let taken = take(&archive, "big.bin", 10, &places).unwrap();
        assert!(taken.path.starts_with(&places.large));
        assert_eq!(read(&taken.path), big);
        let mine = places.mine(false);
        assert_eq!(fs::read_dir(&mine).map(|d| d.count()).unwrap_or(0), 0);
    }

    #[test]
    fn huge_encrypted_and_cancelled_entries_are_refused() {
        let dir = scratch("refused");
        let places = places(&dir, SMALL);
        let archive = zip(&dir, &[("a.txt", b"some text")]);
        assert_eq!(
            take(&archive, "a.txt", MOST + 1, &places).unwrap_err(),
            Refusal::TooLarge(MOST + 1)
        );
        let secret = dir.join("secret.zip");
        fs::write(&secret, SECRET).unwrap();
        assert_eq!(
            take(&secret, "note.txt", 5, &places).unwrap_err(),
            Refusal::Encrypted
        );
        let cancelled = extract(
            &archive,
            "a.txt",
            9,
            &AtomicU64::new(0),
            &AtomicBool::new(true),
            &places,
        );
        assert_eq!(cancelled.unwrap_err(), Refusal::Cancelled);
    }

    #[test]
    fn names_match_however_the_archive_spells_them() {
        assert_eq!(normalize("./project//src/main.rs"), "./project/src/main.rs");
        assert_eq!(normalize("/abs/path/"), "abs/path");
        // 中文.txt in GBK.
        let gbk = [0xd6, 0xd0, 0xce, 0xc4, b'.', b't', b'x', b't'];
        assert_eq!(decode_name(&gbk).unwrap(), "中文.txt");
        assert_eq!(decode_name("ok.txt".as_bytes()).unwrap(), "ok.txt");
    }

    #[test]
    fn the_folders_of_dead_processes_are_swept_away() {
        let dir = scratch("sweep");
        let places = places(&dir, SMALL);
        // A process id no live process has: above the kernel's maximum.
        let dead = places.small.join("4194305/0");
        fs::create_dir_all(&dead).unwrap();
        let own = places.mine(false).join("0");
        fs::create_dir_all(&own).unwrap();
        sweep(&places);
        assert!(!places.small.join("4194305").exists());
        assert!(own.exists());
        clear(&places);
        assert!(!places.mine(false).exists());
    }
}
