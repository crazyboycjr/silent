//! Text, code and hexdumps. Only the lines in view are shaped and drawn,
//! straight from their source, so a 2 MB file or a dump of gigabytes opens
//! at once. Long lines scroll sideways rather than wrap, text can be
//! selected with the mouse and copied, and a minimap in a column at the
//! right shows the lines around the view in their highlighting colours.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::*;
use silent::memory::View;
use silent::vim::Motion;
use silent::{format, hexdump};
use smallvec::SmallVec;
use unicode_width::UnicodeWidthChar;

use crate::content::{Hex, TAB, Text, columns};
use crate::layout;
use crate::overview::{self, Geometry, Mode, Overview, Overviewed, Shown as OverviewShown};
use crate::theme::{Theme, progress};

const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT: f32 = 20.;
const PAD_TOP: f32 = 14.;
const PAD_SIDE: f32 = 18.;
/// Bytes of a line that are drawn; the rest of a minified line is only
/// copied.
const SHOWN: usize = 4000;
/// A copy of a hexdump stops after this many lines (about 20 MB of text).
const COPY_LINES: usize = 1 << 18;
/// Columns that h, l and the arrows move sideways.
const STEP: f32 = 4.;
/// The minimap: the columns drawn, and its margin.
const MINI_COLUMNS: usize = 90;
const MINI_PAD: f32 = 8.;
/// Lines spanning more bytes than this are highlighted one by one for the
/// minimap, each only as far as it is drawn.
const MINI_SPAN: usize = 64 << 10;
/// How long a line jumped to stays lit.
const FLASH: Duration = Duration::from_millis(1200);

pub enum Doc {
    Text(Box<Text>),
    Hex(Hex),
}

impl Doc {
    pub fn line_count(&self) -> usize {
        match self {
            Doc::Text(text) => text.lines.len(),
            Doc::Hex(hex) => hexdump::line_count(hex.bytes.len() as u64) as usize,
        }
    }

    fn line(&self, ix: usize) -> Cow<'_, str> {
        match self {
            Doc::Text(text) => Cow::Borrowed(&text.text[text.lines[ix].clone()]),
            Doc::Hex(hex) => Cow::Owned(hexdump::line(&hex.bytes, ix as u64, self.digits())),
        }
    }

    fn digits(&self) -> usize {
        match self {
            Doc::Text(_) => 0,
            Doc::Hex(hex) => hexdump::offset_digits(hex.bytes.len() as u64),
        }
    }

    /// Columns of the widest line as drawn.
    fn widest(&self) -> usize {
        match self {
            Doc::Text(text) => text.widest.min(SHOWN),
            Doc::Hex(_) => self.digits() + 70,
        }
    }
}

/// A place in the text: a line, and a byte offset into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pos {
    line: usize,
    byte: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Line,
}

/// What was pressed on first, and what the pointer is on now, each grown to
/// the unit the selection started with.
#[derive(Clone)]
struct Selection {
    anchor: Range<Pos>,
    head: Range<Pos>,
}

impl Selection {
    fn range(&self) -> Range<Pos> {
        self.anchor.start.min(self.head.start)..self.anchor.end.max(self.head.end)
    }
}

/// A line as drawn: tabs stretched to spaces, very long lines cut.
struct Shown {
    text: SharedString,
    /// Each tab's byte in the source, and the spaces it gained.
    tabs: SmallVec<[(usize, usize); 4]>,
    /// Source bytes drawn.
    len: usize,
}

impl Shown {
    fn new(source: &str) -> Self {
        if source.len() <= SHOWN && !source.contains('\t') {
            return Self {
                text: SharedString::from(source.to_owned()),
                tabs: SmallVec::new(),
                len: source.len(),
            };
        }
        let mut text = String::with_capacity(source.len().min(SHOWN) + 16);
        let mut tabs = SmallVec::new();
        let mut len = source.len();
        let mut column = 0;
        for (i, c) in source.char_indices() {
            if text.len() >= SHOWN {
                len = i;
                break;
            }
            if c == '\t' {
                let width = TAB - column % TAB;
                tabs.push((i, width - 1));
                text.extend(std::iter::repeat_n(' ', width));
                column += width;
            } else {
                text.push(c);
                column += columns(c.encode_utf8(&mut [0; 4]));
            }
        }
        Self {
            text: text.into(),
            tabs,
            len,
        }
    }

    fn to_shown(&self, byte: usize) -> usize {
        let byte = byte.min(self.len);
        byte + self
            .tabs
            .iter()
            .take_while(|(tab, _)| *tab < byte)
            .map(|(_, extra)| extra)
            .sum::<usize>()
    }

    fn to_source(&self, shown: usize) -> usize {
        let mut gained = 0;
        for &(tab, extra) in &self.tabs {
            let start = tab + gained;
            if shown <= start {
                break;
            }
            if shown <= start + extra {
                // Inside a tab's spaces: the nearer side of the tab.
                return if shown - start > extra.div_ceil(2) {
                    tab + 1
                } else {
                    tab
                };
            }
            gained += extra;
        }
        (shown - gained).min(self.len)
    }
}

/// A line as the minimap draws it: runs of columns with ink, and their colours.
type Ink = SmallVec<[(u16, u16, Hsla); 8]>;

/// The minimap's grid in logical pixels, each a whole number of device
/// pixels so that every line is drawn alike: lines about 2 pixels apart
/// with a device pixel between them, columns about 1 pixel wide.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pitch {
    line: f64,
    column: f32,
    ink: f32,
    scale: f32,
}

impl Pitch {
    fn at(scale: f32) -> Self {
        let line = (2. * scale).round().max(2.);
        Self {
            line: f64::from(line / scale),
            column: (scale.round().max(1.)) / scale,
            ink: (line - 1.) / scale,
            scale,
        }
    }

    pub fn width(self) -> f32 {
        MINI_COLUMNS as f32 * self.column + 2. * MINI_PAD
    }

    /// `y` moved onto the device pixel grid.
    fn snap(self, y: f64) -> f32 {
        (y * f64::from(self.scale)).round() as f32 / self.scale
    }
}

/// The minimap's lines around the view, kept while the view stays near.
#[derive(Default)]
struct Mini {
    first: usize,
    lines: VecDeque<Ink>,
    dark: bool,
}

impl Mini {
    fn get(&self, ix: usize) -> Option<&Ink> {
        self.lines.get(ix.checked_sub(self.first)?)
    }

    /// Makes sure lines `range` are here, making only those that are not.
    fn fill(&mut self, range: Range<usize>, dark: bool, make: impl Fn(Range<usize>) -> Vec<Ink>) {
        if range.is_empty() {
            return;
        }
        let end = self.first + self.lines.len();
        if dark != self.dark
            || self.lines.is_empty()
            || range.end <= self.first
            || range.start >= end
        {
            self.dark = dark;
            self.first = range.start;
            self.lines = make(range).into();
            return;
        }
        if range.start < self.first {
            for ink in make(range.start..self.first).into_iter().rev() {
                self.lines.push_front(ink);
            }
            self.first = range.start;
        }
        let end = self.first + self.lines.len();
        if range.end > end {
            self.lines.extend(make(end..range.end));
        }
        // A screenful either side is kept for scrolling back.
        let slack = range.len();
        while self.first + slack < range.start {
            self.lines.pop_front();
            self.first += 1;
        }
        while self.first + self.lines.len() > range.end + slack {
            self.lines.pop_back();
        }
    }
}

/// The minimap's width at a desktop scale.
pub fn minimap_width(scale: f32) -> f32 {
    Pitch::at(scale).width()
}

/// How many lines a view `height` tall holds, a part line as a part.
fn rows_in(height: Pixels) -> f64 {
    f64::from(((height - px(PAD_TOP)) / px(LINE_HEIGHT)).max(1.))
}

/// Whether `lines` lines need scrolling in a view `height` logical pixels
/// tall; the view scrolls one line past the last.
pub fn scrolls(lines: usize, height: f32) -> bool {
    lines as f64 + 1. - rows_in(px(height)) > 1e-6
}

pub struct LinesPane {
    doc: Rc<Doc>,
    /// The first line in view, with the fraction of it scrolled past.
    top: f64,
    scroll_x: Pixels,
    selection: Option<Selection>,
    dragging: Option<Unit>,
    /// The view's height, as the window's layout gives it.
    height: Pixels,
    /// Where the lines were last drawn, for the pointer.
    area: Rc<Cell<Bounds<Pixels>>>,
    advance: Rc<Cell<Pixels>>,
    overview: Overview,
    mini: Rc<RefCell<Mini>>,
    pitch: Pitch,
    /// A line just jumped to, lit for a moment.
    flash: Option<(usize, Instant)>,
}

impl LinesPane {
    pub fn new(doc: Doc, mode: Mode, view: Option<View>) -> Self {
        let advance = px(7.5);
        let (top, side) = match view {
            Some(View::Lines { top, side }) => (top, side),
            _ => (0., 0.),
        };
        Self {
            doc: Rc::new(doc),
            top,
            scroll_x: advance * side,
            selection: None,
            dragging: None,
            height: px(0.),
            area: Rc::default(),
            advance: Rc::new(Cell::new(advance)),
            overview: Overview::new(mode),
            mini: Rc::default(),
            pitch: Pitch::at(1.),
            flash: None,
        }
    }

    /// What the strip says about the content.
    pub fn summary(&self) -> Vec<String> {
        match self.doc.as_ref() {
            Doc::Text(text) => {
                let mut parts = vec![format::count(text.lines.len() as u64, "line", "lines")];
                parts.extend(text.language.map(str::to_owned));
                if text.pretty {
                    parts.push("pretty-printed".into());
                }
                if text.truncated {
                    parts.push("the first 2 MB".into());
                }
                parts
            }
            Doc::Hex(hex) => {
                let mut parts = vec!["hexdump".to_owned()];
                if hex.truncated {
                    parts.push("the first 2 MB".into());
                }
                parts
            }
        }
    }

    /// Where the view is, to come back to; nothing at the start.
    pub fn view(&self) -> Option<View> {
        let side = self.scroll_x / self.advance.get();
        (self.top > 0.01 || side > 0.01).then_some(View::Lines {
            top: self.top,
            side,
        })
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.overview.mode = mode;
        overview::reveal(self, cx);
    }

    /// Where the view is, for the strip, while the scrollbar is held or
    /// pointed at.
    pub fn scrub_label(&self) -> Option<String> {
        self.overview.scrubbing().then(|| self.label())
    }

    fn font() -> Font {
        Font {
            features: FontFeatures::disable_ligatures(),
            ..font("JetBrains Mono")
        }
    }

    fn rows(&self) -> f64 {
        rows_in(self.height)
    }

    fn max_top(&self) -> f64 {
        (self.doc.line_count() as f64 - self.rows() + 1.).max(0.)
    }

    fn max_scroll_x(&self) -> Pixels {
        let widest = self.advance.get() * (self.doc.widest() + 2) as f32;
        (widest + px(PAD_SIDE) - self.area.get().size.width).max(px(0.))
    }

    fn clamp(&mut self) {
        self.top = self.top.clamp(0., self.max_top());
        self.scroll_x = self.scroll_x.clamp(px(0.), self.max_scroll_x());
    }

    /// Moves as vim's keys and the arrows ask.
    pub fn motion(&mut self, motion: Motion, cx: &mut Context<Self>) {
        let rows = self.rows();
        let half = (rows / 2.).floor().max(1.);
        let page = (rows - 2.).max(1.);
        let step = self.advance.get() * STEP;
        let n = |count: usize| count as f64;
        let before = self.top;
        match motion {
            Motion::Down(k) | Motion::ScrollDown(k) => self.top += n(k),
            Motion::Up(k) | Motion::ScrollUp(k) => self.top -= n(k),
            Motion::HalfDown(k) => self.top += n(k) * half,
            Motion::HalfUp(k) => self.top -= n(k) * half,
            Motion::PageDown(k) => self.top += n(k) * page,
            Motion::PageUp(k) => self.top -= n(k) * page,
            Motion::Right(k) => self.scroll_x += step * k.min(1 << 20) as f32,
            Motion::Left(k) => self.scroll_x -= step * k.min(1 << 20) as f32,
            Motion::Home => self.scroll_x = px(0.),
            Motion::Top => self.top = 0.,
            Motion::Bottom => self.top = f64::MAX,
            Motion::Line(k) => {
                // The line comes to the middle of the view, lit for a moment.
                let ix = k
                    .saturating_sub(1)
                    .min(self.doc.line_count().saturating_sub(1));
                self.top = ix as f64 - ((rows - 1.) / 2.).floor();
                self.flash = Some((ix, Instant::now()));
            }
        }
        self.clamp();
        if self.top != before {
            overview::reveal(self, cx);
        }
        cx.notify();
    }

    /// Half the view sideways, in whole columns.
    pub fn sideways(&mut self, right: bool, cx: &mut Context<Self>) {
        let advance = self.advance.get();
        let half = advance * ((self.area.get().size.width / advance / 2.).floor().max(1.));
        self.scroll_x += if right { half } else { -half };
        self.clamp();
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(LINE_HEIGHT));
        let before = self.top;
        self.top -= f64::from(delta.y / LINE_HEIGHT);
        self.scroll_x -= delta.x;
        self.clamp();
        if self.top != before {
            overview::reveal(self, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The place under `point`, from the line as it is drawn.
    fn hit(&self, point: Point<Pixels>, window: &mut Window) -> Pos {
        let area = self.area.get();
        let row = f64::from((point.y - area.top() - px(PAD_TOP)) / LINE_HEIGHT);
        let last = self.doc.line_count().saturating_sub(1);
        let line = ((self.top + row).floor().max(0.) as usize).min(last);
        let source = self.doc.line(line);
        let shown = Shown::new(&source);
        let run = TextRun {
            len: shown.text.len(),
            font: Self::font(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped =
            window
                .text_system()
                .shape_line(shown.text.clone(), px(FONT_SIZE), &[run], None);
        let x = point.x - area.left() + self.scroll_x;
        Pos {
            line,
            byte: shown.to_source(shaped.closest_index_for_x(x)),
        }
    }

    fn unit_at(&self, pos: Pos, unit: Unit) -> Range<Pos> {
        match unit {
            Unit::Char => pos..pos,
            Unit::Word => {
                let line = self.doc.line(pos.line);
                let word = word(&line, pos.byte);
                Pos {
                    line: pos.line,
                    byte: word.start,
                }..Pos {
                    line: pos.line,
                    byte: word.end,
                }
            }
            Unit::Line => {
                let start = Pos {
                    line: pos.line,
                    byte: 0,
                };
                let end = if pos.line + 1 < self.doc.line_count() {
                    Pos {
                        line: pos.line + 1,
                        byte: 0,
                    }
                } else {
                    Pos {
                        line: pos.line,
                        byte: self.doc.line(pos.line).len(),
                    }
                };
                start..end
            }
        }
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pos = self.hit(event.position, window);
        let unit = match event.click_count {
            1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let range = self.unit_at(pos, unit);
        match &mut self.selection {
            Some(selection) if event.modifiers.shift && unit == Unit::Char => {
                selection.head = range;
            }
            _ => {
                self.selection = Some(Selection {
                    anchor: range.clone(),
                    head: range,
                });
            }
        }
        self.dragging = Some(unit);
        cx.notify();
    }

    fn drag(&mut self, point: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(unit) = self.dragging else {
            return;
        };
        let area = self.area.get();
        // Past the top or bottom edge the view follows, faster the further out.
        let beyond = if point.y < area.top() {
            point.y - area.top()
        } else if point.y > area.bottom() {
            point.y - area.bottom()
        } else {
            px(0.)
        };
        if beyond != px(0.) {
            self.top += f64::from(beyond / LINE_HEIGHT) * 0.5;
            self.clamp();
        }
        let range = self.unit_at(self.hit(point, window), unit);
        if let Some(selection) = &mut self.selection {
            selection.head = range;
        }
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if self.dragging.take().is_none() {
            return;
        }
        if let Some(text) = self.selected_text() {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let last = self.doc.line_count().saturating_sub(1);
        let end = Pos {
            line: last,
            byte: self.doc.line(last).len(),
        };
        let all = Pos { line: 0, byte: 0 }..end;
        self.selection = Some(Selection {
            anchor: all.clone(),
            head: all,
        });
        if let Some(text) = self.selected_text() {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    pub fn copy(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn selected_text(&self) -> Option<String> {
        let range = self.selection.as_ref()?.range();
        if range.start == range.end {
            return None;
        }
        Some(match self.doc.as_ref() {
            Doc::Text(text) => {
                let at = |pos: Pos| {
                    let line = &text.lines[pos.line];
                    line.start + pos.byte.min(line.len())
                };
                text.text[at(range.start)..at(range.end)].to_owned()
            }
            Doc::Hex(_) => {
                let last = range.end.line.min(range.start.line + COPY_LINES);
                let mut out = String::new();
                for ix in range.start.line..=last {
                    let line = self.doc.line(ix);
                    let from = if ix == range.start.line {
                        range.start.byte.min(line.len())
                    } else {
                        0
                    };
                    let to = if ix == range.end.line {
                        range.end.byte.min(line.len())
                    } else {
                        line.len()
                    };
                    out.push_str(&line[from..to.max(from)]);
                    if ix < range.end.line {
                        out.push('\n');
                    }
                }
                out
            }
        })
    }

    /// Where the view is: the line, or the offset in a hexdump.
    fn label(&self) -> String {
        let line = (self.top.floor() as usize).min(self.doc.line_count().saturating_sub(1));
        match self.doc.as_ref() {
            Doc::Text(_) => format!(
                "line {} of {}",
                format::grouped(line as u64 + 1),
                format::grouped(self.doc.line_count() as u64)
            ),
            Doc::Hex(hex) => {
                let offset = line as u64 * hexdump::BYTES_PER_LINE as u64;
                let len = hex.bytes.len().max(1) as u64;
                format!(
                    "{offset:0digits$x} · {}%",
                    offset * 100 / len,
                    digits = self.doc.digits()
                )
            }
        }
    }

    /// The minimap's lines, drawn from the track's top at the geometry's shift.
    fn minimap(
        &self,
        g: Geometry,
        theme: Theme,
        highlight: Arc<HighlightTheme>,
        dark: bool,
    ) -> AnyElement {
        let (doc, mini, pitch) = (self.doc.clone(), self.mini.clone(), self.pitch);
        let shift = g.shift();
        let count = doc.line_count();
        let (base, muted) = (theme.text, theme.muted);
        let painted = mini.clone();
        canvas(
            move |bounds, _, _| {
                let first = ((shift / pitch.line).floor() as usize).min(count);
                let rows = (f64::from(bounds.size.height) / pitch.line).ceil() as usize + 1;
                let range = first..(first + rows).min(count);
                mini.borrow_mut().fill(range.clone(), dark, |lines| {
                    ink_lines(&doc, lines, &highlight, base, muted)
                });
                range
            },
            move |bounds, range, window, _| {
                let mini = painted.borrow();
                let height = px(pitch.ink);
                for ix in range {
                    let Some(ink) = mini.get(ix) else {
                        continue;
                    };
                    let y = bounds.top() + px(pitch.snap(ix as f64 * pitch.line - shift));
                    for &(from, to, color) in ink {
                        let x = bounds.left() + px(MINI_PAD + f32::from(from) * pitch.column);
                        let width = px(f32::from(to - from) * pitch.column);
                        window.paint_quad(fill(
                            Bounds::new(point(x, y), size(width, height)),
                            color.opacity(0.8),
                        ));
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }
}

impl Overviewed for LinesPane {
    fn overview(&mut self) -> &mut Overview {
        &mut self.overview
    }

    fn geometry(&self, track: f64) -> Geometry {
        let content = self.doc.line_count() as f64 + 1.;
        Geometry::new(content, self.rows(), self.top, self.pitch.line, track)
    }

    fn scroll_to(&mut self, top: f64, _: &mut Window, cx: &mut Context<Self>) {
        self.top = top.round();
        self.clamp();
        cx.notify();
    }
}

/// The word, run of spaces or run of punctuation around `byte`.
fn word(line: &str, byte: usize) -> Range<usize> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let byte = byte.min(line.len());
    let Some(at) = line[byte..]
        .chars()
        .next()
        .or_else(|| line[..byte].chars().next_back())
    else {
        return byte..byte;
    };
    let kind = class(at);
    let start = line[..byte]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| class(c) == kind)
        .last()
        .map_or(byte, |(i, _)| i);
    let end = line[byte..]
        .char_indices()
        .find(|&(_, c)| class(c) != kind)
        .map_or(line.len(), |(i, _)| byte + i);
    start..end.max(start)
}

/// Runs for a line: the highlighter's styles over the base, positions moved
/// to where the line is drawn.
fn runs(
    shown: &Shown,
    styles: &[(Range<usize>, HighlightStyle)],
    base: &TextStyle,
) -> Vec<TextRun> {
    let len = shown.text.len();
    let mut runs = Vec::with_capacity(styles.len() * 2 + 1);
    let mut at = 0;
    for (range, style) in styles {
        let start = shown.to_shown(range.start).max(at);
        let end = shown.to_shown(range.end);
        if end <= start {
            continue;
        }
        if start > at {
            runs.push(base.to_run(start - at));
        }
        runs.push(base.clone().highlight(*style).to_run(end - start));
        at = end;
    }
    if at < len {
        runs.push(base.to_run(len - at));
    }
    runs
}

/// A hexdump line in three tones: the offset and empty bytes quiet, and the
/// dots that stand for unprintable bytes quiet too.
fn hex_runs(line: &str, digits: usize, base: &TextStyle, muted: Hsla) -> Vec<TextRun> {
    let quiet = TextStyle {
        color: muted,
        ..base.clone()
    };
    let (hex, bar) = hexdump::columns(digits);
    let bytes = line.as_bytes();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut push = |len: usize, quiet_run: bool| {
        let style = if quiet_run { &quiet } else { base };
        match runs.last_mut() {
            Some(last) if last.color == style.color => last.len += len,
            _ => runs.push(style.to_run(len)),
        }
    };
    if bytes.len() <= bar {
        // The closing offset.
        push(bytes.len(), true);
        return runs;
    }
    push(hex, true);
    let mut at = hex;
    for slot in 0..hexdump::BYTES_PER_LINE {
        if slot == 8 {
            push(1, true);
            at += 1;
        }
        let cell = &bytes[at..at + 3];
        push(3, cell.starts_with(b"00") || cell[0] == b' ');
        at += 3;
    }
    push(bar - at, true);
    for (j, &b) in bytes.iter().enumerate().skip(bar) {
        let edge = j == bar || j + 1 == bytes.len();
        push(1, edge || b == b'.');
    }
    runs
}

/// Ink for a line: each run of visible characters as columns, coloured by
/// `spans` (byte ranges of the line) or else `base`. Spaces leave gaps.
fn ink(line: &str, spans: &[(Range<usize>, Hsla)], base: Hsla) -> Ink {
    let mut out = Ink::new();
    let mut column = 0;
    let mut span = 0;
    for (i, c) in line.char_indices() {
        if column >= MINI_COLUMNS {
            break;
        }
        let width = match c {
            '\t' => TAB - column % TAB,
            c => c.width().unwrap_or(0),
        };
        if width > 0 && !c.is_whitespace() {
            while span < spans.len() && spans[span].0.end <= i {
                span += 1;
            }
            let color = spans
                .get(span)
                .filter(|(range, _)| range.start <= i)
                .map_or(base, |(_, color)| *color);
            let (from, to) = (column as u16, (column + width).min(MINI_COLUMNS) as u16);
            match out.last_mut() {
                Some(last) if last.1 == from && last.2 == color => last.1 = to,
                _ => out.push((from, to, color)),
            }
        }
        column += width;
    }
    out
}

/// The start of `line` that the minimap can show, cut at a character.
fn head(text: &str, line: &Range<usize>) -> Range<usize> {
    let mut end = line.end.min(line.start + MINI_COLUMNS * 4);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    line.start..end
}

/// Ink for lines `range`: highlighted text asks the highlighter once for
/// all of them, unless they are long.
fn ink_lines(
    doc: &Doc,
    range: Range<usize>,
    highlight: &HighlightTheme,
    base: Hsla,
    muted: Hsla,
) -> Vec<Ink> {
    let text = match doc {
        Doc::Hex(_) => {
            let style = TextStyle {
                color: base,
                ..TextStyle::default()
            };
            return range
                .map(|ix| {
                    let line = doc.line(ix);
                    let mut at = 0;
                    let spans: Vec<_> = hex_runs(&line, doc.digits(), &style, muted)
                        .into_iter()
                        .map(|run| {
                            at += run.len;
                            (at - run.len..at, run.color)
                        })
                        .collect();
                    ink(&line, &spans, base)
                })
                .collect();
        }
        Doc::Text(text) => text,
    };
    let Some(highlighter) = &text.highlighter else {
        return range
            .map(|ix| ink(&text.text[head(&text.text, &text.lines[ix])], &[], base))
            .collect();
    };
    let heads: Vec<Range<usize>> = range.map(|ix| head(&text.text, &text.lines[ix])).collect();
    let (Some(first), Some(last)) = (heads.first(), heads.last()) else {
        return Vec::new();
    };
    let span = first.start..last.end;
    let styles = if span.len() <= MINI_SPAN {
        highlighter.styles(&span, highlight)
    } else {
        heads
            .iter()
            .flat_map(|line| highlighter.styles(line, highlight))
            .collect()
    };
    let mut at = 0;
    heads
        .iter()
        .map(|line| {
            while at < styles.len() && styles[at].0.end <= line.start {
                at += 1;
            }
            let spans: Vec<_> = styles[at..]
                .iter()
                .take_while(|(range, _)| range.start < line.end)
                .map(|(range, style)| {
                    let start = range.start.max(line.start) - line.start;
                    let end = range.end.min(line.end) - line.start;
                    (start..end, style.color.unwrap_or(base))
                })
                .collect();
            ink(&text.text[line.clone()], &spans, base)
        })
        .collect()
}

struct Drawn {
    first: usize,
    lines: Vec<(ShapedLine, Shown)>,
    numbers: Vec<ShapedLine>,
}

impl Render for LinesPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window.appearance());
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let highlight: Arc<HighlightTheme> = if dark {
            HighlightTheme::default_dark()
        } else {
            HighlightTheme::default_light()
        };
        let doc = self.doc.clone();
        let count = doc.line_count();
        let top = self.top;
        let scroll_x = self.scroll_x;
        let selection = self.selection.as_ref().map(Selection::range);
        let flash = self.flash.filter(|(_, at)| at.elapsed() < FLASH);
        let gutter = match doc.as_ref() {
            Doc::Text(_) => px(count.to_string().len() as f32 * 8.4 + 22.),
            Doc::Hex(_) => px(PAD_SIDE),
        };
        let (area, advance) = (self.area.clone(), self.advance.clone());
        let view = cx.entity().downgrade();
        let mut base = window.text_style();
        base.font_family = "JetBrains Mono".into();
        base.font_features = FontFeatures::disable_ligatures();
        base.font_size = px(FONT_SIZE).into();
        base.color = theme.text;
        let muted = theme.muted;

        let prepaint = {
            let (doc, area, advance, base, highlight) = (
                doc.clone(),
                area.clone(),
                advance.clone(),
                base.clone(),
                highlight.clone(),
            );
            move |bounds: Bounds<Pixels>, window: &mut Window, _: &mut App| {
                let text_area = Bounds::from_corners(
                    point(bounds.left() + gutter, bounds.top()),
                    bounds.bottom_right(),
                );
                area.set(text_area);
                let system = window.text_system().clone();
                let zero = system.shape_line("0".into(), px(FONT_SIZE), &[base.to_run(1)], None);
                advance.set(zero.width());
                let first = top.floor() as usize;
                let rows = (f32::from(bounds.size.height) / LINE_HEIGHT).ceil() as usize + 1;
                let last = (first + rows).min(count);
                let mut lines = Vec::with_capacity(last.saturating_sub(first));
                let mut numbers = Vec::new();
                let quiet = TextStyle {
                    color: muted.opacity(0.7),
                    ..base.clone()
                };
                for ix in first..last {
                    let source = doc.line(ix);
                    let shown = Shown::new(&source);
                    let runs = match doc.as_ref() {
                        Doc::Text(text) => match &text.highlighter {
                            Some(highlighter) if !source.is_empty() => {
                                let line = text.lines[ix].clone();
                                let end = line.start + shown.len;
                                let styles: Vec<_> = highlighter
                                    .styles(&(line.start..end), highlight.as_ref())
                                    .into_iter()
                                    .map(|(r, style)| {
                                        (
                                            r.start.saturating_sub(line.start)..r.end - line.start,
                                            style,
                                        )
                                    })
                                    .collect();
                                runs(&shown, &styles, &base)
                            }
                            _ => vec![base.to_run(shown.text.len())],
                        },
                        Doc::Hex(_) => hex_runs(&shown.text, doc.digits(), &base, muted),
                    };
                    let shaped = system.shape_line(shown.text.clone(), px(FONT_SIZE), &runs, None);
                    if let Doc::Text(_) = doc.as_ref() {
                        let number: SharedString = (ix + 1).to_string().into();
                        let run = quiet.to_run(number.len());
                        numbers.push(system.shape_line(number, px(FONT_SIZE), &[run], None));
                    }
                    lines.push((shaped, shown));
                }
                Drawn {
                    first,
                    lines,
                    numbers,
                }
            }
        };

        let paint = move |bounds: Bounds<Pixels>,
                          drawn: Drawn,
                          window: &mut Window,
                          cx: &mut App| {
            let text_area = area.get();
            let line_height = px(LINE_HEIGHT);
            let y_of =
                |ix: usize| bounds.top() + px(PAD_TOP) + line_height * ((ix as f64 - top) as f32);
            if let Some((line, at)) = flash {
                // Fades out over the flash, eased so it lingers first.
                let left = 1. - at.elapsed().as_secs_f32() / FLASH.as_secs_f32();
                let band = Bounds::new(
                    point(bounds.left(), y_of(line)),
                    size(bounds.size.width, line_height),
                );
                window.paint_quad(fill(band, progress().opacity(0.3 * left.max(0.).sqrt())));
                window.request_animation_frame();
            }
            window.with_content_mask(Some(ContentMask { bounds: text_area }), |window| {
                for (i, (shaped, shown)) in drawn.lines.iter().enumerate() {
                    let ix = drawn.first + i;
                    let origin = point(text_area.left() - scroll_x, y_of(ix));
                    if let Some(range) = &selection
                        && (range.start.line..=range.end.line).contains(&ix)
                        && range.start != range.end
                    {
                        let from = if ix == range.start.line {
                            shaped.x_for_index(shown.to_shown(range.start.byte))
                        } else {
                            px(0.)
                        };
                        let mut to = if ix == range.end.line {
                            shaped.x_for_index(shown.to_shown(range.end.byte))
                        } else {
                            shaped.width()
                        };
                        if ix < range.end.line {
                            to += advance.get() * 0.6;
                        }
                        if to > from {
                            window.paint_quad(fill(
                                Bounds::from_corners(
                                    point(origin.x + from, origin.y),
                                    point(origin.x + to, origin.y + line_height),
                                ),
                                theme.selection,
                            ));
                        }
                    }
                    let _ = shaped.paint(origin, line_height, TextAlign::Left, None, window, cx);
                }
            });
            for (i, number) in drawn.numbers.iter().enumerate() {
                let x = bounds.left() + gutter - px(14.) - number.width();
                let origin = point(x, y_of(drawn.first + i));
                let _ = number.paint(origin, line_height, TextAlign::Left, None, window, cx);
            }
            // Listened to on every frame: a quick click can be released
            // before the frame after its press is drawn.
            let moved = view.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    let _ = moved.update(cx, |this, cx| this.drag(event.position, window, cx));
                }
            });
            let released = view.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    let _ = released.update(cx, |this, cx| this.release(cx));
                }
            });
        };

        self.height = layout::above_row(window).height;
        self.pitch = Pitch::at(window.scale_factor());
        let scrollable = self.geometry(1.).scrollable();
        let column = scrollable.then(|| {
            let track = f32::from(self.height);
            let shown = match self.overview.shown(scrollable) {
                OverviewShown::Panel => {
                    let g = self.geometry(f64::from(track));
                    let content = self.minimap(g, theme, highlight, dark);
                    Some(overview::panel(&self.overview, g, content, cx))
                }
                OverviewShown::Bar => {
                    let g = self.geometry(overview::bar_track(track));
                    Some(overview::scrollbar(&self.overview, g, theme, cx))
                }
                OverviewShown::Nothing => None,
            };
            let width = self.overview.mode.column(self.pitch.width());
            overview::column(width, Some(theme.rule), shown, cx)
        });
        div()
            .id("lines")
            .size_full()
            .flex()
            .flex_row()
            .bg(theme.surface)
            .on_scroll_wheel(
                cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.scroll(event, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.press(event, window, cx)
                        }),
                    )
                    .child(canvas(prepaint, paint).size_full()),
            )
            .children(column)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Hsla, TextStyle, rgb};
    use silent::hexdump;

    use super::{MINI_COLUMNS, Mini, Pitch, SHOWN, Shown, hex_runs, ink, scrolls, word};

    #[test]
    fn lines_scroll_once_they_outgrow_the_view() {
        // 964 px hold 47.5 lines, and the view scrolls a line past the last.
        assert!(!scrolls(46, 964.));
        assert!(scrolls(47, 964.));
        // Files short enough get a window that holds them, and no minimap.
        for lines in 1..=44 {
            let height = lines.max(8) as f32 * 21. + 40.;
            assert!(!scrolls(lines, height), "{lines}");
        }
    }

    #[test]
    fn tabs_stretch_to_the_next_stop_and_map_back() {
        let shown = Shown::new("a\tbc\td");
        assert_eq!(shown.text.as_ref(), "a   bc  d");
        assert_eq!(shown.to_shown(0), 0);
        assert_eq!(shown.to_shown(2), 4);
        assert_eq!(shown.to_shown(5), 8);
        assert_eq!(shown.to_source(4), 2);
        assert_eq!(shown.to_source(8), 5);
        // Inside a tab: the nearer side.
        assert_eq!(shown.to_source(2), 1);
        assert_eq!(shown.to_source(3), 2);
    }

    #[test]
    fn long_lines_are_cut_where_they_are_drawn() {
        let long = "x".repeat(SHOWN + 50);
        let shown = Shown::new(&long);
        assert_eq!(shown.text.len(), SHOWN);
        assert_eq!(shown.len, SHOWN);
        assert_eq!(shown.to_source(SHOWN), SHOWN);
    }

    #[test]
    fn hexdump_runs_cover_their_lines() {
        let base = TextStyle::default();
        let dumps = [b"\0abc".to_vec(), vec![0; 20], vec![1; 32]];
        for bytes in &dumps {
            for ix in 0..hexdump::line_count(bytes.len() as u64) {
                let line = hexdump::line(bytes, ix, 8);
                let runs = hex_runs(&line, 8, &base, Hsla::default());
                assert_eq!(
                    runs.iter().map(|r| r.len).sum::<usize>(),
                    line.len(),
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn words_are_runs_of_one_kind() {
        let line = "let total_size = a.len();";
        assert_eq!(&line[word(line, 6)], "total_size");
        assert_eq!(&line[word(line, 4)], "total_size");
        assert_eq!(&line[word(line, 3)], " ");
        assert_eq!(&line[word(line, line.len())], "();");
        assert_eq!(word("", 0), 0..0);
        let chinese = "床前明月光，疑是";
        assert_eq!(&chinese[word(chinese, 3)], "床前明月光");
    }

    #[test]
    fn ink_follows_words_colours_and_columns() {
        let (base, keyword): (Hsla, Hsla) = (rgb(0x111111).into(), rgb(0xaa0000).into());
        // `fn` in its colour, a gap, then the rest in the base colour.
        let ink = ink("fn\tmain() 中", &[(0..2, keyword)], base);
        assert_eq!(
            ink.as_slice(),
            [(0, 2, keyword), (4, 10, base), (11, 13, base)]
        );
        let long = "y".repeat(500);
        assert_eq!(
            super::ink(&long, &[], base).as_slice(),
            [(0, MINI_COLUMNS as u16, base)]
        );
    }

    #[test]
    fn the_minimap_draws_on_whole_device_pixels() {
        let device = |pitch: Pitch| {
            let close = |v: f64| (v - v.round()).abs() < 1e-4;
            let line = pitch.line * f64::from(pitch.scale);
            let column = f64::from(pitch.column * pitch.scale);
            let ink = f64::from(pitch.ink * pitch.scale);
            assert!(close(line) && close(column) && close(ink));
            (line.round(), column.round(), ink.round())
        };
        assert_eq!(device(Pitch::at(1.)), (2., 1., 1.));
        assert_eq!(device(Pitch::at(1.7)), (3., 2., 2.));
        assert_eq!(device(Pitch::at(2.)), (4., 2., 3.));
        let pitch = Pitch::at(1.7);
        assert!((pitch.snap(10.3) * 1.7 - 18.).abs() < 1e-4);
    }

    #[test]
    fn the_minimap_keeps_lines_near_the_view() {
        let made = std::cell::RefCell::new(Vec::new());
        let make = |range: std::ops::Range<usize>| {
            made.borrow_mut().push(range.clone());
            range.map(|_| super::Ink::new()).collect()
        };
        let mut mini = Mini::default();
        mini.fill(100..200, false, make);
        mini.fill(110..210, false, make);
        // Only the ten new lines were made.
        assert_eq!(*made.borrow(), [100..200, 200..210]);
        assert!(mini.get(105).is_some() && mini.get(209).is_some());
        mini.fill(5000..5100, false, make);
        assert_eq!(made.borrow().last(), Some(&(5000..5100)));
        assert!(mini.get(105).is_none());
        // The appearance changed: everything is made again.
        mini.fill(5000..5100, true, make);
        assert_eq!(made.borrow().len(), 4);
    }
}
