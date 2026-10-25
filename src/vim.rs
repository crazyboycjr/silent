//! Vim's movement keys for text, documents and pages: a count typed first
//! repeats a key, `gg` and `G` go to either end, and with a count to a line
//! or a page. Keys arrive one at a time; what has been typed of an
//! unfinished command is shown, as vim's `showcmd` does.

/// A movement asked for by the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// j and k: lines in text, pages in PDFs and slides.
    Down(usize),
    Up(usize),
    /// h and l: sideways.
    Left(usize),
    Right(usize),
    /// Ctrl-e and Ctrl-y: the view, a line at a time.
    ScrollDown(usize),
    ScrollUp(usize),
    /// Ctrl-d and Ctrl-u: half a page.
    HalfDown(usize),
    HalfUp(usize),
    /// Ctrl-f and Ctrl-b: a page.
    PageDown(usize),
    PageUp(usize),
    /// gg and G.
    Top,
    Bottom,
    /// `20G` or `20gg`: line or page 20, counted from 1.
    Line(usize),
    /// 0: back to the left edge.
    Home,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not one of these keys; whatever was pending is dropped.
    Ignored,
    /// The start of a command: what has been typed so far.
    Pending(String),
    /// Esc dropped what was pending.
    Cancelled,
    Motion(Motion),
}

/// Counts beyond this are as good as infinite.
const MOST: usize = 1_000_000_000;

#[derive(Clone, Debug, Default)]
pub struct Keys {
    count: Option<usize>,
    g: bool,
}

impl Keys {
    /// Takes a key as GPUI names it (`"j"`, `"5"`, `"escape"`), with Ctrl
    /// and Shift; Shift with a letter is its capital.
    pub fn feed(&mut self, key: &str, ctrl: bool, shift: bool) -> Outcome {
        let count = self.count.take();
        let g = std::mem::take(&mut self.g);
        let n = count.unwrap_or(1);
        let motion = match (ctrl, shift, key) {
            (true, false, "e") => Motion::ScrollDown(n),
            (true, false, "y") => Motion::ScrollUp(n),
            (true, false, "d") => Motion::HalfDown(n),
            (true, false, "u") => Motion::HalfUp(n),
            (true, false, "f") => Motion::PageDown(n),
            (true, false, "b") => Motion::PageUp(n),
            (true, ..) => return Outcome::Ignored,
            (false, false, "escape") if count.is_some() || g => return Outcome::Cancelled,
            (false, false, digit) if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => {
                let d = usize::from(digit.as_bytes()[0] - b'0');
                match count {
                    None if d == 0 => Motion::Home,
                    _ => {
                        self.count = Some((count.unwrap_or(0) * 10 + d).min(MOST));
                        return Outcome::Pending(self.shown());
                    }
                }
            }
            (false, false, "g") if g => count.map_or(Motion::Top, Motion::Line),
            (false, false, "g") => {
                self.count = count;
                self.g = true;
                return Outcome::Pending(self.shown());
            }
            (false, true, "g") => count.map_or(Motion::Bottom, Motion::Line),
            (false, false, "j") => Motion::Down(n),
            (false, false, "k") => Motion::Up(n),
            (false, false, "h") => Motion::Left(n),
            (false, false, "l") => Motion::Right(n),
            _ => return Outcome::Ignored,
        };
        Outcome::Motion(motion)
    }

    /// The count typed so far, or 1, for a key outside vim's that takes one,
    /// such as an arrow; anything pending is dropped.
    pub fn take_count(&mut self) -> usize {
        self.g = false;
        self.count.take().unwrap_or(1)
    }

    pub fn pending(&self) -> Option<String> {
        (self.count.is_some() || self.g).then(|| self.shown())
    }

    fn shown(&self) -> String {
        let mut out = self.count.map(|n| n.to_string()).unwrap_or_default();
        if self.g {
            out.push('g');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds keys written as vim writes them: `<C-d>` for Ctrl-d, `<Esc>`,
    /// capitals for Shift.
    fn feed(keys: &mut Keys, typed: &str) -> Vec<Outcome> {
        let mut out = Vec::new();
        let mut rest = typed;
        while !rest.is_empty() {
            let (key, ctrl, len) = if let Some(tail) = rest.strip_prefix("<C-") {
                (tail[..1].to_owned(), true, 5)
            } else if rest.starts_with("<Esc>") {
                ("escape".to_owned(), false, 5)
            } else {
                (rest[..1].to_owned(), false, 1)
            };
            let shift = key.len() == 1 && key.as_bytes()[0].is_ascii_uppercase();
            out.push(keys.feed(&key.to_ascii_lowercase(), ctrl, shift));
            rest = &rest[len..];
        }
        out
    }

    fn last(typed: &str) -> Outcome {
        feed(&mut Keys::default(), typed).pop().unwrap()
    }

    #[test]
    fn letters_move_once_or_by_their_count() {
        assert_eq!(last("j"), Outcome::Motion(Motion::Down(1)));
        assert_eq!(last("k"), Outcome::Motion(Motion::Up(1)));
        assert_eq!(last("5j"), Outcome::Motion(Motion::Down(5)));
        assert_eq!(last("12h"), Outcome::Motion(Motion::Left(12)));
        assert_eq!(last("3l"), Outcome::Motion(Motion::Right(3)));
        assert_eq!(last("10<C-e>"), Outcome::Motion(Motion::ScrollDown(10)));
        assert_eq!(last("<C-y>"), Outcome::Motion(Motion::ScrollUp(1)));
        assert_eq!(last("<C-d>"), Outcome::Motion(Motion::HalfDown(1)));
        assert_eq!(last("2<C-u>"), Outcome::Motion(Motion::HalfUp(2)));
        assert_eq!(last("<C-f>"), Outcome::Motion(Motion::PageDown(1)));
        assert_eq!(last("<C-b>"), Outcome::Motion(Motion::PageUp(1)));
    }

    #[test]
    fn g_and_capital_g_go_to_the_ends_or_to_a_line() {
        assert_eq!(last("gg"), Outcome::Motion(Motion::Top));
        assert_eq!(last("G"), Outcome::Motion(Motion::Bottom));
        assert_eq!(last("20G"), Outcome::Motion(Motion::Line(20)));
        assert_eq!(last("7gg"), Outcome::Motion(Motion::Line(7)));
        assert_eq!(last("0"), Outcome::Motion(Motion::Home));
        assert_eq!(last("10G"), Outcome::Motion(Motion::Line(10)));
    }

    #[test]
    fn what_is_pending_shows_until_the_command_ends() {
        let mut keys = Keys::default();
        let outcomes = feed(&mut keys, "20g");
        assert_eq!(
            outcomes,
            [
                Outcome::Pending("2".into()),
                Outcome::Pending("20".into()),
                Outcome::Pending("20g".into())
            ]
        );
        assert_eq!(keys.pending().as_deref(), Some("20g"));
        assert_eq!(feed(&mut keys, "g"), [Outcome::Motion(Motion::Line(20))]);
        assert_eq!(keys.pending(), None);
    }

    #[test]
    fn escape_cancels_and_other_keys_drop_the_count() {
        let mut keys = Keys::default();
        feed(&mut keys, "5");
        assert_eq!(feed(&mut keys, "<Esc>"), [Outcome::Cancelled]);
        assert_eq!(keys.pending(), None);
        // With nothing pending, Esc is not ours: it closes the preview.
        assert_eq!(feed(&mut keys, "<Esc>"), [Outcome::Ignored]);
        feed(&mut keys, "5");
        assert_eq!(keys.feed("enter", false, false), Outcome::Ignored);
        assert_eq!(feed(&mut keys, "j"), [Outcome::Motion(Motion::Down(1))]);
        // g then another key: the g is dropped and the key counts alone.
        assert_eq!(
            feed(&mut keys, "gj"),
            [
                Outcome::Pending("g".into()),
                Outcome::Motion(Motion::Down(1))
            ]
        );
        // Media keys and Ctrl with other letters are not ours.
        assert_eq!(keys.feed("m", false, false), Outcome::Ignored);
        assert_eq!(keys.feed("c", true, false), Outcome::Ignored);
    }

    #[test]
    fn arrows_take_the_count() {
        let mut keys = Keys::default();
        feed(&mut keys, "4");
        assert_eq!(keys.take_count(), 4);
        assert_eq!(keys.take_count(), 1);
        assert_eq!(keys.pending(), None);
    }

    #[test]
    fn huge_counts_stop_growing() {
        assert_eq!(
            last("99999999999999999999G"),
            Outcome::Motion(Motion::Line(MOST))
        );
    }
}
