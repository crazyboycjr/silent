//! OpenDocument text and slides: content.xml with the styles it names.
//! Slides are shown as the thumbnail the file carries and their text.

use std::collections::HashMap;
use std::path::Path;

use roxmltree::Node;

use super::{
    Align, Block, Cell, Document, Image, Package, Paragraph, Role, Span, Table, attr, child,
    children, path, rgb,
};

#[derive(Clone, Debug, Default)]
struct Style {
    parent: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    size: Option<f32>,
    color: Option<u32>,
    align: Option<Align>,
    /// For list styles: whether each level is numbered.
    numbered: Vec<bool>,
}

struct Reader<'p> {
    package: &'p mut Package,
    styles: HashMap<String, Style>,
    /// Counters of the numbered list being read, by level.
    counters: Vec<u32>,
}

pub fn read_text(file: &Path) -> Option<Document> {
    let mut package = Package::open(file)?;
    let xml = package.text("content.xml")?;
    let content = roxmltree::Document::parse(&xml).ok()?;
    let styles_xml = package.text("styles.xml").unwrap_or_default();
    let styles = roxmltree::Document::parse(&styles_xml).ok();
    let mut reader = Reader {
        package: &mut package,
        styles: HashMap::new(),
        counters: Vec::new(),
    };
    let mut document = Document {
        page: (595., 842.),
        margins: (57., 57.),
        base_size: 12.,
        blocks: Vec::new(),
    };
    if let Some(styles) = &styles {
        let root = styles.root_element();
        reader.read_styles(child(root, "styles"));
        reader.read_styles(child(root, "automatic-styles"));
        if let Some(layout) = child(root, "automatic-styles")
            .into_iter()
            .flat_map(|s| children(s, "page-layout"))
            .find_map(|l| child(l, "page-layout-properties"))
        {
            let length = |name| attr(layout, name).and_then(points);
            if let (Some(w), Some(h)) = (length("page-width"), length("page-height")) {
                document.page = (w, h);
            }
            if let (Some(l), Some(r)) = (length("margin-left"), length("margin-right")) {
                document.margins = (l, r);
            }
        }
        if let Some(size) = reader.styles.get("Standard").and_then(|s| s.size) {
            document.base_size = size;
        }
    }
    let root = content.root_element();
    reader.read_styles(child(root, "automatic-styles"));
    let body = path(root, &["body", "text"])?;
    reader.blocks(body, &mut document.blocks, 0, None);
    Some(document)
}

/// A presentation as its thumbnail, then each slide's text under its title.
pub fn read_slides(file: &Path) -> Option<Document> {
    let mut package = Package::open(file)?;
    let xml = package.text("content.xml")?;
    let content = roxmltree::Document::parse(&xml).ok()?;
    let thumbnail = package.bytes("Thumbnails/thumbnail.png");
    let mut reader = Reader {
        package: &mut package,
        styles: HashMap::new(),
        counters: Vec::new(),
    };
    let mut document = Document {
        page: (792., 612.),
        margins: (54., 54.),
        base_size: 14.,
        blocks: Vec::new(),
    };
    // The thumbnail is small; drawn larger than half the page it blurs.
    if let Some(data) = thumbnail {
        document.blocks.push(Block::Image(Image {
            data,
            size: (342., 192.),
        }));
    }
    let presentation = path(content.root_element(), &["body", "presentation"])?;
    for (n, page) in children(presentation, "page").enumerate() {
        let frames: Vec<Node> = page
            .descendants()
            .filter(|f| f.tag_name().name() == "frame")
            .collect();
        let title = frames
            .iter()
            .find(|f| attr(**f, "class") == Some("title"))
            .map(|f| text_of(*f));
        let heading = match title.as_deref() {
            Some(title) if !title.trim().is_empty() => format!("{}. {}", n + 1, title.trim()),
            _ => format!("Slide {}", n + 1),
        };
        document.blocks.push(Block::Paragraph(Paragraph {
            role: Role::Heading(2),
            spans: vec![Span {
                text: heading,
                bold: true,
                ..Span::default()
            }],
            ..Paragraph::default()
        }));
        for frame in frames
            .iter()
            .filter(|f| attr(**f, "class") != Some("title"))
        {
            if let Some(box_) = child(*frame, "text-box") {
                reader.blocks(box_, &mut document.blocks, 0, None);
            }
        }
    }
    Some(document)
}

/// All the text under `node`, paragraphs run together.
fn text_of(node: Node) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A length such as `2cm`, `12pt` or `0.5in` in points.
fn points(value: &str) -> Option<f32> {
    let split = value.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = value.split_at(split);
    let number: f32 = number.trim().parse().ok()?;
    Some(
        number
            * match unit {
                "pt" => 1.,
                "cm" => 72. / 2.54,
                "mm" => 72. / 25.4,
                "in" => 72.,
                "pc" => 12.,
                "px" => 0.75,
                _ => return None,
            },
    )
}

impl Reader<'_> {
    fn read_styles(&mut self, parent: Option<Node>) {
        let Some(parent) = parent else {
            return;
        };
        for node in parent.children().filter(|n| n.is_element()) {
            let Some(name) = attr(node, "name") else {
                continue;
            };
            let mut style = Style {
                parent: attr(node, "parent-style-name").map(str::to_owned),
                ..Style::default()
            };
            match node.tag_name().name() {
                "style" => {
                    if let Some(text) = child(node, "text-properties") {
                        style.bold = attr(text, "font-weight")
                            .map(|w| matches!(w, "bold" | "600" | "700" | "800" | "900"));
                        style.italic = attr(text, "font-style").map(|s| s != "normal");
                        style.underline = attr(text, "text-underline-style").map(|s| s != "none");
                        style.strike = attr(text, "text-line-through-style").map(|s| s != "none");
                        style.size = attr(text, "font-size").and_then(points);
                        style.color =
                            attr(text, "color").and_then(|c| rgb(c.trim_start_matches('#')));
                    }
                    if let Some(para) = child(node, "paragraph-properties") {
                        style.align = attr(para, "text-align").map(|a| match a {
                            "center" => Align::Center,
                            "end" | "right" => Align::End,
                            "justify" => Align::Justify,
                            _ => Align::Start,
                        });
                    }
                }
                "list-style" => {
                    for level in node.children().filter(|n| n.is_element()) {
                        let ix: usize = attr(level, "level")
                            .and_then(|l| l.parse().ok())
                            .unwrap_or(1);
                        if style.numbered.len() < ix {
                            style.numbered.resize(ix, false);
                        }
                        style.numbered[ix - 1] =
                            level.tag_name().name() == "list-level-style-number";
                    }
                }
                _ => continue,
            }
            self.styles.insert(name.to_owned(), style);
        }
    }

    /// A style with what its parents set filled in.
    fn style(&self, name: Option<&str>) -> Style {
        let mut chain = Vec::new();
        let mut next = name;
        while let Some(style) = next.and_then(|n| self.styles.get(n)) {
            if chain.len() > 16 {
                break;
            }
            chain.push(style);
            next = style.parent.as_deref();
        }
        chain
            .into_iter()
            .rev()
            .fold(Style::default(), |acc, s| Style {
                parent: None,
                bold: s.bold.or(acc.bold),
                italic: s.italic.or(acc.italic),
                underline: s.underline.or(acc.underline),
                strike: s.strike.or(acc.strike),
                size: s.size.or(acc.size),
                color: s.color.or(acc.color),
                align: s.align.or(acc.align),
                numbered: if s.numbered.is_empty() {
                    acc.numbered
                } else {
                    s.numbered.clone()
                },
            })
    }

    /// `list` is the list style in force and how deep the list is.
    fn blocks(&mut self, parent: Node, out: &mut Vec<Block>, depth: u8, list: Option<&str>) {
        for node in parent.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "h" | "p" => self.paragraph(node, out, None, 0),
                "list" => {
                    let style = attr(node, "style-name").or(list);
                    self.list(node, out, depth, style);
                }
                "table" => out.push(Block::Table(self.table(node))),
                "section" | "table-of-content" | "index-body" | "alphabetical-index"
                | "illustration-index" | "user-index" => self.blocks(node, out, depth, list),
                _ => {}
            }
        }
    }

    fn list(&mut self, list: Node, out: &mut Vec<Block>, depth: u8, style: Option<&str>) {
        let numbered = self
            .style(style)
            .numbered
            .get(usize::from(depth))
            .copied()
            .unwrap_or(false);
        if self.counters.len() <= usize::from(depth) {
            self.counters.resize(usize::from(depth) + 1, 0);
        }
        self.counters[usize::from(depth)] = 0;
        for item in list
            .children()
            .filter(|n| matches!(n.tag_name().name(), "list-item" | "list-header"))
        {
            let mut first = true;
            for node in item.children().filter(|n| n.is_element()) {
                match node.tag_name().name() {
                    "p" | "h" => {
                        let marker = first.then(|| {
                            let counter = &mut self.counters[usize::from(depth)];
                            *counter += 1;
                            if numbered {
                                format!("{counter}.")
                            } else {
                                ["•", "◦", "▪"][usize::from(depth.min(2))].to_owned()
                            }
                        });
                        first = false;
                        self.paragraph(node, out, marker, depth);
                    }
                    "list" => {
                        let nested = attr(node, "style-name").or(style);
                        self.list(node, out, depth + 1, nested);
                    }
                    _ => {}
                }
            }
        }
    }

    fn paragraph(&mut self, node: Node, out: &mut Vec<Block>, marker: Option<String>, depth: u8) {
        let style_name = attr(node, "style-name");
        let style = self.style(style_name);
        let named = |wanted: &str| {
            let mut next = style_name;
            let mut hops = 0;
            while let Some(name) = next {
                if name == wanted || name.starts_with(&format!("{wanted}_")) {
                    return true;
                }
                next = self.styles.get(name).and_then(|s| s.parent.as_deref());
                hops += 1;
                if hops > 16 {
                    break;
                }
            }
            false
        };
        let role = if node.tag_name().name() == "h" {
            Role::Heading(
                attr(node, "outline-level")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(1),
            )
        } else if named("Title") {
            Role::Title
        } else if named("Subtitle") {
            Role::Subtitle
        } else {
            Role::Body
        };
        let mut paragraph = Paragraph {
            role,
            align: style.align.unwrap_or_default(),
            indent: if marker.is_some() {
                18. * f32::from(depth + 1)
            } else {
                0.
            },
            marker,
            spans: Vec::new(),
        };
        let mut images = Vec::new();
        self.inline(node, &style, &mut paragraph.spans, &mut images);
        out.push(Block::Paragraph(paragraph));
        out.extend(images.into_iter().map(Block::Image));
    }

    fn inline(
        &mut self,
        parent: Node,
        style: &Style,
        spans: &mut Vec<Span>,
        images: &mut Vec<Image>,
    ) {
        for node in parent.children() {
            if node.is_text() {
                push(spans, style, node.text().unwrap_or_default(), true);
                continue;
            }
            match node.tag_name().name() {
                "span" => {
                    let own = self.style(attr(node, "style-name"));
                    let merged = Style {
                        bold: own.bold.or(style.bold),
                        italic: own.italic.or(style.italic),
                        underline: own.underline.or(style.underline),
                        strike: own.strike.or(style.strike),
                        size: own.size.or(style.size),
                        color: own.color.or(style.color),
                        ..Style::default()
                    };
                    self.inline(node, &merged, spans, images);
                }
                "s" => {
                    let count = attr(node, "c").and_then(|c| c.parse().ok()).unwrap_or(1);
                    push(spans, style, &" ".repeat(count), false);
                }
                "tab" => push(spans, style, "    ", false),
                "line-break" => push(spans, style, "\n", false),
                "a" | "date" | "time" | "page-number" | "sequence" | "author-name" | "title"
                | "chapter" | "bookmark-ref" | "reference-ref" | "meta" => {
                    self.inline(node, style, spans, images);
                }
                "frame" => {
                    if let Some(image) = child(node, "image") {
                        let href = attr(image, "href").unwrap_or_default();
                        let size = attr(node, "width")
                            .and_then(points)
                            .zip(attr(node, "height").and_then(points))
                            .unwrap_or((300., 200.));
                        if let Some(data) = self.package.bytes(href) {
                            images.push(Image { data, size });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn table(&mut self, table: Node) -> Table {
        let mut out = Table::default();
        let mut rows = Vec::new();
        for node in table.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "table-row" => rows.push(node),
                "table-header-rows" | "table-rows" | "table-row-group" => {
                    rows.extend(children(node, "table-row"));
                }
                _ => {}
            }
        }
        for row in rows {
            let mut cells = Vec::new();
            for cell in row
                .children()
                .filter(|n| matches!(n.tag_name().name(), "table-cell" | "covered-table-cell"))
            {
                let mut blocks = Vec::new();
                if cell.tag_name().name() == "table-cell" {
                    self.blocks(cell, &mut blocks, 0, None);
                }
                cells.push(Cell {
                    span: attr(cell, "number-columns-spanned")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(1),
                    paragraphs: blocks
                        .into_iter()
                        .filter_map(|b| match b {
                            Block::Paragraph(p) => Some(p),
                            _ => None,
                        })
                        .collect(),
                });
            }
            out.rows.push(cells);
        }
        let columns = out
            .rows
            .iter()
            .map(|r| r.iter().map(|c| c.span.max(1)).sum::<usize>())
            .max()
            .unwrap_or(1)
            .max(1);
        out.widths = vec![1. / columns as f32; columns];
        out
    }
}

/// Adds text in `style`, joining it to the last span when they look alike.
/// Text from the XML has its white space collapsed, as OpenDocument asks;
/// spaces written as `<text:s/>` are kept.
fn push(spans: &mut Vec<Span>, style: &Style, text: &str, collapse: bool) {
    let mut span = Span {
        text: String::new(),
        bold: style.bold.unwrap_or(false),
        italic: style.italic.unwrap_or(false),
        underline: style.underline.unwrap_or(false),
        strike: style.strike.unwrap_or(false),
        size: style.size,
        color: style.color,
        font: None,
    };
    let mut spaced = spans.last().is_none_or(|s| s.text.ends_with([' ', '\n']));
    let mut add = |into: &mut String| {
        for c in text.chars() {
            if collapse && c.is_whitespace() {
                if !spaced {
                    into.push(' ');
                    spaced = true;
                }
            } else {
                into.push(c);
                spaced = c == ' ';
            }
        }
    };
    match spans.last_mut() {
        Some(last)
            if Span {
                text: String::new(),
                ..last.clone()
            } == span =>
        {
            add(&mut last.text)
        }
        _ => {
            add(&mut span.text);
            if !span.text.is_empty() {
                spans.push(span);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_convert_to_points() {
        assert_eq!(points("12pt"), Some(12.));
        assert_eq!(points("1in"), Some(72.));
        assert!((points("2.54cm").unwrap() - 72.).abs() < 1e-3);
        assert_eq!(points("bold"), None);
    }
}
