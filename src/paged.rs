//! A column of pages of known sizes in a scroll, as PDFs and slides are
//! shown: where each page is, which one is at the top, moving between them,
//! and the rail of thumbnails beside them. At a zoom of 1 the pages are as
//! wide as the view; zoomed in, they are wider and scroll sideways too, and
//! zoomed out, narrower and in the middle.

use std::ops::Range;

use gpui_kit::*;
use silent::memory::View;
use silent::vim::Motion;

pub const GAP: f32 = 10.;
/// The rail: a thumbnail's width, and the rail's width around it.
pub const THUMB: f32 = 84.;
pub const RAIL: f32 = THUMB + 24.;
/// What Ctrl-e, Ctrl-y, h, l and the engaged arrows scroll.
const NUDGE: f32 = 48.;

pub struct Pages {
    /// Each page's width and height, in any unit; only the aspect counts.
    pub sizes: Vec<(f32, f32)>,
    pub scroll: ScrollHandle,
    /// The pages' width against the view's: 1 when they fill it.
    pub zoom: f32,
}

impl Pages {
    pub fn new(sizes: Vec<(f32, f32)>, zoom: f32) -> Self {
        Self {
            sizes,
            scroll: ScrollHandle::new(),
            zoom,
        }
    }

    pub fn count(&self) -> usize {
        self.sizes.len()
    }

    /// Where each page starts and how tall it is when the column is `width` wide.
    pub fn layout_at(&self, width: f32) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.sizes.iter().scan(0., move |y, &(w, h)| {
            let height = h * width / w.max(1.);
            let top = *y;
            *y += height + GAP;
            Some((top, height))
        })
    }

    pub fn layout(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.layout_at(self.column())
    }

    /// The view's width as last laid out.
    pub fn width(&self) -> f32 {
        f32::from(self.scroll.bounds().size.width).max(1.)
    }

    /// The pages' width as last laid out.
    pub fn column(&self) -> f32 {
        self.column_in(self.width())
    }

    /// The pages' width in a view `width` wide.
    pub fn column_in(&self, width: f32) -> f32 {
        width * self.zoom
    }

    /// How far the pages stand from the left of a view `width` wide, when
    /// they are narrower than it.
    pub fn inset(&self, width: f32) -> f32 {
        ((width - self.column_in(width)) / 2.).max(0.)
    }

    /// The whole column's height when it is `width` wide.
    pub fn height_at(&self, width: f32) -> f32 {
        self.layout_at(width)
            .last()
            .map_or(0., |(top, height)| top + height)
    }

    pub fn view_top(&self) -> f32 {
        -f32::from(self.scroll.offset().y)
    }

    /// How far the view is scrolled sideways.
    pub fn view_left(&self) -> f32 {
        -f32::from(self.scroll.offset().x)
    }

    /// The page at the top of the view, counted from zero.
    pub fn at_top(&self) -> usize {
        let top = self.view_top();
        self.layout()
            .position(|(start, height)| start + height + GAP > top + 1.)
            .unwrap_or(self.count().saturating_sub(1))
    }

    /// The pages in view when the column is `width` wide and `height` tall.
    pub fn in_view(&self, width: f32, height: f32) -> Option<Range<usize>> {
        self.within(width, self.view_top()..self.view_top() + height)
    }

    /// The pages that reach into `span` of the column when it is `width` wide.
    pub fn within(&self, width: f32, span: Range<f32>) -> Option<Range<usize>> {
        let mut range: Option<Range<usize>> = None;
        for (i, (start, h)) in self.layout_at(width).enumerate() {
            if start + h >= span.start && start <= span.end {
                range = Some(range.map_or(i..i + 1, |r| r.start..i + 1));
            } else if start > span.end {
                break;
            }
        }
        range
    }

    pub fn scroll_to(&self, top: f32) {
        let max = f32::from(self.scroll.max_offset().y);
        let mut offset = self.scroll.offset();
        offset.y = px(-top.clamp(0., max.max(0.)));
        self.scroll.set_offset(offset);
    }

    /// Scrolls sideways to `left`, as far as the pages reach.
    pub fn scroll_across(&self, left: f32) {
        let max = self.column() - self.width();
        let mut offset = self.scroll.offset();
        offset.x = px(-left.clamp(0., max.max(0.)));
        self.scroll.set_offset(offset);
    }

    /// Scrolls as a wheel turned beside the scroll asks.
    pub fn wheel(&self, event: &ScrollWheelEvent, window: &Window) {
        let delta = event.delta.pixel_delta(window.line_height());
        self.scroll_to(self.view_top() - f32::from(delta.y));
    }

    pub fn go_to(&self, page: usize) {
        let page = page.min(self.count().saturating_sub(1));
        if let Some((top, _)) = self.layout().nth(page) {
            self.scroll_to(top);
        }
    }

    /// `n` pages on or back: the next page's top, or going back, this
    /// page's own top first when the view is below it.
    pub fn step(&self, down: bool, n: usize) {
        let page = self.at_top();
        let start = self.layout().nth(page).map_or(0., |(top, _)| top);
        let target = if down {
            page.saturating_add(n)
        } else if self.view_top() > start + 1. {
            page.saturating_sub(n - 1)
        } else {
            page.saturating_sub(n)
        };
        self.go_to(target);
    }

    /// The engaged ← and →, `n` times: sideways while the pages reach
    /// further that way, and once they do not, the previous or next page.
    /// How far they reach is as the last layout had it, to the device
    /// pixel.
    pub fn across(&self, right: bool, n: usize) {
        let (left, max) = (self.view_left(), f32::from(self.scroll.max_offset().x));
        let room = if right { max - left } else { left };
        if room > 0.5 {
            let by = NUDGE * n.min(1 << 16) as f32;
            self.scroll_across(left + if right { by } else { -by });
        } else {
            self.step(right, n);
        }
    }

    /// Moves as vim's keys and the arrows ask: j and k by pages, h and l
    /// sideways, the rest by the view's height, `height`, or a nudge.
    pub fn motion(&self, motion: Motion, height: f32) {
        let by = |pixels: f32, k: usize| self.scroll_to(self.view_top() + pixels * k as f32);
        let across = |pixels: f32, k: usize| {
            self.scroll_across(self.view_left() + pixels * k.min(1 << 16) as f32)
        };
        match motion {
            Motion::Down(k) => self.step(true, k),
            Motion::Up(k) => self.step(false, k),
            Motion::ScrollDown(k) => by(NUDGE, k),
            Motion::ScrollUp(k) => by(-NUDGE, k),
            Motion::HalfDown(k) => by(height / 2., k),
            Motion::HalfUp(k) => by(-height / 2., k),
            Motion::PageDown(k) => by(height * 0.9, k),
            Motion::PageUp(k) => by(-height * 0.9, k),
            Motion::Top => self.go_to(0),
            Motion::Bottom => self.go_to(self.count()),
            Motion::Line(k) => self.go_to(k.saturating_sub(1)),
            Motion::Right(k) => across(NUDGE, k),
            Motion::Left(k) => across(-NUDGE, k),
            Motion::Home => {}
        }
    }

    /// The page at `y` down the column when it is `width` wide, and how far
    /// down it, as a share of its height; in the gap below a page, past 1.
    fn page_at(&self, width: f32, y: f32) -> (usize, f32) {
        let mut last = (0, 0.);
        for (i, (top, height)) in self.layout_at(width).enumerate() {
            last = (i, (y - top) / height.max(1.));
            if y < top + height + GAP {
                break;
            }
        }
        last
    }

    /// Zooms to `zoom` in a view of `view`, keeping the place of the pages
    /// at `at`, from the view's top left, where it is: the place under the
    /// pointer, or for a key the middle of the view's top.
    pub fn zoom_to(&mut self, zoom: f32, at: Point<f32>, view: Size<f32>) {
        let width = view.width.max(1.);
        let before = self.column_in(width);
        let x = (self.view_left() + at.x - self.inset(width)) / before;
        let (page, within) = self.page_at(before, self.view_top() + at.y);
        self.zoom = zoom;
        let after = self.column_in(width);
        let (top, height) = self.layout_at(after).nth(page).unwrap_or_default();
        // The offsets are kept within the column at its new size; the next
        // layout would clamp them only then.
        let max_top = (self.height_at(after) - view.height).max(0.);
        let max_left = (after - width).max(0.);
        let top = (top + within * height - at.y).clamp(0., max_top);
        let left = (x * after + self.inset(width) - at.x).clamp(0., max_left);
        self.scroll.set_offset(point(px(-left), px(-top)));
    }

    /// The page at the top, how far down it the view starts, and where
    /// across the pages the view's middle is: always, while they are wider
    /// than the view, for the zoom kept with it.
    pub fn view(&self) -> Option<View> {
        let page = self.at_top();
        let (top, height) = self.layout().nth(page)?;
        let within = ((self.view_top() - top) / height.max(1.)).clamp(0., 1.);
        let (width, column) = (self.width(), self.column());
        let wide = column > width + 0.5;
        let across = if wide {
            ((self.view_left() + width / 2.) / column).clamp(0., 1.)
        } else {
            0.5
        };
        let moved = page > 0 || within > 0.001 || wide;
        moved.then_some(View::Pages {
            page,
            within,
            across,
        })
    }

    /// Goes back to a view, before the column has been laid out: the view
    /// is `width` wide.
    pub fn restore(&self, view: &View, width: f32) {
        let View::Pages {
            page,
            within,
            across,
        } = *view
        else {
            return;
        };
        let column = self.column_in(width);
        if let Some((top, height)) = self.layout_at(column).nth(page) {
            // Laying out clamps this to the column.
            let left = (across * column - width / 2.).max(0.);
            self.scroll
                .set_offset(point(px(-left), px(-(top + within * height))));
        }
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` attribute would come with it.
    use gpui_kit::{Size, point, px};
    use silent::memory::View;

    use super::Pages;

    /// Three portrait pages in a view 600 by 800.
    fn pages(zoom: f32) -> Pages {
        Pages::new(vec![(600., 800.); 3], zoom)
    }

    fn offset(pages: &Pages) -> (f32, f32) {
        (pages.view_left(), pages.view_top())
    }

    const VIEW: Size<f32> = Size {
        width: 600.,
        height: 800.,
    };

    #[test]
    fn a_zoom_keeps_the_place_under_the_pointer() {
        let mut pages = pages(1.);
        // Halfway down the second page, a third across it.
        pages
            .scroll
            .set_offset(point(px(0.), px(-(810. + 400. - 300.))));
        pages.zoom_to(2., point(200., 300.), VIEW);
        // The page is 1200 by 1600 now, the second starting at 1610.
        let (left, top) = offset(&pages);
        assert!((left + 200. - 400.).abs() < 0.01, "{left}");
        assert!((top + 300. - (1610. + 800.)).abs() < 0.01, "{top}");
        // And back: where it was.
        pages.zoom_to(1., point(200., 300.), VIEW);
        assert_eq!(offset(&pages), (0., 910.));
    }

    #[test]
    fn a_key_keeps_the_top_and_the_middle() {
        let mut pages = pages(1.);
        pages.scroll.set_offset(point(px(0.), px(-1210.)));
        pages.zoom_to(1.5, point(300., 0.), VIEW);
        let (left, top) = offset(&pages);
        // 400 down the second page of 800: 600 down it at 1.5, the first
        // page now 1200 tall.
        assert!((top - (1210. + 600.)).abs() < 0.01, "{top}");
        // The middle stays the middle: 900 wide, 150 either side.
        assert!((left - 150.).abs() < 0.01, "{left}");
    }

    #[test]
    fn zoomed_out_pages_stand_in_the_middle_and_do_not_scroll_sideways() {
        let mut pages = pages(1.);
        pages.zoom_to(0.5, point(300., 0.), VIEW);
        assert_eq!(pages.inset(600.), 150.);
        assert_eq!(offset(&pages).0, 0.);
        // Near the end, the top is kept within the column.
        let mut pages = Pages::new(vec![(600., 800.); 3], 1.);
        pages.scroll.set_offset(point(px(0.), px(-1620.)));
        pages.zoom_to(0.5, point(300., 0.), VIEW);
        let height = pages.height_at(300.);
        assert!(offset(&pages).1 <= (height - 800.).max(0.));
    }

    #[test]
    fn a_zoomed_view_comes_back_across_too() {
        let mut pages = pages(2.);
        pages.restore(
            &View::Pages {
                page: 1,
                within: 0.5,
                across: 0.75,
            },
            600.,
        );
        // 0.75 of 1200 is 900, the view's middle: its left at 600.
        let (left, top) = offset(&pages);
        assert_eq!(left, 600.);
        assert_eq!(top, 1610. + 800.);
        pages.zoom = 1.;
        pages.restore(
            &View::Pages {
                page: 0,
                within: 0.,
                across: 0.5,
            },
            600.,
        );
        assert_eq!(offset(&pages), (0., 0.));
    }
}
