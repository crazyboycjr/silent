//! PowerPoint decks: each slide's shapes, pictures and tables where the deck
//! puts them. Placeholders take their place, text sizes, bullets and colours
//! from the slide's layout and master, as PowerPoint draws them.

use std::collections::HashMap;
use std::path::Path;

use roxmltree::{Document as Xml, Node};

use super::{Align, EMU_PER_POINT, Package, Span, attr, child, children, path, rel_attr, rgb};

pub struct Deck {
    /// Slide size in points.
    pub size: (f32, f32),
    /// Each slide's part, in order.
    pub slides: Vec<String>,
}

/// A rectangle in points from the slide's top left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Default)]
pub enum Fill {
    #[default]
    None,
    Color(u32),
    Image(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct Slide {
    pub background: Fill,
    pub items: Vec<Item>,
}

#[derive(Debug)]
pub struct Item {
    pub rect: Rect,
    pub kind: Kind,
}

#[derive(Debug)]
pub enum Kind {
    Shape {
        fill: Option<u32>,
        line: Option<u32>,
        ellipse: bool,
        text: Option<Text>,
    },
    Picture(Vec<u8>),
    Table {
        /// Column widths and row heights in points.
        columns: Vec<f32>,
        rows: Vec<(f32, Vec<Text>)>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Default)]
pub struct Text {
    pub paragraphs: Vec<Paragraph>,
    pub anchor: Anchor,
    /// Left, top, right and bottom insets in points.
    pub insets: [f32; 4],
}

#[derive(Debug, Default)]
pub struct Paragraph {
    pub align: Align,
    pub bullet: Option<String>,
    pub level: u8,
    /// Points, for the line height when the paragraph is empty.
    pub size: f32,
    pub spans: Vec<Span>,
}

pub fn open(file: &Path) -> Option<Deck> {
    let mut package = Package::open(file)?;
    let xml = package.text("ppt/presentation.xml")?;
    let doc = Xml::parse(&xml).ok()?;
    let root = doc.root_element();
    let size = child(root, "sldSz")
        .and_then(|s| {
            let cx: f32 = attr(s, "cx")?.parse().ok()?;
            let cy: f32 = attr(s, "cy")?.parse().ok()?;
            Some((cx / EMU_PER_POINT, cy / EMU_PER_POINT))
        })
        .unwrap_or((720., 540.));
    let rels = package.rels("ppt/presentation.xml");
    let slides = child(root, "sldIdLst")
        .into_iter()
        .flat_map(|list| children(list, "sldId"))
        .filter_map(|id| rels.get(rel_attr(id, "id")?).cloned())
        .collect();
    Some(Deck { size, slides })
}

/// The parts a slide draws on: its own, its layout's and its master's.
struct Parts<'a, 'i> {
    slide: Node<'a, 'i>,
    layout: Option<Node<'a, 'i>>,
    master: Option<Node<'a, 'i>>,
    colors: HashMap<String, u32>,
    fonts: (String, String),
    /// The slide's number, for its slide-number field.
    number: usize,
}

/// Reads slide `index` of the deck at `file`.
pub fn slide(file: &Path, deck: &Deck, index: usize) -> Option<Slide> {
    let mut package = Package::open(file)?;
    let part = deck.slides.get(index)?.clone();
    let slide_rels = package.rels(&part);
    let layout_part = slide_rels
        .values()
        .find(|t| t.contains("slideLayouts/"))
        .cloned();
    let layout_rels = layout_part
        .as_deref()
        .map(|p| package.rels(p))
        .unwrap_or_default();
    let master_part = layout_rels
        .values()
        .find(|t| t.contains("slideMasters/"))
        .cloned();
    let master_rels = master_part
        .as_deref()
        .map(|p| package.rels(p))
        .unwrap_or_default();
    let theme_part = master_rels.values().find(|t| t.contains("theme/")).cloned();

    let slide_xml = package.text(&part)?;
    let layout_xml = layout_part.as_deref().and_then(|p| package.text(p));
    let master_xml = master_part.as_deref().and_then(|p| package.text(p));
    let theme_xml = theme_part.as_deref().and_then(|p| package.text(p));
    let slide_doc = Xml::parse(&slide_xml).ok()?;
    let layout_doc = layout_xml.as_deref().and_then(|x| Xml::parse(x).ok());
    let master_doc = master_xml.as_deref().and_then(|x| Xml::parse(x).ok());
    let theme_doc = theme_xml.as_deref().and_then(|x| Xml::parse(x).ok());

    let master = master_doc.as_ref().map(|d| d.root_element());
    let (colors, fonts) = theme(theme_doc.as_ref().map(|d| d.root_element()), master);
    let parts = Parts {
        slide: slide_doc.root_element(),
        layout: layout_doc.as_ref().map(|d| d.root_element()),
        master,
        colors,
        fonts,
        number: index + 1,
    };

    let mut slide = Slide {
        background: Fill::None,
        items: Vec::new(),
    };
    // The background comes from the first of slide, layout and master that has one.
    let backgrounds = [
        (Some(parts.slide), &slide_rels),
        (parts.layout, &layout_rels),
        (parts.master, &master_rels),
    ];
    for (node, rels) in backgrounds {
        let Some(bg) = node.and_then(|n| path(n, &["cSld", "bg"])) else {
            continue;
        };
        slide.background = background(bg, rels, &parts, &mut package);
        if !matches!(slide.background, Fill::None) {
            break;
        }
    }
    if matches!(slide.background, Fill::None) {
        slide.background = Fill::Color(parts.colors.get("bg1").copied().unwrap_or(0xffffff));
    }
    // Master and layout pictures and shapes that are not placeholders show
    // behind the slide's own, unless the slide hides them.
    let show_master = attr(parts.slide, "showMasterSp") != Some("0");
    if show_master {
        let layout_shows_master = parts
            .layout
            .is_none_or(|l| attr(l, "showMasterSp") != Some("0"));
        if layout_shows_master
            && let Some(tree) = parts.master.and_then(|m| path(m, &["cSld", "spTree"]))
        {
            shapes(
                tree,
                Affine::IDENTITY,
                &parts,
                &master_rels,
                &mut package,
                &mut slide.items,
                false,
            );
        }
        if let Some(tree) = parts.layout.and_then(|l| path(l, &["cSld", "spTree"])) {
            shapes(
                tree,
                Affine::IDENTITY,
                &parts,
                &layout_rels,
                &mut package,
                &mut slide.items,
                false,
            );
        }
    }
    let tree = path(parts.slide, &["cSld", "spTree"])?;
    shapes(
        tree,
        Affine::IDENTITY,
        &parts,
        &slide_rels,
        &mut package,
        &mut slide.items,
        true,
    );
    Some(slide)
}

/// Group transforms: child coordinates to the slide's.
#[derive(Clone, Copy)]
struct Affine {
    scale: (f32, f32),
    offset: (f32, f32),
}

impl Affine {
    const IDENTITY: Affine = Affine {
        scale: (1., 1.),
        offset: (0., 0.),
    };

    fn apply(&self, r: Rect) -> Rect {
        Rect {
            x: r.x * self.scale.0 + self.offset.0,
            y: r.y * self.scale.1 + self.offset.1,
            w: r.w * self.scale.0,
            h: r.h * self.scale.1,
        }
    }
}

fn xfrm_rect(xfrm: Node) -> Option<Rect> {
    let off = child(xfrm, "off")?;
    let ext = child(xfrm, "ext")?;
    let number = |n: Node, name| -> Option<f32> {
        Some(attr(n, name)?.parse::<f32>().ok()? / EMU_PER_POINT)
    };
    Some(Rect {
        x: number(off, "x")?,
        y: number(off, "y")?,
        w: number(ext, "cx")?,
        h: number(ext, "cy")?,
    })
}

/// The placeholder a shape stands for: its type and index.
fn placeholder(shape: Node) -> Option<(String, Option<String>)> {
    let ph = shape
        .children()
        .find(|n| n.tag_name().name().starts_with("nv"))
        .and_then(|nv| path(nv, &["nvPr", "ph"]))?;
    Some((
        attr(ph, "type").unwrap_or("body").to_owned(),
        attr(ph, "idx").map(str::to_owned),
    ))
}

/// The shape on a layout or master that a placeholder of `kind` and `idx` inherits from.
fn inherited<'a, 'i>(
    part: Option<Node<'a, 'i>>,
    kind: &str,
    idx: Option<&str>,
) -> Option<Node<'a, 'i>> {
    let tree = path(part?, &["cSld", "spTree"])?;
    fn family(t: &str) -> &str {
        match t {
            "ctrTitle" | "title" => "title",
            "subTitle" | "body" | "obj" => "body",
            other => other,
        }
    }
    let shapes: Vec<Node> = tree
        .descendants()
        .filter(|n| n.tag_name().name() == "sp")
        .collect();
    let by_idx = idx.and_then(|idx| {
        shapes
            .iter()
            .find(|s| placeholder(**s).is_some_and(|(_, i)| i.as_deref() == Some(idx)))
    });
    by_idx
        .or_else(|| {
            shapes
                .iter()
                .find(|s| placeholder(**s).is_some_and(|(t, _)| family(&t) == family(kind)))
        })
        .copied()
}

fn shapes(
    tree: Node,
    transform: Affine,
    parts: &Parts,
    rels: &HashMap<String, String>,
    package: &mut Package,
    out: &mut Vec<Item>,
    placeholders: bool,
) {
    for node in tree.children().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "sp" => {
                let ph = placeholder(node);
                // A layout's or master's placeholders only lend their place
                // and style; they are not drawn.
                if ph.is_some() && !placeholders {
                    continue;
                }
                if let Some(item) = shape(node, ph, transform, parts, rels, package) {
                    out.push(item);
                }
            }
            "pic" => {
                let rect = path(node, &["spPr", "xfrm"])
                    .and_then(xfrm_rect)
                    .or_else(|| {
                        let (kind, idx) = placeholder(node)?;
                        [parts.layout, parts.master].into_iter().find_map(|p| {
                            path(inherited(p, &kind, idx.as_deref())?, &["spPr", "xfrm"])
                                .and_then(xfrm_rect)
                        })
                    });
                let blip = node.descendants().find(|n| n.tag_name().name() == "blip");
                let target = blip.and_then(|b| rels.get(rel_attr(b, "embed")?));
                if let (Some(rect), Some(data)) = (rect, target.and_then(|t| package.bytes(t))) {
                    out.push(Item {
                        rect: transform.apply(rect),
                        kind: Kind::Picture(data),
                    });
                }
            }
            "grpSp" => {
                let Some(xfrm) = path(node, &["grpSpPr", "xfrm"]) else {
                    shapes(node, transform, parts, rels, package, out, placeholders);
                    continue;
                };
                let outer = xfrm_rect(xfrm);
                let inner = (|| {
                    let off = child(xfrm, "chOff")?;
                    let ext = child(xfrm, "chExt")?;
                    let n = |n: Node, a| -> Option<f32> {
                        Some(attr(n, a)?.parse::<f32>().ok()? / EMU_PER_POINT)
                    };
                    Some(Rect {
                        x: n(off, "x")?,
                        y: n(off, "y")?,
                        w: n(ext, "cx")?,
                        h: n(ext, "cy")?,
                    })
                })();
                let group = match (outer, inner) {
                    (Some(o), Some(i)) if i.w > 0. && i.h > 0. => {
                        let scale = (o.w / i.w, o.h / i.h);
                        let local = Affine {
                            scale,
                            offset: (o.x - i.x * scale.0, o.y - i.y * scale.1),
                        };
                        Affine {
                            scale: (
                                local.scale.0 * transform.scale.0,
                                local.scale.1 * transform.scale.1,
                            ),
                            offset: (
                                local.offset.0 * transform.scale.0 + transform.offset.0,
                                local.offset.1 * transform.scale.1 + transform.offset.1,
                            ),
                        }
                    }
                    _ => transform,
                };
                shapes(node, group, parts, rels, package, out, placeholders);
            }
            "graphicFrame" => {
                let rect = child(node, "xfrm").and_then(xfrm_rect);
                let table = node.descendants().find(|n| n.tag_name().name() == "tbl");
                if let (Some(rect), Some(table)) = (rect, table) {
                    out.push(Item {
                        rect: transform.apply(rect),
                        kind: self::table(table, parts),
                    });
                }
            }
            "AlternateContent" => {
                if let Some(choice) = child(node, "Choice") {
                    shapes(choice, transform, parts, rels, package, out, placeholders);
                }
            }
            _ => {}
        }
    }
}

fn shape(
    node: Node,
    ph: Option<(String, Option<String>)>,
    transform: Affine,
    parts: &Parts,
    _rels: &HashMap<String, String>,
    _package: &mut Package,
) -> Option<Item> {
    let (kind, idx) = ph.clone().unzip();
    let idx = idx.flatten();
    let layout = kind
        .as_deref()
        .and_then(|k| inherited(parts.layout, k, idx.as_deref()));
    let master = kind
        .as_deref()
        .and_then(|k| inherited(parts.master, k, idx.as_deref()));
    let rect = path(node, &["spPr", "xfrm"])
        .and_then(xfrm_rect)
        .or_else(|| {
            layout
                .and_then(|l| path(l, &["spPr", "xfrm"]))
                .and_then(xfrm_rect)
        })
        .or_else(|| {
            master
                .and_then(|m| path(m, &["spPr", "xfrm"]))
                .and_then(xfrm_rect)
        })?;
    let sppr = child(node, "spPr");
    let no_fill = sppr.and_then(|s| child(s, "noFill")).is_some();
    let text_box =
        path(node, &["nvSpPr", "cNvSpPr"]).is_some_and(|n| attr(n, "txBox") == Some("1"));
    let fill = if no_fill {
        None
    } else {
        sppr.and_then(|s| child(s, "solidFill"))
            .and_then(|f| color(f, parts))
            .or_else(|| {
                // Autoshapes take the theme's fill through their style.
                (!text_box && ph.is_none())
                    .then(|| path(node, &["style", "fillRef"]))
                    .flatten()
                    .and_then(|f| color(f, parts))
            })
    };
    let line = sppr
        .and_then(|s| child(s, "ln"))
        .filter(|ln| child(*ln, "noFill").is_none())
        .and_then(|ln| child(ln, "solidFill"))
        .and_then(|f| color(f, parts));
    let ellipse = sppr
        .and_then(|s| child(s, "prstGeom"))
        .is_some_and(|g| attr(g, "prst") == Some("ellipse"));
    let style_kind = match kind.as_deref() {
        Some("title" | "ctrTitle") => "titleStyle",
        Some(_) => "bodyStyle",
        None => "otherStyle",
    };
    let text = child(node, "txBody").map(|body| {
        let sources = [
            Some(body),
            layout.and_then(|l| child(l, "txBody")),
            master.and_then(|m| child(m, "txBody")),
        ];
        let style = parts
            .master
            .and_then(|m| path(m, &["txStyles", style_kind]));
        // An autoshape's text takes its colour from the shape's style.
        let ink = path(node, &["style", "fontRef"]).and_then(|f| color(f, parts));
        self::text(body, &sources, style, ink, parts)
    });
    Some(Item {
        rect: transform.apply(rect),
        kind: Kind::Shape {
            fill,
            line,
            ellipse,
            text: text.filter(|t| t.paragraphs.iter().any(|p| !p.spans.is_empty())),
        },
    })
}

/// Text with properties looked up from the shape's own list style, then its
/// layout's and master's placeholders, then the master's text style.
fn text(
    body: Node,
    sources: &[Option<Node>; 3],
    style: Option<Node>,
    ink: Option<u32>,
    parts: &Parts,
) -> Text {
    let body_pr = |name: &str| {
        sources
            .iter()
            .flatten()
            .filter_map(|s| child(*s, "bodyPr"))
            .find_map(|b| attr(b, name))
    };
    let inset = |name: &str, default: f32| {
        body_pr(name)
            .and_then(|v| v.parse::<f32>().ok())
            .map_or(default, |v| v / EMU_PER_POINT)
    };
    let scale = sources
        .iter()
        .flatten()
        .filter_map(|s| path(*s, &["bodyPr", "normAutofit"]))
        .find_map(|n| attr(n, "fontScale"))
        .and_then(|v| v.parse::<f32>().ok())
        .map_or(1., |v| v / 100_000.);
    let mut out = Text {
        paragraphs: Vec::new(),
        anchor: match body_pr("anchor") {
            Some("ctr") => Anchor::Middle,
            Some("b") => Anchor::Bottom,
            _ => Anchor::Top,
        },
        insets: [
            inset("lIns", 7.2),
            inset("tIns", 3.6),
            inset("rIns", 7.2),
            inset("bIns", 3.6),
        ],
    };
    let mut counters = [0u32; 9];
    for p in children(body, "p") {
        let ppr = child(p, "pPr");
        let level: u8 = ppr
            .and_then(|n| attr(n, "lvl"))
            .and_then(|l| l.parse().ok())
            .unwrap_or(0)
            .min(8);
        let lvl_name = format!("lvl{}pPr", level + 1);
        // This paragraph's own properties first, then each inherited level.
        let mut levels: Vec<Node> = ppr.into_iter().collect();
        levels.extend(
            sources
                .iter()
                .flatten()
                .filter_map(|s| path(*s, &["lstStyle", &lvl_name])),
        );
        levels.extend(style.and_then(|s| child(s, &lvl_name)));
        let first = |name: &str| levels.iter().find_map(|l| attr(*l, name));
        let align = match first("algn") {
            Some("ctr") => Align::Center,
            Some("r") => Align::End,
            Some("just" | "dist") => Align::Justify,
            _ => Align::Start,
        };
        let defaults: Vec<Node> = levels.iter().filter_map(|l| child(*l, "defRPr")).collect();
        let size = defaults
            .iter()
            .find_map(|d| attr(*d, "sz"))
            .and_then(|s| s.parse::<f32>().ok())
            .map_or(18., |s| s / 100.);
        let bullet = levels.iter().find_map(|l| {
            if child(*l, "buNone").is_some() {
                Some(None)
            } else if let Some(c) = child(*l, "buChar") {
                Some(attr(c, "char").map(|c| match c {
                    // Symbol-font bullets have no Unicode meaning.
                    "" | "§" | "Ø" | "ü" | "n" | "q" | "l" => "•".to_owned(),
                    c => c.to_owned(),
                }))
            } else if let Some(auto) = child(*l, "buAutoNum") {
                counters[usize::from(level)] += 1;
                let n = counters[usize::from(level)];
                Some(Some(match attr(auto, "type").unwrap_or("arabicPeriod") {
                    t if t.starts_with("alphaLc") => {
                        format!("{}.", (b'a' + ((n - 1) % 26) as u8) as char)
                    }
                    t if t.starts_with("alphaUc") => {
                        format!("{}.", (b'A' + ((n - 1) % 26) as u8) as char)
                    }
                    t if t.ends_with("ParenR") => format!("{n})"),
                    _ => format!("{n}."),
                }))
            } else {
                None
            }
        });
        let mut paragraph = Paragraph {
            align,
            bullet: bullet.flatten(),
            level,
            size: size * scale,
            spans: Vec::new(),
        };
        for run in p
            .children()
            .filter(|n| matches!(n.tag_name().name(), "r" | "fld" | "br"))
        {
            if run.tag_name().name() == "br" {
                if let Some(last) = paragraph.spans.last_mut() {
                    last.text.push('\n');
                }
                continue;
            }
            let text = if attr(run, "type") == Some("slidenum") {
                parts.number.to_string()
            } else {
                child(run, "t")
                    .and_then(|t| t.text())
                    .unwrap_or_default()
                    .to_owned()
            };
            if text.is_empty() {
                continue;
            }
            let rpr = child(run, "rPr");
            let mut props: Vec<Node> = rpr.into_iter().collect();
            props.extend(defaults.iter().copied());
            let flag = |name: &str| {
                props
                    .iter()
                    .find_map(|n| attr(*n, name))
                    .is_some_and(|v| v == "1" || v == "true")
            };
            let run_size = props
                .iter()
                .find_map(|n| attr(*n, "sz"))
                .and_then(|s| s.parse::<f32>().ok())
                .map_or(size, |s| s / 100.);
            // The run's own colour, then the shape style's, then inherited ones.
            let fill = |n: &Node| child(*n, "solidFill").and_then(|f| color(f, parts));
            let color = rpr
                .as_ref()
                .and_then(fill)
                .or(ink)
                .or_else(|| defaults.iter().find_map(fill))
                .or_else(|| parts.colors.get("tx1").copied());
            let font = props
                .iter()
                .find_map(|n| child(*n, "latin").and_then(|l| attr(l, "typeface")))
                .map(|face| match face {
                    "+mj-lt" => parts.fonts.0.clone(),
                    "+mn-lt" => parts.fonts.1.clone(),
                    face => face.to_owned(),
                });
            paragraph.spans.push(Span {
                text,
                bold: flag("b"),
                italic: flag("i"),
                underline: props
                    .iter()
                    .find_map(|n| attr(*n, "u"))
                    .is_some_and(|u| u != "none"),
                strike: props
                    .iter()
                    .find_map(|n| attr(*n, "strike"))
                    .is_some_and(|s| s != "noStrike"),
                size: Some(run_size * scale),
                color,
                font,
            });
        }
        out.paragraphs.push(paragraph);
    }
    out
}

fn table(table: Node, parts: &Parts) -> Kind {
    let emu = |n: Node, name| {
        attr(n, name)
            .and_then(|v| v.parse::<f32>().ok())
            .map_or(0., |v| v / EMU_PER_POINT)
    };
    let columns = path(table, &["tblGrid"])
        .into_iter()
        .flat_map(|g| children(g, "gridCol"))
        .map(|c| emu(c, "w"))
        .collect();
    let rows = children(table, "tr")
        .map(|tr| {
            let cells = children(tr, "tc")
                .map(|tc| {
                    child(tc, "txBody")
                        .map(|body| text(body, &[Some(body), None, None], None, None, parts))
                        .unwrap_or_default()
                })
                .collect();
            (emu(tr, "h"), cells)
        })
        .collect();
    Kind::Table { columns, rows }
}

fn background(
    bg: Node,
    rels: &HashMap<String, String>,
    parts: &Parts,
    package: &mut Package,
) -> Fill {
    if let Some(pr) = child(bg, "bgPr") {
        if let Some(fill) = child(pr, "solidFill").and_then(|f| color(f, parts)) {
            return Fill::Color(fill);
        }
        if let Some(blip) = child(pr, "blipFill").and_then(|f| child(f, "blip"))
            && let Some(data) = rel_attr(blip, "embed")
                .and_then(|id| rels.get(id))
                .and_then(|t| package.bytes(t))
        {
            return Fill::Image(data);
        }
        if let Some(stop) = pr.descendants().find(|n| n.tag_name().name() == "gs")
            && let Some(fill) = color(stop, parts)
        {
            return Fill::Color(fill);
        }
    }
    if let Some(reference) = child(bg, "bgRef")
        && let Some(fill) = color(reference, parts)
    {
        return Fill::Color(fill);
    }
    Fill::None
}

/// The colour a fill element names, with the theme's colours and the
/// master's colour map behind scheme names.
fn color(fill: Node, parts: &Parts) -> Option<u32> {
    let spec = fill.children().find(|n| n.is_element())?;
    let base = match spec.tag_name().name() {
        "srgbClr" => rgb(attr(spec, "val")?)?,
        "sysClr" => rgb(attr(spec, "lastClr")?)?,
        "schemeClr" => *parts.colors.get(attr(spec, "val")?)?,
        "prstClr" => match attr(spec, "val")? {
            "black" => 0x000000,
            "white" => 0xffffff,
            "red" => 0xff0000,
            "green" => 0x008000,
            "blue" => 0x0000ff,
            "yellow" => 0xffff00,
            "gray" | "grey" => 0x808080,
            _ => return None,
        },
        _ => return None,
    };
    let percent = |name: &str| {
        child(spec, name)
            .and_then(|n| attr(n, "val")?.parse::<f32>().ok())
            .map(|v| v / 100_000.)
    };
    let mut channels = [
        (base >> 16) as f32,
        ((base >> 8) & 0xff) as f32,
        (base & 0xff) as f32,
    ];
    if let Some(m) = percent("lumMod") {
        let offset = percent("lumOff").unwrap_or(0.) * 255.;
        for c in &mut channels {
            *c = (*c * m + offset).clamp(0., 255.);
        }
    } else if let Some(t) = percent("tint") {
        for c in &mut channels {
            *c = 255. - (255. - *c) * t;
        }
    } else if let Some(s) = percent("shade") {
        for c in &mut channels {
            *c *= s;
        }
    }
    let [r, g, b] = channels.map(|c| c.round() as u32);
    Some(r << 16 | g << 8 | b)
}

/// The theme's colours under both their own names and the names the
/// master's colour map gives them, and its heading and body fonts.
fn theme(theme: Option<Node>, master: Option<Node>) -> (HashMap<String, u32>, (String, String)) {
    let mut colors = HashMap::new();
    let mut fonts = (String::from("Calibri Light"), String::from("Calibri"));
    if let Some(elements) = theme.and_then(|t| path(t, &["themeElements"])) {
        if let Some(scheme) = child(elements, "clrScheme") {
            for entry in scheme.children().filter(|n| n.is_element()) {
                let value = entry.children().find(|n| n.is_element()).and_then(|c| {
                    rgb(attr(c, "val")
                        .filter(|_| c.tag_name().name() == "srgbClr")
                        .or_else(|| attr(c, "lastClr"))?)
                });
                if let Some(value) = value {
                    colors.insert(entry.tag_name().name().to_owned(), value);
                }
            }
        }
        if let Some(scheme) = child(elements, "fontScheme") {
            let face = |name| {
                path(scheme, &[name, "latin"])
                    .and_then(|l| attr(l, "typeface"))
                    .map(str::to_owned)
            };
            fonts = (
                face("majorFont").unwrap_or(fonts.0),
                face("minorFont").unwrap_or(fonts.1),
            );
        }
    }
    let map = master.and_then(|m| child(m, "clrMap"));
    for (alias, default) in [
        ("bg1", "lt1"),
        ("tx1", "dk1"),
        ("bg2", "lt2"),
        ("tx2", "dk2"),
    ] {
        let target = map.and_then(|m| attr(m, alias)).unwrap_or(default);
        if let Some(value) = colors.get(target).copied() {
            colors.insert(alias.to_owned(), value);
        }
    }
    colors.entry("tx1".into()).or_insert(0x000000);
    colors.entry("bg1".into()).or_insert(0xffffff);
    (colors, fonts)
}
