//! The strip's colours, worked out from what lies under it. Its text is
//! white on a dark veil or near-black on a light one, and the veil is as
//! faint as it can be while the text keeps WCAG's AA contrast, 4.5:1,
//! against everything under it, blended as the screen blends them.

use std::ops::Range;
use std::sync::LazyLock;

/// A colour as the window's pixels hold it: sRGB-encoded channels, 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: ((rgb >> 16) & 0xff) as f32 / 255.,
            g: ((rgb >> 8) & 0xff) as f32 / 255.,
            b: (rgb & 0xff) as f32 / 255.,
        }
    }

    /// WCAG's relative luminance.
    pub fn luminance(self) -> f32 {
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }

    /// This colour laid over `under` at `alpha`. GPUI draws into an 8-bit
    /// surface that is not sRGB, so blending mixes the encoded values.
    pub fn over(self, alpha: f32, under: Rgb) -> Rgb {
        let mix = |top: f32, below: f32| top * alpha + below * (1. - alpha);
        Rgb {
            r: mix(self.r, under.r),
            g: mix(self.g, under.g),
            b: mix(self.b, under.b),
        }
    }
}

fn linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn encoded(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1. / 2.4) - 0.055
    }
}

/// WCAG's contrast ratio, from 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (a.luminance(), b.luminance());
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// WCAG AA for text of the strip's size.
pub const AA: f32 = 4.5;
/// Light text, and the warm black veil it sits on.
pub const LIGHT_INK: Rgb = Rgb::hex(0xffffff);
pub const DARK_VEIL: Rgb = Rgb::hex(0x0e0b09);
/// Dark text, and the warm white veil it sits on.
pub const DARK_INK: Rgb = Rgb::hex(0x1b1714);
pub const LIGHT_VEIL: Rgb = Rgb::hex(0xfbf8f3);
/// A veil of the other lightness is taken only when it saves this much
/// opacity, so that the strip does not flip for nothing.
const MARGIN: f32 = 0.1;

/// The strip's veil and, with it, its text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scrim {
    /// A dark veil under light text; otherwise a light veil under dark text.
    pub dark: bool,
    pub alpha: f32,
}

impl Scrim {
    pub fn veil(self) -> Rgb {
        if self.dark { DARK_VEIL } else { LIGHT_VEIL }
    }

    pub fn ink(self) -> Rgb {
        if self.dark { LIGHT_INK } else { DARK_INK }
    }

    /// Whether the text reads over every colour in `under`.
    pub fn reads(self, under: &[Rgb]) -> bool {
        reads(self.ink(), self.veil(), self.alpha, under)
    }
}

fn reads(ink: Rgb, veil: Rgb, alpha: f32, under: &[Rgb]) -> bool {
    under
        .iter()
        .all(|&u| contrast(ink, veil.over(alpha, u)) >= AA)
}

/// The least opacity of `veil`, in hundredths, at which `ink` keeps AA
/// contrast over every colour in `under`. The veil alone keeps it, so a
/// whole veil always does.
pub fn least_alpha(ink: Rgb, veil: Rgb, under: &[Rgb]) -> f32 {
    let reads = |hundredths: u32| reads(ink, veil, hundredths as f32 / 100., under);
    // Blending moves each colour steadily toward the veil, so this halves
    // a range whose low end fails and whose high end reads.
    let (mut low, mut high) = (0, 100);
    if reads(low) {
        return 0.;
    }
    while high - low > 1 {
        let mid = (low + high) / 2;
        if reads(mid) {
            high = mid;
        } else {
            low = mid;
        }
    }
    high as f32 / 100.
}

/// The veil for text over `under`: whichever lightness needs less opacity,
/// though the one `dark` names is kept unless the other saves [`MARGIN`].
pub fn choose(under: &[Rgb], dark: bool) -> Scrim {
    let on_dark = least_alpha(LIGHT_INK, DARK_VEIL, under);
    let on_light = least_alpha(DARK_INK, LIGHT_VEIL, under);
    let dark = if dark {
        on_dark <= on_light + MARGIN
    } else {
        on_dark + MARGIN < on_light
    };
    Scrim {
        dark,
        alpha: if dark { on_dark } else { on_light },
    }
}

/// Whether light text suits `ground` better than dark text does.
pub fn is_dark(ground: Rgb) -> bool {
    contrast(LIGHT_INK, ground) > contrast(DARK_INK, ground)
}

/// sRGB-encoded bytes in linear light.
static LINEAR: LazyLock<[f32; 256]> =
    LazyLock::new(|| std::array::from_fn(|i| linear(i as f32 / 255.)));

/// The mean colour of each `cell` × `cell` block of rows `rows` of a BGRA
/// image `width` pixels wide, a few dozen pixels of each sampled and
/// averaged in linear light, as the eye averages fine detail. Transparent
/// pixels show `ground`. Blocks cut short by the image's edges count too.
pub fn cells(bgra: &[u8], width: usize, rows: Range<usize>, cell: usize, ground: Rgb) -> Vec<Rgb> {
    let height = bgra.len() / (width * 4).max(1);
    let rows = rows.start.min(height)..rows.end.min(height);
    let cell = cell.max(1);
    let step = (cell / 6).max(1);
    let ground = [linear(ground.b), linear(ground.g), linear(ground.r)];
    let mut out = Vec::new();
    for top in rows.clone().step_by(cell) {
        for left in (0..width).step_by(cell) {
            let mut sum = [0f32; 3];
            let mut count = 0;
            for y in (top..(top + cell).min(rows.end)).step_by(step) {
                for x in (left..(left + cell).min(width)).step_by(step) {
                    let at = (y * width + x) * 4;
                    let alpha = f32::from(bgra[at + 3]) / 255.;
                    for c in 0..3 {
                        let shown = LINEAR[usize::from(bgra[at + c])];
                        sum[c] += shown * alpha + ground[c] * (1. - alpha);
                    }
                    count += 1;
                }
            }
            let mean = |c: usize| encoded(sum[c] / count as f32);
            out.push(Rgb {
                r: mean(2),
                g: mean(1),
                b: mean(0),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgb = Rgb::hex(0xffffff);
    const BLACK: Rgb = Rgb::hex(0x000000);

    fn close(a: f32, b: f32, within: f32) -> bool {
        (a - b).abs() <= within
    }

    fn bare(dark: bool) -> Scrim {
        Scrim { dark, alpha: 0. }
    }

    /// The scrim reads, and a hundredth less would not.
    fn least(scrim: Scrim, under: &[Rgb]) -> bool {
        let less = Scrim {
            alpha: scrim.alpha - 0.01,
            ..scrim
        };
        scrim.reads(under) && (scrim.alpha == 0. || !less.reads(under))
    }

    #[test]
    fn luminance_and_contrast_are_wcags() {
        assert_eq!(WHITE.luminance(), 1.);
        assert_eq!(BLACK.luminance(), 0.);
        // #777777 is the grey WCAG examples put at 4.48:1 on white.
        assert!(close(Rgb::hex(0x777777).luminance(), 0.184, 1e-3));
        assert!(close(contrast(WHITE, Rgb::hex(0x777777)), 4.48, 0.01));
        assert!(close(contrast(WHITE, BLACK), 21., 1e-4));
        assert_eq!(contrast(BLACK, WHITE), contrast(WHITE, BLACK));
        assert_eq!(contrast(Rgb::hex(0x336699), Rgb::hex(0x336699)), 1.);
    }

    #[test]
    fn blending_mixes_the_encoded_values() {
        let grey = Rgb::hex(0x808080);
        assert_eq!(WHITE.over(0., grey), grey);
        assert_eq!(WHITE.over(1., grey), WHITE);
        let half = BLACK.over(0.5, WHITE);
        assert!(close(half.r, 0.5, 1e-6) && close(half.g, 0.5, 1e-6));
    }

    #[test]
    fn the_veils_and_inks_read_on_their_own() {
        assert!(contrast(LIGHT_INK, DARK_VEIL) >= AA);
        assert!(contrast(DARK_INK, LIGHT_VEIL) >= AA);
    }

    #[test]
    fn plain_grounds_need_no_veil() {
        assert_eq!(choose(&[WHITE], false), bare(false));
        assert_eq!(choose(&[BLACK], true), bare(true));
        // A pale picture takes dark text, a dark one light text, whichever
        // lightness was asked for.
        let pale = [Rgb::hex(0xe8e0d0), Rgb::hex(0xf4efe6)];
        assert_eq!(choose(&pale, true), bare(false));
        let night = [Rgb::hex(0x10141c), Rgb::hex(0x2a2f3a)];
        assert_eq!(choose(&night, false), bare(true));
    }

    #[test]
    fn a_mid_grey_needs_a_little_veil_and_no_more() {
        let grey = [Rgb::hex(0x7a7a7a)];
        let scrim = choose(&grey, true);
        assert!(
            scrim.dark && scrim.alpha > 0. && scrim.alpha < 0.1,
            "{scrim:?}"
        );
        assert!(least(scrim, &grey));
    }

    #[test]
    fn mixed_grounds_get_the_veil_that_reads_over_all_of_it() {
        let under = [WHITE, BLACK, Rgb::hex(0xc04020), Rgb::hex(0x30a0e0)];
        for dark in [false, true] {
            let scrim = choose(&under, dark);
            assert!(least(scrim, &under), "{scrim:?}");
            assert!(scrim.alpha > 0.3 && scrim.alpha < 0.7, "{scrim:?}");
        }
    }

    #[test]
    fn the_asked_lightness_holds_unless_the_other_saves_enough() {
        // White text needs a strong veil on this light grey; dark text none.
        let light = [Rgb::hex(0xb0b0b0)];
        assert_eq!(choose(&light, true), bare(false));
        // Near the middle either text needs only a little, and the
        // lightness asked for is kept.
        let middle = [Rgb::hex(0x7f7f7f)];
        for dark in [false, true] {
            let scrim = choose(&middle, dark);
            assert_eq!(scrim.dark, dark);
            assert!(least(scrim, &middle) && scrim.alpha < MARGIN, "{scrim:?}");
        }
    }

    #[test]
    fn text_under_the_strip_is_veiled_enough_to_read_over() {
        // In text views the paper and its ink are both under the strip.
        for (page, dark) in [
            ([Rgb::hex(0xf6f1e9), Rgb::hex(0x2b2520)], false),
            ([Rgb::hex(0x26211d), Rgb::hex(0xede5d9)], true),
        ] {
            let scrim = choose(&page, dark);
            assert_eq!(scrim.dark, dark);
            assert!(least(scrim, &page), "{scrim:?}");
            assert!(scrim.alpha > 0.3 && scrim.alpha < 0.7, "{scrim:?}");
        }
    }

    #[test]
    fn grounds_choose_their_text() {
        assert!(is_dark(BLACK));
        assert!(is_dark(Rgb::hex(0x26211d)));
        assert!(!is_dark(WHITE));
        assert!(!is_dark(Rgb::hex(0xf6f1e9)));
        assert!(!is_dark(Rgb::hex(0xa0a0a0)));
    }

    #[test]
    fn cells_average_blocks_of_the_rows_asked_for() {
        // Four pixels by three: the top row red, below it white on the
        // left and black on the right.
        let px = |r: u8, g: u8, b: u8| [b, g, r, 255];
        let mut image = Vec::new();
        for _ in 0..4 {
            image.extend(px(255, 0, 0));
        }
        for _ in 0..2 {
            image.extend(px(255, 255, 255));
            image.extend(px(255, 255, 255));
            image.extend(px(0, 0, 0));
            image.extend(px(0, 0, 0));
        }
        let cells = cells(&image, 4, 1..3, 2, BLACK);
        assert_eq!(cells.len(), 2);
        assert!(close(cells[0].luminance(), 1., 1e-4));
        assert!(close(cells[1].luminance(), 0., 1e-4));
        // Half white and half black averages to a mid tone in light.
        let mixed = super::cells(&image, 4, 1..3, 4, BLACK);
        assert_eq!(mixed.len(), 1);
        assert!(close(mixed[0].luminance(), 0.5, 1e-3));
        // Rows past the image are not there to average.
        assert_eq!(super::cells(&image, 4, 3..9, 2, BLACK).len(), 0);
    }

    #[test]
    fn transparent_pixels_show_the_ground() {
        let clear = [0u8, 0, 0, 0].repeat(4);
        let ground = Rgb::hex(0xebe4d8);
        let cells = cells(&clear, 2, 0..2, 2, ground);
        assert_eq!(cells.len(), 1);
        assert!(close(cells[0].r, ground.r, 1e-3) && close(cells[0].b, ground.b, 1e-3));
    }
}
