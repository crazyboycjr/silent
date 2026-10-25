//! Word documents: word/document.xml, with the styles, the list numbering
//! and the pictures it refers to.

use std::collections::HashMap;
use std::path::Path;

use roxmltree::Node;

use super::{
    Align, Block, Cell, Document, EMU_PER_POINT, Image, Package, Paragraph, Role, Span,
    TWIPS_PER_POINT, Table, attr, child, children, on, path, rel_attr, rgb,
};

/// Run properties, each unset until a style or the run sets it.
#[derive(Clone, Debug, Default)]
struct Props {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    size: Option<f32>,
    color: Option<u32>,
    font: Option<String>,
}

impl Props {
    fn read(rpr: Option<Node>) -> Self {
        let Some(rpr) = rpr else {
            return Self::default();
        };
        let underline = child(rpr, "u").map(|u| !matches!(attr(u, "val"), Some("none") | None));
        let fonts = child(rpr, "rFonts");
        Self {
            bold: on(child(rpr, "b")),
            italic: on(child(rpr, "i")),
            underline,
            strike: on(child(rpr, "strike")).or(on(child(rpr, "dstrike"))),
            size: child(rpr, "sz")
                .and_then(|n| attr(n, "val")?.parse::<f32>().ok())
                .map(|half| half / 2.),
            color: child(rpr, "color").and_then(|n| rgb(attr(n, "val")?)),
            font: fonts
                .and_then(|f| {
                    attr(f, "ascii")
                        .or(attr(f, "hAnsi"))
                        .or(attr(f, "eastAsia"))
                })
                .map(str::to_owned),
        }
    }

    /// `self` with what `over` sets laid over it.
    fn under(&self, over: &Props) -> Props {
        Props {
            bold: over.bold.or(self.bold),
            italic: over.italic.or(self.italic),
            underline: over.underline.or(self.underline),
            strike: over.strike.or(self.strike),
            size: over.size.or(self.size),
            color: over.color.or(self.color),
            font: over.font.clone().or_else(|| self.font.clone()),
        }
    }

    fn span(&self, text: String) -> Span {
        Span {
            text,
            bold: self.bold.unwrap_or(false),
            italic: self.italic.unwrap_or(false),
            underline: self.underline.unwrap_or(false),
            strike: self.strike.unwrap_or(false),
            size: self.size,
            color: self.color,
            font: self.font.clone(),
        }
    }
}

#[derive(Debug, Default)]
struct Style {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    numbering: Option<(String, u8)>,
    align: Option<Align>,
    indent: Option<f32>,
    props: Props,
}

struct Level {
    format: String,
    text: String,
    start: u32,
    indent: Option<f32>,
}

struct Reader<'p> {
    package: &'p mut Package,
    rels: HashMap<String, String>,
    styles: HashMap<String, Style>,
    default_style: Option<String>,
    defaults: Props,
    /// numId to the abstract numbering's levels.
    lists: HashMap<String, std::rc::Rc<Vec<Level>>>,
    counters: HashMap<String, [u32; 9]>,
}

pub fn read(file: &Path) -> Option<Document> {
    let mut package = Package::open(file)?;
    let xml = package.text("word/document.xml")?;
    let doc = roxmltree::Document::parse(&xml).ok()?;
    let body = child(doc.root_element(), "body")?;
    let rels = package.rels("word/document.xml");
    let mut reader = Reader {
        rels,
        styles: HashMap::new(),
        default_style: None,
        defaults: Props::default(),
        lists: HashMap::new(),
        counters: HashMap::new(),
        package: &mut package,
    };
    reader.read_styles();
    reader.read_numbering();
    let mut document = Document {
        page: (612., 792.),
        margins: (72., 72.),
        base_size: reader.defaults.size.unwrap_or(10.),
        blocks: Vec::new(),
    };
    if let Some(section) = child(body, "sectPr") {
        let twips = |n: Option<Node>, name| -> Option<f32> {
            Some(attr(n?, name)?.parse::<f32>().ok()? / TWIPS_PER_POINT)
        };
        let size = child(section, "pgSz");
        let margins = child(section, "pgMar");
        if let (Some(w), Some(h)) = (twips(size, "w"), twips(size, "h")) {
            document.page = (w, h);
        }
        if let (Some(l), Some(r)) = (twips(margins, "left"), twips(margins, "right")) {
            document.margins = (l, r);
        }
    }
    reader.blocks(body, &mut document.blocks);
    Some(document)
}

impl Reader<'_> {
    fn read_styles(&mut self) {
        let Some(xml) = self.package.text("word/styles.xml") else {
            return;
        };
        let Ok(doc) = roxmltree::Document::parse(&xml) else {
            return;
        };
        let root = doc.root_element();
        self.defaults = Props::read(path(root, &["docDefaults", "rPrDefault", "rPr"]));
        for node in children(root, "style") {
            let Some(id) = attr(node, "styleId") else {
                continue;
            };
            let ppr = child(node, "pPr");
            let numbering = ppr.and_then(|p| child(p, "numPr")).and_then(|n| {
                let id = attr(child(n, "numId")?, "val")?.to_owned();
                let level = child(n, "ilvl")
                    .and_then(|l| attr(l, "val")?.parse().ok())
                    .unwrap_or(0);
                Some((id, level))
            });
            let style = Style {
                name: child(node, "name")
                    .and_then(|n| attr(n, "val"))
                    .unwrap_or_default()
                    .to_lowercase(),
                based_on: child(node, "basedOn")
                    .and_then(|n| attr(n, "val"))
                    .map(str::to_owned),
                outline: ppr
                    .and_then(|p| child(p, "outlineLvl"))
                    .and_then(|n| attr(n, "val")?.parse().ok()),
                numbering,
                align: ppr.and_then(|p| child(p, "jc")).map(alignment),
                indent: indent(ppr),
                props: Props::read(child(node, "rPr")),
            };
            if attr(node, "type") == Some("paragraph")
                && matches!(attr(node, "default"), Some("1" | "true"))
            {
                self.default_style = Some(id.to_owned());
            }
            self.styles.insert(id.to_owned(), style);
        }
    }

    fn read_numbering(&mut self) {
        let Some(xml) = self.package.text("word/numbering.xml") else {
            return;
        };
        let Ok(doc) = roxmltree::Document::parse(&xml) else {
            return;
        };
        let root = doc.root_element();
        let mut abstracts = HashMap::new();
        for node in children(root, "abstractNum") {
            let Some(id) = attr(node, "abstractNumId") else {
                continue;
            };
            let mut levels: Vec<Level> = Vec::new();
            for lvl in children(node, "lvl") {
                let value = |name| child(lvl, name).and_then(|n| attr(n, "val"));
                levels.push(Level {
                    format: value("numFmt").unwrap_or("decimal").to_owned(),
                    text: value("lvlText").unwrap_or("").to_owned(),
                    start: value("start").and_then(|s| s.parse().ok()).unwrap_or(1),
                    indent: indent(child(lvl, "pPr")),
                });
            }
            abstracts.insert(id.to_owned(), std::rc::Rc::new(levels));
        }
        for node in children(root, "num") {
            let abstract_id = child(node, "abstractNumId").and_then(|n| attr(n, "val"));
            if let (Some(id), Some(levels)) = (
                attr(node, "numId"),
                abstract_id.and_then(|a| abstracts.get(a)),
            ) {
                self.lists.insert(id.to_owned(), levels.clone());
            }
        }
    }

    /// A style and those it is based on, nearest first.
    fn chain(&self, id: Option<&str>) -> Vec<&Style> {
        let mut chain = Vec::new();
        let mut next = id;
        while let Some(style) = next.and_then(|id| self.styles.get(id)) {
            if chain.len() > 16 {
                break;
            }
            chain.push(style);
            next = style.based_on.as_deref();
        }
        chain
    }

    fn props(&self, chain: &[&Style]) -> Props {
        chain
            .iter()
            .rev()
            .fold(self.defaults.clone(), |props, style| {
                props.under(&style.props)
            })
    }

    fn blocks(&mut self, parent: Node, out: &mut Vec<Block>) {
        for node in parent.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "p" => self.paragraph(node, out),
                "tbl" => out.push(Block::Table(self.table(node))),
                "sdt" => {
                    if let Some(content) = child(node, "sdtContent") {
                        self.blocks(content, out);
                    }
                }
                "customXml" | "ins" | "smartTag" => self.blocks(node, out),
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, p: Node, out: &mut Vec<Block>) {
        let ppr = child(p, "pPr");
        let style_id = ppr
            .and_then(|n| child(n, "pStyle"))
            .and_then(|n| attr(n, "val"))
            .map(str::to_owned)
            .or_else(|| self.default_style.clone());
        let chain = self.chain(style_id.as_deref());
        let outline = ppr
            .and_then(|n| child(n, "outlineLvl"))
            .and_then(|n| attr(n, "val")?.parse::<u8>().ok())
            .or_else(|| chain.iter().find_map(|s| s.outline));
        let role = role(&chain, outline);
        let align = ppr
            .and_then(|n| child(n, "jc"))
            .map(alignment)
            .or_else(|| chain.iter().find_map(|s| s.align))
            .unwrap_or_default();
        let numbering = ppr
            .and_then(|n| child(n, "numPr"))
            .and_then(|n| {
                let id = attr(child(n, "numId")?, "val")?.to_owned();
                let level = child(n, "ilvl")
                    .and_then(|l| attr(l, "val")?.parse().ok())
                    .unwrap_or(0);
                Some((id, level))
            })
            .or_else(|| chain.iter().find_map(|s| s.numbering.clone()));
        let base = self.props(&chain);
        let own_indent = indent(ppr).or_else(|| chain.iter().find_map(|s| s.indent));
        let (marker, list_indent) = match numbering {
            Some((id, level)) if id != "0" => {
                let list_indent = self
                    .lists
                    .get(&id)
                    .and_then(|levels| levels.get(usize::from(level.min(8)))?.indent);
                (
                    self.marker(&id, level),
                    list_indent.or(Some(18. * f32::from(level + 1))),
                )
            }
            _ => (None, None),
        };
        let mut paragraph = Paragraph {
            role,
            align,
            marker,
            indent: own_indent.or(list_indent).unwrap_or(0.).max(0.),
            spans: Vec::new(),
        };
        let mut images = Vec::new();
        self.inline(p, &base, &mut paragraph.spans, &mut images);
        if !(paragraph.spans.is_empty() && paragraph.marker.is_none() && !images.is_empty()) {
            out.push(Block::Paragraph(paragraph));
        }
        out.extend(images.into_iter().map(Block::Image));
    }

    fn inline(
        &mut self,
        parent: Node,
        base: &Props,
        spans: &mut Vec<Span>,
        images: &mut Vec<Image>,
    ) {
        for node in parent.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "r" => self.run(node, base, spans, images),
                "hyperlink" | "ins" | "smartTag" | "fldSimple" | "customXml" | "bdo" | "dir" => {
                    self.inline(node, base, spans, images);
                }
                "sdt" => {
                    if let Some(content) = child(node, "sdtContent") {
                        self.inline(content, base, spans, images);
                    }
                }
                _ => {}
            }
        }
    }

    fn run(&mut self, r: Node, base: &Props, spans: &mut Vec<Span>, images: &mut Vec<Image>) {
        let rpr = child(r, "rPr");
        let style = rpr
            .and_then(|n| child(n, "rStyle"))
            .and_then(|n| attr(n, "val"));
        let chain = self.chain(style);
        let props = chain
            .iter()
            .rev()
            .fold(base.clone(), |props, s| props.under(&s.props))
            .under(&Props::read(rpr));
        let mut text = String::new();
        for node in r.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "t" => text.push_str(node.text().unwrap_or_default()),
                "tab" | "ptab" => text.push_str("    "),
                "br" | "cr" if attr(node, "type") != Some("page") => text.push('\n'),
                "noBreakHyphen" => text.push('-'),
                "drawing" => self.drawing(node, images),
                "AlternateContent" => {
                    let choice = child(node, "Choice");
                    for drawing in choice
                        .into_iter()
                        .flat_map(|c| c.descendants())
                        .filter(|n| n.tag_name().name() == "drawing")
                    {
                        self.drawing(drawing, images);
                    }
                }
                _ => {}
            }
        }
        if text.is_empty() {
            return;
        }
        let span = props.span(text);
        match spans.last_mut() {
            Some(last)
                if Span {
                    text: last.text.clone(),
                    ..span.clone()
                } == *last =>
            {
                last.text.push_str(&span.text);
            }
            _ => spans.push(span),
        }
    }

    fn drawing(&mut self, drawing: Node, images: &mut Vec<Image>) {
        let Some(holder) = drawing
            .children()
            .find(|n| matches!(n.tag_name().name(), "inline" | "anchor"))
        else {
            return;
        };
        let extent = child(holder, "extent").and_then(|e| {
            let cx: f32 = attr(e, "cx")?.parse().ok()?;
            let cy: f32 = attr(e, "cy")?.parse().ok()?;
            Some((cx / EMU_PER_POINT, cy / EMU_PER_POINT))
        });
        let Some(blip) = holder.descendants().find(|n| n.tag_name().name() == "blip") else {
            return;
        };
        let target = rel_attr(blip, "embed")
            .and_then(|id| self.rels.get(id))
            .cloned();
        if let Some(data) = target.and_then(|t| self.package.bytes(&t)) {
            images.push(Image {
                data,
                size: extent.unwrap_or((300., 200.)),
            });
        }
    }

    fn table(&mut self, tbl: Node) -> Table {
        let grid: Vec<f32> = path(tbl, &["tblGrid"])
            .into_iter()
            .flat_map(|g| children(g, "gridCol"))
            .map(|c| attr(c, "w").and_then(|w| w.parse().ok()).unwrap_or(1.0))
            .collect();
        let total: f32 = grid.iter().sum::<f32>().max(1.);
        let mut table = Table {
            widths: grid.iter().map(|w| w / total).collect(),
            rows: Vec::new(),
        };
        for tr in children(tbl, "tr") {
            let mut row = Vec::new();
            for tc in children(tr, "tc") {
                let tcpr = child(tc, "tcPr");
                let span = tcpr
                    .and_then(|p| child(p, "gridSpan"))
                    .and_then(|n| attr(n, "val")?.parse().ok())
                    .unwrap_or(1);
                // A cell merged into the one above is drawn empty.
                let merged = tcpr
                    .and_then(|p| child(p, "vMerge"))
                    .is_some_and(|m| attr(m, "val") != Some("restart"));
                let mut blocks = Vec::new();
                if !merged {
                    self.blocks(tc, &mut blocks);
                }
                row.push(Cell {
                    span,
                    paragraphs: flatten(blocks),
                });
            }
            table.rows.push(row);
        }
        if table.widths.is_empty() {
            let columns = table.rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
            table.widths = vec![1. / columns as f32; columns];
        }
        table
    }

    /// A list paragraph's bullet or number, counting as Word does.
    fn marker(&mut self, id: &str, level: u8) -> Option<String> {
        let levels = self.lists.get(id)?.clone();
        let ix = usize::from(level.min(8));
        let lvl = levels.get(ix)?;
        let counters = self.counters.entry(id.to_owned()).or_insert([0; 9]);
        counters[ix] = if counters[ix] == 0 {
            lvl.start
        } else {
            counters[ix] + 1
        };
        for deeper in &mut counters[ix + 1..] {
            *deeper = 0;
        }
        match lvl.format.as_str() {
            "bullet" => return Some(["•", "◦", "▪"][ix.min(2)].to_owned()),
            "none" => return None,
            _ => {}
        }
        let mut text = lvl.text.clone();
        for k in (0..=ix).rev() {
            let format = levels.get(k).map_or("decimal", |l| l.format.as_str());
            let n = counters[k].max(levels.get(k).map_or(1, |l| l.start));
            text = text.replace(&format!("%{}", k + 1), &number(n, format));
        }
        Some(text)
    }
}

/// Paragraphs of a table cell; nested tables give up their cells' paragraphs.
fn flatten(blocks: Vec<Block>) -> Vec<Paragraph> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(p) => out.push(p),
            Block::Table(t) => out.extend(t.rows.into_iter().flatten().flat_map(|c| c.paragraphs)),
            Block::Image(_) => {}
        }
    }
    out
}

fn role(chain: &[&Style], outline: Option<u8>) -> Role {
    for style in chain {
        match style.name.as_str() {
            "title" => return Role::Title,
            "subtitle" => return Role::Subtitle,
            name => {
                if let Some(n) = name.strip_prefix("heading ").and_then(|n| n.parse().ok()) {
                    return Role::Heading(n);
                }
            }
        }
    }
    match outline {
        Some(level) if level < 9 => Role::Heading(level + 1),
        _ => Role::Body,
    }
}

/// A paragraph's left indent in points, from its `w:ind`.
fn indent(ppr: Option<Node>) -> Option<f32> {
    let ind = child(ppr?, "ind")?;
    let twips: f32 = attr(ind, "left").or(attr(ind, "start"))?.parse().ok()?;
    Some(twips / TWIPS_PER_POINT)
}

fn alignment(jc: Node) -> Align {
    match attr(jc, "val") {
        Some("center") => Align::Center,
        Some("right" | "end") => Align::End,
        Some("both" | "distribute") => Align::Justify,
        _ => Align::Start,
    }
}

/// `n` written the way a list level's format asks.
pub fn number(n: u32, format: &str) -> String {
    let letters = |n: u32, base: u8| {
        let letter = (base + ((n - 1) % 26) as u8) as char;
        letter.to_string().repeat(((n - 1) / 26 + 1) as usize)
    };
    match format {
        "lowerLetter" if n > 0 => letters(n, b'a'),
        "upperLetter" if n > 0 => letters(n, b'A'),
        "lowerRoman" => roman(n).to_lowercase(),
        "upperRoman" => roman(n),
        "decimalZero" => format!("{n:02}"),
        "decimalEnclosedCircle" | "decimalEnclosedCircleChinese" if (1..=20).contains(&n) => {
            char::from_u32(0x2460 + n - 1).map_or(n.to_string(), String::from)
        }
        "chineseCounting"
        | "chineseCountingThousand"
        | "ideographTraditional"
        | "japaneseCounting"
        | "taiwaneseCounting"
            if (1..=99).contains(&n) =>
        {
            chinese(n)
        }
        _ => n.to_string(),
    }
}

fn roman(mut n: u32) -> String {
    const NUMERALS: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

fn chinese(n: u32) -> String {
    const DIGITS: [&str; 10] = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    match n {
        0..=9 => DIGITS[n as usize].to_owned(),
        10 => "十".to_owned(),
        11..=19 => format!("十{}", DIGITS[(n % 10) as usize]),
        _ => format!(
            "{}十{}",
            DIGITS[(n / 10) as usize],
            if n.is_multiple_of(10) {
                ""
            } else {
                DIGITS[(n % 10) as usize]
            }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_numbers_read_as_their_format_asks() {
        assert_eq!(number(3, "decimal"), "3");
        assert_eq!(number(2, "lowerLetter"), "b");
        assert_eq!(number(27, "upperLetter"), "AA");
        assert_eq!(number(14, "lowerRoman"), "xiv");
        assert_eq!(number(3, "decimalEnclosedCircle"), "③");
        assert_eq!(number(12, "chineseCounting"), "十二");
        assert_eq!(number(30, "chineseCounting"), "三十");
    }
}
