//! Office documents read the way macOS Quick Look reads them: the package is
//! opened and its XML parsed here, with no office suite, into small models
//! that the window draws itself. Word processing (docx, odt, and the old
//! binary doc), spreadsheets (xlsx, xls, ods, through calamine),
//! presentations (pptx, odp, and the old ppt as an outline) and the
//! previews iWork packages carry.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use roxmltree::Node;

pub mod biff;
pub mod doc;
pub mod docx;
pub mod odf;
pub mod ole;
pub mod ppt;
pub mod pptx;
pub mod sheet;

/// Points in an EMU, the unit of DrawingML.
pub const EMU_PER_POINT: f32 = 12_700.;
/// Points in a twip, the unit of WordprocessingML.
pub const TWIPS_PER_POINT: f32 = 20.;

/// A zip package: an Office Open XML, OpenDocument or iWork file.
pub struct Package {
    zip: zip::ZipArchive<File>,
}

impl Package {
    pub fn open(path: &Path) -> Option<Self> {
        Some(Self {
            zip: zip::ZipArchive::new(File::open(path).ok()?).ok()?,
        })
    }

    pub fn bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        let mut entry = self.zip.by_name(name.trim_start_matches('/')).ok()?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }

    pub fn text(&mut self, name: &str) -> Option<String> {
        String::from_utf8(self.bytes(name)?).ok()
    }

    /// The unpacked size of each entry whose name passes `wanted`.
    pub fn sizes(&mut self, wanted: impl Fn(&str) -> bool) -> Vec<(String, u64)> {
        (0..self.zip.len())
            .filter_map(|i| {
                let entry = self.zip.by_index_raw(i).ok()?;
                let name = entry.name().ok()?;
                wanted(&name).then(|| (name.into_owned(), entry.size()))
            })
            .collect()
    }

    /// The relationships of the part `part`: id to the target's path.
    pub fn rels(&mut self, part: &str) -> HashMap<String, String> {
        let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
        let Some(xml) = self.text(&format!("{dir}/_rels/{file}.rels")) else {
            return HashMap::new();
        };
        let Ok(doc) = roxmltree::Document::parse(&xml) else {
            return HashMap::new();
        };
        doc.root_element()
            .children()
            .filter(|n| n.is_element())
            .filter(|n| attr(*n, "TargetMode") != Some("External"))
            .filter_map(|n| Some((attr(n, "Id")?.to_owned(), resolve(dir, attr(n, "Target")?))))
            .collect()
    }
}

/// A relationship's target, relative to the folder of the part it is from.
pub fn resolve(dir: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        dir.split('/').filter(|p| !p.is_empty()).collect()
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// An attribute by its local name, whatever its namespace: transitional
/// and strict Office files name their namespaces differently.
pub fn attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value())
}

/// A relationship attribute such as `r:id`, which can sit beside a plain
/// attribute of the same local name.
pub fn rel_attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| {
            a.name() == name
                && a.namespace()
                    .is_some_and(|ns| ns.ends_with("/relationships"))
        })
        .map(|a| a.value())
}

/// The first child element with local name `name`.
pub fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

/// Child elements with local name `name`.
pub fn children<'a, 'i>(node: Node<'a, 'i>, name: &str) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children()
        .filter(move |n| n.is_element() && n.tag_name().name() == name)
}

/// Follows a path of local names down from `node`.
pub fn path<'a, 'i>(node: Node<'a, 'i>, names: &[&str]) -> Option<Node<'a, 'i>> {
    names.iter().try_fold(node, |node, name| child(node, name))
}

/// `true`, `1` and `on`, or an element present without a value, are on.
pub fn on(node: Option<Node>) -> Option<bool> {
    let node = node?;
    Some(!matches!(
        attr(node, "val"),
        Some("0" | "false" | "off" | "none")
    ))
}

/// A colour as `RRGGBB` hex.
pub fn rgb(hex: &str) -> Option<u32> {
    (hex.len() == 6)
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

/// Whether `head` starts an OLE2 compound file, the container of the
/// binary Office formats.
pub fn is_ole(head: &[u8]) -> bool {
    head.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1])
}

/// Whether the file at `path` is an OLE2 compound file.
pub fn is_ole_file(path: &Path) -> bool {
    let mut head = [0; 8];
    File::open(path).is_ok_and(|mut file| file.read_exact(&mut head).is_ok()) && is_ole(&head)
}

/// Little-endian numbers in the binary formats: BIFF, Word and PowerPoint.
pub(crate) fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

pub(crate) fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

pub(crate) fn f64_at(data: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// What a word-processing document holds, in reading order.
#[derive(Debug, Default)]
pub struct Document {
    /// Page size and side margins, in points.
    pub page: (f32, f32),
    pub margins: (f32, f32),
    /// The body text's size in points.
    pub base_size: f32,
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
    Image(Image),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Body,
    Title,
    Subtitle,
    Heading(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

#[derive(Debug, Default)]
pub struct Paragraph {
    pub role: Role,
    pub align: Align,
    /// A list's bullet or number, hung to the left of the text.
    pub marker: Option<String>,
    /// Where the text starts, in points from the column's left edge.
    pub indent: f32,
    pub spans: Vec<Span>,
}

impl Paragraph {
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Points.
    pub size: Option<f32>,
    pub color: Option<u32>,
    pub font: Option<String>,
}

#[derive(Debug, Default)]
pub struct Table {
    /// Each column's share of the width.
    pub widths: Vec<f32>,
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Debug, Default)]
pub struct Cell {
    /// Grid columns the cell spans.
    pub span: usize,
    pub paragraphs: Vec<Paragraph>,
}

/// An encoded picture and its size on the page, in points.
#[derive(Debug)]
pub struct Image {
    pub data: Vec<u8>,
    pub size: (f32, f32),
}

/// The largest picture an iWork package carries for previews.
pub fn iwork_preview(path: &Path) -> Option<Vec<u8>> {
    let mut package = Package::open(path)?;
    let (name, _) = package
        .sizes(|name| {
            let lower = name.to_ascii_lowercase();
            (lower.starts_with("preview") || lower.starts_with("quicklook/"))
                && (lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".png"))
        })
        .into_iter()
        .max_by_key(|(_, size)| *size)?;
    package.bytes(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_resolve_against_their_part() {
        assert_eq!(
            resolve("ppt/slides", "../media/image1.png"),
            "ppt/media/image1.png"
        );
        assert_eq!(resolve("word", "media/image1.png"), "word/media/image1.png");
        assert_eq!(
            resolve("ppt", "/ppt/slides/slide1.xml"),
            "ppt/slides/slide1.xml"
        );
        assert_eq!(resolve("", "word/document.xml"), "word/document.xml");
    }

    #[test]
    fn attributes_match_whatever_their_namespace() {
        let xml = r#"<w:p xmlns:w="urn:w"><w:b w:val="0"/><w:i/></w:p>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let p = doc.root_element();
        assert_eq!(on(child(p, "b")), Some(false));
        assert_eq!(on(child(p, "i")), Some(true));
        assert_eq!(on(child(p, "u")), None);
    }
}
