//! A column of pages of known sizes in a vertical scroll, as PDFs and slides
//! are shown: where each page is, which one is at the top, moving between them.

use std::ops::Range;

use gpui_kit::*;

pub const GAP: f32 = 10.;

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
        self.layout_at(f32::from(self.scroll.bounds().size.width).max(1.))
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
        let top = self.view_top();
        let bottom = top + height;
        let mut range: Option<Range<usize>> = None;
        for (i, (start, h)) in self.layout_at(width).enumerate() {
            if start + h >= top && start <= bottom {
                range = Some(range.map_or(i..i + 1, |r| r.start..i + 1));
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

    pub fn go_to(&self, page: usize) {
        let page = page.min(self.count().saturating_sub(1));
        if let Some((top, _)) = self.layout().nth(page) {
            self.scroll_to(top);
        }
    }

    /// The next page's top, or this page's when the view is below it.
    pub fn step(&self, down: bool) {
        let page = self.at_top();
        let start = self.layout().nth(page).map_or(0., |(top, _)| top);
        let target = if down {
            page + 1
        } else if self.view_top() > start + 1. {
            page
        } else {
            page.saturating_sub(1)
        };
        self.go_to(target);
    }

    pub fn scroll_by_page(&self, window: &Window, down: bool) {
        let height = f32::from(window.viewport_size().height) * 0.9;
        self.scroll_to(self.view_top() + if down { height } else { -height });
    }

    /// How far down the column the view is, from 0 to 1.
    pub fn fraction(&self) -> f32 {
        let max = f32::from(self.scroll.max_offset().y);
        if max > 0. {
            (self.view_top() / max).clamp(0., 1.)
        } else {
            0.
        }
    }
}
