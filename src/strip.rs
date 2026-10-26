//! The strip of facts over the bottom of the window: its modes, its size,
//! and its colours, which come from what it lies over. Pictures and video
//! give their own pixels; text, documents and cards their theme's ground
//! and ink, which are both under it.

use std::sync::Arc;

use gpui_kit::*;
use silent::memory;
use silent::scrim::{self, Rgb, Scrim};

/// The strip's line, and the fade above it to nothing.
pub const ROW: f32 = 34.;
pub const FADE: f32 = 22.;
/// A video's or a sound's playbar: the seek track over the same line,
/// under a taller fade.
pub const TRACK: f32 = 18.;
pub const PLAYBAR_FADE: f32 = 30.;
/// The line's inset from the window's sides, and its letters.
pub const INSET: f32 = 14.;
const TEXT: f32 = 13.;
const LEADING: f32 = 18.;
/// How far a notice's veil reaches beyond its word.
const CHIP: (f32, f32) = (8., 3.);
/// The blocks a picture is averaged in, in logical pixels: about the
/// height of the strip's letters.
const CELL: f32 = 16.;

/// The strip's line of text, the same in every mode: shown while the
/// pointer moves or always, under a playbar's track, and as a notice while
/// the strip is off. One face with figures of one width, one size and one
/// leading, centred in one height at one inset, so that every word sits on
/// the same baseline.
pub fn line(ink: Hsla) -> Div {
    div()
        .flex_none()
        .h(px(ROW))
        .px(px(INSET))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.))
        .font_family("Noto Sans")
        .font_features(figures())
        .text_size(px(TEXT))
        .line_height(px(LEADING))
        .text_color(ink)
}

/// Figures of one width, so that times and counts do not shift as they change.
pub fn figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// `word` on a veil reaching a little past it, which leaves the word where
/// it would be without one.
pub fn chip(word: String, veil: Hsla) -> Div {
    div()
        .flex_none()
        .relative()
        .child(
            div()
                .absolute()
                .left(px(-CHIP.0))
                .right(px(-CHIP.0))
                .top(px(-CHIP.1))
                .bottom(px(-CHIP.1))
                .rounded(px(6.))
                .bg(veil),
        )
        .child(div().relative().child(word))
}

/// What i cycles through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Shown while the pointer moves, fading when it rests.
    #[default]
    Auto,
    /// Never shown; keys still answer with a word for a second.
    Off,
    /// Always shown.
    On,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Auto => Mode::Off,
            Mode::Off => Mode::On,
            Mode::On => Mode::Auto,
        }
    }

    /// The mode chosen last for files of `family`, kept until the next reboot.
    pub fn load(family: &str) -> Self {
        match memory::setting(&format!("strip-{family}")).as_deref() {
            Some("off") => Mode::Off,
            Some("on") => Mode::On,
            _ => Mode::Auto,
        }
    }

    pub fn save(self, family: &str) {
        let name = match self {
            Mode::Auto => "auto",
            Mode::Off => "off",
            Mode::On => "on",
        };
        memory::set_setting(&format!("strip-{family}"), name);
    }

    pub fn describe(self) -> &'static str {
        match self {
            Mode::Auto => "Strip while moving",
            Mode::Off => "Strip off",
            Mode::On => "Strip on",
        }
    }
}

/// A colour for GPUI.
pub fn hsla(c: Rgb, alpha: f32) -> Hsla {
    Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: alpha,
    }
    .into()
}

pub fn rgb_of(c: Hsla) -> Rgb {
    let c = c.to_rgb();
    Rgb {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

/// The veil for a strip over flat colours, the first of them its ground.
pub fn over_colours(colours: &[Hsla]) -> Scrim {
    let under: Vec<Rgb> = colours.iter().copied().map(rgb_of).collect();
    scrim::choose(&under, under.first().copied().is_some_and(scrim::is_dark))
}

/// What lies under the bottom `band` of a view `area` wide and tall that
/// shows `image` whole and centred, on `ground` where the image leaves it
/// bare: the image's blocks there, and the ground when it shows.
pub fn under_image(image: &RenderImage, area: Size<Pixels>, band: f32, ground: Rgb) -> Vec<Rgb> {
    let pixels = image.size(0);
    let (iw, ih) = (pixels.width.0.max(1) as f32, pixels.height.0.max(1) as f32);
    let (w, h) = (f32::from(area.width), f32::from(area.height));
    let Some(bytes) = image.as_bytes(0) else {
        return vec![ground];
    };
    let fit = (w / iw).min(h / ih).max(1e-6);
    let top = (h - ih * fit) / 2.;
    let rows = ((h - band - top) / fit).max(0.)..((h - top) / fit).min(ih);
    let mut under = if rows.is_empty() {
        Vec::new()
    } else {
        let cell = (CELL / fit).round().max(1.) as usize;
        let rows = rows.start.floor() as usize..rows.end.ceil() as usize;
        scrim::cells(bytes, iw as usize, rows, cell, ground)
    };
    // Bare ground beside or below the picture.
    if ih * fit < h - 0.5 || iw * fit < w - 0.5 || under.is_empty() {
        under.push(ground);
    }
    under
}

/// The veil for a strip over `under`, keeping `before`'s lightness unless
/// the other saves enough, and letting its opacity down gently: by at
/// most a tenth at a time, never below what the text needs.
pub fn follow(under: &[Rgb], before: Option<Scrim>) -> Scrim {
    let dark = match before {
        Some(before) => before.dark,
        None => scrim::is_dark(mean(under)),
    };
    let wanted = scrim::choose(under, dark);
    match before {
        Some(before) if before.dark == wanted.dark => Scrim {
            alpha: wanted.alpha.max(before.alpha - 0.1),
            ..wanted
        },
        _ => wanted,
    }
}

fn mean(colours: &[Rgb]) -> Rgb {
    let n = colours.len().max(1) as f32;
    let sum = colours
        .iter()
        .fold((0., 0., 0.), |(r, g, b), c| (r + c.r, g + c.g, b + c.b));
    Rgb {
        r: sum.0 / n,
        g: sum.1 / n,
        b: sum.2 / n,
    }
}

/// The veil as a background: clear at the top, at its opacity from `solid`
/// of the way down.
pub fn background(scrim: Scrim, solid: f32) -> Background {
    let veil = scrim.veil();
    linear_gradient(
        180.,
        linear_color_stop(hsla(veil, 0.), 0.),
        linear_color_stop(hsla(veil, scrim.alpha), solid),
    )
}
