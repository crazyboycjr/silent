//! The card of keys that ? shows over the content: the keys of the file in
//! view first, then silent's general keys, then Nautilus's, in sections.
//! Keys are drawn as keycaps, letters as capitals as a keyboard prints
//! them, and a key that needs Shift with a Shift cap of its own.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use silent::markup::Markup;

use crate::theme::Theme;

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
            Subject::Hexdump => "Keys for hexdumps",
            Subject::Source(Markup::Markdown) | Subject::Rendered(Markup::Markdown) => {
                "Keys for Markdown"
            }
            Subject::Source(Markup::Html) | Subject::Rendered(Markup::Html) => "Keys for HTML",
            Subject::Pdf => "Keys for PDFs",
            Subject::Slides => "Keys for slides",
            Subject::Document => "Keys for documents",
            Subject::Sheet { .. } => "Keys for spreadsheets",
            Subject::Archive => "Keys for archives",
            Subject::Video => "Keys for video",
            Subject::Audio => "Keys for sound",
            Subject::Picture => "Keys for pictures",
            Subject::Folder => "Keys for folders",
            Subject::Card => "Keys",
        }
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

/// A row of keys and what they do, a row of a key that cycles through
/// modes, named under it, or a few words on the rows after it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    Keys(&'static str, &'static str),
    Modes(&'static str, &'static str, &'static str),
    Note(&'static str),
}

use Line::{Keys, Modes, Note};

struct Section {
    title: &'static str,
    lines: Vec<Line>,
}

const AFTER_A_CLICK: Line = Note("After a click inside");
const INFO_LINE: Line = Modes("I", "Show the info line", "while moving, never or always");
const TURN: Line = Keys("R / Shift+R", "Turn a quarter clockwise or back");
const TEXT_SIZE: [Line; 4] = [
    Keys("Ctrl+Shift+=", "Make the text bigger"),
    Keys("Ctrl+−", "Make the text smaller"),
    Keys("Ctrl+0", "Reset the text size"),
    Keys("Ctrl+wheel", "Make the text bigger or smaller"),
];
const SCREEN: Line = Keys("Shift+↑ ↓", "Scroll a screen up or down");
const HALF_SCREEN: [Line; 2] = [
    Keys("Ctrl+D U", "Scroll half a screen down or up"),
    Keys("Ctrl+F B", "Scroll a screen down or up"),
];

fn sections(shown: Shown) -> Vec<Section> {
    let subject = shown.subject;
    let mut out = Vec::new();
    match subject {
        Subject::Video | Subject::Audio => out.push(playback()),
        Subject::Archive => out.push(entries()),
        Subject::Pdf => out.push(pages(false)),
        Subject::Slides => out.push(pages(true)),
        Subject::Sheet { tabs } => out.push(sheets(tabs)),
        _ => {}
    }
    out.push(view(subject));
    out.extend(moving(subject));
    out.extend(vim(subject));
    out.extend(copying(subject));
    out.push(general(shown));
    if shown.nautilus {
        out.push(nautilus(shown));
    }
    out
}

fn playback() -> Section {
    Section {
        title: "Playback",
        lines: vec![
            Keys("K", "Pause or play"),
            Keys("J L", "Skip back or forward 10 seconds"),
            Keys("Shift+← →", "Skip back or forward 30 seconds"),
            Keys("M", "Mute or unmute"),
            AFTER_A_CLICK,
            Keys("← →", "Skip back or forward 5 seconds"),
            Keys("↑ ↓", "Turn the volume up or down"),
            Keys("click", "Pause or play"),
        ],
    }
}

fn entries() -> Section {
    Section {
        title: "Entries",
        lines: vec![
            Note("After a click inside the list"),
            Keys("↑ ↓", "Move up or down"),
            Keys("← →", "Close or open the folder"),
            Keys("Enter Space", "Show the file over the archive"),
        ],
    }
}

fn pages(slides: bool) -> Section {
    let (title, step, ends) = if slides {
        (
            "Slides",
            "Go to the previous or next slide",
            "Go to the first or the last slide",
        )
    } else {
        (
            "Pages",
            "Go to the previous or next page",
            "Go to the first or the last page",
        )
    };
    Section {
        title,
        lines: vec![
            Keys("M", "Show or hide the thumbnails"),
            Keys("Shift+↑ ↓", step),
            Keys("Home End", ends),
            AFTER_A_CLICK,
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
        SCREEN,
        Keys("Home End", "Go to the first or the last row"),
        Note("After a click on a cell"),
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
        Subject::Text | Subject::Hexdump | Subject::Source(_) => lines.push(Modes(
            "M",
            "Show the minimap",
            "while moving, always or never",
        )),
        Subject::Document => lines.push(Keys("M", "Show or hide the minimap")),
        _ => {}
    }
    lines.push(INFO_LINE);
    if matches!(subject, Subject::Text | Subject::Source(_)) {
        lines.push(Keys("W", "Wrap or unwrap long lines"));
    }
    match subject {
        Subject::Source(_) => lines.push(Keys("S", "Show the rendered page")),
        Subject::Rendered(_) => lines.push(Keys("S", "Show the source")),
        _ => {}
    }
    if matches!(subject, Subject::Picture | Subject::Pdf | Subject::Video) {
        lines.push(TURN);
    }
    if subject.zooms() {
        lines.extend(TEXT_SIZE);
    }
    Section {
        title: "View",
        lines,
    }
}

fn moving(subject: Subject) -> Option<Section> {
    let sideways = match subject {
        Subject::Text | Subject::Hexdump | Subject::Source(_) => true,
        Subject::Rendered(_) | Subject::Document => false,
        _ => return None,
    };
    let mut lines = vec![SCREEN];
    if sideways {
        lines.push(Keys("Shift+← →", "Scroll half a screen sideways"));
    }
    lines.extend([
        Keys("Home End", "Go to the start or the end"),
        AFTER_A_CLICK,
        Keys("↑ ↓", "Scroll a line up or down"),
    ]);
    if sideways {
        lines.push(Keys("← →", "Scroll left or right"));
    }
    Some(Section {
        title: "Moving",
        lines,
    })
}

fn vim(subject: Subject) -> Option<Section> {
    let lines = match subject {
        Subject::Text | Subject::Hexdump | Subject::Source(_) => vec![
            Keys("J K", "Scroll a line down or up"),
            Keys("H L", "Scroll left or right"),
            Keys("0", "Go back to the left edge"),
            HALF_SCREEN[0],
            HALF_SCREEN[1],
            Keys("G G / Shift+G", "Go to the start or the end"),
            Keys("2 0 Shift+G", "Go to line 20"),
        ],
        Subject::Rendered(_) => vec![
            Keys("J K", "Scroll a line down or up"),
            HALF_SCREEN[0],
            HALF_SCREEN[1],
            Keys("G G / Shift+G", "Go to the start or the end"),
        ],
        Subject::Document => vec![
            Keys("J K", "Scroll a line down or up"),
            HALF_SCREEN[0],
            HALF_SCREEN[1],
            Keys("G G / Shift+G", "Go to the start or the end"),
            Keys("2 0 Shift+G", "Go to paragraph 20"),
        ],
        Subject::Pdf => vec![
            Keys("J K", "Go to the next or previous page"),
            HALF_SCREEN[0],
            HALF_SCREEN[1],
            Keys("G G / Shift+G", "Go to the first or the last page"),
            Keys("2 0 Shift+G", "Go to page 20"),
        ],
        Subject::Slides => vec![
            Keys("J K", "Go to the next or previous slide"),
            HALF_SCREEN[0],
            HALF_SCREEN[1],
            Keys("G G / Shift+G", "Go to the first or the last slide"),
            Keys("2 0 Shift+G", "Go to slide 20"),
        ],
        _ => return None,
    };
    Some(Section {
        title: "Vim keys",
        lines,
    })
}

fn copying(subject: Subject) -> Option<Section> {
    let lines = match subject {
        Subject::Text | Subject::Hexdump | Subject::Source(_) | Subject::Rendered(_) => vec![
            Keys("drag", "Select text"),
            Keys("Ctrl+A", "Select all"),
            Keys("Ctrl+C", "Copy the selected text"),
        ],
        Subject::Pdf => vec![
            Keys("drag", "Select text"),
            Keys("Ctrl+C", "Copy the selected text"),
        ],
        _ => return None,
    };
    Some(Section {
        title: "Copying",
        lines,
    })
}

fn general(shown: Shown) -> Section {
    let mut lines = vec![
        Keys("F", "Enter or leave fullscreen"),
        Keys("?", "Show or hide this help"),
    ];
    if shown.inside {
        lines.push(Keys("Space", "Go back to the archive"));
        lines.push(Keys("Esc", "Close everything"));
    } else {
        lines.push(Keys("Space Esc", "Close"));
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
    /// What the mouse does: `click`, `wheel`.
    Mouse(&'static str),
}

/// `Shift+← →` as tokens: keys separated by spaces are pressed one after
/// the other or are alternatives, as the words say; `+` joins keys held
/// together; `/` stands between two ways; lowercase words are the mouse's.
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
            let mouse = part.len() > 1 && part.chars().all(|c| c.is_ascii_lowercase() || c == '-');
            out.push(if mouse {
                Token::Mouse(part)
            } else {
                Token::Cap(part)
            });
        }
    }
    out
}

const FACE: &str = "Noto Sans";
const TITLE: f32 = 15.;
const HEADER: f32 = 13.;
const TEXT: f32 = 13.;
const NOTE: f32 = 12.;
/// A row: its keycaps, and beside them the words, in lines as tall as a cap.
const ROW: f32 = 26.;
const CAP: f32 = 20.;
const CAP_TEXT: f32 = 11.5;
const CAP_PAD: f32 = 5.;
const ARROW: f32 = 12.;
const CAP_GAP: f32 = 3.;
const PLUS_PAD: f32 = 3.;
const OR_GAP: f32 = 12.;
/// Between the keys and what they do, and between two columns.
const GAP: f32 = 16.;
const GUTTER: f32 = 40.;
const PAD_X: f32 = 24.;
const PAD_Y: f32 = 18.;
const UNDER_TITLE: f32 = 10.;
/// A header's line, the room below it, and the room either side of the
/// rule between two sections.
const HEADER_LINE: f32 = 20.;
const UNDER_HEADER: f32 = 4.;
const SECTION_GAP: f32 = 10.;
const NOTE_LINE: f32 = 18.;
const NOTE_ABOVE: f32 = 4.;
/// Rows a card has at least before it goes in two columns, and the words
/// a narrow column leaves room for, wrapping longer ones.
const TWO_COLUMNS_FROM: usize = 12;
const NARROW_WORDS: f32 = 180.;

/// How wide `text` is drawn.
fn width(window: &Window, text: &'static str, size: f32, weight: FontWeight) -> f32 {
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
        .shape_line(SharedString::from(text), px(size), &[run], None);
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
    if arrow(label).is_some() {
        return CAP;
    }
    (width(window, label, CAP_TEXT, FontWeight::MEDIUM) + 2. * CAP_PAD + 2.).max(CAP)
}

/// How wide a row's keys are drawn.
fn keys_width(window: &Window, keys: &[Token]) -> f32 {
    let mut total = 0.;
    for (i, token) in keys.iter().enumerate() {
        let after_cap = i > 0 && matches!(keys[i - 1], Token::Cap(_) | Token::Mouse(_));
        total += match *token {
            Token::Cap(label) => cap_width(window, label) + if after_cap { CAP_GAP } else { 0. },
            Token::Plus => width(window, "+", CAP_TEXT, FontWeight::NORMAL) + 2. * PLUS_PAD,
            Token::Or => OR_GAP,
            Token::Mouse(word) => {
                width(window, word, TEXT, FontWeight::NORMAL) + if after_cap { CAP_GAP } else { 0. }
            }
        };
    }
    total
}

fn keys_element(keys: &[Token], theme: Theme) -> Div {
    let mut row = div().flex_none().flex().flex_row().items_center();
    for (i, token) in keys.iter().enumerate() {
        let after_cap = i > 0 && matches!(keys[i - 1], Token::Cap(_) | Token::Mouse(_));
        if after_cap && matches!(token, Token::Cap(_) | Token::Mouse(_)) {
            row = row.child(div().flex_none().w(px(CAP_GAP)));
        }
        row = row.child(match *token {
            Token::Cap(label) => keycap(label, theme),
            Token::Plus => div()
                .flex_none()
                .px(px(PLUS_PAD))
                .text_size(px(CAP_TEXT))
                .text_color(theme.muted)
                .child("+"),
            Token::Or => div().flex_none().w(px(OR_GAP)),
            Token::Mouse(word) => div()
                .flex_none()
                .text_size(px(TEXT))
                .text_color(theme.muted)
                .child(word),
        });
    }
    row
}

/// A key as a keycap: a rounded box with a thin edge, a little thicker at
/// the bottom, on a fill a shade lighter than the card.
fn keycap(label: &'static str, theme: Theme) -> Div {
    let cap = div()
        .flex_none()
        .h(px(CAP))
        .min_w(px(CAP))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_b_2()
        .border_color(theme.key_edge)
        .bg(theme.key);
    match arrow(label) {
        Some(icon) => cap.w(px(CAP)).child(
            svg()
                .path(icon.path())
                .size(px(ARROW))
                .text_color(theme.text),
        ),
        None => cap
            .px(px(CAP_PAD))
            .text_size(px(CAP_TEXT))
            .line_height(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text)
            .child(label),
    }
}

/// How tall each section is drawn with `words` wide for what keys do:
/// for splitting the sections between two columns.
fn heights(sections: &[Section], words: f32, window: &Window) -> Vec<f32> {
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
                        ROW + CAP * ((wide / words).ceil() - 1.).max(0.)
                    }
                    Modes(..) => ROW + NOTE_LINE,
                    Note(_) => NOTE_ABOVE + NOTE_LINE,
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
        .text_color(theme.text)
        .child(section.title);
    let row = move |keys: &'static str, what: &'static str, modes: Option<&'static str>| {
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
                    .children(modes.map(|modes| {
                        div()
                            .text_size(px(NOTE))
                            .line_height(px(NOTE_LINE))
                            .text_color(theme.muted)
                            .child(modes)
                    })),
            )
    };
    let lines = section.lines.iter().map(move |line| match *line {
        Keys(keys, what) => row(keys, what, None),
        Modes(keys, what, modes) => row(keys, what, Some(modes)),
        Note(words) => div()
            .pt(px(NOTE_ABOVE))
            .text_size(px(NOTE))
            .line_height(px(NOTE_LINE))
            .text_color(theme.muted)
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
/// and the window is wide enough for them, the words wrapping in a narrow
/// window rather than the card scrolling.
pub fn card(shown: Shown, theme: Theme, window: &Window) -> Stateful<Div> {
    let sections = sections(shown);
    let rows: Vec<Line> = sections
        .iter()
        .flat_map(|s| s.lines.iter().copied())
        .collect();
    let key_width = rows
        .iter()
        .filter_map(|line| match *line {
            Keys(keys, _) | Modes(keys, ..) => Some(keys_width(window, &tokens(keys))),
            Note(_) => None,
        })
        .fold(0., f32::max)
        .ceil();
    let words = rows
        .iter()
        .map(|line| match *line {
            Keys(_, what) => width(window, what, TEXT, FontWeight::NORMAL),
            Modes(_, what, modes) => width(window, what, TEXT, FontWeight::NORMAL)
                .max(width(window, modes, NOTE, FontWeight::NORMAL)),
            Note(words) => width(window, words, NOTE, FontWeight::NORMAL) - key_width - GAP,
        })
        .fold(0., f32::max)
        .ceil();
    let natural = key_width + GAP + words;
    let room = f32::from(window.viewport_size().width) * 0.94 - 2. * PAD_X - 2.;
    let narrow = ((room - GUTTER) / 2.).floor();
    let two = if rows.len() < TWO_COLUMNS_FROM {
        None
    } else if 2. * natural + GUTTER <= room {
        Some(natural)
    } else {
        (narrow - key_width - GAP >= NARROW_WORDS).then_some(narrow)
    };
    let title = div()
        .pb(px(UNDER_TITLE))
        .text_size(px(TITLE))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .child(shown.subject.title());
    let column_of = |sections: &[Section], width: Option<f32>| {
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
    let (body, width) = match two {
        Some(column) => {
            let at = split(&heights(&sections, column - key_width - GAP, window));
            let body = div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(GUTTER))
                .child(column_of(&sections[..at], Some(column)))
                .child(column_of(&sections[at..], Some(column)));
            (body, 2. * column + GUTTER)
        }
        None => (column_of(&sections, None), natural),
    };
    div()
        .id("help-card")
        .w(px(width + 2. * PAD_X + 2.))
        .max_w(relative(0.94))
        .max_h(relative(0.94))
        .overflow_y_scroll()
        .px(px(PAD_X))
        .pt(px(PAD_Y))
        .pb(px(PAD_Y))
        .rounded(px(10.))
        .bg(theme.surface)
        .border_1()
        .border_color(theme.rule)
        .font_family(FACE)
        .child(title)
        .child(body)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` attribute would come with it.
    use silent::markup::Markup;

    use super::{Keys, Modes, Note, Shown, Subject, Token, sections, split, tokens};

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

    fn shown(subject: Subject) -> Shown {
        Shown {
            subject,
            inside: false,
            nautilus: true,
        }
    }

    #[test]
    fn tokens_are_caps_joined_by_plus_and_mouse_words() {
        let (cap, plus) = (Token::Cap, Token::Plus);
        assert_eq!(
            tokens("Shift+← →"),
            [cap("Shift"), plus, cap("←"), cap("→")]
        );
        assert_eq!(
            tokens("G G / Shift+G"),
            [cap("G"), cap("G"), Token::Or, cap("Shift"), plus, cap("G")]
        );
        assert_eq!(
            tokens("Ctrl+wheel"),
            [cap("Ctrl"), plus, Token::Mouse("wheel")]
        );
        assert_eq!(tokens("double-click"), [Token::Mouse("double-click")]);
        assert_eq!(tokens("Ctrl+−"), [cap("Ctrl"), plus, cap("−")]);
    }

    #[test]
    fn letters_are_capitals_and_named_keys_are_spelled_one_way() {
        let named = [
            "Shift", "Ctrl", "Esc", "Space", "Enter", "Delete", "F2", "Home", "End",
        ];
        for subject in every_subject() {
            for keys in keys_of(shown(subject)) {
                for token in tokens(keys) {
                    let Token::Cap(cap) = token else { continue };
                    let letters = cap.chars().filter(char::is_ascii_alphabetic).count();
                    assert!(
                        letters == 0
                            || (letters == 1 && cap.chars().all(|c| !c.is_ascii_lowercase()))
                            || named.contains(&cap),
                        "{cap} in {keys}"
                    );
                }
            }
        }
    }

    #[test]
    fn descriptions_are_sentences_in_the_users_words() {
        for subject in every_subject() {
            for section in sections(shown(subject)) {
                for line in section.lines {
                    let (Keys(_, words) | Modes(_, words, _) | Note(words)) = line;
                    let first = words.chars().next().unwrap();
                    assert!(first.is_uppercase(), "{words}");
                    assert!(!words.ends_with('.'), "{words}");
                    let lower = words.to_lowercase();
                    for internal in ["strip", "rail", "engage", "overview"] {
                        assert!(!lower.contains(internal), "{words}");
                    }
                    // Every three-way mode is named alike.
                    if let Modes(_, _, modes) = line {
                        let mut names: Vec<&str> = modes
                            .split([',', ' '])
                            .filter(|w| !w.is_empty() && *w != "or")
                            .collect();
                        names.sort();
                        assert_eq!(names, ["always", "moving", "never", "while"], "{modes}");
                    }
                }
            }
        }
    }

    #[test]
    fn keys_that_do_nothing_for_the_file_are_left_out() {
        let has = |subject, keys: &str| keys_of(shown(subject)).contains(&keys);
        // Only text wraps; only pictures, PDFs and video turn.
        assert!(has(Subject::Text, "W") && !has(Subject::Hexdump, "W"));
        assert!(has(Subject::Picture, "R / Shift+R") && !has(Subject::Card, "R / Shift+R"));
        assert!(has(Subject::Video, "R / Shift+R") && !has(Subject::Audio, "R / Shift+R"));
        assert!(!has(Subject::Slides, "R / Shift+R"));
        // The source of Markdown, not of plain text.
        assert!(has(Subject::Rendered(Markup::Markdown), "S") && !has(Subject::Text, "S"));
        // Sideways only where lines can be wider than the window.
        assert!(has(Subject::Text, "H L") && !has(Subject::Rendered(Markup::Html), "H L"));
        // The text size where there is text to size.
        assert!(has(Subject::Document, "Ctrl+0") && !has(Subject::Pdf, "Ctrl+0"));
        // A workbook of one sheet has no sheets to go to.
        assert!(has(Subject::Sheet { tabs: true }, "Shift+← →"));
        assert!(!has(Subject::Sheet { tabs: false }, "Shift+← →"));
        // Vim's keys where they move.
        assert!(!has(Subject::Archive, "J K") && !has(Subject::Sheet { tabs: true }, "J K"));
    }

    #[test]
    fn the_files_own_keys_come_first_and_nautilus_last() {
        let titles = |subject| -> Vec<&str> {
            sections(shown(subject)).iter().map(|s| s.title).collect()
        };
        assert_eq!(
            titles(Subject::Text),
            ["View", "Moving", "Vim keys", "Copying", "General", "Nautilus"]
        );
        assert_eq!(titles(Subject::Video), ["Playback", "View", "General", "Nautilus"]);
        assert_eq!(titles(Subject::Archive), ["Entries", "View", "General", "Nautilus"]);
        assert_eq!(
            titles(Subject::Pdf),
            ["Pages", "View", "Vim keys", "Copying", "General", "Nautilus"]
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
