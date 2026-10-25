//! File-type icons from the Lucide set that gpui-component bundles, so they
//! look like the rest of the toolkit. Only the few used here are embedded.

use gpui_kit::{Hsla, SharedString};
use gpui_kit_assets::IconName;
use silent::detect::Kind;

use crate::theme::accent;

gpui_kit_assets::icon_assets!(pub Icons, [
    Folder,
    FolderOpen,
    File,
    FileText,
    FileCode,
    FileImage,
    FileVideoCamera,
    FileMusic,
    FileArchive,
    FilePenLine,
    FileSpreadsheet,
    FileChartPie,
    FileDigit,
]);

/// The icon and its colour for an entry called `name`.
pub fn for_entry(name: &str, dir: bool, open: bool) -> (SharedString, Hsla) {
    if dir {
        let icon = if open {
            IconName::FolderOpen
        } else {
            IconName::Folder
        };
        return (icon.path(), accent(Kind::Folder));
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let (icon, color) = match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "heic" | "heif" | "jxl" | "svg"
        | "bmp" | "tif" | "tiff" | "ico" | "psd" | "xcf" | "dng" | "cr2" | "nef" | "arw" => {
            (IconName::FileImage, accent(Kind::Image))
        }
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg"
        | "m2ts" | "3gp" | "ogv" => (IconName::FileVideoCamera, accent(Kind::Video)),
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "wma" | "aiff" | "mid"
        | "midi" => (IconName::FileMusic, accent(Kind::Audio)),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "lz4" | "lzma"
        | "lz" | "cab" | "iso" | "deb" | "rpm" | "apk" | "jar" | "whl" => {
            (IconName::FileArchive, accent(Kind::Archive))
        }
        "pdf" => (IconName::FileText, accent(Kind::Pdf)),
        "doc" | "docx" | "odt" | "rtf" | "pages" | "epub" => {
            (IconName::FilePenLine, accent(Kind::Document))
        }
        "xls" | "xlsx" | "ods" | "csv" | "tsv" | "numbers" => {
            (IconName::FileSpreadsheet, accent(Kind::Spreadsheet))
        }
        "ppt" | "pptx" | "odp" | "key" => (IconName::FileChartPie, accent(Kind::Presentation)),
        "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "py" | "js" | "mjs" | "ts" | "tsx" | "jsx"
        | "go" | "java" | "kt" | "swift" | "rb" | "php" | "lua" | "sh" | "bash" | "zsh"
        | "fish" | "pl" | "hs" | "ml" | "el" | "scala" | "css" | "scss" | "html" | "htm"
        | "xml" | "json" | "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" | "sql" | "cmake"
        | "mk" | "zig" | "nix" => (IconName::FileCode, accent(Kind::Text)),
        "txt" | "md" | "rst" | "log" | "nfo" | "tex" | "org" | "" => {
            (IconName::FileText, accent(Kind::Text))
        }
        "bin" | "dat" | "exe" | "dll" | "so" | "o" | "a" | "obj" | "class" | "pyc" | "wasm"
        | "elf" | "dylib" | "lib" | "img" | "rom" => (IconName::FileDigit, accent(Kind::Binary)),
        _ => (IconName::File, accent(Kind::Other)),
    };
    (icon.path(), color)
}
