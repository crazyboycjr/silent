//! The text size Ctrl with = and - set for text and code, Markdown and HTML
//! and documents: one level for all of them, kept with silent's other
//! settings, so that the next file opens at it in a window sized for it.

use gpui_kit::{ListState, Pixels};
use silent::memory;

const SETTING: &str = "text-zoom";
/// The levels, in percent: steps of about a tenth.
const LEVELS: [u16; 19] = [
    50, 55, 60, 67, 75, 80, 90, 100, 110, 120, 133, 150, 165, 180, 200, 220, 240, 270, 300,
];

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
        .filter(|percent| LEVELS.contains(percent))
        .unwrap_or(100);
    f32::from(percent) / 100.
}

/// The level after `zoom` from `level`, kept for the next file.
pub fn change(level: f32, zoom: Zoom) -> f32 {
    let percent = next(level, zoom);
    memory::set_setting(SETTING, &percent.to_string());
    f32::from(percent) / 100.
}

fn next(level: f32, zoom: Zoom) -> u16 {
    let at = LEVELS
        .iter()
        .position(|&percent| f32::from(percent) >= level * 100. - 0.5)
        .unwrap_or(LEVELS.len() - 1);
    match zoom {
        Zoom::In => LEVELS[(at + 1).min(LEVELS.len() - 1)],
        Zoom::Out => LEVELS[at.saturating_sub(1)],
        Zoom::Reset => 100,
    }
}

/// `Text size 110%`, for the info line.
pub fn describe(level: f32) -> String {
    format!("Text size {}%", (level * 100.).round())
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
        assert_eq!(LEVELS[LEVELS.len() - 1], 300);
        for pair in LEVELS.windows(2) {
            let ratio = f32::from(pair[1]) / f32::from(pair[0]);
            assert!((1.06..=1.13).contains(&ratio), "{pair:?}");
        }
        assert_eq!(next(1., Zoom::In), 110);
        assert_eq!(next(1., Zoom::Out), 90);
        assert_eq!(next(3., Zoom::In), 300);
        assert_eq!(next(0.5, Zoom::Out), 50);
        assert_eq!(next(1.33, Zoom::In), 150);
        assert_eq!(next(2.4, Zoom::Reset), 100);
    }
}
