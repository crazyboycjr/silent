//! What a file is, and so how to preview it.

use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::FileTypeExt;
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
    /// One file squeezed by gzip, xz and the like, shown as what is inside.
    Compressed,
    /// Shown as `hexdump -C` shows it.
    Binary,
    /// Word processing: docx, odt, Pages.
    Document,
    /// xlsx, xls, ods, Numbers.
    Spreadsheet,
    /// pptx, odp, Keynote.
    Presentation,
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
];

/// Single files compressed on their own; libarchive undoes all of these.
const COMPRESSED: &[&str] = &[
    "application/gzip",
    "application/x-bzip2",
    "application/x-bzip",
    "application/x-xz",
    "application/zstd",
    "application/x-lz4",
    "application/x-lzma",
    "application/x-lzip",
    "application/x-compress",
];

/// Machine code and other formats that are binary whatever their first bytes say.
const BINARY: &[&str] = &[
    "application/x-executable",
    "application/x-pie-executable",
    "application/x-sharedlib",
    "application/x-object",
    "application/x-core",
    "application/x-dosexec",
    "application/x-msdownload",
    "application/vnd.microsoft.portable-executable",
    "application/x-mach-binary",
    "application/x-java",
    "application/wasm",
    "application/vnd.sqlite3",
];

/// Office and iWork files by the start of their type, WPS Office's own
/// types among them; legacy .doc and .ppt are not here and stay cards.
const OFFICE: &[(&str, Kind)] = &[
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml",
        Kind::Document,
    ),
    ("application/vnd.ms-word.document", Kind::Document),
    ("application/vnd.ms-word.template", Kind::Document),
    ("application/vnd.oasis.opendocument.text", Kind::Document),
    ("application/vnd.apple.pages", Kind::Document),
    ("application/x-iwork-pages", Kind::Document),
    ("application/wps-office.docx", Kind::Document),
    ("application/wps-office.dotx", Kind::Document),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml",
        Kind::Spreadsheet,
    ),
    ("application/vnd.ms-excel", Kind::Spreadsheet),
    (
        "application/vnd.oasis.opendocument.spreadsheet",
        Kind::Spreadsheet,
    ),
    ("application/vnd.apple.numbers", Kind::Spreadsheet),
    ("application/x-iwork-numbers", Kind::Spreadsheet),
    ("application/wps-office.xls", Kind::Spreadsheet),
    (
        "application/vnd.openxmlformats-officedocument.presentationml",
        Kind::Presentation,
    ),
    (
        "application/vnd.ms-powerpoint.presentation",
        Kind::Presentation,
    ),
    (
        "application/vnd.oasis.opendocument.presentation",
        Kind::Presentation,
    ),
    ("application/vnd.apple.keynote", Kind::Presentation),
    ("application/x-iwork-keynote", Kind::Presentation),
    ("application/wps-office.pptx", Kind::Presentation),
];

/// The kind of an Office or iWork file, when its first bytes are the zip or
/// the OLE2 container its type promises; an Office lock file or a key named
/// `.key` is not.
fn office(content_type: &str, head: &[u8]) -> Option<Kind> {
    let kind = OFFICE
        .iter()
        .find(|(prefix, _)| content_type.starts_with(prefix))?
        .1;
    let zip = head.starts_with(b"PK\x03\x04");
    let ole = head.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    (zip || (ole && kind == Kind::Spreadsheet)).then_some(kind)
}

/// Types better shown as a card than as their bytes: documents and fonts,
/// until they get previews of their own.
const CARDS: &[&str] = &[
    "application/msword",
    "application/vnd.ms-excel",
    "application/vnd.ms-powerpoint",
    "application/vnd.ms-word",
    "application/vnd.openxmlformats-officedocument",
    "application/vnd.oasis.opendocument",
    // WPS Office registers its own types for Office files, docx and all.
    "application/wps-office",
    "application/vnd.apple",
    "application/x-iwork",
    "application/epub+zip",
    "application/x-mobipocket-ebook",
    "application/vnd.amazon",
    "application/x-font",
    "application/vnd.ms-fontobject",
    "font/",
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
    let file_type = metadata.file_type();
    let (content_type, kind) = if file_type.is_dir() {
        ("inode/directory".to_owned(), Kind::Folder)
    } else if file_type.is_file() {
        let head = read_head(&path)?;
        let (content_type, _) = gio::content_type_guess(Some(&path), head.as_slice());
        let kind = classify(&content_type, &head);
        (content_type.into(), kind)
    } else {
        // Opening a pipe would wait for a writer; devices and sockets are no better.
        let special = if file_type.is_fifo() {
            "inode/fifo"
        } else if file_type.is_socket() {
            "inode/socket"
        } else if file_type.is_char_device() {
            "inode/chardevice"
        } else {
            "inode/blockdevice"
        };
        (special.to_owned(), Kind::Other)
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

/// How a file of `content_type` whose first bytes are `head` is previewed.
/// `head` is empty when the bytes are unknown, as for remote files.
pub fn classify(content_type: &str, head: &[u8]) -> Kind {
    let is_a = |supertype: &str| gio::content_type_is_a(content_type, supertype);
    let listed = |list: &[&str]| {
        list.iter()
            .any(|listed| gio::content_type_equals(content_type, listed))
    };
    let major = content_type.split('/').next().unwrap_or_default();
    if content_type == "inode/directory" {
        Kind::Folder
    } else if major == "inode" {
        Kind::Other
    } else if listed(ARCHIVES) {
        Kind::Archive
    } else if listed(COMPRESSED) {
        Kind::Compressed
    } else if is_a("application/pdf") {
        Kind::Pdf
    } else if major == "image" {
        Kind::Image
    } else if major == "video" {
        Kind::Video
    } else if major == "audio" || content_type == "application/ogg" {
        Kind::Audio
    } else if let Some(kind) = office(content_type, head) {
        kind
    } else if listed(BINARY) {
        Kind::Binary
    } else if (is_a("text/plain") && !has_nul(head)) || looks_like_text(head) {
        Kind::Text
    } else if CARDS.iter().any(|card| content_type.starts_with(card)) || head.is_empty() {
        Kind::Other
    } else {
        Kind::Binary
    }
}

fn has_nul(head: &[u8]) -> bool {
    !utf16_bom(head) && head.contains(&0)
}

fn utf16_bom(head: &[u8]) -> bool {
    head.starts_with(&[0xff, 0xfe]) || head.starts_with(&[0xfe, 0xff])
}

/// Text without a type of its own: UTF-16 with its byte-order mark, or no
/// NUL bytes and valid as UTF-8 or as GB18030 (which covers GBK), save for
/// a character cut off at the end of the sample.
pub fn looks_like_text(head: &[u8]) -> bool {
    if head.is_empty() {
        return false;
    }
    if utf16_bom(head) {
        return true;
    }
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) if e.error_len().is_none() => true,
        Err(_) => (0..4.min(head.len())).any(|cut| {
            encoding_rs::GB18030
                .decode_without_bom_handling_and_without_replacement(&head[..head.len() - cut])
                .is_some()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_map_to_previews() {
        let elf = b"\x7fELF\x02\x01\x01\0";
        let code = b"fn main() {}\n";
        let cases: [(&str, &[u8], Kind); 22] = [
            ("image/jpeg", elf, Kind::Image),
            ("image/svg+xml", code, Kind::Image),
            ("video/mp4", elf, Kind::Video),
            ("audio/flac", elf, Kind::Audio),
            ("application/pdf", elf, Kind::Pdf),
            ("text/x-rust", code, Kind::Text),
            ("application/json", code, Kind::Text),
            ("application/x-shellscript", code, Kind::Text),
            ("application/zip", elf, Kind::Archive),
            ("application/x-compressed-tar", elf, Kind::Archive),
            ("application/gzip", elf, Kind::Compressed),
            ("application/zstd", elf, Kind::Compressed),
            ("application/x-xz", elf, Kind::Compressed),
            ("inode/directory", &[], Kind::Folder),
            ("inode/fifo", &[], Kind::Other),
            ("application/x-executable", elf, Kind::Binary),
            ("application/x-sharedlib", elf, Kind::Binary),
            ("application/octet-stream", elf, Kind::Binary),
            (
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                elf,
                Kind::Other,
            ),
            ("application/wps-office.docx", elf, Kind::Other),
            ("application/epub+zip", elf, Kind::Other),
            ("font/ttf", elf, Kind::Other),
        ];
        for (content_type, head, kind) in cases {
            assert_eq!(classify(content_type, head), kind, "{content_type}");
        }
        assert_eq!(classify("application/octet-stream", &[]), Kind::Other);
    }

    #[test]
    fn office_files_need_their_container() {
        let zip = b"PK\x03\x04\x14\0";
        let ole = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1, 0, 0, 0, 0];
        let docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        assert_eq!(classify(docx, zip), Kind::Document);
        assert_eq!(classify("application/wps-office.docx", zip), Kind::Document);
        assert_eq!(
            classify("application/wps-office.xlsx", zip),
            Kind::Spreadsheet
        );
        assert_eq!(
            classify("application/wps-office.xls", &ole),
            Kind::Spreadsheet
        );
        assert_eq!(
            classify("application/vnd.ms-excel", &ole),
            Kind::Spreadsheet
        );
        assert_eq!(
            classify("application/vnd.apple.keynote", zip),
            Kind::Presentation
        );
        assert_eq!(
            classify("application/wps-office.pptx", zip),
            Kind::Presentation
        );
        // Legacy Word and PowerPoint, an Office lock file, a PEM key named .key.
        assert_eq!(classify("application/wps-office.doc", &ole), Kind::Other);
        assert_eq!(classify("application/vnd.ms-powerpoint", &ole), Kind::Other);
        assert_eq!(
            classify("application/wps-office.pptx", b"\x03cjr      "),
            Kind::Text
        );
        assert_eq!(
            classify(
                "application/vnd.apple.keynote",
                b"-----BEGIN PRIVATE KEY-----\n"
            ),
            Kind::Text
        );
    }

    #[test]
    fn untyped_bytes_are_text_or_binary() {
        let octets = "application/octet-stream";
        assert_eq!(classify(octets, "plain words\n".as_bytes()), Kind::Text);
        assert_eq!(classify(octets, "中文 ".as_bytes()), Kind::Text);
        // A multi-byte character cut by the sample's end.
        assert_eq!(classify(octets, &"文字".as_bytes()[..4]), Kind::Text);
        // 静夜思 in GB18030, and the same cut in the middle of a character.
        let gbk = [0xbe, 0xb2, 0xd2, 0xb9, 0xcb, 0xbc, 0x0a];
        assert_eq!(classify(octets, &gbk), Kind::Text);
        assert_eq!(classify(octets, &gbk[..5]), Kind::Text);
        assert_eq!(classify(octets, &[0xff, 0xfe, 0x41, 0x00]), Kind::Text);
        assert_eq!(classify(octets, b"ELF\0\x02\x01"), Kind::Binary);
        assert_eq!(
            classify(octets, &[0x41, 0xff, 0x80, 0x80, 0x41]),
            Kind::Binary
        );
        // Typed as text by its name, but full of NUL bytes.
        assert_eq!(classify("text/plain", b"a\0b\0c\0"), Kind::Binary);
    }
}
