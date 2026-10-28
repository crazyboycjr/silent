//! The card of keys that ? shows over the content: the keys of the file in
//! view first, then silent's general keys, then Nautilus's, in sections.
//! Keys are drawn as keycaps, letters as vim writes them, as typed: j is
//! the key, J is Shift with it, with no Shift cap of its own.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use silent::markup::Markup;

use crate::theme::Theme;
use crate::{overview, strip};

/// What the window shows, as far as its keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Text,
    Hexdump,
    /// Markdown or HTML shown as its source.
    Source(Markup),
    Rendered(Markup),
    Pdf,
    Slides,
    Document,
    Sheet {
        tabs: bool,
    },
    Archive,
    Video,
    Audio,
    Picture,
    Folder,
    /// A card, or a picture that does not turn, such as an iWork preview.
    Card,
}

impl Subject {
    pub fn zooms(self) -> bool {
        matches!(
            self,
            Subject::Text
                | Subject::Hexdump
                | Subject::Source(_)
                | Subject::Rendered(_)
                | Subject::Document
        )
    }

    fn title(self) -> &'static str {
        match self {
            Subject::Text => "Keys for text and code",
            Subject::Hexdump => "Keys for binary files",
            Subject::Source(Markup::Markdown) | Subject::Rendered(Markup::Markdown) => {
                "Keys for Markdown"
            }
            Subject::Source(Markup::Html) | Subject::Rendered(Markup::Html) => "Keys for HTML",
            Subject::Pdf => "Keys for PDFs",
            Subject::Slides => "Keys for slides",
            Subject::Document => "Keys for documents",
            Subject::Sheet { .. } => "Keys for spreadsheets",
            Subject::Archive => "Keys for archives",
            Subject::Video => "Keys for videos",
            Subject::Audio => "Keys for audio",
            Subject::Picture => "Keys for pictures",
            Subject::Folder => "Keys for folders",
            Subject::Card => "Keys",
        }
    }

    /// What a click puts the keys on, for the rows that follow it.
    fn after_a_click(self) -> Line {
        Note(match self {
            Subject::Pdf | Subject::Document => "After a click on the page",
            Subject::Slides => "After a click on the slide",
            Subject::Sheet { .. } => "After a click on a cell",
            Subject::Archive => "After a click on the list",
            Subject::Video => "After a click on the video",
            Subject::Audio => "After a click on the card",
            _ => "After a click on the text",
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Shown {
    pub subject: Subject,
    /// Shown over the archive it was taken out of.
    pub inside: bool,
    /// Nautilus's own preview, whose arrows, F2 and Delete act on its files.
    pub nautilus: bool,
}

/// A row of keys and what they do, a row of a key that goes through modes,
/// named in their order under it, or a few words on the rows after it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    Keys(&'static str, &'static str),
    Modes(&'static str, &'static str, &'static [&'static str]),
    Note(&'static str),
}

use Line::{Keys, Modes, Note};

struct Section {
    title: &'static str,
    lines: Vec<Line>,
}

/// The modes m and i go through, in their order, as the info line names
/// them when the keys are pressed.
const MINIMAP: [&str; 3] = [
    overview::Mode::Auto.name(),
    overview::Mode::On.name(),
    overview::Mode::Off.name(),
];
const INFO_LINE: [&str; 3] = [
    strip::Mode::Auto.name(),
    strip::Mode::Off.name(),
    strip::Mode::On.name(),
];

const SCREEN: Line = Keys("Shift+↑ ↓", "Scroll a screen up or down");
const PAGE_KEYS: Line = Keys("PgUp / PgDn", "Scroll a screen up or down");
const HALF_SCREEN: [Line; 2] = [
    Keys("Ctrl+u / d", "Scroll half a screen up or down"),
    Keys("Ctrl+b / f", "Scroll a screen up or down"),
];

fn sections(shown: Shown) -> Vec<Section> {
    let subject = shown.subject;
    let mut out = Vec::new();
    match subject {
        Subject::Video | Subject::Audio => out.push(playback(subject)),
        Subject::Archive => out.push(contents()),
        Subject::Pdf => out.push(pages(false)),
        Subject::Slides => out.push(pages(true)),
        Subject::Sheet { tabs } => out.push(sheets(tabs)),
        _ => {}
    }
    out.push(view(subject));
    out.extend(navigation(subject));
    out.extend(selection(subject));
    out.extend(vim(subject));
    out.push(general(shown));
    if shown.nautilus {
        out.push(nautilus(shown));
    }
    out
}

fn playback(subject: Subject) -> Section {
    Section {
        title: "Playback",
        lines: vec![
            Keys("k", "Pause or play"),
            Keys("j / l", "Skip back or forward 10 seconds"),
            Keys("Shift+← →", "Skip back or forward 30 seconds"),
            Keys("m", "Mute or unmute"),
            subject.after_a_click(),
            Keys("← →", "Skip back or forward 5 seconds"),
            Keys("↑ ↓", "Turn the volume up or down"),
            Keys("@Click", "Pause or play"),
        ],
    }
}

fn contents() -> Section {
    Section {
        title: "Contents",
        lines: vec![
            Subject::Archive.after_a_click(),
            Keys("↑ ↓", "Move the selection up or down"),
            Keys("← →", "Close or open the folder"),
            Keys("Enter / Space", "Preview the selected file"),
        ],
    }
}

fn pages(slides: bool) -> Section {
    let (title, step, ends, subject) = if slides {
        (
            "Slides",
            "Go to the previous or next slide",
            "Go to the first or the last slide",
            Subject::Slides,
        )
    } else {
        (
            "Pages",
            "Go to the previous or next page",
            "Go to the first or the last page",
            Subject::Pdf,
        )
    };
    Section {
        title,
        lines: vec![
            PAGE_KEYS,
            Keys("Shift+↑ ↓", step),
            Keys("Home / End", ends),
            subject.after_a_click(),
            Keys("← →", step),
            Keys("↑ ↓", "Scroll up or down"),
        ],
    }
}

fn sheets(tabs: bool) -> Section {
    let mut lines = Vec::new();
    if tabs {
        lines.push(Keys("Shift+← →", "Go to the previous or next sheet"));
    }
    lines.extend([
        PAGE_KEYS,
        SCREEN,
        Keys("Home / End", "Go to the first or the last row"),
        Subject::Sheet { tabs }.after_a_click(),
        Keys("← → ↑ ↓", "Move the cell cursor"),
    ]);
    Section {
        title: "Sheets",
        lines,
    }
}

fn view(subject: Subject) -> Section {
    let mut lines = Vec::new();
    match subject {
        Subject::Text | Subject::Hexdump | Subject::Source(_) => {
            lines.push(Modes("m", "Show the minimap", &MINIMAP))
        }
        Subject::Document => lines.push(Keys("m", "Show or hide the minimap")),
        Subject::Pdf | Subject::Slides => lines.push(Keys("m", "Show or hide the thumbnails")),
        _ => {}
    }
    lines.push(Modes("i", "Show the info line", &INFO_LINE));
    if matches!(subject, Subject::Text | Subject::Source(_)) {
        lines.push(Keys("w", "Wrap or unwrap long lines"));
    }
    match subject {
        Subject::Source(_) => lines.push(Keys("s", "Show the rendered page")),
        Subject::Rendered(_) => lines.push(Keys("s", "Show the source")),
        _ => {}
    }
    if matches!(subject, Subject::Picture | Subject::Pdf | Subject::Video) {
        lines.push(Keys("r / R", "Rotate clockwise or counterclockwise"));
    }
    if subject.zooms() {
        lines.extend([
            Keys("Ctrl+Shift+=", "Make the text bigger"),
            Keys("Ctrl+−", "Make the text smaller"),
            Keys("Ctrl+0", "Reset the text size"),
            Keys("Ctrl+@Scroll", "Make the text bigger or smaller"),
        ]);
    }
    Section {
        title: "View",
        lines,
    }
}

fn navigation(subject: Subject) -> Option<Section> {
    let sideways = match subject {
        Subject::Text | Subject::Hexdump | Subject::Source(_) => true,
        Subject::Rendered(_) | Subject::Document => false,
        _ => return None,
    };
    let mut lines = vec![PAGE_KEYS, SCREEN];
    if sideways {
        lines.push(Keys("Shift+← →", "Scroll half a screen sideways"));
    }
    lines.extend([
        Keys("Home / End", "Go to the start or the end"),
        subject.after_a_click(),
        Keys("↑ ↓", "Scroll a line up or down"),
    ]);
    if sideways {
        lines.push(Keys("← →", "Scroll left or right"));
    }
    Some(Section {
        title: "Navigation",
        lines,
    })
}

fn selection(subject: Subject) -> Option<Section> {
    let lines = match subject {
        Subject::Text | Subject::Hexdump | Subject::Source(_) | Subject::Rendered(_) => vec![
            Keys("@Drag", "Select text"),
            Keys("Ctrl+a", "Select all"),
            Keys("Ctrl+c", "Copy the selected text"),
        ],
        Subject::Pdf => vec![
            Keys("@Drag", "Select text"),
            Keys("Ctrl+c", "Copy the selected text"),
        ],
        _ => return None,
    };
    Some(Section {
        title: "Selection",
        lines,
    })
}

fn vim(subject: Subject) -> Option<Section> {
    let ends = [
        Keys("g g", "Go to the start"),
        Keys("G", "Go to the end"),
    ];
    let mut lines = match subject {
        Subject::Text | Subject::Hexdump | Subject::Source(_) => vec![
            Keys("k / j", "Scroll a line up or down"),
            Keys("h / l", "Scroll left or right"),
            Keys("0", "Go back to the left edge"),
        ],
        Subject::Rendered(_) | Subject::Document => {
            vec![Keys("k / j", "Scroll a line up or down")]
        }
        Subject::Pdf => vec![Keys("k / j", "Go to the previous or next page")],
        Subject::Slides => vec![Keys("k / j", "Go to the previous or next slide")],
        _ => return None,
    };
    lines.extend(HALF_SCREEN);
    lines.extend(match subject {
        Subject::Pdf => [
            Keys("g g", "Go to the first page"),
            Keys("G", "Go to the last page"),
        ],
        Subject::Slides => [
            Keys("g g", "Go to the first slide"),
            Keys("G", "Go to the last slide"),
        ],
        _ => ends,
    });
    lines.extend(match subject {
        Subject::Rendered(_) => None,
        Subject::Document => Some(Keys("2 0 G", "Go to paragraph 20")),
        Subject::Pdf => Some(Keys("2 0 G", "Go to page 20")),
        Subject::Slides => Some(Keys("2 0 G", "Go to slide 20")),
        _ => Some(Keys("2 0 G", "Go to line 20")),
    });
    Some(Section {
        title: "Vim keys",
        lines,
    })
}

fn general(shown: Shown) -> Section {
    let mut lines = vec![
        Keys("f", "Enter or leave fullscreen"),
        Keys("?", "Show or hide this help"),
    ];
    if shown.inside {
        lines.push(Keys("Space", "Go back to the archive"));
        lines.push(Keys("Esc", "Close everything"));
    } else {
        lines.push(Keys("Space / Esc", "Close"));
        // Nautilus opens a folder itself; elsewhere a folder stays put.
        if !shown.nautilus && shown.subject != Subject::Folder {
            lines.push(Keys("Enter", "Open in its app"));
        }
    }
    Section {
        title: "General",
        lines,
    }
}

fn nautilus(shown: Shown) -> Section {
    let mut lines = vec![Keys("← → ↑ ↓", "Move through the files")];
    if !shown.inside {
        lines.push(if shown.subject == Subject::Folder {
            Keys("Enter", "Open the folder")
        } else {
            Keys("Enter", "Open in its app")
        });
        lines.push(Keys("F2", "Rename"));
        lines.push(Keys("Delete", "Move to the trash"));
    }
    Section {
        title: "Nautilus",
        lines,
    }
}

/// A part of a row's keys as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// A key, as a keycap.
    Cap(&'static str),
    /// Between keys held together.
    Plus,
    /// Between two ways of doing the same.
    Or,
    /// What the mouse does: `Click`, `Scroll`, `Drag`.
    Mouse(&'static str),
}

/// `Shift+← →` as tokens: keys side by side are pressed one after the
/// other (`g g`), or are arrows; `+` joins keys held together; `/` stands
/// between two ways; `@` marks what the mouse does.
fn tokens(keys: &'static str) -> Vec<Token> {
    let mut out = Vec::new();
    for word in keys.split(' ') {
        if word == "/" {
            out.push(Token::Or);
            continue;
        }
        for (i, part) in word.split('+').enumerate() {
            if i > 0 {
                out.push(Token::Plus);
            }
            out.push(match part.strip_prefix('@') {
                Some(mouse) => Token::Mouse(mouse),
                None => Token::Cap(part),
            });
        }
    }
    out
}

const FACE: &str = "Noto Sans";
/// The face of a cap of one character, where l, I and 1 look apart.
const CAP_FACE: &str = "JetBrains Mono";
const TITLE: f32 = 17.;
const TITLE_LINE: f32 = 22.;
const HEADER: f32 = 12.;
const HEADER_LINE: f32 = 18.;
const TEXT: f32 = 13.;
const SMALL: f32 = 12.;
const SMALL_LINE: f32 = 18.;
/// A row: its keycaps, and beside them the words, in lines as tall as a cap.
const ROW: f32 = 26.;
const CAP: f32 = 20.;
/// A cap of one character, letter or arrow, all as wide.
const CAP_ONE: f32 = 22.;
const CAP_TEXT: f32 = 11.5;
const CAP_PAD: f32 = 5.;
const ARROW: f32 = 12.;
const CAP_GAP: f32 = 3.;
const JOIN_PAD: f32 = 3.;
/// The arrow between two modes, and the room either side of it.
const MODE_ARROW: f32 = 9.;
const MODE_ARROW_PAD: f32 = 2.;
/// The share of the room beside a dock the card may take.
const SHARE: f32 = 0.96;
/// Between the keys and what they do, and between two columns.
const GAP: f32 = 12.;
const GUTTER: f32 = 28.;
/// The card's margins: the title's and the last row's own room above and
/// below them make the four edges look alike.
const PAD_X: f32 = 24.;
const PAD_TOP: f32 = 19.;
const PAD_BOTTOM: f32 = 21.;
const UNDER_TITLE: f32 = 12.;
const UNDER_HEADER: f32 = 4.;
const SECTION_GAP: f32 = 10.;
const NOTE_ABOVE: f32 = 4.;
/// Measured widths and the laid-out text differ by a fraction of a pixel.
const SLACK: f32 = 4.;
/// Rows a card has at least before it goes in two columns, and the words
/// a narrow column leaves room for, wrapping longer ones.
const TWO_COLUMNS_FROM: usize = 12;
const NARROW_WORDS: f32 = 180.;

/// How wide `text` is drawn.
fn width(window: &Window, text: &str, size: f32, weight: FontWeight) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: Font {
            weight,
            ..font(FACE)
        },
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(SharedString::from(text.to_owned()), px(size), &[run], None);
    f32::from(line.width())
}

/// The arrows are drawn, as the glyphs of the faces at hand are small and
/// sit low.
fn arrow(label: &str) -> Option<IconName> {
    Some(match label {
        "←" => IconName::ArrowLeft,
        "→" => IconName::ArrowRight,
        "↑" => IconName::ArrowUp,
        "↓" => IconName::ArrowDown,
        _ => return None,
    })
}

fn cap_width(window: &Window, label: &'static str) -> f32 {
    if label.chars().count() == 1 {
        return CAP_ONE;
    }
    (width(window, label, CAP_TEXT, FontWeight::MEDIUM) + 2. * CAP_PAD + 2.).max(CAP_ONE)
}

/// How wide a row's keys are drawn.
fn keys_width(window: &Window, keys: &[Token]) -> f32 {
    let join = width(window, "+", CAP_TEXT, FontWeight::NORMAL) + 2. * JOIN_PAD;
    let mut total = 0.;
    for (i, token) in keys.iter().enumerate() {
        let side_by_side = i > 0 && matches!(keys[i - 1], Token::Cap(_) | Token::Mouse(_));
        let gap = if side_by_side { CAP_GAP } else { 0. };
        total += match *token {
            Token::Cap(label) => cap_width(window, label) + gap,
            Token::Plus | Token::Or => join,
            Token::Mouse(word) => width(window, word, TEXT, FontWeight::NORMAL) + gap,
        };
    }
    total
}

fn keys_element(keys: &[Token], theme: Theme) -> Div {
    let joiner = |sign: &'static str| {
        div()
            .flex_none()
            .px(px(JOIN_PAD))
            .text_size(px(CAP_TEXT))
            .text_color(theme.caption)
            .child(sign)
    };
    let mut row = div().flex_none().flex().flex_row().items_center();
    for (i, token) in keys.iter().enumerate() {
        let side_by_side = i > 0 && matches!(keys[i - 1], Token::Cap(_) | Token::Mouse(_));
        if side_by_side && matches!(token, Token::Cap(_) | Token::Mouse(_)) {
            row = row.child(div().flex_none().w(px(CAP_GAP)));
        }
        row = row.child(match *token {
            Token::Cap(label) => keycap(label, theme),
            Token::Plus => joiner("+"),
            Token::Or => joiner("/"),
            Token::Mouse(word) => div()
                .flex_none()
                .text_size(px(TEXT))
                .line_height(px(CAP))
                .text_color(theme.text)
                .child(word),
        });
    }
    row
}

/// A key as a keycap: a rounded box with a thin edge, a little thicker at
/// the bottom, on a fill a shade lighter than the card; one character to a
/// cap of one width, in a face that tells l, I and 1 apart.
fn keycap(label: &'static str, theme: Theme) -> Div {
    let one = label.chars().count() == 1;
    let cap = div()
        .flex_none()
        .h(px(CAP))
        .min_w(px(CAP_ONE))
        .when(one, |cap| cap.w(px(CAP_ONE)))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_b_2()
        .border_color(theme.key_edge)
        .bg(theme.key);
    match arrow(label) {
        Some(icon) => cap.child(
            svg()
                .path(icon.path())
                .size(px(ARROW))
                .text_color(theme.text),
        ),
        None => cap
            .when(!one, |cap| cap.px(px(CAP_PAD)))
            .when(one, |cap| cap.font_family(CAP_FACE))
            .text_size(px(CAP_TEXT))
            .line_height(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text)
            .child(label),
    }
}

/// The modes a key goes through, in their order, with an arrow before
/// each but the first, which a narrow column wraps with it.
fn modes_element(modes: &'static [&'static str], theme: Theme) -> Div {
    let modes = modes.iter().enumerate().map(move |(i, mode)| {
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .when(i > 0, |mode| {
                mode.child(
                    svg()
                        .path(IconName::ArrowRight.path())
                        .size(px(MODE_ARROW))
                        .mx(px(MODE_ARROW_PAD))
                        .text_color(theme.caption),
                )
            })
            .child(*mode)
    });
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .text_size(px(SMALL))
        .line_height(px(SMALL_LINE))
        .text_color(theme.caption)
        .children(modes)
}

/// How wide a mode line is drawn.
fn modes_width(window: &Window, modes: &[&str]) -> f32 {
    let words: f32 = modes
        .iter()
        .map(|mode| width(window, mode, SMALL, FontWeight::NORMAL))
        .sum();
    words + (modes.len().saturating_sub(1) as f32) * (MODE_ARROW + 2. * MODE_ARROW_PAD)
}

/// How tall each section is drawn with `words` wide for what keys do: for
/// splitting the sections between two columns.
fn heights(sections: &[Section], words: f32, window: &Window) -> Vec<f32> {
    let lines_of = |wide: f32| (wide / words).ceil().max(1.);
    sections
        .iter()
        .enumerate()
        .map(|(i, section)| {
            let lines: f32 = section
                .lines
                .iter()
                .map(|line| match *line {
                    Keys(_, what) => {
                        let wide = width(window, what, TEXT, FontWeight::NORMAL);
                        ROW + CAP * (lines_of(wide) - 1.)
                    }
                    Modes(_, what, modes) => {
                        let wide = width(window, what, TEXT, FontWeight::NORMAL);
                        ROW + CAP * (lines_of(wide) - 1.)
                            + SMALL_LINE * lines_of(modes_width(window, modes))
                    }
                    Note(_) => NOTE_ABOVE + SMALL_LINE,
                })
                .sum();
            let rule = if i == 0 { 0. } else { 2. * SECTION_GAP + 1. };
            rule + HEADER_LINE + UNDER_HEADER + lines
        })
        .collect()
}

/// Where to split sections of `heights` between two columns, in order, so
/// that the taller column is as short as it can be.
fn split(heights: &[f32]) -> usize {
    let total: f32 = heights.iter().sum();
    let mut best = (f32::MAX, 1);
    let mut left = 0.;
    for (i, h) in heights.iter().enumerate().take(heights.len().saturating_sub(1)) {
        left += h;
        let taller = left.max(total - left);
        if taller < best.0 {
            best = (taller, i + 1);
        }
    }
    best.1
}

fn section_element(section: &Section, first: bool, key_width: f32, theme: Theme) -> Div {
    let header = div()
        .pb(px(UNDER_HEADER))
        .text_size(px(HEADER))
        .line_height(px(HEADER_LINE))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.caption)
        .child(section.title);
    let row = move |keys: &'static str, what: &'static str, modes: Option<&'static [&str]>| {
        div()
            .flex()
            .flex_row()
            .items_start()
            .py(px((ROW - CAP) / 2.))
            .child(
                div()
                    .flex_none()
                    .w(px(key_width))
                    .child(keys_element(&tokens(keys), theme)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl(px(GAP))
                    .text_size(px(TEXT))
                    .line_height(px(CAP))
                    .text_color(theme.text)
                    .child(what)
                    .children(modes.map(|modes| modes_element(modes, theme))),
            )
    };
    let lines = section.lines.iter().map(move |line| match *line {
        Keys(keys, what) => row(keys, what, None),
        Modes(keys, what, modes) => row(keys, what, Some(modes)),
        Note(words) => div()
            .pt(px(NOTE_ABOVE))
            .text_size(px(SMALL))
            .line_height(px(SMALL_LINE))
            .text_color(theme.caption)
            .child(words),
    });
    div()
        .flex()
        .flex_col()
        .when(!first, |section| {
            section
                .mt(px(SECTION_GAP))
                .pt(px(SECTION_GAP))
                .border_t_1()
                .border_color(theme.rule)
        })
        .child(header)
        .children(lines)
}

/// The card for `shown`, in one column, or in two when there are many rows
/// and the room beside the dock is wide enough for them, each as wide as it
/// needs, or both as wide, the words wrapping, in a narrow window rather
/// than the card scrolling.
pub fn card(shown: Shown, theme: Theme, room: Size<Pixels>, window: &Window) -> Stateful<Div> {
    let sections = sections(shown);
    let rows: usize = sections.iter().map(|s| s.lines.len()).sum();
    // The keys' column of each column of the card is as wide as its widest.
    let key_width_of = |sections: &[Section]| {
        sections
            .iter()
            .flat_map(|s| s.lines.iter())
            .filter_map(|line| match *line {
                Keys(keys, _) | Modes(keys, ..) => Some(keys_width(window, &tokens(keys))),
                Note(_) => None,
            })
            .fold(0., f32::max)
            .ceil()
    };
    // The width a column of `sections` takes with no word wrapped.
    let natural_of = |sections: &[Section]| {
        let key_width = key_width_of(sections);
        let words = sections
            .iter()
            .flat_map(|s| s.lines.iter())
            .map(|line| match *line {
                Keys(_, what) => width(window, what, TEXT, FontWeight::NORMAL),
                Modes(_, what, modes) => {
                    width(window, what, TEXT, FontWeight::NORMAL).max(modes_width(window, modes))
                }
                Note(words) => width(window, words, SMALL, FontWeight::NORMAL) - key_width - GAP,
            })
            .fold(0., f32::max);
        (key_width + GAP + words + SLACK).ceil()
    };
    let key_width = key_width_of(&sections);
    let natural = natural_of(&sections);
    let room = f32::from(room.width) * SHARE - 2. * PAD_X - 2.;
    let two = (rows >= TWO_COLUMNS_FROM).then(|| {
        let at = split(&heights(&sections, f32::MAX, window));
        let (left, right) = (natural_of(&sections[..at]), natural_of(&sections[at..]));
        if left + right + GUTTER <= room {
            return Some((at, left, right));
        }
        let narrow = ((room - GUTTER) / 2.).floor();
        let at = split(&heights(&sections, narrow - key_width - GAP, window));
        (narrow - key_width - GAP >= NARROW_WORDS).then_some((at, narrow, narrow))
    });
    let title = div()
        .pb(px(UNDER_TITLE))
        .text_size(px(TITLE))
        .line_height(px(TITLE_LINE))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .child(shown.subject.title());
    let column_of = |sections: &[Section], width: Option<f32>| {
        let key_width = key_width_of(sections);
        div()
            .flex()
            .flex_col()
            .when_some(width, |column, width| column.flex_none().w(px(width)))
            .children(
                sections
                    .iter()
                    .enumerate()
                    .map(|(i, s)| section_element(s, i == 0, key_width, theme)),
            )
    };
    let (body, width) = match two.flatten() {
        Some((at, left, right)) => {
            let body = div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(GUTTER))
                .child(column_of(&sections[..at], Some(left)))
                .child(column_of(&sections[at..], Some(right)));
            (body, left + right + GUTTER)
        }
        None => (column_of(&sections, None), natural),
    };
    div()
        .id("help-card")
        .w(px(width + 2. * PAD_X + 2.))
        .max_w(relative(SHARE))
        .max_h(relative(SHARE))
        .overflow_y_scroll()
        .px(px(PAD_X))
        .pt(px(PAD_TOP))
        .pb(px(PAD_BOTTOM))
        .rounded(px(10.))
        .bg(theme.surface)
        .border_1()
        .border_color(theme.key_edge)
        .font_family(FACE)
        .child(title)
        .child(body)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` attribute would come with it.
    use silent::markup::Markup;

    use super::{
        INFO_LINE, Keys, MINIMAP, Modes, Note, Shown, Subject, Token, sections, split, tokens,
    };
    use crate::{overview, strip};

    fn every_subject() -> Vec<Subject> {
        vec![
            Subject::Text,
            Subject::Hexdump,
            Subject::Source(Markup::Markdown),
            Subject::Rendered(Markup::Html),
            Subject::Pdf,
            Subject::Slides,
            Subject::Document,
            Subject::Sheet { tabs: true },
            Subject::Sheet { tabs: false },
            Subject::Archive,
            Subject::Video,
            Subject::Audio,
            Subject::Picture,
            Subject::Folder,
            Subject::Card,
        ]
    }

    fn keys_of(shown: Shown) -> Vec<&'static str> {
        sections(shown)
            .into_iter()
            .flat_map(|s| s.lines)
            .filter_map(|line| match line {
                Keys(keys, _) | Modes(keys, ..) => Some(keys),
                Note(_) => None,
            })
            .collect()
    }

    fn words_of(shown: Shown) -> Vec<&'static str> {
        sections(shown)
            .into_iter()
            .flat_map(|s| s.lines)
            .map(|line| match line {
                Keys(_, words) | Modes(_, words, _) | Note(words) => words,
            })
            .collect()
    }

    fn shown(subject: Subject) -> Shown {
        Shown {
            subject,
            inside: false,
            nautilus: true,
        }
    }

    #[test]
    fn tokens_are_caps_joined_by_plus_or_slash_and_mouse_words() {
        let (cap, plus) = (Token::Cap, Token::Plus);
        assert_eq!(
            tokens("Shift+← →"),
            [cap("Shift"), plus, cap("←"), cap("→")]
        );
        assert_eq!(
            tokens("g g / G"),
            [cap("g"), cap("g"), Token::Or, cap("G")]
        );
        assert_eq!(tokens("Ctrl+u / d"), [cap("Ctrl"), plus, cap("u"), Token::Or, cap("d")]);
        assert_eq!(
            tokens("Ctrl+@Scroll"),
            [cap("Ctrl"), plus, Token::Mouse("Scroll")]
        );
        assert_eq!(
            tokens("2 0 G"),
            [cap("2"), cap("0"), cap("G")]
        );
    }

    #[test]
    fn alternatives_have_a_slash_and_side_by_side_is_a_sequence_or_arrows() {
        for subject in every_subject() {
            for keys in keys_of(shown(subject)) {
                let tokens = tokens(keys);
                for pair in tokens.windows(2) {
                    if let [Token::Cap(a), Token::Cap(b)] = pair {
                        let arrows = [*a, *b].iter().all(|k| "←→↑↓".contains(*k));
                        // g g, and a count typed before a key: 2 0 G.
                        let sequence = a == b || a.parse::<u8>().is_ok();
                        assert!(arrows || sequence, "{a} {b} in {keys}");
                    }
                }
            }
        }
    }

    #[test]
    fn letters_are_as_typed_and_named_keys_are_spelled_one_way() {
        let named = [
            "Shift", "Ctrl", "Esc", "Space", "Enter", "Delete", "F2", "Home", "End", "PgUp",
            "PgDn",
        ];
        // The keys that take Shift with a letter, written as the capital.
        let shifted = ["R", "G"];
        for subject in every_subject() {
            for keys in keys_of(shown(subject)) {
                let tokens = tokens(keys);
                for token in &tokens {
                    let Token::Cap(cap) = *token else { continue };
                    let letter = cap.len() == 1 && cap.chars().all(|c| c.is_ascii_alphabetic());
                    assert!(
                        !cap.chars().any(|c| c.is_ascii_alphabetic())
                            || named.contains(&cap)
                            || (letter && (cap.chars().all(|c| c.is_ascii_lowercase())
                                || shifted.contains(&cap))),
                        "{cap} in {keys}"
                    );
                }
                // No Shift cap before a letter: J is the key, not Shift+J.
                for three in tokens.windows(3) {
                    if let [Token::Cap("Shift"), Token::Plus, Token::Cap(key)] = three {
                        assert!(
                            !key.chars().any(|c| c.is_ascii_alphabetic()),
                            "Shift+{key} in {keys}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn descriptions_are_sentences_in_the_users_words_and_one_order() {
        for subject in every_subject() {
            for words in words_of(shown(subject)) {
                let first = words.chars().next().unwrap();
                assert!(first.is_uppercase(), "{words}");
                assert!(!words.ends_with('.'), "{words}");
                let lower = words.to_lowercase();
                for unwanted in [
                    "strip", "rail", "engage", "overview", "down or up", "next or previous",
                    "inside",
                ] {
                    assert!(!lower.contains(unwanted), "{words}");
                }
            }
        }
    }

    #[test]
    fn modes_are_named_and_ordered_as_the_keys_go_through_them() {
        let family = overview::Family::Minimap;
        let mut mode = overview::Mode::Auto;
        for name in MINIMAP {
            assert_eq!(mode.name(), name);
            mode = mode.next(family);
        }
        let mut mode = strip::Mode::Auto;
        for name in INFO_LINE {
            assert_eq!(mode.name(), name);
            mode = mode.next();
        }
    }

    #[test]
    fn keys_that_do_nothing_for_the_file_are_left_out() {
        let has = |subject, keys: &str| keys_of(shown(subject)).contains(&keys);
        // Only text wraps; only pictures, PDFs and video turn.
        assert!(has(Subject::Text, "w") && !has(Subject::Hexdump, "w"));
        assert!(has(Subject::Picture, "r / R") && !has(Subject::Card, "r / R"));
        assert!(has(Subject::Video, "r / R") && !has(Subject::Audio, "r / R"));
        assert!(!has(Subject::Slides, "r / R"));
        // The source of Markdown, not of plain text.
        assert!(has(Subject::Rendered(Markup::Markdown), "s") && !has(Subject::Text, "s"));
        // Sideways only where lines can be wider than the window.
        assert!(has(Subject::Text, "h / l") && !has(Subject::Rendered(Markup::Html), "h / l"));
        // The text size where there is text to size.
        assert!(has(Subject::Document, "Ctrl+0") && !has(Subject::Pdf, "Ctrl+0"));
        // A workbook of one sheet has no sheets to go to.
        assert!(has(Subject::Sheet { tabs: true }, "Shift+← →"));
        assert!(!has(Subject::Sheet { tabs: false }, "Shift+← →"));
        // Page keys wherever they scroll.
        for subject in [Subject::Text, Subject::Pdf, Subject::Sheet { tabs: false }] {
            assert!(has(subject, "PgUp / PgDn"), "{subject:?}");
        }
        assert!(!has(Subject::Picture, "PgUp / PgDn") && !has(Subject::Video, "PgUp / PgDn"));
        // Vim's keys where they move.
        assert!(!has(Subject::Archive, "k / j") && !has(Subject::Sheet { tabs: true }, "k / j"));
    }

    #[test]
    fn the_files_own_keys_come_first_and_nautilus_last() {
        let titles = |subject| -> Vec<&str> {
            sections(shown(subject)).iter().map(|s| s.title).collect()
        };
        assert_eq!(
            titles(Subject::Text),
            ["View", "Navigation", "Selection", "Vim keys", "General", "Nautilus"]
        );
        assert_eq!(titles(Subject::Video), ["Playback", "View", "General", "Nautilus"]);
        assert_eq!(titles(Subject::Archive), ["Contents", "View", "General", "Nautilus"]);
        assert_eq!(
            titles(Subject::Pdf),
            ["Pages", "View", "Selection", "Vim keys", "General", "Nautilus"]
        );
        assert_eq!(titles(Subject::Picture), ["View", "General", "Nautilus"]);
    }

    #[test]
    fn nautilus_keys_only_for_its_own_preview() {
        let mut yazi = shown(Subject::Text);
        yazi.nautilus = false;
        let keys = keys_of(yazi);
        assert!(!keys.contains(&"F2") && !keys.contains(&"← → ↑ ↓"));
        assert!(keys.contains(&"Enter"));
        // A file taken out of an archive is Nautilus's only as its archive.
        let inside = Shown {
            subject: Subject::Text,
            inside: true,
            nautilus: true,
        };
        let keys = keys_of(inside);
        assert!(keys.contains(&"Space") && keys.contains(&"Esc"));
        assert!(!keys.contains(&"Enter") && !keys.contains(&"Delete"));
        assert!(keys.contains(&"← → ↑ ↓"));
    }

    #[test]
    fn columns_split_where_they_come_out_even() {
        assert_eq!(split(&[4., 4., 4., 4.]), 2);
        assert_eq!(split(&[10., 2., 3., 4.]), 1);
        assert_eq!(split(&[2., 3., 4., 10.]), 3);
    }
}
