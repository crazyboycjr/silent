//! Files squeezed on their own by gzip, xz, zstd, bzip2, lz4 and the like:
//! the head of what is inside, and how large it is unpacked when the format
//! says so.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The first bytes of the unpacked data.
pub struct Head {
    pub bytes: Vec<u8>,
    /// Everything was unpacked: there was no more.
    pub complete: bool,
}

/// Unpacks at most `limit` bytes of `path`, through libarchive's filters.
pub fn head(path: &Path, limit: usize) -> io::Result<Head> {
    let mut sink = Capped {
        bytes: Vec::new(),
        limit,
        full: false,
    };
    let unpacked = compress_tools::uncompress_data(File::open(path)?, &mut sink);
    match unpacked {
        Ok(_) => Ok(Head {
            bytes: sink.bytes,
            complete: true,
        }),
        // Stopped at the limit, or a damaged file whose start still reads.
        Err(_) if !sink.bytes.is_empty() => Ok(Head {
            bytes: sink.bytes,
            complete: false,
        }),
        Err(error) => Err(io::Error::other(error)),
    }
}

struct Capped {
    bytes: Vec<u8>,
    limit: usize,
    full: bool,
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = self.limit - self.bytes.len();
        if buf.len() > room {
            self.bytes.extend_from_slice(&buf[..room]);
            self.full = true;
            return Err(io::Error::new(io::ErrorKind::WriteZero, "limit reached"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The name of what is inside: `notes.json.gz` holds `notes.json`.
pub fn inner_name(name: &str) -> &str {
    const SUFFIXES: [&str; 9] = [
        ".gz", ".xz", ".zst", ".bz2", ".lz4", ".lzma", ".lz", ".z", ".bz",
    ];
    SUFFIXES
        .iter()
        .find_map(|suffix| {
            let cut = name.len().checked_sub(suffix.len())?;
            (cut > 0 && name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(suffix))
                .then(|| &name[..cut])
        })
        .unwrap_or(name)
}

/// The unpacked size that gzip, xz, zstd, lz4, lzma and lzip record, read
/// from the file's header or trailer. gzip keeps it modulo 4 GiB and only
/// for its last member, so it is a hint to check against what was unpacked.
pub fn recorded_size(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let mut start = [0u8; 32];
    let read = file.read(&mut start).ok()?;
    let start = &start[..read];
    let len = file.metadata().ok()?.len();
    let mut tail = |n: u64| -> Option<Vec<u8>> {
        let n = n.min(len);
        let mut buf = vec![0; n as usize];
        file.seek(SeekFrom::End(-(n as i64))).ok()?;
        file.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    if start.starts_with(&[0x1f, 0x8b]) {
        let t = tail(4)?;
        return Some(u32::from_le_bytes(t.try_into().ok()?) as u64);
    }
    if start.starts_with(b"\xfd7zXZ\0") {
        let footer = tail(12)?;
        if &footer[10..] != b"YZ" {
            return None;
        }
        let backward = (u32::from_le_bytes(footer[4..8].try_into().ok()?) as u64 + 1) * 4;
        let index = tail(12 + backward)?;
        return xz_index_size(&index[..backward as usize]);
    }
    if start.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return zstd_content_size(&start[4..]);
    }
    if start.starts_with(&[0x04, 0x22, 0x4d, 0x18]) {
        let flags = *start.get(4)?;
        if flags & 0x08 == 0 {
            return None;
        }
        return Some(u64::from_le_bytes(start.get(6..14)?.try_into().ok()?));
    }
    if start.starts_with(b"LZIP") {
        let t = tail(16)?;
        return Some(u64::from_le_bytes(t[..8].try_into().ok()?));
    }
    if start.first() == Some(&0x5d) && start.len() >= 13 {
        let size = u64::from_le_bytes(start[5..13].try_into().ok()?);
        return (size != u64::MAX).then_some(size);
    }
    None
}

/// The sum of the blocks' unpacked sizes in an xz stream's index.
fn xz_index_size(index: &[u8]) -> Option<u64> {
    let mut at = 1;
    if index.first() != Some(&0) {
        return None;
    }
    let mut varint = || -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..63).step_by(7) {
            let byte = *index.get(at)?;
            at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    };
    let records = varint()?;
    let mut total = 0u64;
    for _ in 0..records {
        varint()?;
        total = total.checked_add(varint()?)?;
    }
    Some(total)
}

/// The content size in a zstd frame header, when the frame carries one.
fn zstd_content_size(header: &[u8]) -> Option<u64> {
    let descriptor = *header.first()?;
    let single_segment = descriptor & 0x20 != 0;
    let size_bytes = match descriptor >> 6 {
        0 if single_segment => 1,
        0 => return None,
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let dictionary = [0, 1, 2, 4][(descriptor & 3) as usize];
    let at = 1 + usize::from(!single_segment) + dictionary;
    let field = header.get(at..at + size_bytes)?;
    let mut value = [0u8; 8];
    value[..size_bytes].copy_from_slice(field);
    let value = u64::from_le_bytes(value);
    Some(if size_bytes == 2 { value + 256 } else { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    // `hello hello hello hello\n` (24 bytes) through each tool.
    const GZIP: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xcb, 0x48, 0xcd, 0xc9, 0xc9,
        0x57, 0xc8, 0x40, 0x27, 0xb9, 0x00, 0x00, 0x88, 0x59, 0x0b, 0x18, 0x00, 0x00, 0x00,
    ];
    const XZ: &[u8] = &[
        0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00, 0x00, 0x04, 0xe6, 0xd6, 0xb4, 0x46, 0x04, 0xc0, 0x14,
        0x18, 0x21, 0x01, 0x16, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0xf3,
        0x46, 0x45, 0xe0, 0x00, 0x17, 0x00, 0x0c, 0x5d, 0x00, 0x34, 0x19, 0x49, 0xee, 0x8d, 0xe9,
        0x56, 0x0a, 0xc1, 0x21, 0xb0, 0x00, 0x00, 0x6c, 0x14, 0xed, 0x1b, 0x3b, 0x68, 0xbb, 0x91,
        0x00, 0x01, 0x30, 0x18, 0x8e, 0x1b, 0xac, 0xec, 0x1f, 0xb6, 0xf3, 0x7d, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x04, 0x59, 0x5a,
    ];
    const ZSTD: &[u8] = &[
        0x28, 0xb5, 0x2f, 0xfd, 0x24, 0x18, 0x6d, 0x00, 0x00, 0x38, 0x68, 0x65, 0x6c, 0x6c, 0x6f,
        0x20, 0x0a, 0x01, 0x00, 0x99, 0x4b, 0x11, 0xa8, 0x7c, 0x2e, 0xa8,
    ];
    const LZ4: &[u8] = &[
        0x04, 0x22, 0x4d, 0x18, 0x6c, 0x40, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x35,
        0x0f, 0x00, 0x00, 0x00, 0x69, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x20, 0x06, 0x00, 0x50, 0x65,
        0x6c, 0x6c, 0x6f, 0x0a, 0x00, 0x00, 0x00, 0x00, 0xe2, 0xff, 0x03, 0x42,
    ];

    /// `bytes` in a file of its own: tests run side by side.
    fn scratch(test: &str, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("silent-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn recorded_sizes_are_read() {
        for (name, bytes) in [
            ("h.gz", GZIP),
            ("h.xz", XZ),
            ("h.zst", ZSTD),
            ("h.lz4", LZ4),
        ] {
            assert_eq!(
                recorded_size(&scratch("sizes", name, bytes)),
                Some(24),
                "{name}"
            );
        }
    }

    #[test]
    fn heads_unpack_and_stop_at_the_limit() {
        for (name, bytes) in [
            ("h.gz", GZIP),
            ("h.xz", XZ),
            ("h.zst", ZSTD),
            ("h.lz4", LZ4),
        ] {
            let path = scratch("heads", name, bytes);
            let whole = head(&path, 100).unwrap();
            assert_eq!(whole.bytes, b"hello hello hello hello\n", "{name}");
            assert!(whole.complete);
            let cut = head(&path, 5).unwrap();
            assert_eq!(cut.bytes, b"hello", "{name}");
            assert!(!cut.complete);
        }
    }

    #[test]
    fn inner_names_lose_the_suffix() {
        assert_eq!(inner_name("statink.json.gz"), "statink.json");
        assert_eq!(inner_name("notes.TXT.XZ"), "notes.TXT");
        assert_eq!(inner_name("log.zst"), "log");
        assert_eq!(inner_name(".gz"), ".gz");
        assert_eq!(inner_name("plain"), "plain");
    }
}
