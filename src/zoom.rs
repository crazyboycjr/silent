//! The text size Ctrl with = and - set for text and code, Markdown and HTML,
//! documents and spreadsheets: one level for all of them, kept with
//! silent's other settings, so that the next file opens at it in a window
//! sized for it. The same keys zoom PDFs, slides and pictures, each file at
//! a level of its own, which the view memory keeps with where it was left.

use gpui_kit::{ListState, Pixels};
use silent::memory;

const SETTING: &str = "text-zoom";
/// The levels, in percent: steps of about a tenth. Text goes up to
/// [`TEXT_MOST`], pages and pictures to the last.
const LEVELS: [u16; 22] = [
    50, 55, 60, 67, 75, 80, 90, 100, 110, 120, 133, 150, 165, 180, 200, 220, 240, 270, 300, 330,
    360, 400,
];
const TEXT_MOST: u16 = 300;

/// What a key or the wheel asks of the text size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

/// The level set last, as a factor.
pub fn level() -> f32 {
    let percent = memory::setting(SETTING)
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|percent| LEVELS.contains(percent) && *percent <= TEXT_MOST)
        .unwrap_or(100);
    f32::from(percent) / 100.
}

/// The level after `zoom` from `level`, kept for the next file.
pub fn change(level: f32, zoom: Zoom) -> f32 {
    let percent = next(level, zoom, TEXT_MOST);
    let after = f32::from(percent) / 100.;
    if after != level {
        memory::set_setting(SETTING, &percent.to_string());
    }
    after
}

/// The zoom of a PDF, slides or a picture after `zoom` from `level`, 1 when
/// it fits the window as it opened.
pub fn step(level: f32, zoom: Zoom) -> f32 {
    f32::from(next(level, zoom, LEVELS[LEVELS.len() - 1])) / 100.
}

fn next(level: f32, zoom: Zoom, most: u16) -> u16 {
    let last = LEVELS
        .iter()
        .position(|&l| l == most)
        .unwrap_or(LEVELS.len() - 1);
    let levels = &LEVELS[..=last];
    let at = levels
        .iter()
        .position(|&percent| f32::from(percent) >= level * 100. - 0.5)
        .unwrap_or(levels.len() - 1);
    match zoom {
        Zoom::In => levels[(at + 1).min(levels.len() - 1)],
        Zoom::Out => levels[at.saturating_sub(1)],
        Zoom::Reset => 100,
    }
}

/// `Text size 110%`, for the info line.
pub fn describe(level: f32) -> String {
    format!("Text size {}%", (level * 100.).round())
}

/// `Zoom 150%`, for the info line over a PDF, slides or a picture.
pub fn describe_zoom(level: f32) -> String {
    format!("Zoom {}%", (level * 100.).round())
}

/// A place in a list of blocks held where it is on the screen while the
/// blocks are drawn at another size: the block, how far down it, and how
/// far down the view.
pub struct Held {
    item: usize,
    within: f32,
    y: Pixels,
    /// Laid out at the new size once, so that the block's place is known.
    laid_out: bool,
}

/// The place under `y`, in window coordinates.
pub fn hold(list: &ListState, y: Pixels) -> Option<Held> {
    let view = list.viewport_bounds();
    let top = list.logical_scroll_top().item_ix;
    (top..list.item_count()).find_map(|item| {
        let bounds = list.bounds_for_item(item)?;
        (bounds.bottom() > y).then(|| Held {
            item,
            within: ((y - bounds.top()) / bounds.size.height).clamp(0., 1.),
            y: y - view.top(),
            laid_out: false,
        })
    })
}

/// Brings the place held back under the pointer, once the blocks have been
/// laid out at their new size; whether it is done.
pub fn keep(list: &ListState, held: &mut Held) -> bool {
    if !held.laid_out {
        held.laid_out = true;
        return false;
    }
    if let Some(bounds) = list.bounds_for_item(held.item) {
        let now = bounds.top() + bounds.size.height * held.within - list.viewport_bounds().top();
        list.scroll_by(now - held.y);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_step_by_about_a_tenth_from_half_to_three_times() {
        assert_eq!(LEVELS[0], 50);
        assert_eq!(LEVELS[LEVELS.len() - 1], 400);
        for pair in LEVELS.windows(2) {
            let ratio = f32::from(pair[1]) / f32::from(pair[0]);
            assert!((1.06..=1.13).contains(&ratio), "{pair:?}");
        }
        let text = |level, zoom| next(level, zoom, TEXT_MOST);
        assert_eq!(text(1., Zoom::In), 110);
        assert_eq!(text(1., Zoom::Out), 90);
        assert_eq!(text(3., Zoom::In), 300);
        assert_eq!(text(0.5, Zoom::Out), 50);
        assert_eq!(text(1.33, Zoom::In), 150);
        assert_eq!(text(2.4, Zoom::Reset), 100);
    }

    #[test]
    fn pages_and_pictures_zoom_to_four_times() {
        assert_eq!(step(3., Zoom::In), 3.3);
        assert_eq!(step(3.6, Zoom::In), 4.);
        assert_eq!(step(4., Zoom::In), 4.);
        assert_eq!(step(4., Zoom::Out), 3.6);
        assert_eq!(step(0.5, Zoom::Out), 0.5);
        assert_eq!(step(2.7, Zoom::Reset), 1.);
        assert_eq!(describe_zoom(1.33), "Zoom 133%");
    }
}
