//! Word-processing documents as a column on a page: headings, paragraphs
//! with their emphasis, lists, tables and pictures. Only the blocks in view
//! are laid out, and pictures are decoded when they first come into view.
//! Docked at the right, a miniature of the whole column shows where the
//! view is.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use silent::detect::Kind as FileKind;
use silent::memory::View;
use silent::office::miniature::{Kind, Miniature, size_of};
use silent::office::{Align, Block, Document, Paragraph, Role, Span, Table, family};
use silent::vim::Motion;

use crate::content::{Paper, decode_still};
use crate::overview::{self, Docking, Geometry, Mode, Overview, Overviewed, Shown};
use crate::paged::{RAIL, THUMB};
use crate::theme::{Theme, accent, progress};
use crate::zoom::{self, Held};

/// The page, warm white as paper is.
pub const PAPER: u32 = 0xfffdf8;
pub const INK: u32 = 0x1f1a16;
/// How long a block jumped to stays lit.
const FLASH: Duration = Duration::from_millis(1200);

pub struct DocPane {
    doc: Rc<Document>,
    mini: Rc<Miniature>,
    list: ListState,
    images: HashMap<usize, Arc<RenderImage>>,
    pending: HashSet<usize>,
    overview: Overview,
    /// Pixels per point, as last drawn.
    scale: f32,
    /// A block jumped to: brought to the middle once it is laid out, and
    /// lit for a moment.
    centre: Option<usize>,
    flash: Option<(usize, Instant)>,
    /// The text size, 1 at 100%, and the place under the pointer as it
    /// changed.
    zoom: f32,
    held: Option<Held>,
}

impl DocPane {
    /// Lets the window's atlas go of the pictures drawn.
    pub fn drop_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (_, image) in self.images.drain() {
            cx.drop_image(image, Some(window));
        }
    }

    pub fn new(
        paper: Paper,
        mode: Mode,
        docked: bool,
        view: Option<View>,
        zoom: f32,
        window: &Window,
    ) -> Self {
        let doc = paper.document;
        let width = overview::beside(window, if docked { RAIL } else { 0. }).width;
        let scale = f32::from(width) / doc.page.0.max(1.) * zoom;
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
            mini: Rc::new(paper.miniature),
            list,
            images: HashMap::new(),
            pending: HashSet::new(),
            overview: Overview::new(mode, docked),
            scale,
            centre: None,
            flash: None,
            zoom,
            held: None,
        }
    }

    /// The column's text at `level`, laid out again at the window's width,
    /// with the place under the pointer at `at`, or else the block at the
    /// top, kept where it is.
    pub fn set_zoom(&mut self, level: f32, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        self.zoom = level;
        self.held = at.and_then(|at| zoom::hold(&self.list, at.y));
        self.list.remeasure();
        cx.notify();
    }

    pub fn view(&self) -> Option<View> {
        let top = self.list.logical_scroll_top();
        let offset = f32::from(top.offset_in_item);
        (top.item_ix > 0 || offset > 0.5).then_some(View::Blocks {
            block: top.item_ix,
            offset,
        })
    }

    /// How far down the document the view is, for the strip, while the
    /// scrollbar is held or pointed at.
    pub fn scrub_label(&self) -> Option<String> {
        let percent = (self.geometry(1.).fraction() * 100.).round();
        self.overview.scrubbing().then(|| format!("{percent}%"))
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.overview.mode = mode;
        overview::reveal(self, cx);
    }

    pub fn docking(&self) -> Docking {
        Docking {
            width: RAIL,
            docked: self.overview.docked,
            wanted: self.overview.mode == Mode::On && self.geometry(1.).scrollable(),
        }
    }

    pub fn set_docked(&mut self, docked: bool, cx: &mut Context<Self>) {
        self.overview.docked = docked;
        cx.notify();
    }

    fn dock(&self) -> f32 {
        if self.overview.docked { RAIL } else { 0. }
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

    /// Points down the miniature's column. The view runs from the block at
    /// its top to the one at its bottom, each as far into it as the list
    /// has it laid out, taken as far into the miniature's block: the
    /// miniature guesses heights, and this keeps the two in step.
    fn geometry(&self, track: f64) -> Geometry {
        let viewport = self.list.viewport_bounds();
        let top = self.list.logical_scroll_top();
        let into = |ix: usize, pixels: Pixels| {
            let (_, height) = self.mini.blocks.get(ix).copied().unwrap_or_default();
            match self.list.bounds_for_item(ix) {
                Some(drawn) if drawn.size.height > px(0.) => {
                    height * (pixels / drawn.size.height).clamp(0., 1.)
                }
                _ => f32::from(pixels) / self.scale,
            }
        };
        let y_top = self
            .mini
            .y_of(top.item_ix, into(top.item_ix, top.offset_in_item));
        let mut y_bottom = y_top + f32::from(viewport.size.height) / self.scale;
        for ix in top.item_ix..self.doc.blocks.len() {
            let Some(drawn) = self.list.bounds_for_item(ix) else {
                break;
            };
            if drawn.top() >= viewport.bottom() {
                break;
            }
            y_bottom = self
                .mini
                .y_of(ix, into(ix, viewport.bottom() - drawn.top()));
        }
        Geometry::new(
            f64::from(self.mini.height),
            f64::from((y_bottom - y_top).max(1.)),
            f64::from(y_top),
            f64::from(THUMB / self.doc.page.0.max(1.)),
            track,
        )
    }

    fn scroll_to(&mut self, top: f64, _: &mut Window, cx: &mut Context<Self>) {
        let (item_ix, within) = self.mini.block_at(top as f32);
        let (_, height) = self.mini.blocks.get(item_ix).copied().unwrap_or_default();
        let offset_in_item = match self.list.bounds_for_item(item_ix) {
            Some(drawn) if height > 0. => drawn.size.height * (within / height),
            _ => px(within * self.scale),
        };
        self.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item,
        });
        cx.notify();
    }
}

/// The miniature on the dock, from the geometry's shift down: the column as
/// a strip of paper with its lines, rules and pictures.
fn sketch(mini: Rc<Miniature>, page: f32, g: Geometry, theme: Theme) -> AnyElement {
    let (scale, shift) = (g.scale as f32, g.shift() as f32);
    let margin = (page - mini.width) / 2.;
    let ink: Hsla = rgb(INK).into();
    let rule = theme.muted.opacity(0.6);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let hairline = px(1. / window.scale_factor());
            let left = bounds.left() + px((RAIL - THUMB) / 2.);
            let top = bounds.top() - px(shift);
            let paper = Bounds::from_corners(
                point(left, top.max(bounds.top())),
                point(
                    left + px(THUMB),
                    (top + px(mini.height * scale)).min(bounds.bottom()),
                ),
            );
            window.paint_quad(quad(
                paper,
                px(0.),
                rgb(PAPER),
                hairline,
                theme.muted.opacity(0.35),
                BorderStyle::Solid,
            ));
            let span = shift / scale..(shift + f32::from(bounds.size.height)) / scale;
            for mark in mini.marks_within(span.start, span.end) {
                let color = match mark.kind {
                    Kind::Heading => ink.opacity(0.75),
                    Kind::Text => ink.opacity(0.32),
                    Kind::Rule => rule,
                    Kind::Picture => accent(FileKind::Image).opacity(0.35),
                };
                let origin = point(
                    left + px((margin + mark.x) * scale),
                    top + px(mark.y * scale),
                );
                let extent = size(
                    px(mark.w * scale).max(hairline),
                    px(mark.h * scale).max(hairline),
                );
                window.paint_quad(fill(Bounds::new(origin, extent), color));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

impl Render for DocPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.centre_block(window);
        if let Some(held) = &mut self.held {
            if zoom::keep(&self.list, held) {
                self.held = None;
            } else {
                window.request_animation_frame();
            }
        }
        let theme = Theme::of(window.appearance());
        let width = f32::from(overview::beside(window, self.dock()).width);
        let doc = self.doc.clone();
        // Pixels per point: the page's width fills the view's, at 100%.
        let scale = width / doc.page.0.max(1.) * self.zoom;
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
        let (dock, bar) = match self.overview.shown(self.geometry(1.).scrollable()) {
            Shown::Docked => {
                let g = self.geometry(f64::from(height));
                let content = sketch(self.mini.clone(), self.doc.page.0, g, theme);
                let panel = overview::panel(&self.overview, g, RAIL, content, theme, cx);
                let dock = overview::dock(RAIL, theme.backdrop, None, Some(panel)).on_scroll_wheel(
                    cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        let delta = event.delta.pixel_delta(window.line_height());
                        this.list.scroll_by(-delta.y);
                        cx.notify();
                    }),
                );
                (Some(dock), None)
            }
            Shown::Bar => {
                let g = self.geometry(overview::bar_track(height));
                (
                    None,
                    Some(overview::scrollbar(&self.overview, g, theme, cx)),
                )
            }
            Shown::Overlay | Shown::Nothing => (None, None),
        };
        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(theme.backdrop)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    .bg(rgb(PAPER))
                    .pt(px(margin.min(48.)))
                    .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                        overview::reveal(this, cx);
                    }))
                    .child(rows)
                    .children(bar),
            )
            .children(dock)
    }
}
