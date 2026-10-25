//! Word 97–2003 documents, and WPS Writer's files in the same format: the
//! OLE2 file's WordDocument stream, read through its piece table into
//! paragraphs and tables, with headings from the style sheet, lists with
//! their bullets and numbers, and each run's bold, italic, underline, size
//! and colour. Pictures, headers and notes are left out; a field shows its
//! result.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use super::docx::number;
use super::ole::Compound;
use super::{
    Align, Block, Cell, Document, Paragraph, Role, Span, TWIPS_PER_POINT, Table, u16_at, u32_at,
};

/// The first FIB of Word 97; Word 6 and 95 lay theirs out otherwise.
const WORD97: u16 = 0x00C1;
/// Places in FibRgFcLcb97 of the structures read from the table stream.
const STSHF: usize = 1;
const PLCF_SED: usize = 6;
const PLCF_BTE_CHPX: usize = 12;
const PLCF_BTE_PAPX: usize = 13;
const CLX: usize = 33;
const PLF_LST: usize = 73;
const PLF_LFO: usize = 74;
/// A style that is no style, as `istdBase` writes it.
const NO_STYLE: usize = 0x0FFF;

pub fn read(path: &Path) -> Option<Document> {
    let file = Compound::open(path)?;
    let word = file.stream("WordDocument")?;
    let fib = Fib::read(&word)?;
    let table = file.stream(if fib.table1 { "1Table" } else { "0Table" })?;
    parse(&word, &table, &fib)
}

/// The File Information Block at the start of the WordDocument stream.
struct Fib {
    /// The table stream is 1Table rather than 0Table.
    table1: bool,
    /// Characters in the main text, which comes first.
    text: u32,
    /// Where each structure is in the table stream, and its length.
    places: Vec<(u32, u32)>,
}

impl Fib {
    fn read(word: &[u8]) -> Option<Self> {
        if u16_at(word, 0)? != 0xA5EC || u16_at(word, 2)? < WORD97 {
            return None;
        }
        let flags = u16_at(word, 0x0A)?;
        // Encrypted or obfuscated text cannot be read.
        if flags & 0x0100 != 0 {
            return None;
        }
        let shorts = usize::from(u16_at(word, 0x20)?);
        let longs_at = 0x22 + shorts * 2;
        let longs = usize::from(u16_at(word, longs_at)?);
        let text = u32_at(word, longs_at + 2 + 3 * 4)?;
        let places_at = longs_at + 2 + longs * 4;
        let count = usize::from(u16_at(word, places_at)?);
        let places = (0..count)
            .map_while(|i| {
                let at = places_at + 2 + 8 * i;
                Some((u32_at(word, at)?, u32_at(word, at + 4)?))
            })
            .collect();
        Some(Self {
            table1: flags & 0x0200 != 0,
            text,
            places,
        })
    }

    /// Structure `index` of FibRgFcLcb, from the table stream.
    fn get<'t>(&self, table: &'t [u8], index: usize) -> Option<&'t [u8]> {
        let &(at, len) = self.places.get(index)?;
        let at = at as usize;
        (len > 0)
            .then(|| table.get(at..at.checked_add(len as usize)?))
            .flatten()
    }
}

/// A piece of the text: its characters, and where they are in the stream.
#[derive(Debug, PartialEq)]
struct Piece {
    cps: Range<u32>,
    /// The byte where the piece starts.
    at: u32,
    /// One byte a character, in Windows-1252, rather than two in UTF-16.
    compressed: bool,
}

/// The piece table from the Clx: formatting records first, then the pieces.
fn pieces(clx: &[u8]) -> Option<Vec<Piece>> {
    let mut at = 0;
    while *clx.get(at)? == 1 {
        at += 3 + usize::from(u16_at(clx, at + 1)?);
    }
    if *clx.get(at)? != 2 {
        return None;
    }
    let len = u32_at(clx, at + 1)? as usize;
    let plc = clx.get(at + 5..at + 5 + len)?;
    let n = len.checked_sub(4)? / 12;
    let cp = |i: usize| u32_at(plc, 4 * i);
    (0..n)
        .map(|i| {
            let raw = u32_at(plc, 4 * (n + 1) + 8 * i + 2)?;
            let compressed = raw & 0x4000_0000 != 0;
            let fc = raw & 0x3FFF_FFFF;
            Some(Piece {
                cps: cp(i)?..cp(i + 1)?,
                at: if compressed { fc / 2 } else { fc },
                compressed,
            })
        })
        .collect()
}

/// The characters of `cps`, each with the byte it starts at, which is
/// what the formatting is looked up by.
fn characters(word: &[u8], pieces: &[Piece], cps: Range<u32>) -> Vec<(char, u32)> {
    let mut out = Vec::new();
    for piece in pieces {
        let start = piece.cps.start.max(cps.start);
        let end = piece.cps.end.min(cps.end);
        if start >= end {
            continue;
        }
        let skip = (start - piece.cps.start) as usize;
        let count = (end - start) as usize;
        if piece.compressed {
            let from = piece.at as usize + skip;
            let Some(bytes) = word.get(from..from + count) else {
                continue;
            };
            // Windows-1252 has a character for every byte.
            let text = encoding_rs::WINDOWS_1252
                .decode_without_bom_handling(bytes)
                .0;
            out.extend(text.chars().zip((from as u32)..));
        } else {
            let from = piece.at as usize + 2 * skip;
            let Some(bytes) = word.get(from..from + 2 * count) else {
                continue;
            };
            let units = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair));
            let mut fc = from as u32;
            for c in char::decode_utf16(units) {
                let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
                out.push((c, fc));
                fc += 2 * c.len_utf16() as u32;
            }
        }
    }
    out
}

/// The runs of a bin table, PlcfBteChpx or PlcfBtePapx: the formatted
/// pages it points to in the WordDocument stream, unpacked into the bytes
/// they cover and their properties, in order.
fn runs<'w>(word: &'w [u8], plcf: Option<&[u8]>, papx: bool) -> Vec<(Range<u32>, &'w [u8])> {
    let mut out = Vec::new();
    let Some(plcf) = plcf else {
        return out;
    };
    let n = plcf.len().saturating_sub(4) / 8;
    for i in 0..n {
        let Some(pn) = u32_at(plcf, 4 * (n + 1) + 4 * i) else {
            break;
        };
        let at = (pn & 0x003F_FFFF) as usize * 512;
        let Some(page) = word.get(at..at + 512) else {
            continue;
        };
        let count = usize::from(page[511]);
        for r in 0..count {
            let (Some(start), Some(end)) = (u32_at(page, 4 * r), u32_at(page, 4 * r + 4)) else {
                break;
            };
            let props = if papx {
                papx_at(page, 4 * (count + 1) + 13 * r)
            } else {
                chpx_at(page, 4 * (count + 1) + r)
            };
            out.push((start..end, props.unwrap_or_default()));
        }
    }
    out
}

fn chpx_at(page: &[u8], at: usize) -> Option<&[u8]> {
    let offset = 2 * usize::from(*page.get(at)?);
    if offset == 0 {
        return None;
    }
    let len = usize::from(*page.get(offset)?);
    page.get(offset + 1..offset + 1 + len)
}

/// A paragraph's style and sprms, GrpPrlAndIstd.
fn papx_at(page: &[u8], at: usize) -> Option<&[u8]> {
    let offset = 2 * usize::from(*page.get(at)?);
    if offset == 0 {
        return None;
    }
    match usize::from(*page.get(offset)?) {
        0 => {
            let len = 2 * usize::from(*page.get(offset + 1)?);
            page.get(offset + 2..offset + 2 + len)
        }
        cb => page.get(offset + 1..offset + 2 * cb),
    }
}

/// The run of `runs` that covers byte `fc`.
fn run_at<'w>(runs: &[(Range<u32>, &'w [u8])], fc: u32) -> &'w [u8] {
    let i = runs.partition_point(|(range, _)| range.end <= fc);
    match runs.get(i) {
        Some((range, props)) if range.start <= fc => props,
        _ => &[],
    }
}

/// The single property modifiers of a grpprl, each with its operand.
fn sprms(grpprl: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let sprm = u16_at(grpprl, at)?;
        let size = match sprm >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            // A table's definition counts its operand in two bytes.
            _ if sprm == 0xD608 => usize::from(u16_at(grpprl, at + 2)?) + 1,
            _ => usize::from(*grpprl.get(at + 2)?) + 1,
        };
        let operand = grpprl.get(at + 2..at + 2 + size)?;
        at += 2 + size;
        Some((sprm, operand))
    })
}

/// Character properties, each unset until a style or a run sets it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Chars {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    size: Option<f32>,
    color: Option<u32>,
}

impl Chars {
    /// `grpprl` laid over these; toggles count against `style`.
    fn with(&self, grpprl: &[u8], style: &Chars) -> Chars {
        let mut out = self.clone();
        let toggle = |op: &[u8], styled: Option<bool>| match op.first() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            Some(0x80) => styled,
            Some(0x81) => Some(!styled.unwrap_or(false)),
            _ => None,
        };
        for (sprm, op) in sprms(grpprl) {
            match sprm {
                0x0835 => out.bold = toggle(op, style.bold).or(out.bold),
                0x0836 => out.italic = toggle(op, style.italic).or(out.italic),
                0x0837 | 0x2A53 => out.strike = toggle(op, style.strike).or(out.strike),
                0x2A3E => out.underline = op.first().map(|&kul| kul != 0),
                0x4A43 => out.size = u16_at(op, 0).map(|half| f32::from(half) / 2.),
                0x2A42 => out.color = op.first().and_then(|&ico| ico_color(ico)),
                0x6870 => {
                    out.color = u32_at(op, 0)
                        .filter(|cv| cv >> 24 == 0)
                        .map(|cv| ((cv & 0xFF) << 16) | (cv & 0xFF00) | ((cv >> 16) & 0xFF))
                }
                _ => {}
            }
        }
        out
    }

    fn span(&self, text: String) -> Span {
        Span {
            text,
            bold: self.bold.unwrap_or(false),
            italic: self.italic.unwrap_or(false),
            underline: self.underline.unwrap_or(false),
            strike: self.strike.unwrap_or(false),
            size: self.size,
            // Black is the paper's ink already; it would not follow it.
            color: self.color.filter(|&c| c != 0),
            font: None,
        }
    }
}

/// The sixteen colours of the old `ico` index.
fn ico_color(ico: u8) -> Option<u32> {
    const COLORS: [u32; 16] = [
        0x000000, 0x0000FF, 0x00FFFF, 0x00FF00, 0xFF00FF, 0xFF0000, 0xFFFF00, 0xFFFFFF, 0x000080,
        0x008080, 0x008000, 0x800080, 0x800000, 0x808000, 0x808080, 0xC0C0C0,
    ];
    COLORS.get(usize::from(ico).checked_sub(1)?).copied()
}

/// Paragraph properties.
#[derive(Clone, Debug, Default)]
struct Para {
    in_table: bool,
    /// The mark that ends a table row rather than a cell.
    row_end: bool,
    /// How deep in nested tables, 1 for a table at the top.
    depth: u32,
    align: Align,
    /// The left indent, in points.
    indent: Option<f32>,
    /// A heading's outline level, from 0.
    outline: Option<u8>,
    /// The list, by its 1-based list format index, and the level in it.
    list: u16,
    level: u8,
    /// A table row's column edges, in twips.
    edges: Vec<i16>,
}

impl Para {
    fn with(&self, grpprl: &[u8]) -> Para {
        let mut out = self.clone();
        for (sprm, op) in sprms(grpprl) {
            let byte = op.first().copied().unwrap_or(0);
            match sprm {
                0x2416 => out.in_table = byte != 0,
                0x2417 => out.row_end = byte != 0,
                0x6649 => out.depth = u32_at(op, 0).unwrap_or(0),
                0x2403 | 0x2461 => {
                    out.align = match byte {
                        1 => Align::Center,
                        2 => Align::End,
                        3 | 4 => Align::Justify,
                        _ => Align::Start,
                    }
                }
                0x840F | 0x845E => {
                    out.indent =
                        u16_at(op, 0).map(|twips| f32::from(twips as i16) / TWIPS_PER_POINT)
                }
                0x2640 => out.outline = (byte < 9).then_some(byte),
                0x460B => out.list = u16_at(op, 0).unwrap_or(0),
                0x260A => out.level = byte.min(8),
                0xD608 => {
                    // Two bytes of size, the cell count, then each edge.
                    let cells = usize::from(op.get(2).copied().unwrap_or(0));
                    out.edges = (0..=cells)
                        .map_while(|i| u16_at(op, 3 + 2 * i).map(|e| e as i16))
                        .collect();
                }
                _ => {}
            }
        }
        out
    }
}

/// A style from the style sheet, with what it inherits laid in.
#[derive(Clone, Debug, Default)]
struct Style {
    role: Role,
    bullet: bool,
    para: Para,
    chars: Chars,
}

/// The style sheet, by style index; slots without a style are `None`.
fn styles(stsh: Option<&[u8]>) -> Vec<Option<Style>> {
    let Some(stsh) = stsh else {
        return Vec::new();
    };
    struct Raw<'s> {
        sti: u16,
        base: usize,
        para: &'s [u8],
        chars: &'s [u8],
    }
    let (Some(header), Some(count), Some(base_size)) =
        (u16_at(stsh, 0), u16_at(stsh, 2), u16_at(stsh, 4))
    else {
        return Vec::new();
    };
    let mut raw: Vec<Option<Raw>> = Vec::with_capacity(usize::from(count));
    let mut at = 2 + usize::from(header);
    for _ in 0..count {
        let Some(len) = u16_at(stsh, at).map(usize::from) else {
            break;
        };
        let std = stsh.get(at + 2..at + 2 + len).unwrap_or_default();
        at += 2 + len;
        raw.push((|| {
            let sti = u16_at(std, 0)? & 0x0FFF;
            let kind = u16_at(std, 2)? & 0xF;
            let base = usize::from(u16_at(std, 2)? >> 4);
            let upxs = usize::from(u16_at(std, 4)? & 0xF);
            // The name, then the formatting, each part at an even offset.
            let name = usize::from(base_size);
            let mut upx = name + 2 + 2 * usize::from(u16_at(std, name)?) + 2;
            let mut parts = Vec::with_capacity(upxs);
            for _ in 0..upxs {
                upx += upx % 2;
                let len = usize::from(u16_at(std, upx)?);
                parts.push(std.get(upx + 2..upx + 2 + len)?);
                upx += 2 + len;
            }
            let (para, chars) = match (kind, parts.as_slice()) {
                // A paragraph style: its PAPX starts with the style index.
                (1, [papx, chpx, ..]) => (papx.get(2..).unwrap_or_default(), *chpx),
                (2, [chpx, ..]) => (&[][..], *chpx),
                _ => (&[][..], &[][..]),
            };
            Some(Raw {
                sti,
                base,
                para,
                chars,
            })
        })());
    }
    // Each style over what it is based on, the bases first.
    let mut done: Vec<Option<Style>> = vec![None; raw.len()];
    for i in 0..raw.len() {
        resolve(i, &raw, &mut done, 0);
    }
    return done;

    fn resolve(i: usize, raw: &[Option<Raw>], done: &mut [Option<Style>], depth: usize) {
        if done[i].is_some() || depth > 16 {
            return;
        }
        let Some(style) = &raw[i] else {
            return;
        };
        let base = match style.base {
            NO_STYLE => Style::default(),
            b if b < raw.len() && b != i => {
                resolve(b, raw, done, depth + 1);
                done[b].clone().unwrap_or_default()
            }
            _ => Style::default(),
        };
        let role = match style.sti {
            n @ 1..=9 => Role::Heading(n as u8),
            62 => Role::Title,
            74 => Role::Subtitle,
            _ => base.role,
        };
        let chars = base.chars.with(style.chars, &base.chars);
        done[i] = Some(Style {
            role,
            bullet: matches!(style.sti, 48 | 54..=57) || base.bullet,
            para: base.para.with(style.para),
            chars,
        });
    }
}

/// The first section's page size and side margins, in points.
fn page(word: &[u8], plcf: Option<&[u8]>) -> ((f32, f32), (f32, f32)) {
    let (mut page, mut margins) = ((612., 792.), (90., 90.));
    let sepx = plcf
        .and_then(|plcf| {
            let n = plcf.len().checked_sub(4)? / 16;
            (n > 0).then(|| u32_at(plcf, 4 * (n + 1) + 2))?
        })
        .filter(|&at| at != u32::MAX)
        .and_then(|at| {
            let at = at as usize;
            word.get(at + 2..at + 2 + usize::from(u16_at(word, at)?))
        });
    for (sprm, op) in sprms(sepx.unwrap_or_default()) {
        let Some(points) = u16_at(op, 0).map(|twips| f32::from(twips) / TWIPS_PER_POINT) else {
            continue;
        };
        match sprm {
            0xB01F => page.0 = points,
            0xB020 => page.1 = points,
            0xB021 => margins.0 = points,
            0xB022 => margins.1 = points,
            _ => {}
        }
    }
    (page, margins)
}

/// A list level: its first number, how numbers are written, and the
/// text around them, where characters 0 to 8 stand for each level's number.
#[derive(Clone, Debug)]
struct Level {
    start: u32,
    format: u8,
    text: Vec<u16>,
    indent: Option<f32>,
}

/// The lists, and how far each has counted.
#[derive(Default)]
struct Lists {
    /// Each list's levels.
    lists: Vec<Vec<Level>>,
    /// The list each list format index stands for, from 1.
    formats: Vec<Option<usize>>,
    counters: HashMap<usize, [u32; 9]>,
}

impl Lists {
    /// PlfLst, with the levels that follow it, and PlfLfo.
    fn read(table: &[u8], fib: &Fib) -> Self {
        let mut out = Lists::default();
        let mut ids = Vec::new();
        if let Some(&(at, _)) = fib.places.get(PLF_LST) {
            let at = at as usize;
            let count = u16_at(table, at).map_or(0, usize::from).min(4096);
            let mut level_at = at + 2 + 28 * count;
            for i in 0..count {
                let lstf = at + 2 + 28 * i;
                let (Some(id), Some(&flags)) = (u32_at(table, lstf), table.get(lstf + 26)) else {
                    break;
                };
                let simple = flags & 1 != 0;
                let mut levels = Vec::new();
                for _ in 0..if simple { 1 } else { 9 } {
                    let Some((level, len)) = level(table, level_at) else {
                        break;
                    };
                    levels.push(level);
                    level_at += len;
                }
                ids.push(id);
                out.lists.push(levels);
            }
        }
        if let Some(lfo) = fib.get(table, PLF_LFO) {
            let count = u32_at(lfo, 0).unwrap_or(0).min(65536) as usize;
            out.formats = (0..count)
                .map(|i| {
                    let id = u32_at(lfo, 4 + 16 * i)?;
                    ids.iter().position(|&known| known == id)
                })
                .collect();
        }
        out
    }

    /// The bullet or number of the next paragraph at `level` of list
    /// format `format`, counting as Word does, and the indent it brings.
    fn marker(&mut self, format: u16, level: u8) -> Option<(Option<String>, Option<f32>)> {
        let list = (*self.formats.get(usize::from(format).checked_sub(1)?)?)?;
        let levels = self.lists.get(list)?;
        let ix = usize::from(level).min(levels.len().checked_sub(1)?);
        let current = levels.get(ix)?;
        let counters = self.counters.entry(list).or_insert([0; 9]);
        counters[ix] = if counters[ix] == 0 {
            current.start
        } else {
            counters[ix] + 1
        };
        for deeper in &mut counters[ix + 1..] {
            *deeper = 0;
        }
        let marker = match current.format {
            // No number at all.
            0xFF => None,
            23 => Some(bullet(&current.text, ix)),
            _ => {
                let mut out = String::new();
                for &unit in &current.text {
                    match unit {
                        0..=8 => {
                            let k = usize::from(unit);
                            let (start, format) =
                                levels.get(k).map_or((1, 0), |l| (l.start, l.format));
                            out.push_str(&number(counters[k].max(start), nfc(format)));
                        }
                        unit => out.extend(char::decode_utf16([unit]).flatten()),
                    }
                }
                Some(out)
            }
        };
        Some((marker, current.indent))
    }
}

/// An LVL at `at`, and its length.
fn level(table: &[u8], at: usize) -> Option<(Level, usize)> {
    let start = u32_at(table, at)?;
    let format = *table.get(at + 4)?;
    let chpx = usize::from(*table.get(at + 24)?);
    let papx = usize::from(*table.get(at + 25)?);
    let grpprl = table.get(at + 28..at + 28 + papx)?;
    let text_at = at + 28 + papx + chpx;
    let count = usize::from(u16_at(table, text_at)?);
    let text = (0..count)
        .map(|i| u16_at(table, text_at + 2 + 2 * i))
        .collect::<Option<Vec<u16>>>()?;
    let level = Level {
        start,
        format,
        text,
        indent: Para::default().with(grpprl).indent,
    };
    Some((level, 28 + papx + chpx + 2 + 2 * count))
}

/// The number format of a list level, as WordprocessingML names it.
fn nfc(code: u8) -> &'static str {
    match code {
        1 => "upperRoman",
        2 => "lowerRoman",
        3 => "upperLetter",
        4 => "lowerLetter",
        22 => "decimalZero",
        18 | 28 | 29 => "decimalEnclosedCircle",
        10 | 33 | 37 | 39 => "chineseCounting",
        _ => "decimal",
    }
}

/// A bullet as drawn. Bullets are mostly Symbol's and Wingdings' glyphs,
/// written as their codes in the private area or as plain letters: the
/// common ones become their Unicode look-alikes.
fn bullet(text: &[u16], level: usize) -> String {
    let glyph = |code: u32| match code {
        0xB7 => Some('•'),
        0xA7 => Some('▪'),
        0xD8 => Some('➢'),
        0xFC => Some('✓'),
        0x6C => Some('●'),
        0x6E => Some('■'),
        0x71 => Some('❑'),
        0x75 => Some('◆'),
        0x76 => Some('❖'),
        // Courier New's o, Word's second-level bullet.
        0x6F => Some('◦'),
        _ => None,
    };
    let shown: String = char::decode_utf16(text.iter().copied())
        .flatten()
        .filter_map(|c| match c as u32 {
            code @ 0xF000..=0xF0FF => glyph(code - 0xF000),
            code if c.is_ascii_alphabetic() || c == '§' || c == 'Ø' || c == 'ü' => {
                glyph(code).or(Some(c))
            }
            _ => Some(c),
        })
        .collect();
    if shown.trim().is_empty() {
        ["•", "◦", "▪"][level.min(2)].to_owned()
    } else {
        shown
    }
}

fn parse(word: &[u8], table: &[u8], fib: &Fib) -> Option<Document> {
    let pieces = pieces(fib.get(table, CLX)?)?;
    let styles = styles(fib.get(table, STSHF));
    let chpx = runs(word, fib.get(table, PLCF_BTE_CHPX), false);
    let papx = runs(word, fib.get(table, PLCF_BTE_PAPX), true);
    let (page, margins) = page(word, fib.get(table, PLCF_SED));
    let text = characters(word, &pieces, 0..fib.text);
    let style_of = |istd: usize| styles.get(istd).cloned().flatten().unwrap_or_default();
    let normal = style_of(0);
    let mut lists = Lists::read(table, fib);
    let mut builder = Builder::default();
    // Field codes are hidden and their results shown; fields nest, and
    // can run over paragraphs.
    let mut fields: Vec<bool> = Vec::new();
    let mut shown: Vec<(char, u32)> = Vec::new();
    for &(c, fc) in &text {
        match c {
            '\u{13}' => fields.push(false),
            '\u{14}' => {
                if let Some(result) = fields.last_mut() {
                    *result = true;
                }
            }
            '\u{15}' => {
                fields.pop();
            }
            '\r' | '\u{07}' | '\u{0C}' => {
                // The paragraph's properties are found by its mark.
                let grpprl = run_at(&papx, fc);
                let istd = u16_at(grpprl, 0).map_or(0, usize::from);
                let style = style_of(istd);
                let para = style.para.with(grpprl.get(2..).unwrap_or_default());
                for &(c, fc) in &shown {
                    builder.push(c, &style.chars.with(run_at(&chpx, fc), &style.chars));
                }
                shown.clear();
                let (marker, list_indent) = match para.list {
                    // 0xF801 keeps a list's indent without its number.
                    0 | 0xF801 => (style.bullet.then(|| "•".to_owned()), None),
                    list => lists.marker(list, para.level).unwrap_or_default(),
                };
                let paragraph = Paragraph {
                    role: match para.outline {
                        Some(level) if style.role == Role::Body => Role::Heading(level + 1),
                        _ => style.role,
                    },
                    align: para.align,
                    marker,
                    indent: para.indent.or(list_indent).unwrap_or(0.).max(0.),
                    spans: Vec::new(),
                };
                builder.end(paragraph, c, &para);
            }
            _ if fields.iter().all(|&result| result) => shown.push((c, fc)),
            _ => {}
        }
    }
    let mut blocks = builder.finish();
    // A document's text ends with a paragraph mark; an empty last paragraph adds nothing.
    while matches!(blocks.last(), Some(Block::Paragraph(p)) if p.spans.is_empty()) {
        blocks.pop();
    }
    Some(Document {
        page,
        margins,
        base_size: normal.chars.size.unwrap_or(10.),
        blocks,
    })
}

/// Paragraphs and tables as the marks end them.
#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    /// The paragraph being read.
    spans: Vec<Span>,
    text: String,
    chars: Chars,
    /// The cell, row and table being read.
    cell: Vec<Paragraph>,
    row: Vec<Cell>,
    table: Option<Table>,
}

impl Builder {
    /// A character of the paragraph, with its properties; the controls
    /// for pictures, notes and the like are dropped.
    fn push(&mut self, c: char, chars: &Chars) {
        let mut buffer = [0; 4];
        let shown: &str = match c {
            '\u{0B}' => "\n",
            '\t' => "    ",
            '\u{1E}' => "-",
            c if (c as u32) < 0x20 => return,
            c => c.encode_utf8(&mut buffer),
        };
        if &self.chars != chars {
            self.flush();
            self.chars = chars.clone();
        }
        self.text.push_str(shown);
    }

    fn flush(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.spans.push(self.chars.span(text));
        }
    }

    /// The paragraph that `mark` ends, in a table or not.
    fn end(&mut self, mut paragraph: Paragraph, mark: char, para: &Para) {
        self.flush();
        paragraph.spans.append(&mut self.spans);
        if !para.in_table {
            self.close_table();
            self.blocks.push(Block::Paragraph(paragraph));
            return;
        }
        let nested = para.depth > 1;
        match mark {
            '\u{07}' if para.row_end && !nested => {
                let table = self.table.get_or_insert_with(Table::default);
                if table.widths.is_empty() && para.edges.len() > 1 {
                    let total = f32::from(para.edges[para.edges.len() - 1] - para.edges[0]).max(1.);
                    table.widths = para
                        .edges
                        .windows(2)
                        .map(|e| f32::from(e[1] - e[0]).max(0.) / total)
                        .collect();
                }
                if !self.cell.is_empty() {
                    let paragraphs = std::mem::take(&mut self.cell);
                    self.row.push(Cell {
                        span: 1,
                        paragraphs,
                    });
                }
                table.rows.push(std::mem::take(&mut self.row));
            }
            '\u{07}' if !nested => {
                self.cell.push(paragraph);
                let paragraphs = std::mem::take(&mut self.cell);
                self.row.push(Cell {
                    span: 1,
                    paragraphs,
                });
            }
            // A paragraph in a cell, or a nested table's, run into its cell.
            _ => {
                if !(nested && para.row_end) {
                    self.cell.push(paragraph);
                }
            }
        }
    }

    fn close_table(&mut self) {
        if !self.row.is_empty() || !self.cell.is_empty() {
            let paragraphs = std::mem::take(&mut self.cell);
            if !paragraphs.is_empty() {
                self.row.push(Cell {
                    span: 1,
                    paragraphs,
                });
            }
            let row = std::mem::take(&mut self.row);
            self.table.get_or_insert_with(Table::default).rows.push(row);
        }
        if let Some(mut table) = self.table.take() {
            let columns = table.rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
            if table.widths.len() != columns {
                table.widths = vec![1. / columns as f32; columns];
            }
            self.blocks.push(Block::Table(table));
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.close_table();
        self.blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WordDocument stream and its table stream, written the way Word
    /// writes them: a FIB, the text in pieces, and paragraph properties in
    /// a formatted page.
    struct Doc {
        word: Vec<u8>,
        table: Vec<u8>,
    }

    /// FIB fields this reader looks at: the main text's length, and the
    /// places of the piece table, the paragraph and character bin tables.
    fn fib(text: u32, places: &[(usize, u32, u32)]) -> Vec<u8> {
        let mut fib = vec![0u8; 0x22];
        fib[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
        fib[2..4].copy_from_slice(&WORD97.to_le_bytes());
        fib[0x20..0x22].copy_from_slice(&0u16.to_le_bytes());
        // FibRgLw: 22 longs, ccpText fourth.
        fib.extend(22u16.to_le_bytes());
        for i in 0..22u32 {
            fib.extend(if i == 3 { text } else { 0 }.to_le_bytes());
        }
        fib.extend(93u16.to_le_bytes());
        let mut pairs = vec![(0u32, 0u32); 93];
        for &(index, at, len) in places {
            pairs[index] = (at, len);
        }
        for (at, len) in pairs {
            fib.extend(at.to_le_bytes());
            fib.extend(len.to_le_bytes());
        }
        fib
    }

    /// A paragraph page: each run ends at a byte and carries sprms.
    fn papx_page(start: u32, runs: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut page = vec![0u8; 512];
        let mut fc = start;
        page[0..4].copy_from_slice(&fc.to_le_bytes());
        let mut free = 510;
        for (r, (end, grpprl)) in runs.iter().enumerate() {
            fc = *end;
            page[4 * (r + 1)..4 * (r + 2)].copy_from_slice(&fc.to_le_bytes());
            // istd 0, then the sprms, padded to a whole number of words.
            let mut papx = vec![0u8, 0];
            papx.extend(grpprl);
            if papx.len() % 2 == 1 {
                papx.push(0);
            }
            let cb = papx.len() / 2;
            free -= 2 + papx.len();
            free -= free % 2;
            page[free] = 0;
            page[free + 1] = cb as u8;
            page[free + 2..free + 2 + papx.len()].copy_from_slice(&papx);
            page[4 * (runs.len() + 1) + 13 * r] = (free / 2) as u8;
        }
        page[511] = runs.len() as u8;
        page
    }

    fn doc(text: &str, compressed_head: usize, papx: &[(usize, Vec<u8>)]) -> Doc {
        // The first characters are one byte each, the rest UTF-16.
        let head: Vec<char> = text.chars().take(compressed_head).collect();
        let tail: Vec<char> = text.chars().skip(compressed_head).collect();
        let mut word = fib(text.chars().count() as u32, &[]);
        word.resize(1024, 0);
        let head_at = word.len() as u32;
        word.extend(head.iter().map(|&c| c as u8));
        let tail_at = word.len() as u32;
        for c in &tail {
            word.extend((*c as u16).to_le_bytes());
        }
        // Paragraph runs end just past each mark's byte.
        let byte_of = |i: usize| {
            if i < head.len() {
                head_at + i as u32
            } else {
                tail_at + 2 * (i - head.len()) as u32
            }
        };
        let runs: Vec<(u32, Vec<u8>)> = papx
            .iter()
            .map(|(mark, grpprl)| {
                (
                    byte_of(*mark) + if *mark < head.len() { 1 } else { 2 },
                    grpprl.clone(),
                )
            })
            .collect();
        while !word.len().is_multiple_of(512) {
            word.push(0);
        }
        let page = (word.len() / 512) as u32;
        word.extend(papx_page(head_at, &runs));
        let mut table = Vec::new();
        // Clx: one piece table of two pieces.
        let mut plc = Vec::new();
        let n = head.len() as u32;
        for cp in [0, n, text.chars().count() as u32] {
            plc.extend(cp.to_le_bytes());
        }
        plc.extend([0, 0]);
        plc.extend(((head_at * 2) | 0x4000_0000).to_le_bytes());
        plc.extend([0, 0]);
        plc.extend([0, 0]);
        plc.extend(tail_at.to_le_bytes());
        plc.extend([0, 0]);
        let clx_at = table.len() as u32;
        table.push(2);
        table.extend((plc.len() as u32).to_le_bytes());
        table.extend(&plc);
        let clx_len = table.len() as u32 - clx_at;
        let bte_at = table.len() as u32;
        table.extend(head_at.to_le_bytes());
        table.extend((tail_at + 2 * tail.len() as u32).to_le_bytes());
        table.extend(page.to_le_bytes());
        let bte_len = table.len() as u32 - bte_at;
        let fib = fib(
            text.chars().count() as u32,
            &[(CLX, clx_at, clx_len), (PLCF_BTE_PAPX, bte_at, bte_len)],
        );
        word[..fib.len()].copy_from_slice(&fib);
        Doc { word, table }
    }

    fn read(doc: &Doc) -> Document {
        let fib = Fib::read(&doc.word).unwrap();
        parse(&doc.word, &doc.table, &fib).unwrap()
    }

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph(p) => p.text(),
                Block::Table(t) => t
                    .rows
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|c| {
                                c.paragraphs
                                    .iter()
                                    .map(Paragraph::text)
                                    .collect::<Vec<_>>()
                                    .join("/")
                            })
                            .collect::<Vec<_>>()
                            .join("|")
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Block::Image(_) => "picture".into(),
            })
            .collect()
    }

    #[test]
    fn text_comes_from_one_byte_and_two_byte_pieces() {
        let text = "Café\rNaïve 床前明月光\r";
        let doc = doc(text, 5, &[]);
        let read = read(&doc);
        assert_eq!(texts(&read.blocks), ["Café", "Naïve 床前明月光"]);
    }

    #[test]
    fn fields_show_their_results_and_controls_are_dropped() {
        let text = "See \u{13}HYPERLINK \"x\"\u{14}the site\u{15} now\u{0B}next\u{07}\r";
        let doc = doc(text, 0, &[]);
        let read = read(&doc);
        assert_eq!(texts(&read.blocks)[0], "See the site now\nnext");
    }

    #[test]
    fn cell_and_row_marks_make_a_table() {
        // Two rows of two cells; the second row's first cell has two paragraphs.
        let text = "Before\ra\u{07}b\u{07}\u{07}c1\rc2\u{07}d\u{07}\u{07}After\r";
        let in_table = vec![0x16, 0x24, 1];
        let row_end = vec![0x16, 0x24, 1, 0x17, 0x24, 1];
        let marks: Vec<usize> = text
            .char_indices()
            .filter(|(_, c)| matches!(c, '\r' | '\u{07}'))
            .map(|(i, _)| text[..i].chars().count())
            .collect();
        let papx: Vec<(usize, Vec<u8>)> = marks
            .iter()
            .enumerate()
            .map(|(k, &m)| {
                let props = match k {
                    0 | 8 => vec![],
                    3 | 7 => row_end.clone(),
                    _ => in_table.clone(),
                };
                (m, props)
            })
            .collect();
        let doc = doc(text, text.chars().count(), &papx);
        let read = read(&doc);
        assert_eq!(texts(&read.blocks), ["Before", "a|b\nc1/c2|d", "After"]);
    }

    #[test]
    fn sprms_are_read_by_their_operand_sizes() {
        // Bold on, a size of 14 points, a colour, and a table definition.
        let grpprl = [
            0x35, 0x08, 0x01, 0x43, 0x4A, 28, 0, 0x70, 0x68, 0x10, 0x20, 0x30, 0x00, 0x08, 0xD6,
            0x06, 0x00, 1, 0, 0, 0x10, 0x27,
        ];
        let read: Vec<u16> = sprms(&grpprl).map(|(s, _)| s).collect();
        assert_eq!(read, [0x0835, 0x4A43, 0x6870, 0xD608]);
        let chars = Chars::default().with(&grpprl, &Chars::default());
        assert_eq!(chars.bold, Some(true));
        assert_eq!(chars.size, Some(14.));
        assert_eq!(chars.color, Some(0x102030));
        let para = Para::default().with(&grpprl);
        assert_eq!(para.edges, [0, 10000]);
    }

    #[test]
    fn lists_count_by_level_and_write_their_numbers() {
        let level = |format: u8, text: &str| Level {
            start: 1,
            format,
            text: text.encode_utf16().collect(),
            indent: None,
        };
        let mut lists = Lists {
            lists: vec![vec![
                level(0, "\u{0}."),
                level(4, "\u{0}.\u{1})"),
                level(23, "\u{F0B7}"),
            ]],
            formats: vec![Some(0)],
            counters: HashMap::new(),
        };
        let mut next = |level: u8| lists.marker(1, level).unwrap().0.unwrap();
        assert_eq!(next(0), "1.");
        assert_eq!(next(1), "1.a)");
        assert_eq!(next(1), "1.b)");
        // Symbol's bullet, in the private area.
        assert_eq!(next(2), "•");
        assert_eq!(bullet(&[0x71], 0), "❑");
        assert_eq!(bullet(&[0xF0B7], 0), "•");
        assert_eq!(bullet(&[0x2013], 0), "–");
        assert_eq!(next(0), "2.");
        assert_eq!(next(1), "2.a)");
        assert_eq!(nfc(37), "chineseCounting");
        assert_eq!(number(12, nfc(37)), "十二");
    }

    #[test]
    fn toggles_count_against_the_style() {
        let bold_style = Chars {
            bold: Some(true),
            ..Chars::default()
        };
        let flip = [0x35, 0x08, 0x81];
        assert_eq!(bold_style.with(&flip, &bold_style).bold, Some(false));
        assert_eq!(
            Chars::default().with(&flip, &Chars::default()).bold,
            Some(true)
        );
    }
}
