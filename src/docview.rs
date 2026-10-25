//! Word-processing documents as a column on a page: headings, paragraphs
//! with their emphasis, lists, tables and pictures. Only the blocks in view
//! are laid out, and pictures are decoded when they first come into view.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use silent::memory::View;
use silent::office::{Align, Block, Document, Paragraph, Role, Span, Table};
use silent::vim::Motion;

use crate::content::decode_still;
use crate::overview::{self, Geometry, Mode, Overview, Overviewed, Shown};
use crate::theme::{Theme, progress};

/// The page, warm white as paper is.
const PAPER: u32 = 0xfffdf8;
const INK: u32 = 0x1f1a16;
/// How long a block jumped to stays lit.
const FLASH: Duration = Duration::from_millis(1200);

pub struct DocPane {
    doc: Rc<Document>,
    list: ListState,
    images: HashMap<usize, Arc<RenderImage>>,
    pending: HashSet<usize>,
    /// A document has no minimap: the slim scrollbar shows while it scrolls.
    overview: Overview,
    /// Pixels per point, as last drawn.
    scale: f32,
    /// A block jumped to: brought to the middle once it is laid out, and
    /// lit for a moment.
    centre: Option<usize>,
    flash: Option<(usize, Instant)>,
}

impl DocPane {
    pub fn new(doc: Document, view: Option<View>, window: &Window) -> Self {
        let scale = f32::from(window.viewport_size().width) / doc.page.0.max(1.);
        // Blocks not laid out yet count as two lines, so that the scrollbar
        // is about right from the start.
        let guess = px(doc.base_size * scale * 1.3 * 2.5);
        let list = ListState::new(doc.blocks.len(), ListAlignment::Top, px(1000.))
            .with_uniform_item_height(guess);
        if let Some(View::Blocks { block, offset }) = view {
            list.scroll_to(ListOffset {
                item_ix: block,
                offset_in_item: px(offset),
            });
        }
        Self {
            doc: Rc::new(doc),
            list,
            images: HashMap::new(),
            pending: HashSet::new(),
            overview: Overview::new(Mode::Off),
            scale,
            centre: None,
            flash: None,
        }
    }

    pub fn view(&self) -> Option<View> {
        let top = self.list.logical_scroll_top();
        let offset = f32::from(top.offset_in_item);
        (top.item_ix > 0 || offset > 0.5).then_some(View::Blocks {
            block: top.item_ix,
            offset,
        })
    }

    /// Moves as vim's keys and the arrows ask: j and k by a line of body
    /// text, `20G` to the twentieth block.
    pub fn motion(&mut self, motion: Motion, window: &Window, cx: &mut Context<Self>) {
        let height = window.viewport_size().height;
        let line = px(self.doc.base_size * self.scale * 1.3);
        let by = |pixels: Pixels, k: usize| self.list.scroll_by(pixels * k.min(1 << 16) as f32);
        match motion {
            Motion::Down(k) | Motion::ScrollDown(k) => by(line, k),
            Motion::Up(k) | Motion::ScrollUp(k) => by(-line, k),
            Motion::HalfDown(k) => by(height / 2., k),
            Motion::HalfUp(k) => by(-height / 2., k),
            Motion::PageDown(k) => by(height * 0.9, k),
            Motion::PageUp(k) => by(-height * 0.9, k),
            Motion::Top | Motion::Bottom => {
                let item_ix = if motion == Motion::Top {
                    0
                } else {
                    self.doc.blocks.len()
                };
                self.list.scroll_to(ListOffset {
                    item_ix,
                    offset_in_item: px(0.),
                });
            }
            Motion::Line(k) => {
                let ix = k
                    .saturating_sub(1)
                    .min(self.doc.blocks.len().saturating_sub(1));
                self.list.scroll_to(ListOffset {
                    item_ix: ix,
                    offset_in_item: px(0.),
                });
                self.centre = Some(ix);
                self.flash = Some((ix, Instant::now()));
            }
            Motion::Left(_) | Motion::Right(_) | Motion::Home => return,
        }
        overview::reveal(self, cx);
    }

    /// Brings the block jumped to into the middle, once it has been laid
    /// out; a block taller than the view goes to its top.
    fn centre_block(&mut self, window: &mut Window) {
        let Some(ix) = self.centre else {
            return;
        };
        let Some(block) = self.list.bounds_for_item(ix) else {
            window.request_animation_frame();
            return;
        };
        self.centre = None;
        let view = self.list.viewport_bounds();
        let delta = if block.size.height > view.size.height * 0.8 {
            block.top() - view.top() - px(24.)
        } else {
            block.center().y - view.center().y
        };
        self.list.scroll_by(delta);
    }

    /// Decodes picture `ix` in the background, the first time it is drawn.
    fn want(&mut self, ix: usize, longest: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.images.contains_key(&ix) || !self.pending.insert(ix) {
            return;
        }
        let doc = self.doc.clone();
        let Some(Block::Image(image)) = doc.blocks.get(ix) else {
            return;
        };
        let data = image.data.clone();
        cx.spawn_in(window, async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { decode_still(&data, longest).map(|(image, _)| image) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(image) = decoded {
                    this.images.insert(ix, image);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// The families installed for the faces Office documents name most:
/// Carlito and Caladea are metric twins of Calibri and Cambria.
pub fn family(font: Option<&str>) -> &'static str {
    let Some(font) = font else {
        return "Carlito";
    };
    let lower = font.to_lowercase();
    if lower.contains("calibri") || lower.contains("aptos") {
        "Carlito"
    } else if lower.contains("cambria") {
        "Caladea"
    } else if lower.contains("courier") || lower.contains("consolas") || lower.contains("mono") {
        "Liberation Mono"
    } else if lower.contains("times")
        || lower.contains("georgia")
        || lower.contains("garamond")
        || lower.contains("serif") && !lower.contains("sans")
        || lower.contains("song")
        || lower.contains("宋")
        || lower.contains("ming")
    {
        "Liberation Serif"
    } else if lower.contains("arial")
        || lower.contains("helvetica")
        || lower.contains("liberation sans")
    {
        "Liberation Sans"
    } else {
        "Noto Sans"
    }
}

/// A paragraph's runs as one styled text: weight, slant, lines and colour
/// per run, on the paragraph's font and size.
pub fn styled(spans: &[Span], default: Hsla) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let style = HighlightStyle {
            color: Some(span.color.map_or(default, |c| rgb(c).into())),
            font_weight: span.bold.then_some(FontWeight::BOLD),
            font_style: span.italic.then_some(FontStyle::Italic),
            underline: span.underline.then(|| UnderlineStyle {
                thickness: px(1.),
                ..UnderlineStyle::default()
            }),
            strikethrough: span.strike.then(|| StrikethroughStyle {
                thickness: px(1.),
                ..StrikethroughStyle::default()
            }),
            ..HighlightStyle::default()
        };
        highlights.push((start..text.len(), style));
    }
    StyledText::new(text).with_highlights(highlights)
}

fn text_align(align: Align) -> TextAlign {
    match align {
        Align::Center => TextAlign::Center,
        Align::End => TextAlign::Right,
        Align::Start | Align::Justify => TextAlign::Left,
    }
}

/// Points for a paragraph: its largest run, or its role's size.
fn size_of(paragraph: &Paragraph, base: f32) -> f32 {
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

fn paragraph(p: &Paragraph, base: f32, scale: f32, ink: Hsla) -> Div {
    let size = size_of(p, base);
    let heading = !matches!(p.role, Role::Body);
    let mut spans = p.spans.clone();
    if heading {
        for span in &mut spans {
            span.bold |= !matches!(p.role, Role::Subtitle);
        }
    }
    let font = family(p.spans.iter().find_map(|s| s.font.as_deref()));
    let line = px(size * scale * 1.3);
    let text = div()
        .flex_1()
        .min_w_0()
        .text_align(text_align(p.align))
        .child(styled(&spans, ink));
    let mut row = div()
        .flex()
        .flex_row()
        .font_family(font)
        .text_size(px(size * scale))
        .line_height(line)
        .text_color(ink)
        .min_h(line)
        .pb(px(size * scale * if heading { 0.35 } else { 0.45 }))
        .when(heading, |row| row.pt(px(size * scale * 0.6)));
    let indent = px(p.indent * scale);
    match &p.marker {
        // The marker hangs in the indent, ending a little before the text.
        Some(marker) => {
            row = row.child(
                div()
                    .flex_none()
                    .w(indent.max(px(size * scale * 1.6)))
                    .pr(px(size * scale * 0.5))
                    .flex()
                    .justify_end()
                    .child(marker.clone()),
            );
        }
        None => row = row.pl(indent),
    }
    row.child(text)
}

fn table(t: &Table, base: f32, scale: f32, ink: Hsla, rule: Hsla) -> Div {
    let size = base * 0.95;
    let mut grid = div()
        .flex()
        .flex_col()
        .w_full()
        .my(px(size * scale * 0.6))
        .border_t_1()
        .border_l_1()
        .border_color(rule);
    for row in &t.rows {
        let mut line = div().flex().flex_row().w_full();
        let mut column = 0;
        for cell in row {
            let share: f32 = t.widths.iter().skip(column).take(cell.span.max(1)).sum();
            column += cell.span.max(1);
            let content = cell.paragraphs.iter().map(|p| {
                let small = Paragraph {
                    marker: p.marker.clone(),
                    spans: p.spans.clone(),
                    align: p.align,
                    indent: p.indent,
                    role: Role::Body,
                };
                paragraph(&small, size, scale, ink).pb(px(0.))
            });
            line = line.child(
                div()
                    .flex_basis(relative(share.max(0.01)))
                    .flex_grow(1.)
                    .min_w_0()
                    .px(px(5. * scale))
                    .py(px(3. * scale))
                    .border_r_1()
                    .border_b_1()
                    .border_color(rule)
                    .children(content),
            );
        }
        grid = grid.child(line);
    }
    grid
}

impl Overviewed for DocPane {
    fn overview(&mut self) -> &mut Overview {
        &mut self.overview
    }

    /// Pixels: the list knows the heights of the blocks laid out so far,
    /// and guesses the rest.
    fn geometry(&self, track: f64) -> Geometry {
        let view = f64::from(self.list.viewport_bounds().size.height);
        let max = f64::from(self.list.max_offset_for_scrollbar().y);
        let top = -f64::from(self.list.scroll_px_offset_for_scrollbar().y);
        Geometry::new(max + view, view, top, 1., track)
    }

    fn scroll_to(&mut self, top: f64, _: &mut Window, cx: &mut Context<Self>) {
        self.list
            .set_offset_from_scrollbar(point(px(0.), px(-top as f32)));
        cx.notify();
    }

    fn on_drag(&mut self, held: bool) {
        if held {
            self.list.scrollbar_drag_started();
        } else {
            self.list.scrollbar_drag_ended();
        }
    }
}

impl Render for DocPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.centre_block(window);
        let theme = Theme::of(window.appearance());
        let width = f32::from(window.viewport_size().width);
        let doc = self.doc.clone();
        // Pixels per point: the page's width fills the window's.
        let scale = width / doc.page.0.max(1.);
        self.scale = scale;
        let margin = (doc.margins.0 * scale).clamp(16., width / 4.);
        let content = width - margin * 2.;
        let ink: Hsla = rgb(INK).into();
        let rule = theme.muted.opacity(0.5);
        let device = window.scale_factor();
        let view = cx.entity();
        let flash = self.flash.filter(|(_, at)| at.elapsed() < FLASH);
        if flash.is_some() {
            window.request_animation_frame();
        }
        let rows = list(self.list.clone(), move |ix, window, cx| {
            let block = match &doc.blocks[ix] {
                Block::Paragraph(p) => paragraph(p, doc.base_size, scale, ink).into_any_element(),
                Block::Table(t) => table(t, doc.base_size, scale, ink, rule).into_any_element(),
                Block::Image(image) => {
                    let w = (image.size.0 * scale).min(content).max(1.);
                    let h = w * image.size.1 / image.size.0.max(1.);
                    let decoded = view.read(cx).images.get(&ix).cloned();
                    if decoded.is_none() {
                        let longest = (w.max(h) * device).ceil() as u32;
                        view.update(cx, |pane, cx| pane.want(ix, longest, window, cx));
                    }
                    let frame = div().w(px(w)).h(px(h)).my(px(6. * scale));
                    match decoded {
                        Some(image) => frame
                            .child(img(ImageSource::Render(image)).size_full())
                            .into_any_element(),
                        None => frame.bg(rule.opacity(0.25)).into_any_element(),
                    }
                }
            };
            let lit = flash.filter(|(block, _)| *block == ix).map(|(_, at)| {
                let left = 1. - at.elapsed().as_secs_f32() / FLASH.as_secs_f32();
                progress().opacity(0.3 * left.max(0.).sqrt())
            });
            div()
                .px(px(margin))
                .when_some(lit, |row, lit| row.bg(lit))
                .child(block)
                .into_any_element()
        })
        .size_full();
        let height = f32::from(window.viewport_size().height);
        let overview =
            (self.overview.shown(self.geometry(1.).scrollable()) == Shown::Bar).then(|| {
                let g = self.geometry(overview::bar_track(height));
                let label = format!("{}%", (g.fraction() * 100.).round());
                overview::scrollbar(&self.overview, g, label, theme, cx)
            });
        div()
            .size_full()
            .relative()
            .bg(rgb(PAPER))
            .pt(px(margin.min(48.)))
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                overview::reveal(this, cx);
            }))
            .child(rows)
            .children(overview)
    }
}
