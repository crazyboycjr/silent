//! PowerPoint 97–2003 decks as an outline: each slide's title, then its
//! text, from the text atoms of the PowerPoint Document stream. The slides
//! are found the way PowerPoint finds them, through the current user's last
//! edit and the persist directory, so that slides saved over by quick saves
//! read as they were left.

use std::collections::HashMap;
use std::path::Path;

use super::ole::Compound;
use super::{Block, Document, Paragraph, Role, Span, u16_at, u32_at};
/// Master units, in which slide sizes are given, per point.
const MASTER_PER_POINT: f32 = 8.;

/// A slide's texts, each with its kind: title, body, and so on.
type Texts = Vec<(u32, String)>;

const DOCUMENT: u16 = 0x03E8;
const DOCUMENT_ATOM: u16 = 0x03E9;
const SLIDE: u16 = 0x03EE;
const SLIDE_PERSIST: u16 = 0x03F3;
const TEXT_HEADER: u16 = 0x0F9F;
const TEXT_CHARS: u16 = 0x0FA0;
const TEXT_BYTES: u16 = 0x0FA8;
const OUTLINE_TEXT_REF: u16 = 0x0F9E;
const SLIDE_LIST: u16 = 0x0FF0;
const USER_EDIT: u16 = 0x0FF5;
const PERSIST_DIRECTORY: u16 = 0x1772;

pub fn read(path: &Path) -> Option<Document> {
    let file = Compound::open(path)?;
    let deck = file.stream("PowerPoint Document")?;
    let user = file.stream("Current User").unwrap_or_default();
    outline(&deck, &user)
}

/// A record: its type and instance, and its body.
#[derive(Clone, Copy)]
struct Record<'d> {
    kind: u16,
    instance: u16,
    container: bool,
    body: &'d [u8],
}

/// The records one after another in `data`.
fn records(data: &[u8]) -> impl Iterator<Item = Record<'_>> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let head = u16_at(data, at)?;
        let kind = u16_at(data, at + 2)?;
        let len = u32_at(data, at + 4)? as usize;
        let body = data.get(at + 8..(at + 8).checked_add(len)?)?;
        at += 8 + len;
        Some(Record {
            kind,
            instance: head >> 4,
            container: head & 0xF == 0xF,
            body,
        })
    })
}

/// The record at `at`.
fn record_at(data: &[u8], at: usize) -> Option<Record<'_>> {
    records(data.get(at..)?).next()
}

/// Where each persist object is in the stream, by its id: every edit since
/// the first, the newest last so that it wins.
fn persist_directory(deck: &[u8], user: &[u8]) -> Option<HashMap<u32, usize>> {
    // CurrentUserAtom: header, size, token, then the offset of the last edit.
    let mut edit_at = u32_at(user, 16)? as usize;
    let mut edits = Vec::new();
    while edits.len() < 4096 {
        let edit = record_at(deck, edit_at).filter(|r| r.kind == USER_EDIT)?;
        edits.push(edit);
        let last = u32_at(edit.body, 8)? as usize;
        if last == 0 || last == edit_at {
            break;
        }
        edit_at = last;
    }
    let mut directory = HashMap::new();
    for edit in edits.iter().rev() {
        let at = u32_at(edit.body, 12)? as usize;
        let Some(dir) = record_at(deck, at).filter(|r| r.kind == PERSIST_DIRECTORY) else {
            continue;
        };
        let mut i = 0;
        while let Some(entry) = u32_at(dir.body, i) {
            let (first, count) = (entry & 0x000F_FFFF, entry >> 20);
            for k in 0..count {
                if let Some(offset) = u32_at(dir.body, i + 4 + 4 * k as usize) {
                    directory.insert(first + k, offset as usize);
                }
            }
            i += 4 + 4 * count as usize;
        }
    }
    Some(directory)
}

/// What a deck holds: the slide size, and each slide's texts with their
/// kinds, in slide order.
struct Slides {
    size: (f32, f32),
    slides: Vec<Texts>,
}

fn slides(deck: &[u8], user: &[u8]) -> Option<Slides> {
    let directory = persist_directory(deck, user)?;
    let edit = record_at(deck, u32_at(user, 16)? as usize)?;
    let document_id = u32_at(edit.body, 16)?;
    let document = record_at(deck, *directory.get(&document_id)?).filter(|r| r.kind == DOCUMENT)?;
    let mut size = (720., 540.);
    let mut slides = Vec::new();
    for record in records(document.body) {
        match record.kind {
            DOCUMENT_ATOM => {
                if let (Some(x), Some(y)) = (u32_at(record.body, 0), u32_at(record.body, 4)) {
                    size = (x as f32 / MASTER_PER_POINT, y as f32 / MASTER_PER_POINT);
                }
            }
            SLIDE_LIST if record.instance == 0 => {
                // Each slide's persist atom, then the outline text kept here.
                let mut listed: Vec<(Option<u32>, Texts)> = Vec::new();
                let mut kind = 4;
                for item in records(record.body) {
                    match item.kind {
                        SLIDE_PERSIST => listed.push((u32_at(item.body, 0), Vec::new())),
                        TEXT_HEADER => kind = u32_at(item.body, 0).unwrap_or(4),
                        TEXT_CHARS | TEXT_BYTES => {
                            if let Some((_, texts)) = listed.last_mut() {
                                texts.push((kind, text(item)));
                            }
                        }
                        _ => {}
                    }
                }
                for (id, outline) in listed {
                    let own = id
                        .and_then(|id| directory.get(&id))
                        .and_then(|&at| record_at(deck, at))
                        .filter(|r| r.kind == SLIDE)
                        .map(|slide| {
                            let mut texts = Vec::new();
                            collect(slide.body, &outline, &mut texts, 0);
                            texts
                        })
                        .unwrap_or_default();
                    // Text kept in the outline, unless the slide refers to it.
                    slides.push(if own.is_empty() { outline } else { own });
                }
            }
            _ => {}
        }
    }
    Some(Slides { size, slides })
}

/// The texts in a slide's drawing, in order: their own, or the outline's
/// that they refer to.
fn collect(data: &[u8], outline: &[(u32, String)], out: &mut Texts, depth: usize) {
    if depth > 32 {
        return;
    }
    let mut kind = 4;
    for record in records(data) {
        match record.kind {
            TEXT_HEADER => kind = u32_at(record.body, 0).unwrap_or(4),
            TEXT_CHARS | TEXT_BYTES => out.push((kind, text(record))),
            OUTLINE_TEXT_REF => {
                if let Some(text) = u32_at(record.body, 0).and_then(|i| outline.get(i as usize)) {
                    out.push(text.clone());
                }
            }
            _ if record.container => collect(record.body, outline, out, depth + 1),
            _ => {}
        }
    }
}

/// A text atom's characters: UTF-16, or one byte each (the low byte of
/// UTF-16, so Latin-1).
fn text(atom: Record) -> String {
    if atom.kind == TEXT_BYTES {
        return atom.body.iter().map(|&b| char::from(b)).collect();
    }
    let units = atom
        .body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Titles, then the rest; a slide with no title is called by its number.
fn outline(deck: &[u8], user: &[u8]) -> Option<Document> {
    let Slides { size, slides } = slides(deck, user).or_else(|| flat(deck))?;
    if slides.is_empty() {
        return None;
    }
    let mut blocks = Vec::new();
    for (n, texts) in slides.iter().enumerate() {
        // Title (0) and centred title (6).
        let title = texts
            .iter()
            .find(|(kind, text)| matches!(kind, 0 | 6) && !text.trim().is_empty())
            .map(|(_, text)| text.split(['\r', '\u{0B}']).collect::<Vec<_>>().join(" "));
        let heading = match &title {
            Some(title) => format!("{}. {}", n + 1, title.trim()),
            None => format!("Slide {}", n + 1),
        };
        blocks.push(Block::Paragraph(Paragraph {
            role: Role::Heading(2),
            spans: vec![Span {
                text: heading,
                bold: true,
                ..Span::default()
            }],
            ..Paragraph::default()
        }));
        for (kind, text) in texts {
            if matches!(kind, 0 | 6) && title.is_some() {
                continue;
            }
            // Body, half and quarter body placeholders are bulleted lists.
            let bulleted = matches!(kind, 1 | 7 | 8);
            for line in text.split('\r').map(|l| l.replace('\u{0B}', "\n")) {
                if line.trim().is_empty() {
                    continue;
                }
                blocks.push(Block::Paragraph(Paragraph {
                    marker: bulleted.then(|| "•".to_owned()),
                    indent: if bulleted { 18. } else { 0. },
                    spans: vec![Span {
                        text: line,
                        ..Span::default()
                    }],
                    ..Paragraph::default()
                }));
            }
        }
    }
    Some(Document {
        page: size,
        margins: (54., 54.),
        base_size: 14.,
        blocks,
    })
}

/// Without a usable edit history: the slides as they lie in the stream.
fn flat(deck: &[u8]) -> Option<Slides> {
    let slides: Vec<Texts> = records(deck)
        .filter(|r| r.kind == SLIDE)
        .map(|slide| {
            let mut texts = Vec::new();
            collect(slide.body, &[], &mut texts, 0);
            texts
        })
        .collect();
    Some(Slides {
        size: (720., 540.),
        slides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: u16, instance: u16, body: &[u8]) -> Vec<u8> {
        let container = matches!(
            kind,
            DOCUMENT | SLIDE | SLIDE_LIST | 0xF002 | 0xF003 | 0xF004 | 0xF00D | 0x040C
        );
        let head = (instance << 4) | if container { 0xF } else { 0 };
        let mut out = head.to_le_bytes().to_vec();
        out.extend(kind.to_le_bytes());
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    fn chars(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn u32s(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// A deck of two slides: the first's text kept in the outline, the
    /// second's in its drawing; then a quick save that replaced slide two.
    fn deck() -> (Vec<u8>, Vec<u8>) {
        let mut deck = Vec::new();
        let slide = |title: &str, body: &str| {
            let mut textbox = record(TEXT_HEADER, 0, &u32s(&[0]));
            textbox.extend(record(TEXT_CHARS, 0, &chars(title)));
            let mut body_box = record(TEXT_HEADER, 0, &u32s(&[1]));
            body_box.extend(record(TEXT_BYTES, 0, body.as_bytes()));
            let mut shapes = record(0xF00D, 0, &textbox);
            shapes.extend(record(0xF00D, 0, &body_box));
            let drawing = record(0x040C, 0, &record(0xF002, 0, &shapes));
            record(SLIDE, 0, &drawing)
        };
        // Slide one: an empty drawing that refers to the outline's texts.
        let first_at = deck.len() as u32;
        let refs = [
            record(OUTLINE_TEXT_REF, 0, &u32s(&[0])),
            record(OUTLINE_TEXT_REF, 0, &u32s(&[1])),
        ]
        .concat();
        deck.extend(record(SLIDE, 0, &record(0x040C, 0, &refs)));
        let second_at = deck.len() as u32;
        deck.extend(slide("Old title", "old"));
        let mut list = record(SLIDE_PERSIST, 0, &u32s(&[2, 0, 2, 256, 0]));
        list.extend(record(TEXT_HEADER, 0, &u32s(&[0])));
        list.extend(record(TEXT_CHARS, 0, &chars("Welcome")));
        list.extend(record(TEXT_HEADER, 0, &u32s(&[1])));
        list.extend(record(TEXT_CHARS, 0, &chars("First point\rSecond point")));
        list.extend(record(SLIDE_PERSIST, 0, &u32s(&[3, 0, 0, 257, 0])));
        let mut document = record(DOCUMENT_ATOM, 0, &u32s(&[5760, 4320]));
        document.extend(record(SLIDE_LIST, 0, &list));
        let document_at = deck.len() as u32;
        deck.extend(record(DOCUMENT, 0, &document));
        let dir_at = deck.len() as u32;
        // Ids 1, 2 and 3 from offset entries.
        deck.extend(record(
            PERSIST_DIRECTORY,
            0,
            &u32s(&[3 << 20 | 1, document_at, first_at, second_at]),
        ));
        let edit_at = deck.len() as u32;
        deck.extend(record(USER_EDIT, 0, &u32s(&[0, 0, 0, dir_at, 1, 4, 0])));
        // The quick save: a new slide two and a directory for it alone.
        let newer_at = deck.len() as u32;
        deck.extend(slide("Results", "Up 4%\u{0B}and rising"));
        let dir2_at = deck.len() as u32;
        deck.extend(record(
            PERSIST_DIRECTORY,
            0,
            &u32s(&[1 << 20 | 3, newer_at]),
        ));
        let edit2_at = deck.len() as u32;
        deck.extend(record(
            USER_EDIT,
            0,
            &u32s(&[0, 0, edit_at, dir2_at, 1, 4, 0]),
        ));
        let mut user = vec![0u8; 16];
        user.extend(edit2_at.to_le_bytes());
        (deck, user)
    }

    #[test]
    fn slides_read_through_the_last_edit() {
        let (deck, user) = deck();
        let document = outline(&deck, &user).unwrap();
        assert_eq!(document.page, (720., 540.));
        let lines: Vec<(String, Option<String>)> = document
            .blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph(p) => (p.text(), p.marker.clone()),
                _ => unreachable!(),
            })
            .collect();
        let bullet = Some("•".to_owned());
        assert_eq!(
            lines,
            [
                ("1. Welcome".to_owned(), None),
                ("First point".to_owned(), bullet.clone()),
                ("Second point".to_owned(), bullet.clone()),
                ("2. Results".to_owned(), None),
                ("Up 4%\nand rising".to_owned(), bullet),
            ]
        );
    }

    #[test]
    fn without_an_edit_history_slides_are_read_in_place() {
        let (deck, _) = deck();
        let document = outline(&deck, &[]).unwrap();
        let first = match &document.blocks[0] {
            Block::Paragraph(p) => p.text(),
            _ => unreachable!(),
        };
        // The first slide's text lived in the outline, which this misses.
        assert_eq!(first, "Slide 1");
        assert!(
            document
                .blocks
                .iter()
                .any(|b| matches!(b, Block::Paragraph(p) if p.text() == "3. Results"))
        );
    }
}
