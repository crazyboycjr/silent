//! A document's column drawn small, for its overview: where each block
//! sits down the column, and marks for its lines of text, its tables and
//! its pictures. It is worked out from the text alone, with average letter
//! widths and the spacing the window gives paragraphs, so it has the
//! column's shape without laying any of it out.

use unicode_width::UnicodeWidthChar;

use super::{Block, Document, Paragraph, Role, Table, family};

/// Something to draw, in points: across from the column's left edge, and
/// down from its top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A line of a title or heading.
    Heading,
    /// A line of body text.
    Text,
    /// A table's frame, and the lines between its rows and cells.
    Rule,
    Picture,
}

#[derive(Debug, Default)]
pub struct Miniature {
    /// Each block's top and height, in points down the column.
    pub blocks: Vec<(f32, f32)>,
    /// In the order of their tops.
    pub marks: Vec<Mark>,
    /// The column's width and height, in points.
    pub width: f32,
    pub height: f32,
}

/// What the window adds around blocks, in their own size: a line is 1.3
/// of it, a paragraph ends 0.45 below its text (a heading 0.35, after 0.6
/// above), and table cells are padded 3 points, pictures 6.
const LINE: f32 = 1.3;
const PARAGRAPH_AFTER: f32 = 0.45;
const HEADING_AFTER: f32 = 0.35;
const HEADING_BEFORE: f32 = 0.6;
const CELL_PAD: (f32, f32) = (5., 3.);
const PICTURE_PAD: f32 = 6.;
/// The text of a line takes this much of its height, centred.
const INK: f32 = 0.42;

impl Miniature {
    pub fn of(doc: &Document) -> Self {
        let width = column(doc);
        let mut out = Self {
            width,
            ..Self::default()
        };
        for block in &doc.blocks {
            let top = out.height;
            let height = match block {
                Block::Paragraph(p) => paragraph(p, doc.base_size, 0., top, width, &mut out.marks),
                Block::Table(t) => table(t, doc.base_size, top, width, &mut out.marks),
                Block::Image(image) => {
                    let w = image.size.0.min(width).max(1.);
                    let h = w * image.size.1 / image.size.0.max(1.);
                    out.marks.push(Mark {
                        x: 0.,
                        y: top + PICTURE_PAD,
                        w,
                        h,
                        kind: Kind::Picture,
                    });
                    h + 2. * PICTURE_PAD
                }
            };
            out.blocks.push((top, height));
            out.height += height;
        }
        out
    }

    /// The block `y` points down the column, and how far into it; past the
    /// end, the end of the last.
    pub fn block_at(&self, y: f32) -> (usize, f32) {
        let ix = self
            .blocks
            .partition_point(|&(top, height)| top + height <= y)
            .min(self.blocks.len().saturating_sub(1));
        let (top, height) = self.blocks.get(ix).copied().unwrap_or_default();
        (ix, (y - top).clamp(0., height))
    }

    /// Points down the column to `within` points into block `ix`.
    pub fn y_of(&self, ix: usize, within: f32) -> f32 {
        match self.blocks.get(ix) {
            Some(&(top, height)) => top + within.clamp(0., height),
            None => self.height,
        }
    }

    /// The marks that reach into `top..bottom`.
    pub fn marks_within(&self, top: f32, bottom: f32) -> impl Iterator<Item = &Mark> {
        // Marks are in the order of their tops, and none is taller than a
        // picture, so those starting a page above can still reach in.
        let tallest = self
            .marks
            .iter()
            .filter(|m| m.kind == Kind::Picture)
            .fold(0f32, |tallest, m| tallest.max(m.h));
        let first = self.marks.partition_point(|m| m.y < top - tallest);
        self.marks[first..]
            .iter()
            .take_while(move |m| m.y <= bottom)
            .filter(move |m| m.y + m.h >= top)
    }
}

/// The text column's width in points, as the window draws it: the page
/// less its left margin on both sides.
pub fn column(doc: &Document) -> f32 {
    let page = doc.page.0.max(1.);
    let margin = doc.margins.0.clamp(page / 50., page / 4.);
    page - 2. * margin
}

/// Points for a paragraph: its largest run, or its role's size.
pub fn size_of(paragraph: &Paragraph, base: f32) -> f32 {
    let own = paragraph
        .spans
        .iter()
        .filter_map(|s| s.size)
        .fold(None, |max: Option<f32>, s| {
            Some(max.map_or(s, |m| m.max(s)))
        });
    own.unwrap_or(match paragraph.role {
        Role::Title => 26.,
        Role::Subtitle => 15.,
        Role::Heading(1) => 20.,
        Role::Heading(2) => 16.,
        Role::Heading(3) => 14.,
        _ => base,
    })
}

/// A paragraph at `top`, `x` points into a column `width` wide; its marks
/// go to `marks`, and its height comes back.
fn paragraph(p: &Paragraph, base: f32, x: f32, top: f32, width: f32, marks: &mut Vec<Mark>) -> f32 {
    let size = size_of(p, base);
    let heading = p.role != Role::Body;
    // A list's marker hangs in the indent, which is at least this wide.
    let indent = if p.marker.is_some() {
        p.indent.max(size * 1.6)
    } else {
        p.indent
    };
    let room = (width - indent).max(size);
    let font = family(p.spans.iter().find_map(|s| s.font.as_deref()));
    let lines = wrap(&p.text(), size, room, breadth(font));
    let before = if heading { size * HEADING_BEFORE } else { 0. };
    let line = size * LINE;
    let kind = if heading { Kind::Heading } else { Kind::Text };
    let first = top + before + line * (1. - INK) / 2.;
    if p.marker.is_some() {
        marks.push(Mark {
            x: x + indent - size * 1.1,
            y: first,
            w: size * 0.5,
            h: line * INK,
            kind,
        });
    }
    for (i, &w) in lines.iter().enumerate() {
        marks.push(Mark {
            x: x + indent,
            y: first + i as f32 * line,
            w,
            h: line * INK,
            kind,
        });
    }
    let after = size
        * if heading {
            HEADING_AFTER
        } else {
            PARAGRAPH_AFTER
        };
    before + lines.len().max(1) as f32 * line + after
}

/// A table at `top`; its cells' text is set at 0.95 of the body's size.
fn table(t: &Table, base: f32, top: f32, width: f32, marks: &mut Vec<Mark>) -> f32 {
    let size = base * 0.95;
    let margin = size * 0.6;
    let total: f32 = t.widths.iter().sum::<f32>().max(1e-3);
    let mut y = top + margin;
    let rule = |marks: &mut Vec<Mark>, x: f32, y: f32, w: f32, h: f32| {
        marks.push(Mark {
            x,
            y,
            w,
            h,
            kind: Kind::Rule,
        })
    };
    rule(marks, 0., y, width, 0.);
    for row in &t.rows {
        let mut x = 0.;
        let mut column = 0;
        let mut height: f32 = 0.;
        let mut cells = Vec::new();
        for cell in row {
            let share: f32 = t.widths.iter().skip(column).take(cell.span.max(1)).sum();
            column += cell.span.max(1);
            let w = if t.widths.is_empty() {
                width / row.len().max(1) as f32
            } else {
                width * share / total
            };
            let mut inner = Vec::new();
            let mut h = 0.;
            for p in &cell.paragraphs {
                let small = Paragraph {
                    role: Role::Body,
                    spans: p.spans.clone(),
                    ..Paragraph::default()
                };
                let room = (w - 2. * CELL_PAD.0).max(size);
                let at = y + CELL_PAD.1 + h;
                h += paragraph(&small, size, x + CELL_PAD.0, at, room, &mut inner)
                    - size * PARAGRAPH_AFTER;
            }
            height = height.max(h + 2. * CELL_PAD.1);
            cells.push((x, inner));
            x += w;
        }
        for (x, inner) in cells {
            rule(marks, x, y, 0., height);
            marks.extend(inner);
        }
        rule(marks, width, y, 0., height);
        y += height;
        rule(marks, 0., y, width, 0.);
    }
    // Rows can push marks of a tall cell past a later rule; keep them in
    // the order of their tops.
    let start = marks.partition_point(|m| m.y < top);
    marks[start..].sort_by(|a, b| a.y.total_cmp(&b.y));
    y + margin - top
}

/// How wide a family's letters run against Carlito's, the face Word's
/// Calibri is drawn in and the most common.
fn breadth(family: &str) -> f32 {
    match family {
        "Caladea" => 1.04,
        "Liberation Sans" => 1.14,
        "Liberation Mono" => 1.36,
        "Noto Sans" => 1.2,
        _ => 1.,
    }
}

/// The widths of the lines `text` wraps into at `size` points in `room`,
/// its letters `breadth` times as wide as Carlito's: words go whole onto a
/// line while they fit, and a word wider than a line, or any run of
/// Chinese or Japanese, breaks wherever the line ends.
pub fn wrap(text: &str, size: f32, room: f32, breadth: f32) -> Vec<f32> {
    // Carlito's average advances, in ems.
    let advance = |c: char| -> f32 {
        size * match c.width().unwrap_or(0) {
            0 => 0.,
            2 => 1.,
            _ if c.is_whitespace() => 0.226 * breadth,
            _ if c.is_uppercase() => 0.58 * breadth,
            _ if c.is_ascii_digit() => 0.507 * breadth,
            _ => 0.44 * breadth,
        }
    };
    let space = advance(' ');
    let mut lines = Vec::new();
    let mut line: f32 = 0.;
    for word in text.split_whitespace() {
        let wide = word.chars().any(|c| c.width() == Some(2));
        let w: f32 = word.chars().map(advance).sum();
        let gap = if line > 0. { space } else { 0. };
        if !wide && line + gap + w <= room {
            line += gap + w;
            continue;
        }
        if !wide && w <= room {
            lines.push(line);
            line = w;
            continue;
        }
        // Broken where the line ends, letter by letter.
        line += gap;
        for c in word.chars() {
            let a = advance(c);
            if line + a > room && line > 0. {
                lines.push(line);
                line = 0.;
            }
            line += a;
        }
    }
    if line > 0. || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::{Cell, Image, Span};

    fn text(role: Role, words: &str) -> Block {
        Block::Paragraph(Paragraph {
            role,
            spans: vec![Span {
                text: words.into(),
                ..Span::default()
            }],
            ..Paragraph::default()
        })
    }

    fn doc(blocks: Vec<Block>) -> Document {
        Document {
            page: (612., 792.),
            margins: (72., 72.),
            base_size: 11.,
            blocks,
        }
    }

    #[test]
    fn words_wrap_whole_and_long_runs_break() {
        // 11 pt in a 100 pt line: "aaaa" is 19.36 pt, a space 2.49.
        let lines = wrap("aaaa aaaa aaaa aaaa aaaa", 11., 100., 1.);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|&w| w <= 100.));
        // In a wider face the same words take more room.
        assert_eq!(wrap("aaaa aaaa aaaa aaaa aaaa", 11., 100., 1.2).len(), 2);
        assert_eq!(wrap(&"aaaa ".repeat(8), 11., 100., 1.2).len(), 3);
        // Chinese breaks anywhere: 30 characters of 11 pt in 100 pt lines.
        let chinese = wrap(&"床".repeat(30), 11., 100., 1.);
        assert_eq!(chinese.len(), 4);
        assert!((chinese[3] - 33.).abs() < 1e-3);
        assert_eq!(wrap("", 11., 100., 1.), [0.]);
    }

    #[test]
    fn blocks_follow_each_other_down_the_column() {
        let d = doc(vec![
            text(Role::Title, "Quarterly Notes"),
            text(Role::Body, &"word ".repeat(200)),
            Block::Image(Image {
                data: Vec::new(),
                size: (800., 450.),
            }),
        ]);
        let mini = Miniature::of(&d);
        assert_eq!(mini.width, 468.);
        assert_eq!(mini.blocks.len(), 3);
        let (title, body, picture) = (mini.blocks[0], mini.blocks[1], mini.blocks[2]);
        // The title: one line of 26 pt with the room a heading takes.
        assert!((title.1 - 26. * (0.6 + 1.3 + 0.35)).abs() < 1e-3);
        assert_eq!(body.0, title.0 + title.1);
        // 200 words of 11 pt wrap onto about ten lines.
        let lines = (body.1 - 11. * 0.45) / (11. * 1.3);
        assert!((8.0..14.0).contains(&lines), "{lines}");
        // The picture is shrunk to the column, keeping its shape.
        assert!((picture.1 - (468. * 450. / 800. + 12.)).abs() < 1e-3);
        assert!((mini.height - (picture.0 + picture.1)).abs() < 1e-3);
        // Marks are in order down the column.
        assert!(mini.marks.windows(2).all(|w| w[0].y <= w[1].y));
        assert!(mini.marks.iter().any(|m| m.kind == Kind::Heading));
        assert!(mini.marks.iter().any(|m| m.kind == Kind::Picture));
    }

    #[test]
    fn tables_are_framed_and_as_tall_as_their_fullest_cell() {
        let cell = |words: &str| Cell {
            span: 1,
            paragraphs: vec![Paragraph {
                spans: vec![Span {
                    text: words.into(),
                    ..Span::default()
                }],
                ..Paragraph::default()
            }],
        };
        let table = Table {
            widths: vec![1., 1.],
            rows: vec![
                vec![cell("Format"), cell("Notes")],
                vec![cell("docx"), cell(&"long note ".repeat(30))],
            ],
        };
        let mini = Miniature::of(&doc(vec![Block::Table(table)]));
        let rules = mini.marks.iter().filter(|m| m.kind == Kind::Rule).count();
        // Above and below each row, and on either side of each cell.
        assert_eq!(rules, 1 + 2 * 3 + 2);
        let size = 11. * 0.95;
        let one_line = size * 1.3 + 6.;
        let height = mini.blocks[0].1 - 2. * size * 0.6;
        assert!(height > 2. * one_line + size * 1.3, "{height}");
    }

    #[test]
    fn places_in_the_column_map_to_blocks_and_back() {
        let d = doc(vec![
            text(Role::Body, "one"),
            text(Role::Body, "two"),
            text(Role::Body, "three"),
        ]);
        let mini = Miniature::of(&d);
        let (top, height) = mini.blocks[1];
        assert_eq!(mini.block_at(top + height / 2.), (1, height / 2.));
        assert_eq!(mini.y_of(1, height / 2.), top + height / 2.);
        assert_eq!(mini.block_at(-5.), (0, 0.));
        let last = mini.blocks[2];
        assert_eq!(mini.block_at(1e6), (2, last.1));
        assert_eq!(Miniature::of(&doc(Vec::new())).block_at(10.), (0, 0.));
    }

    #[test]
    fn only_the_marks_in_range_are_drawn() {
        let blocks = (0..100)
            .map(|i| text(Role::Body, &format!("paragraph {i}")))
            .collect();
        let mini = Miniature::of(&doc(blocks));
        let (top, bottom) = (mini.height / 2., mini.height / 2. + 100.);
        let within: Vec<_> = mini.marks_within(top, bottom).collect();
        assert!(!within.is_empty() && within.len() < 20);
        assert!(within.iter().all(|m| m.y + m.h >= top && m.y <= bottom));
    }
}
