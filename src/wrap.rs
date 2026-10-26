//! Soft wrapping: where the lines of a text break into rows at a width, and
//! the rows of a whole file, kept as the first row of each line and the
//! starts of the rows after it. Rows break where a word starts, or beside
//! East Asian characters, as long as the word fits on a row of its own;
//! a longer one breaks anywhere. Spaces at a row's end hang past its edge
//! rather than start the next row, and punctuation keeps to its word.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

/// How wide characters are drawn, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    /// A column: every narrow character, in a monospaced face.
    pub column: f32,
    /// East Asian wide characters, as the face they fall back to draws them.
    pub wide: f32,
    /// Emoji, which are also two columns.
    pub emoji: f32,
    /// A tab stretches to the next multiple of this many columns.
    pub tab: usize,
}

impl Measure {
    /// The width of `c` at `column` columns into its row, and its columns.
    fn of(&self, c: char, column: usize) -> (f32, usize) {
        if c == '\t' {
            let n = self.tab - column % self.tab;
            return (n as f32 * self.column, n);
        }
        match c.width().unwrap_or(0) {
            0 => (0., 0),
            1 => (self.column, 1),
            n if emoji(c) => (self.emoji, n),
            n => (self.wide, n),
        }
    }

    fn run(&self, text: &str) -> (f32, usize) {
        text.chars().fold((0., 0), |(x, column), c| {
            let (w, n) = self.of(c, column);
            (x + w, column + n)
        })
    }

    /// The most a byte can take outside a tab: a wide character has three
    /// bytes or more, an emoji four.
    fn per_byte(&self) -> f32 {
        self.column.max(self.wide / 3.).max(self.emoji / 4.)
    }
}

fn emoji(c: char) -> bool {
    matches!(u32::from(c), 0x1f000..=0x1faff | 0x2600..=0x27bf | 0x2b00..=0x2bff)
}

fn wide(c: char) -> bool {
    !c.is_ascii() && c.width() == Some(2)
}

/// Punctuation that does not start a row.
fn closing(c: char) -> bool {
    matches!(
        c,
        ',' | '.'
            | ';'
            | ':'
            | '?'
            | '!'
            | ')'
            | ']'
            | '}'
            | '，'
            | '。'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '）'
            | '］'
            | '｝'
            | '》'
            | '〉'
            | '」'
            | '』'
            | '】'
            | '〕'
            | '〗'
            | '”'
            | '’'
            | '…'
            | '～'
    )
}

/// Punctuation that does not end a row.
fn opening(c: char) -> bool {
    matches!(
        c,
        '(' | '['
            | '{'
            | '（'
            | '［'
            | '｛'
            | '《'
            | '〈'
            | '「'
            | '『'
            | '【'
            | '〔'
            | '〖'
            | '“'
            | '‘'
    )
}

/// Whether a row may start at `c`, after `before`.
fn may_break(before: char, c: char) -> bool {
    if c.is_whitespace() {
        return false;
    }
    before.is_whitespace() || ((wide(before) || wide(c)) && !closing(c) && !opening(before))
}

/// Pushes where each row of `line` after its first starts, in bytes, for
/// rows `width` pixels wide.
pub fn breaks(line: &str, width: f32, measure: &Measure, out: &mut Vec<u32>) {
    let mut start = 0;
    let (mut x, mut column) = (0., 0);
    // The last place after the row's start where it may break.
    let mut chance = None;
    let mut before = None;
    for (i, c) in line.char_indices() {
        if i > start && before.is_some_and(|b| may_break(b, c)) {
            chance = Some(i);
        }
        let (w, n) = measure.of(c, column);
        if x + w > width && i > start && !c.is_whitespace() {
            let at = chance.take().unwrap_or(i);
            out.push(at as u32);
            start = at;
            (x, column) = measure.run(&line[at..i]);
        }
        // Only a tab's width depends on its column, and a tab never breaks.
        x += w;
        column += n;
        before = Some(c);
    }
}

/// The rows a text's lines wrap into at one width.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// The width the rows were made for.
    pub width: f32,
    /// The first row of each line, and how many rows there are last.
    first: Vec<u32>,
    /// Where each row after a line's first starts, in bytes into its line,
    /// line after line.
    starts: Vec<u32>,
}

impl Layout {
    /// `lines` of `text` in rows `width` pixels wide. Only lines that may
    /// be too wide are measured.
    pub fn new(text: &str, lines: &[Range<usize>], width: f32, measure: &Measure) -> Self {
        let fits = (width / measure.per_byte()) as usize;
        let mut first = Vec::with_capacity(lines.len() + 1);
        let mut starts = Vec::new();
        for (ix, line) in lines.iter().enumerate() {
            first.push((ix + starts.len()) as u32);
            let line = &text[line.clone()];
            if line.len() > fits || line.contains('\t') {
                breaks(line, width, measure, &mut starts);
            }
        }
        first.push((lines.len() + starts.len()) as u32);
        Self {
            width,
            first,
            starts,
        }
    }

    pub fn len(&self) -> usize {
        self.first.last().map_or(0, |&n| n as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lines(&self) -> usize {
        self.first.len().saturating_sub(1)
    }

    /// The first row of `line`.
    pub fn row_of(&self, line: usize) -> usize {
        self.first[line.min(self.lines())] as usize
    }

    /// How many rows `line` takes.
    pub fn rows_of(&self, line: usize) -> usize {
        (self.first[line + 1] - self.first[line]) as usize
    }

    /// The line `row` is in, and which of its rows it is; past the last
    /// row, the line after the last.
    pub fn locate(&self, row: usize) -> (usize, usize) {
        if row >= self.len() {
            return (self.lines(), 0);
        }
        let line = self.first.partition_point(|&f| f as usize <= row) - 1;
        (line, row - self.first[line] as usize)
    }

    /// The starts of the rows of `line` after its first.
    fn starts_of(&self, line: usize) -> &[u32] {
        let extra = self.first[line] as usize - line;
        &self.starts[extra..extra + self.rows_of(line) - 1]
    }

    /// The bytes of row `k` of `line`: up to where the next row starts, or
    /// for its last row, to `usize::MAX`, the line's end.
    pub fn span(&self, line: usize, k: usize) -> Range<usize> {
        let starts = self.starts_of(line);
        let start = k.checked_sub(1).map_or(0, |i| starts[i] as usize);
        start..starts.get(k).map_or(usize::MAX, |&s| s as usize)
    }

    /// Which row of `line` holds `byte`.
    pub fn row_at(&self, line: usize, byte: usize) -> usize {
        self.starts_of(line)
            .partition_point(|&s| s as usize <= byte)
    }

    /// A top in rows as a line, with how far through its rows the top is:
    /// what the view memory keeps, which a view of another width reads
    /// back about right.
    pub fn logical(&self, top: f64) -> f64 {
        let row = top.floor().max(0.);
        let (line, k) = self.locate(row as usize);
        if line >= self.lines() {
            return self.lines() as f64;
        }
        line as f64 + (k as f64 + top - row) / self.rows_of(line) as f64
    }

    pub fn from_logical(&self, logical: f64) -> f64 {
        let line = logical.floor().max(0.);
        if line as usize >= self.lines() {
            return self.len() as f64;
        }
        let within = logical - line;
        self.row_of(line as usize) as f64 + within * self.rows_of(line as usize) as f64
    }
}

/// What is at the top of the view, whether the lines wrap or not: the row
/// of a line that starts at a byte, and how much of the row is scrolled
/// past. Wrapping, unwrapping or wrapping at another width keeps it there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub line: usize,
    pub byte: usize,
    pub past: f64,
}

impl Anchor {
    /// At `top`: rows down `layout`, or lines down `lines` unwrapped lines.
    pub fn of(top: f64, layout: Option<&Layout>, lines: usize) -> Self {
        let row = top.floor().max(0.);
        let past = top.max(0.) - row;
        match layout {
            Some(layout) => {
                let (line, k) = layout.locate(row as usize);
                if line >= layout.lines() {
                    return Self {
                        line: layout.lines().saturating_sub(1),
                        byte: 0,
                        past: 0.,
                    };
                }
                Self {
                    line,
                    byte: layout.span(line, k).start,
                    past,
                }
            }
            None => Self {
                line: (row as usize).min(lines.saturating_sub(1)),
                byte: 0,
                past,
            },
        }
    }

    /// The top that shows it at the top again. Unwrapped, the line itself
    /// comes to the top, whole unless the anchor was its first row.
    pub fn top(self, layout: Option<&Layout>) -> f64 {
        match layout {
            Some(layout) if self.line < layout.lines() => {
                (layout.row_of(self.line) + layout.row_at(self.line, self.byte)) as f64 + self.past
            }
            Some(layout) => layout.len() as f64,
            None if self.byte == 0 => self.line as f64 + self.past,
            None => self.line as f64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widths in columns: wide characters two, emoji two.
    const COLUMNS: Measure = Measure {
        column: 1.,
        wide: 2.,
        emoji: 2.,
        tab: 4,
    };

    /// `line` broken into rows `width` columns wide.
    fn rows(line: &str, width: f32) -> Vec<&str> {
        rows_by(line, width, &COLUMNS)
    }

    fn rows_by<'a>(line: &'a str, width: f32, measure: &Measure) -> Vec<&'a str> {
        let mut starts = Vec::new();
        breaks(line, width, measure, &mut starts);
        let mut cuts = vec![0];
        cuts.extend(starts.iter().map(|&s| s as usize));
        cuts.push(line.len());
        cuts.windows(2).map(|w| &line[w[0]..w[1]]).collect()
    }

    #[test]
    fn rows_break_where_words_start() {
        assert_eq!(
            rows("the quick brown fox", 10.),
            ["the quick ", "brown fox"]
        );
        assert_eq!(rows("short", 10.), ["short"]);
        assert_eq!(rows("", 10.), [""]);
        // Exactly full: no empty row after it.
        assert_eq!(rows("0123456789", 10.), ["0123456789"]);
        assert_eq!(
            rows("one two three four five six", 9.),
            ["one two ", "three ", "four five ", "six"]
        );
    }

    #[test]
    fn a_word_longer_than_a_row_breaks_anywhere() {
        assert_eq!(rows("abcdefghijkl", 5.), ["abcde", "fghij", "kl"]);
        // It starts a row of its own first, as a word that fits would.
        assert_eq!(rows("ab cdefghijkl", 5.), ["ab ", "cdefg", "hijkl"]);
        // A row too narrow for a single wide character still moves on.
        assert_eq!(rows("中文", 1.), ["中", "文"]);
    }

    #[test]
    fn spaces_hang_past_the_edge() {
        assert_eq!(rows("ab      cd", 4.), ["ab      ", "cd"]);
        assert_eq!(rows("word   ", 4.), ["word   "]);
    }

    #[test]
    fn chinese_breaks_between_characters_but_not_before_its_commas() {
        let line = "床前明月光，疑是地上霜。";
        // Five characters to a row, but the comma would start the second,
        // so 光 goes down with it.
        assert_eq!(rows(line, 10.), ["床前明月", "光，疑是地", "上霜。"]);
        assert_eq!(rows("“引号”里的字", 6.), ["“引号”", "里的字"]);
        // Latin after Chinese may start a row, and a word that fits keeps whole.
        assert_eq!(rows("中文English", 6.), ["中文", "Englis", "h"]);
        assert_eq!(rows("中文 a word", 7.), ["中文 a ", "word"]);
    }

    #[test]
    fn tabs_stretch_from_the_start_of_their_row() {
        // A tab takes the columns to the next stop of four.
        assert_eq!(rows("ab\tcd", 6.), ["ab\tcd"]);
        assert_eq!(rows("ab\tcde", 6.), ["ab\t", "cde"]);
        // On a row of its own, the tab stretches from that row's start.
        assert_eq!(rows("abcdef\tg\th", 8.), ["abcdef\t", "g\th"]);
    }

    #[test]
    fn widths_come_from_the_measure() {
        // Chinese as the fallback face draws it, narrower than two columns.
        let measure = Measure {
            column: 7.5,
            wide: 12.5,
            emoji: 15.,
            tab: 4,
        };
        let line = "一二三四五六七八九十";
        assert_eq!(
            rows_by(line, 50., &measure),
            ["一二三四", "五六七八", "九十"]
        );
        assert_eq!(rows_by("😀😀😀😀", 45., &measure), ["😀😀😀", "😀"]);
        assert_eq!(rows_by("abcdefgh", 45., &measure), ["abcdef", "gh"]);
    }

    fn layout(text: &str, width: f32) -> (Layout, Vec<Range<usize>>) {
        let mut lines = Vec::new();
        let mut start = 0;
        for line in text.split('\n') {
            lines.push(start..start + line.len());
            start += line.len() + 1;
        }
        (Layout::new(text, &lines, width, &COLUMNS), lines)
    }

    #[test]
    fn rows_map_to_lines_and_back() {
        let text = "short\nthe quick brown fox jumps\n\nabcdefghijklmnopqrstuvwxy\nend";
        let (layout, _) = layout(text, 10.);
        // short | the quick , brown fox , jumps | (empty) | abcdefghij , klmnopqrst , uvwxy | end
        assert_eq!(layout.len(), 9);
        assert_eq!(
            (0..5).map(|l| layout.row_of(l)).collect::<Vec<_>>(),
            [0, 1, 4, 5, 8]
        );
        assert_eq!(
            (0..5).map(|l| layout.rows_of(l)).collect::<Vec<_>>(),
            [1, 3, 1, 3, 1]
        );
        assert_eq!(layout.locate(0), (0, 0));
        assert_eq!(layout.locate(3), (1, 2));
        assert_eq!(layout.locate(4), (2, 0));
        assert_eq!(layout.locate(7), (3, 2));
        assert_eq!(layout.locate(8), (4, 0));
        // Past the end: the line after the last.
        assert_eq!(layout.locate(9), (5, 0));
        assert_eq!(layout.span(1, 0), 0..10);
        assert_eq!(layout.span(1, 1), 10..20);
        assert_eq!(layout.span(1, 2), 20..usize::MAX);
        assert_eq!(layout.span(0, 0), 0..usize::MAX);
        assert_eq!(layout.row_at(1, 0), 0);
        assert_eq!(layout.row_at(1, 9), 0);
        assert_eq!(layout.row_at(1, 10), 1);
        assert_eq!(layout.row_at(1, 24), 2);
        assert_eq!(layout.row_at(3, 25), 2);
    }

    #[test]
    fn lines_that_surely_fit_are_not_measured_but_come_out_the_same() {
        let text = "a\tb\nabcdefghij\nabcdefghijk\n中文中文中\n中文中文中文";
        let (layout, lines) = layout(text, 10.);
        for (ix, line) in lines.iter().enumerate() {
            let mut starts = Vec::new();
            breaks(&text[line.clone()], 10., &COLUMNS, &mut starts);
            assert_eq!(layout.rows_of(ix), starts.len() + 1, "line {ix}");
        }
        assert_eq!(layout.rows_of(2), 2);
        assert_eq!(layout.rows_of(4), 2);
    }

    #[test]
    fn a_toggle_keeps_the_line_at_the_top() {
        let text = "intro\nthe quick brown fox jumps over the lazy dog\nmiddle\nlast";
        let (wrapped, _) = layout(text, 10.);
        let lines = 4;
        // Unwrapped, line 1 at the top with a quarter of it scrolled past.
        let anchor = Anchor::of(1.25, None, lines);
        let top = anchor.top(Some(&wrapped));
        assert_eq!(top, 1.25);
        assert_eq!(wrapped.locate(top as usize), (1, 0));
        // Back again: the same line, as it was.
        assert_eq!(Anchor::of(top, Some(&wrapped), lines).top(None), 1.25);
        // From further down the wrapped line, unwrapping shows that line whole.
        let down = Anchor::of(3.5, Some(&wrapped), lines);
        assert_eq!((down.line, down.byte), (1, 20));
        assert_eq!(down.top(None), 1.);
        // At another width, the row that holds the same words comes to the top.
        let (narrow, _) = layout(text, 6.);
        let top = down.top(Some(&narrow));
        let (line, k) = narrow.locate(top as usize);
        assert_eq!(line, 1);
        assert!(narrow.span(1, k).contains(&20), "{:?}", narrow.span(1, k));
        assert_eq!(top.fract(), 0.5);
    }

    #[test]
    fn the_view_memory_reads_back_through_the_line() {
        let text = "intro\nthe quick brown fox jumps over the lazy dog\nend";
        let (layout, _) = layout(text, 10.);
        // Row 3 is the third of line 1's five rows.
        assert_eq!(layout.rows_of(1), 5);
        let logical = layout.logical(3.);
        assert!((logical - 1.4).abs() < 1e-9);
        assert!((layout.from_logical(logical) - 3.).abs() < 1e-9);
        // Unwrapped, the same number is a line and a fraction of it.
        assert_eq!(layout.from_logical(2.), layout.row_of(2) as f64);
        assert_eq!(layout.from_logical(99.), layout.len() as f64);
    }
}
