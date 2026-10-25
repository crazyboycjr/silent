//! Text, code and hexdumps. Only the lines in view are shaped and drawn,
//! straight from their source, so a 2 MB file or a dump of gigabytes opens
//! at once. Long lines scroll sideways rather than wrap, and text can be
//! selected with the mouse and copied.

use std::borrow::Cow;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::*;
use silent::{format, hexdump};
use smallvec::SmallVec;

use crate::content::{Hex, TAB, Text, columns};
use crate::scrub::{self, Scrubbed, Scrubber};
use crate::theme::Theme;

const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT: f32 = 20.;
const PAD_TOP: f32 = 14.;
const PAD_SIDE: f32 = 18.;
/// Bytes of a line that are drawn; the rest of a minified line is only
/// copied.
const SHOWN: usize = 4000;
/// A copy of a hexdump stops after this many lines (about 20 MB of text).
const COPY_LINES: usize = 1 << 18;

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

pub struct LinesPane {
    doc: Rc<Doc>,
    /// The first line in view, with the fraction of it scrolled past.
    top: f64,
    scroll_x: Pixels,
    selection: Option<Selection>,
    dragging: Option<Unit>,
    /// Where the lines were last drawn, for the pointer.
    area: Rc<Cell<Bounds<Pixels>>>,
    advance: Rc<Cell<Pixels>>,
    scrubber: Scrubber,
}

impl LinesPane {
    pub fn new(doc: Doc) -> Self {
        Self {
            doc: Rc::new(doc),
            top: 0.,
            scroll_x: px(0.),
            selection: None,
            dragging: None,
            area: Rc::default(),
            advance: Rc::new(Cell::new(px(7.5))),
            scrubber: Scrubber::default(),
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

    fn font() -> Font {
        Font {
            features: FontFeatures::disable_ligatures(),
            ..font("JetBrains Mono")
        }
    }

    fn rows(&self) -> f64 {
        let height = self.area.get().size.height;
        f64::from(((height - px(PAD_TOP)) / px(LINE_HEIGHT)).max(1.))
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

    pub fn scroll_by_page(&mut self, down: bool, cx: &mut Context<Self>) {
        let page = (self.rows() - 2.).max(1.);
        self.top += if down { page } else { -page };
        self.clamp();
        cx.notify();
    }

    pub fn scroll_to_end(&mut self, end: bool, cx: &mut Context<Self>) {
        self.top = if end { self.max_top() } else { 0. };
        self.clamp();
        cx.notify();
    }

    pub fn set_shift(&mut self, held: bool, cx: &mut Context<Self>) {
        if self.scrubber.held != held {
            self.scrubber.held = held;
            cx.notify();
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(LINE_HEIGHT));
        self.top -= f64::from(delta.y / LINE_HEIGHT);
        self.scroll_x -= delta.x;
        self.clamp();
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
}

impl Scrubbed for LinesPane {
    fn scrubber(&mut self) -> &mut Scrubber {
        &mut self.scrubber
    }

    fn scrub_to(&mut self, fraction: f32, _: &mut Window, cx: &mut Context<Self>) {
        self.top = (f64::from(fraction) * self.max_top()).round();
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

struct Drawn {
    first: usize,
    lines: Vec<(ShapedLine, Shown)>,
    numbers: Vec<ShapedLine>,
}

impl Render for LinesPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window.appearance());
        let highlight: Arc<HighlightTheme> = match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => {
                HighlightTheme::default_dark()
            }
            _ => HighlightTheme::default_light(),
        };
        let doc = self.doc.clone();
        let count = doc.line_count();
        let top = self.top;
        let scroll_x = self.scroll_x;
        let selection = self.selection.as_ref().map(Selection::range);
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
            let (doc, area, advance, base) =
                (doc.clone(), area.clone(), advance.clone(), base.clone());
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

        let scrubber = self.scrubber.shown().then(|| {
            let fraction = match self.max_top() {
                0. => 0.,
                max => (self.top / max) as f32,
            };
            scrub::track(&self.scrubber, fraction, self.label(), theme, cx)
        });
        div()
            .id("lines")
            .size_full()
            .relative()
            .bg(theme.surface)
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.press(event, window, cx)
                }),
            )
            .on_scroll_wheel(
                cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.scroll(event, cx)),
            )
            .child(canvas(prepaint, paint).size_full())
            .children(scrubber)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Hsla, TextStyle};
    use silent::hexdump;

    use super::{SHOWN, Shown, hex_runs, word};

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
}
