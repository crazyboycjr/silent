//! A column of pages of known sizes in a vertical scroll, as PDFs and slides
//! are shown: where each page is, which one is at the top, moving between
//! them, and the rail of thumbnails beside them.

use std::ops::Range;

use gpui_kit::*;
use silent::memory::View;
use silent::vim::Motion;

pub const GAP: f32 = 10.;
/// The rail: a thumbnail's width, and the rail's width around it.
pub const THUMB: f32 = 84.;
pub const RAIL: f32 = THUMB + 24.;
/// What Ctrl-e, Ctrl-y and the engaged ↑ and ↓ scroll.
const NUDGE: f32 = 48.;

pub struct Pages {
    /// Each page's width and height, in any unit; only the aspect counts.
    pub sizes: Vec<(f32, f32)>,
    pub scroll: ScrollHandle,
}

impl Pages {
    pub fn new(sizes: Vec<(f32, f32)>) -> Self {
        Self {
            sizes,
            scroll: ScrollHandle::new(),
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
        self.layout_at(self.width())
    }

    /// The column's width as last laid out.
    pub fn width(&self) -> f32 {
        f32::from(self.scroll.bounds().size.width).max(1.)
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

    /// Moves as vim's keys and the arrows ask: j and k by pages, the rest
    /// by the view's height, `height`, or a nudge.
    pub fn motion(&self, motion: Motion, height: f32) {
        let by = |pixels: f32, k: usize| self.scroll_to(self.view_top() + pixels * k as f32);
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
            Motion::Left(_) | Motion::Right(_) | Motion::Home => {}
        }
    }

    /// The page at the top and how far down it the view starts.
    pub fn view(&self) -> Option<View> {
        let page = self.at_top();
        let (top, height) = self.layout().nth(page)?;
        let within = ((self.view_top() - top) / height.max(1.)).clamp(0., 1.);
        (page > 0 || within > 0.001).then_some(View::Pages { page, within })
    }

    /// Goes back to a view, before the column has been laid out: its width
    /// is `width`.
    pub fn restore(&self, view: &View, width: f32) {
        let View::Pages { page, within } = *view else {
            return;
        };
        if let Some((top, height)) = self.layout_at(width).nth(page) {
            // Laying out clamps this to the column.
            self.scroll
                .set_offset(point(px(0.), px(-(top + within * height))));
        }
    }
}
