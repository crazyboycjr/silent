//! The text on a PDF page, for selecting and copying. hayro's interpreter
//! runs with a device that keeps only glyphs: what each one says, its box,
//! and where the pen started and ended, so that copied text takes its spaces
//! from the gaps between words and its line breaks from the baselines.

use hayro::hayro_interpret::font::Glyph as Drawn;
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
    InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt, interpret_page,
};
use kurbo::{Affine, BezPath, Point, Rect, Shape};

/// A rectangle as fractions of the page, from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug)]
struct Glyph {
    text: String,
    /// The glyph's box, in points from the page's top left.
    rect: Rect,
    baseline: f64,
    /// Where the pen started, and where it ended after the glyph's advance.
    start: f64,
    end: f64,
    em: f64,
}

impl Glyph {
    fn blank(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }
}

pub struct PageText {
    glyphs: Vec<Glyph>,
    width: f64,
    height: f64,
}

struct Collector {
    glyphs: Vec<Glyph>,
}

impl<'a> Device<'a> for Collector {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    // Invisible glyphs count too: they are the text over scanned pages.
    fn draw_glyph(
        &mut self,
        glyph: &Drawn<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _: &Paint<'a>,
        _: &GlyphDrawMode,
    ) {
        let text = match glyph.as_unicode() {
            Some(BfString::Char(c)) => unligature(c.to_string()),
            Some(BfString::String(s)) => unligature(s),
            None => return,
        };
        // Glyph space has 1000 units to the em.
        let m = transform * glyph_transform;
        let [a, b, c, d, _, _] = m.as_coeffs();
        let em = 1000. * (a * d - b * c).abs().sqrt();
        let origin = m * Point::ORIGIN;
        let (outline, advance) = match glyph {
            Drawn::Outline(o) => (Some(o.outline().bounding_box()), o.advance_width()),
            Drawn::Type3(_) => (None, None),
        };
        let rect = match outline.filter(|b| b.area() > 0.) {
            Some(b) => m.transform_rect_bbox(b),
            None => Rect::new(
                origin.x,
                origin.y - em * 0.7,
                origin.x + em * 0.25,
                origin.y + em * 0.2,
            ),
        };
        let end = advance
            .map(|w| (m * Point::new(f64::from(w), 0.)).x)
            .filter(|&end| end > origin.x)
            .unwrap_or(rect.x1);
        self.glyphs.push(Glyph {
            text,
            rect,
            baseline: origin.y,
            start: origin.x,
            end,
            em,
        });
    }
}

impl PageText {
    /// Reads page `index`. This parses the page again without drawing it.
    pub fn read(document: &gpui_pdf::Document, index: usize) -> Option<Self> {
        let page = document.pages().get(index)?;
        let (width, height) = page.render_dimensions();
        let cache = InterpreterCache::new();
        let mut context = Context::new(
            page.initial_transform(true).to_kurbo(),
            Rect::new(0., 0., f64::from(width), f64::from(height)),
            &cache,
            page.xref(),
            InterpreterSettings::default(),
        );
        let mut collector = Collector { glyphs: Vec::new() };
        interpret_page(page, &mut context, &mut collector);
        Some(Self {
            glyphs: collector.glyphs,
            width: f64::from(width).max(1.),
            height: f64::from(height).max(1.),
        })
    }

    fn nearest(&self, (x, y): (f32, f32)) -> Option<usize> {
        let point = Point::new(f64::from(x) * self.width, f64::from(y) * self.height);
        let distance = |r: &Rect| {
            let dx = (r.x0 - point.x).max(point.x - r.x1).max(0.);
            let dy = (r.y0 - point.y).max(point.y - r.y1).max(0.);
            dx * dx + dy * dy
        };
        self.glyphs
            .iter()
            .enumerate()
            .filter(|(_, g)| !g.blank())
            .min_by(|(_, a), (_, b)| distance(&a.rect).total_cmp(&distance(&b.rect)))
            .map(|(i, _)| i)
    }

    /// The text from the glyph nearest `from` to the one nearest `to`, in
    /// the order the page draws them, and its boxes, one for each line.
    pub fn select(&self, from: (f32, f32), to: (f32, f32)) -> Option<(String, Vec<Area>)> {
        let (a, b) = (self.nearest(from)?, self.nearest(to)?);
        let glyphs = &self.glyphs[a.min(b)..=a.max(b)];
        let mut areas: Vec<(Rect, f64)> = Vec::new();
        for glyph in glyphs.iter().filter(|g| !g.blank()) {
            match areas.last_mut() {
                Some((rect, baseline)) if same_line(*baseline, glyph) => {
                    *rect = rect.union(glyph.rect);
                }
                _ => areas.push((glyph.rect, glyph.baseline)),
            }
        }
        let areas = areas
            .into_iter()
            .map(|(r, _)| Area {
                x: (r.x0 / self.width) as f32,
                y: (r.y0 / self.height) as f32,
                w: (r.width() / self.width) as f32,
                h: (r.height() / self.height) as f32,
            })
            .collect();
        Some((join(glyphs), areas))
    }
}

/// Typeset ligatures as the letters they join, as they are searched for.
fn unligature(text: String) -> String {
    if !text.contains(|c| ('\u{fb00}'..='\u{fb06}').contains(&c)) {
        return text;
    }
    text.chars()
        .map(|c| match c {
            '\u{fb00}' => "ff".into(),
            '\u{fb01}' => "fi".into(),
            '\u{fb02}' => "fl".into(),
            '\u{fb03}' => "ffi".into(),
            '\u{fb04}' => "ffl".into(),
            '\u{fb05}' | '\u{fb06}' => "st".into(),
            c => c.to_string(),
        })
        .collect()
}

fn same_line(baseline: f64, glyph: &Glyph) -> bool {
    (glyph.baseline - baseline).abs() < glyph.em.max(1e-3) * 0.5
}

/// The glyphs' text, with a space where a gap is wider than letters are
/// spaced and a line break where the baseline moves.
fn join(glyphs: &[Glyph]) -> String {
    let mut out = String::new();
    let mut previous: Option<&Glyph> = None;
    for glyph in glyphs {
        if glyph.blank() {
            if !out.is_empty() && !out.ends_with([' ', '\n']) {
                out.push(' ');
            }
            continue;
        }
        if let Some(p) = previous {
            let em = p.em.max(glyph.em);
            if !same_line(p.baseline, glyph) {
                out.truncate(out.trim_end_matches(' ').len());
                out.push('\n');
            } else if (glyph.start - p.end > em * 0.12 || glyph.start < p.start - em)
                && !out.ends_with([' ', '\n'])
            {
                out.push(' ');
            }
        }
        out.push_str(&glyph.text);
        previous = Some(glyph);
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(text: &str, start: f64, width: f64, baseline: f64) -> Glyph {
        Glyph {
            text: text.into(),
            rect: Rect::new(start, baseline - 7., start + width, baseline + 2.),
            baseline,
            start,
            end: start + width,
            em: 10.,
        }
    }

    #[test]
    fn gaps_become_spaces_and_baselines_line_breaks() {
        let glyphs = vec![
            glyph("E", 0., 6., 20.),
            glyph("m", 6.2, 8., 20.),
            glyph("R", 17., 6., 20.),
            glyph(".", 23., 2., 20.),
            glyph("n", 0., 5., 34.),
            glyph("o", 5., 5., 34.),
        ];
        assert_eq!(join(&glyphs), "Em R.\nno");
    }

    #[test]
    fn spaces_on_the_page_are_kept_once() {
        let glyphs = vec![
            glyph("a", 0., 5., 20.),
            glyph(" ", 5., 2.5, 20.),
            glyph("b", 9., 5., 20.),
        ];
        assert_eq!(join(&glyphs), "a b");
    }

    #[test]
    fn a_selection_has_one_box_per_line() {
        let text = PageText {
            glyphs: vec![
                glyph("a", 10., 5., 20.),
                glyph("b", 15., 5., 20.),
                glyph("c", 10., 5., 34.),
            ],
            width: 100.,
            height: 100.,
        };
        let (quote, areas) = text.select((0.11, 0.18), (0.12, 0.33)).unwrap();
        assert_eq!(quote, "ab\nc");
        assert_eq!(areas.len(), 2);
        assert!((areas[0].w - 0.1).abs() < 1e-6);
    }
}
