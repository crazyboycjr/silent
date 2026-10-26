//! The window's parts, none drawn over another: the content, a column at
//! its right for the overview of a long view, and a row along the bottom for
//! the strip.

use gpui_kit::*;

/// The row's height in logical pixels.
pub const ROW: f32 = silent::geometry::ROW as f32;

/// Whether the strip stays in its row; otherwise it shows while the pointer
/// moves and fades when it rests.
pub const STRIP_ALWAYS: bool = false;

/// Whether the minimap, the rail or the scrollbar stays in its column;
/// otherwise it shows while the view moves and fades when it rests.
pub const COLUMN_ALWAYS: bool = false;

/// What the content and its column share: the window above the row.
pub fn above_row(window: &Window) -> Size<Pixels> {
    let viewport = window.viewport_size();
    size(viewport.width, (viewport.height - px(ROW)).max(px(1.)))
}
