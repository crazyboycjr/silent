//! What a file is, and so how to preview it.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gio::glib;
use gio::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
    Audio,
    Pdf,
    Text,
    Archive,
    Folder,
    Other,
}

#[derive(Clone, Debug)]
pub struct Facts {
    pub uri: String,
    /// The local path, when the file has one.
    pub path: Option<PathBuf>,
    pub name: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub content_type: String,
    pub kind: Kind,
}

/// Archive types listed as their entries. Office documents, e-books and the
/// like are zip files too, but deserve their own preview.
const ARCHIVES: &[&str] = &[
    "application/zip",
    "application/x-tar",
    "application/x-compressed-tar",
    "application/x-bzip-compressed-tar",
    "application/x-bzip2-compressed-tar",
    "application/x-xz-compressed-tar",
    "application/x-zstd-compressed-tar",
    "application/x-lzma-compressed-tar",
    "application/x-lz4-compressed-tar",
    "application/x-lzip-compressed-tar",
    "application/x-tarz",
    "application/x-7z-compressed",
    "application/vnd.rar",
    "application/x-cpio",
    "application/x-archive",
    "application/vnd.debian.binary-package",
    "application/x-rpm",
    "application/x-cd-image",
    "application/x-iso9660-image",
    "application/x-xar",
    "application/x-lha",
    "application/vnd.ms-cab-compressed",
    "application/x-java-archive",
    "application/vnd.android.package-archive",
    "application/gzip",
    "application/x-bzip2",
    "application/x-xz",
    "application/zstd",
];

/// Looks at the file behind `uri`. Local files are read directly, which
/// keeps GVfs out of the common case; other locations go through GIO.
pub fn probe(uri: &str) -> io::Result<Facts> {
    match glib::filename_from_uri(uri) {
        Ok((path, _)) => probe_path(uri, path),
        Err(_) => probe_remote(uri),
    }
}

fn probe_path(uri: &str, path: PathBuf) -> io::Result<Facts> {
    let metadata = std::fs::metadata(&path)?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let (content_type, kind) = if metadata.is_dir() {
        ("inode/directory".to_owned(), Kind::Folder)
    } else {
        let head = read_head(&path)?;
        let (content_type, _) = gio::content_type_guess(Some(&path), head.as_slice());
        let kind = classify(&content_type, &head);
        (content_type.into(), kind)
    };
    Ok(Facts {
        uri: uri.to_owned(),
        path: Some(path),
        name,
        size: metadata.len(),
        modified: metadata.modified().ok(),
        content_type,
        kind,
    })
}

fn probe_remote(uri: &str) -> io::Result<Facts> {
    let file = gio::File::for_uri(uri);
    let info = file
        .query_info(
            "standard::content-type,standard::display-name,standard::size,standard::type,time::modified",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .map_err(io::Error::other)?;
    let content_type = info
        .content_type()
        .map_or_else(|| "application/octet-stream".to_owned(), Into::into);
    let kind = if info.file_type() == gio::FileType::Directory {
        Kind::Folder
    } else {
        classify(&content_type, &[])
    };
    Ok(Facts {
        uri: uri.to_owned(),
        path: file.path(),
        name: info.display_name().into(),
        size: info.size().max(0) as u64,
        modified: info.modification_date_time().map(|t| {
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t.to_unix().max(0) as u64)
        }),
        content_type,
        kind,
    })
}

fn read_head(path: &Path) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(4096);
    File::open(path)?.take(4096).read_to_end(&mut head)?;
    Ok(head)
}

pub fn classify(content_type: &str, head: &[u8]) -> Kind {
    let is_a = |supertype: &str| gio::content_type_is_a(content_type, supertype);
    let major = content_type.split('/').next().unwrap_or_default();
    if content_type == "inode/directory" {
        Kind::Folder
    } else if ARCHIVES
        .iter()
        .any(|archive| gio::content_type_equals(content_type, archive))
    {
        Kind::Archive
    } else if is_a("application/pdf") {
        Kind::Pdf
    } else if major == "image" {
        Kind::Image
    } else if major == "video" {
        Kind::Video
    } else if major == "audio" || content_type == "application/ogg" {
        Kind::Audio
    } else if is_a("text/plain") || looks_like_text(head) {
        Kind::Text
    } else {
        Kind::Other
    }
}

/// Text without a type of its own: no NUL bytes, and UTF-8 save for a
/// character cut off at the end of the sample.
fn looks_like_text(head: &[u8]) -> bool {
    !head.is_empty()
        && !head.contains(&0)
        && match std::str::from_utf8(head) {
            Ok(_) => true,
            Err(e) => e.error_len().is_none(),
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_map_to_previews() {
        let cases = [
            ("image/jpeg", Kind::Image),
            ("image/svg+xml", Kind::Image),
            ("video/mp4", Kind::Video),
            ("audio/flac", Kind::Audio),
            ("application/pdf", Kind::Pdf),
            ("text/x-rust", Kind::Text),
            ("application/json", Kind::Text),
            ("application/x-shellscript", Kind::Text),
            ("application/zip", Kind::Archive),
            ("application/x-compressed-tar", Kind::Archive),
            ("inode/directory", Kind::Folder),
            ("application/x-executable", Kind::Other),
            (
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                Kind::Other,
            ),
            ("application/epub+zip", Kind::Other),
        ];
        for (content_type, kind) in cases {
            assert_eq!(classify(content_type, &[]), kind, "{content_type}");
        }
    }

    #[test]
    fn untyped_text_is_recognised_by_its_bytes() {
        let octets = "application/octet-stream";
        assert_eq!(classify(octets, "plain words\n".as_bytes()), Kind::Text);
        assert_eq!(classify(octets, "中文 ".as_bytes()), Kind::Text);
        // A multi-byte character cut by the sample's end.
        assert_eq!(classify(octets, &"文字".as_bytes()[..4]), Kind::Text);
        assert_eq!(classify(octets, b"ELF\0\x02\x01"), Kind::Other);
        assert_eq!(
            classify(octets, &[0xff, 0xfe, 0x41, 0x80, 0x80]),
            Kind::Other
        );
    }
}
