//! What each key does, by what the window shows and whether the content is
//! engaged. Until the user clicks inside the content, the arrow keys browse
//! Nautilus's files; after the click they act on the content, and they
//! browse again once the window loses the focus or shows another file.

use std::time::Duration;

use silent::dbus::Direction;
use silent::vim::{Keys, Motion, Outcome};

/// The kinds of view, as far as keys care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Pictures, folders and cards: nothing to act on.
    Still,
    Video,
    Audio,
    Pdf,
    Slides,
    /// Text, code and hexdumps.
    Lines,
    Doc,
    Sheet,
    Archive,
}

impl Kind {
    /// Whether a click makes the arrows act on the content.
    pub fn engages(self) -> bool {
        self != Kind::Still
    }

    fn vim(self) -> bool {
        matches!(self, Kind::Lines | Kind::Doc | Kind::Pdf | Kind::Slides)
    }

    fn media(self) -> bool {
        matches!(self, Kind::Video | Kind::Audio)
    }
}

/// What a key asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Not a key of silent's.
    Pass,
    /// Taken, with nothing to do, so that it does not browse either.
    Nothing,
    /// Esc: every preview goes, and the window with them.
    Close,
    /// Space: the top preview goes, showing the one beneath it, the archive
    /// a file was taken out of, or the window closes if it was the last.
    Off,
    Select(Direction),
    Open,
    Rename,
    Trash,
    Fullscreen,
    /// A movement through the content: lines in text, pages in PDFs.
    Move(Motion),
    /// Half the view sideways.
    Sideways(bool),
    /// What has been typed of a vim command, for the strip.
    Pending(String),
    /// Esc dropped a pending vim command.
    Cancelled,
    /// M: the minimap, the thumbnail rail or a document's overview goes to
    /// its next mode.
    Overview,
    /// i: the strip goes to its next mode.
    Strip,
    /// w: text wraps at the window's width, or scrolls sideways again.
    Wrap,
    /// Enter or Space in an engaged archive: look inside the selected
    /// entry, which goes on top, or open or close the selected folder;
    /// with nothing selected, the key's usual act.
    Inside(Box<Act>),
    SelectAll,
    Copy,
    /// The previous or next sheet of a workbook.
    Sheet(bool),
    /// A spreadsheet's cell cursor, by columns and rows.
    Cursor(i32, i32),
    Pause,
    Mute,
    /// Back or forward by this much; accurately to the frame or to the
    /// nearest keyframe.
    Seek {
        forward: bool,
        by: Duration,
        accurate: bool,
    },
    /// Louder or quieter by a step.
    Volume(bool),
}

/// What taking the top preview off does, with `beneath` previews under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Off {
    /// The preview beneath shows again, as it was left.
    Back,
    Close,
}

pub fn off(beneath: usize) -> Off {
    if beneath == 0 { Off::Close } else { Off::Back }
}

/// What J and L skip.
pub const SKIP: Duration = Duration::from_secs(10);
/// What the arrows skip in engaged media.
pub const STEP: Duration = Duration::from_secs(5);
/// What Shift with an arrow skips.
pub const LEAP: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
}

/// The act for `key` (as GPUI names it) pressed in a view of `kind`. `held`
/// is a key repeating while held down. Vim's keys go through `vim`, which
/// keeps what is pending between keys.
pub fn route(kind: Kind, engaged: bool, key: &str, mods: Mods, held: bool, vim: &mut Keys) -> Act {
    let Mods { ctrl, shift } = mods;
    let arrow = match key {
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        _ => None,
    };
    let scrolls = kind.engages() && !kind.media() && kind != Kind::Archive;
    // Keys that take a vim count are seen before vim, which drops a count
    // for any key it does not know.
    if let Some(direction) = arrow {
        let count = vim.take_count();
        return if ctrl {
            Act::Pass
        } else if shift {
            shift_arrow(kind, direction)
        } else if engaged {
            engaged_arrow(kind, direction, count)
        } else {
            Act::Select(direction)
        };
    }
    if matches!(key, "pageup" | "pagedown") && scrolls && !ctrl {
        let n = vim.take_count();
        return Act::Move(if key == "pagedown" {
            Motion::PageDown(n)
        } else {
            Motion::PageUp(n)
        });
    }
    if kind.vim() {
        match vim.feed(key, ctrl, shift) {
            Outcome::Motion(motion) => return Act::Move(motion),
            Outcome::Pending(typed) => return Act::Pending(typed),
            Outcome::Cancelled => return Act::Cancelled,
            Outcome::Ignored => {}
        }
    }
    if ctrl {
        return match (shift, key) {
            (false, "a") if kind == Kind::Lines => Act::SelectAll,
            (false, "c") if matches!(kind, Kind::Lines | Kind::Pdf) => Act::Copy,
            _ => Act::Pass,
        };
    }
    let once = |act: Act| if held { Act::Nothing } else { act };
    match (shift, key) {
        // Space toggles the preview of the selection, as in Nautilus: on
        // for an archive's selected entry, off for the preview on top.
        (false, "space") if engaged && kind == Kind::Archive => {
            once(Act::Inside(Box::new(Act::Off)))
        }
        (false, "enter") if engaged && kind == Kind::Archive => {
            once(Act::Inside(Box::new(Act::Open)))
        }
        // Shift may still be down from Shift with an arrow.
        (_, "space") => once(Act::Off),
        (_, "escape") => once(Act::Close),
        (false, "enter") => once(Act::Open),
        (false, "f2") => once(Act::Rename),
        (false, "delete") => once(Act::Trash),
        (false, "f") => once(Act::Fullscreen),
        (_, "home" | "end") if scrolls => Act::Move(if key == "end" {
            Motion::Bottom
        } else {
            Motion::Top
        }),
        (false, "m") if kind.media() => once(Act::Mute),
        (false, "m") if matches!(kind, Kind::Lines | Kind::Doc | Kind::Pdf | Kind::Slides) => {
            once(Act::Overview)
        }
        (false, "i") => once(Act::Strip),
        (false, "w") if kind == Kind::Lines => once(Act::Wrap),
        (false, "k") if kind.media() => once(Act::Pause),
        (false, "j" | "l") if kind.media() => Act::Seek {
            forward: key == "l",
            by: SKIP,
            accurate: false,
        },
        _ => Act::Pass,
    }
}

/// Shift with an arrow: by pages and half widths in text, sheets, seconds.
fn shift_arrow(kind: Kind, direction: Direction) -> Act {
    let forward = matches!(direction, Direction::Down | Direction::Right);
    let vertical = matches!(direction, Direction::Up | Direction::Down);
    match kind {
        Kind::Lines | Kind::Doc | Kind::Sheet if vertical => Act::Move(if forward {
            Motion::PageDown(1)
        } else {
            Motion::PageUp(1)
        }),
        Kind::Pdf | Kind::Slides if vertical => Act::Move(if forward {
            Motion::Down(1)
        } else {
            Motion::Up(1)
        }),
        Kind::Lines => Act::Sideways(forward),
        Kind::Sheet => Act::Sheet(forward),
        Kind::Video | Kind::Audio if !vertical => Act::Seek {
            forward,
            by: LEAP,
            accurate: true,
        },
        _ => Act::Nothing,
    }
}

/// The arrows once the content is engaged, `count` times.
fn engaged_arrow(kind: Kind, direction: Direction, count: usize) -> Act {
    let forward = matches!(direction, Direction::Down | Direction::Right);
    let vertical = matches!(direction, Direction::Up | Direction::Down);
    match kind {
        Kind::Lines | Kind::Doc => Act::Move(match direction {
            Direction::Down => Motion::Down(count),
            Direction::Up => Motion::Up(count),
            Direction::Right => Motion::Right(count),
            Direction::Left => Motion::Left(count),
        }),
        // Pages sideways, the view up and down.
        Kind::Pdf | Kind::Slides => Act::Move(match direction {
            Direction::Right => Motion::Down(count),
            Direction::Left => Motion::Up(count),
            Direction::Down => Motion::ScrollDown(count),
            Direction::Up => Motion::ScrollUp(count),
        }),
        Kind::Sheet => {
            let n = count.min(i32::MAX as usize) as i32;
            let n = if forward { n } else { -n };
            if vertical {
                Act::Cursor(0, n)
            } else {
                Act::Cursor(n, 0)
            }
        }
        Kind::Video | Kind::Audio if vertical => Act::Volume(direction == Direction::Up),
        Kind::Video | Kind::Audio => Act::Seek {
            forward,
            by: STEP,
            accurate: true,
        },
        // The archive's tree takes its arrows before they get here.
        Kind::Archive => Act::Nothing,
        Kind::Still => Act::Select(direction),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(kind: Kind, engaged: bool, key: &str) -> Act {
        route(
            kind,
            engaged,
            key,
            Mods::default(),
            false,
            &mut Keys::default(),
        )
    }

    fn shifted(kind: Kind, key: &str) -> Act {
        let mods = Mods {
            ctrl: false,
            shift: true,
        };
        route(kind, false, key, mods, false, &mut Keys::default())
    }

    #[test]
    fn arrows_browse_until_the_content_is_engaged() {
        for kind in [
            Kind::Still,
            Kind::Video,
            Kind::Audio,
            Kind::Pdf,
            Kind::Slides,
            Kind::Lines,
            Kind::Doc,
            Kind::Sheet,
        ] {
            assert_eq!(
                press(kind, false, "right"),
                Act::Select(Direction::Right),
                "{kind:?}"
            );
            assert_eq!(press(kind, false, "up"), Act::Select(Direction::Up));
        }
        // A picture has nothing to engage: its arrows always browse.
        assert_eq!(
            press(Kind::Still, true, "left"),
            Act::Select(Direction::Left)
        );
    }

    #[test]
    fn engaged_arrows_act_on_the_content() {
        assert_eq!(press(Kind::Lines, true, "down"), Act::Move(Motion::Down(1)));
        assert_eq!(press(Kind::Lines, true, "left"), Act::Move(Motion::Left(1)));
        assert_eq!(press(Kind::Doc, true, "up"), Act::Move(Motion::Up(1)));
        assert_eq!(press(Kind::Pdf, true, "right"), Act::Move(Motion::Down(1)));
        assert_eq!(
            press(Kind::Slides, true, "down"),
            Act::Move(Motion::ScrollDown(1))
        );
        assert_eq!(press(Kind::Sheet, true, "left"), Act::Cursor(-1, 0));
        assert_eq!(press(Kind::Sheet, true, "down"), Act::Cursor(0, 1));
        assert_eq!(press(Kind::Video, true, "up"), Act::Volume(true));
        assert_eq!(press(Kind::Audio, true, "down"), Act::Volume(false));
        assert_eq!(
            press(Kind::Video, true, "left"),
            Act::Seek {
                forward: false,
                by: STEP,
                accurate: true
            }
        );
    }

    #[test]
    fn a_count_carries_to_engaged_arrows_and_drops_when_browsing() {
        let mut vim = Keys::default();
        let mods = Mods::default();
        assert_eq!(
            route(Kind::Lines, true, "5", mods, false, &mut vim),
            Act::Pending("5".into())
        );
        assert_eq!(
            route(Kind::Lines, true, "down", mods, false, &mut vim),
            Act::Move(Motion::Down(5))
        );
        route(Kind::Lines, false, "5", mods, false, &mut vim);
        assert_eq!(
            route(Kind::Lines, false, "down", mods, false, &mut vim),
            Act::Select(Direction::Down)
        );
        assert_eq!(vim.pending(), None);
    }

    #[test]
    fn shift_arrows_page_scroll_sideways_switch_sheets_and_leap() {
        assert_eq!(shifted(Kind::Lines, "down"), Act::Move(Motion::PageDown(1)));
        assert_eq!(shifted(Kind::Lines, "up"), Act::Move(Motion::PageUp(1)));
        assert_eq!(shifted(Kind::Lines, "right"), Act::Sideways(true));
        assert_eq!(shifted(Kind::Doc, "down"), Act::Move(Motion::PageDown(1)));
        assert_eq!(shifted(Kind::Pdf, "down"), Act::Move(Motion::Down(1)));
        assert_eq!(shifted(Kind::Sheet, "left"), Act::Sheet(false));
        assert_eq!(
            shifted(Kind::Video, "right"),
            Act::Seek {
                forward: true,
                by: LEAP,
                accurate: true
            }
        );
        // Never a file change.
        assert_eq!(shifted(Kind::Still, "right"), Act::Nothing);
    }

    #[test]
    fn vim_letters_belong_to_documents_and_media_keep_theirs() {
        assert_eq!(press(Kind::Lines, false, "j"), Act::Move(Motion::Down(1)));
        assert_eq!(press(Kind::Pdf, false, "k"), Act::Move(Motion::Up(1)));
        assert_eq!(press(Kind::Doc, false, "g"), Act::Pending("g".into()));
        assert_eq!(press(Kind::Video, false, "k"), Act::Pause);
        assert_eq!(press(Kind::Video, false, "m"), Act::Mute);
        assert_eq!(
            press(Kind::Audio, false, "l"),
            Act::Seek {
                forward: true,
                by: SKIP,
                accurate: false
            }
        );
        assert_eq!(press(Kind::Lines, false, "m"), Act::Overview);
        assert_eq!(press(Kind::Pdf, false, "m"), Act::Overview);
        assert_eq!(press(Kind::Doc, false, "m"), Act::Overview);
        assert_eq!(press(Kind::Sheet, false, "m"), Act::Pass);
        // i is the strip's everywhere, vim's documents included.
        for kind in [
            Kind::Still,
            Kind::Video,
            Kind::Lines,
            Kind::Doc,
            Kind::Archive,
        ] {
            assert_eq!(press(kind, false, "i"), Act::Strip, "{kind:?}");
        }
        // Sheets and archives take no vim keys.
        assert_eq!(press(Kind::Sheet, false, "j"), Act::Pass);
    }

    #[test]
    fn w_wraps_text_once_per_press() {
        assert_eq!(press(Kind::Lines, false, "w"), Act::Wrap);
        assert_eq!(press(Kind::Lines, true, "w"), Act::Wrap);
        for kind in [Kind::Doc, Kind::Pdf, Kind::Sheet, Kind::Video, Kind::Still] {
            assert_eq!(press(kind, false, "w"), Act::Pass, "{kind:?}");
        }
        // A count typed first is dropped, as vim drops one before a key it
        // does not know.
        let mut vim = Keys::default();
        let mods = Mods::default();
        route(Kind::Lines, false, "5", mods, false, &mut vim);
        assert_eq!(
            route(Kind::Lines, false, "w", mods, false, &mut vim),
            Act::Wrap
        );
        assert_eq!(vim.pending(), None);
        assert_eq!(
            route(Kind::Lines, false, "w", mods, true, &mut vim),
            Act::Nothing
        );
    }

    #[test]
    fn enter_and_space_look_inside_an_engaged_archive() {
        assert_eq!(
            press(Kind::Archive, true, "enter"),
            Act::Inside(Box::new(Act::Open))
        );
        assert_eq!(
            press(Kind::Archive, true, "space"),
            Act::Inside(Box::new(Act::Off))
        );
        // Not engaged, they open the archive and take its preview off.
        assert_eq!(press(Kind::Archive, false, "enter"), Act::Open);
        assert_eq!(press(Kind::Archive, false, "space"), Act::Off);
        assert_eq!(press(Kind::Lines, true, "space"), Act::Off);
        assert_eq!(press(Kind::Archive, true, "escape"), Act::Close);
    }

    #[test]
    fn space_takes_previews_off_the_stack_one_by_one_and_escape_all() {
        // A file inside an archive inside an archive: two lie beneath it.
        let mut stack = vec!["outer.zip", "inner.tar"];
        let mut top = "notes.txt";
        let mut shown = Vec::new();
        loop {
            shown.push(top);
            assert_eq!(press(Kind::Lines, false, "space"), Act::Off);
            match off(stack.len()) {
                Off::Back => top = stack.pop().unwrap(),
                Off::Close => break,
            }
        }
        assert_eq!(shown, ["notes.txt", "inner.tar", "outer.zip"]);
        // Esc from two deep closes everything at once.
        assert_eq!(press(Kind::Lines, false, "escape"), Act::Close);
        assert_eq!(off(0), Off::Close);
        assert_eq!(off(2), Off::Back);
    }

    #[test]
    fn escape_cancels_a_pending_command_before_it_closes() {
        let mut vim = Keys::default();
        let mods = Mods::default();
        route(Kind::Pdf, false, "1", mods, false, &mut vim);
        assert_eq!(
            route(Kind::Pdf, false, "escape", mods, false, &mut vim),
            Act::Cancelled
        );
        assert_eq!(
            route(Kind::Pdf, false, "escape", mods, false, &mut vim),
            Act::Close
        );
        assert_eq!(press(Kind::Video, true, "space"), Act::Off);
        assert_eq!(shifted(Kind::Lines, "space"), Act::Off);
        assert_eq!(shifted(Kind::Lines, "escape"), Act::Close);
    }

    #[test]
    fn held_keys_repeat_moves_but_not_toggles() {
        let mut vim = Keys::default();
        let mods = Mods::default();
        assert_eq!(
            route(Kind::Video, false, "k", mods, true, &mut vim),
            Act::Nothing
        );
        assert_eq!(
            route(Kind::Lines, false, "j", mods, true, &mut vim),
            Act::Move(Motion::Down(1))
        );
        assert_eq!(
            route(Kind::Lines, false, "space", mods, true, &mut vim),
            Act::Nothing
        );
    }

    #[test]
    fn page_keys_and_copying() {
        assert_eq!(
            press(Kind::Sheet, false, "pagedown"),
            Act::Move(Motion::PageDown(1))
        );
        assert_eq!(press(Kind::Pdf, false, "home"), Act::Move(Motion::Top));
        assert_eq!(press(Kind::Still, false, "pagedown"), Act::Pass);
        let ctrl = Mods {
            ctrl: true,
            shift: false,
        };
        let mut vim = Keys::default();
        assert_eq!(
            route(Kind::Lines, false, "a", ctrl, false, &mut vim),
            Act::SelectAll
        );
        assert_eq!(
            route(Kind::Pdf, false, "c", ctrl, false, &mut vim),
            Act::Copy
        );
        assert_eq!(
            route(Kind::Lines, false, "d", ctrl, false, &mut vim),
            Act::Move(Motion::HalfDown(1))
        );
        assert_eq!(
            route(Kind::Video, false, "c", ctrl, false, &mut vim),
            Act::Pass
        );
    }
}
