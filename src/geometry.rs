//! How big a preview window is and where it goes. The content fills the
//! window, unless a minimap or a thumbnail rail is docked at its right: then
//! the window is wider by the dock, and the content keeps its size and place.

/// A rectangle in root-window pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn contains(&self, (x, y): (i32, i32)) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    pub fn centre(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }

    fn distance_squared(&self, (x, y): (i32, i32)) -> i64 {
        let dx = (self.x - x).max(x - (self.x + self.width - 1)).max(0) as i64;
        let dy = (self.y - y).max(y - (self.y + self.height - 1)).max(0) as i64;
        dx * dx + dy * dy
    }
}

/// The monitor a preview opens on, and the desktop's scale (`Xft.dpi` / 96).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub monitor: Rect,
    pub scale: f64,
}

/// How the pixels of an image are stretched to its display size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Smooth,
    /// Whole-number enlargement of a tiny image, pixel for pixel.
    Nearest,
}

/// The display size of an image or video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    pub width: i32,
    pub height: i32,
    pub filter: Filter,
}

/// No preview grows past this share of its monitor.
const MAX_SHARE: f64 = 0.85;
/// Content shorter than this on its long side, in logical pixels, is enlarged further.
const MIN_LONG_SIDE: f64 = 240.0;
/// Images up to this many pixels on their long side are icons or pixel art.
const PIXEL_ART: f64 = 64.0;

impl Screen {
    /// The largest window a preview may have.
    pub fn bounds(&self) -> (f64, f64) {
        (
            self.monitor.width as f64 * MAX_SHARE,
            self.monitor.height as f64 * MAX_SHARE,
        )
    }

    /// `logical` pixels in root-window pixels.
    pub fn pixels(&self, logical: f64) -> i32 {
        (logical * self.scale).round() as i32
    }

    /// The window around content of `size` with a dock `dock` logical
    /// pixels wide at its right.
    pub fn window(&self, size: (i32, i32), dock: f64) -> (i32, i32) {
        (size.0 + self.pixels(dock), size.1)
    }

    /// The largest content beside a dock `dock` logical pixels wide.
    fn room(&self, dock: f64) -> (f64, f64) {
        let (bw, bh) = self.bounds();
        ((bw - f64::from(self.pixels(dock))).max(1.0), bh)
    }

    /// Display size for an image or video of `natural` pixels. It keeps the
    /// content's aspect, enlarges small content by the desktop scale (and up
    /// to twice that while it would still look small), and never leaves the
    /// bounds.
    pub fn fit_media(&self, natural: (u32, u32)) -> Fit {
        let (w, h) = (natural.0.max(1) as f64, natural.1.max(1) as f64);
        let (bw, bh) = self.bounds();
        let s = self.scale;
        let fill = (bw / w).min(bh / h);
        let mut scale = if fill <= s {
            fill
        } else {
            (fill / 2.0).clamp(s, 2.0 * s)
        };
        let long = w.max(h);
        if long * scale < MIN_LONG_SIDE * s {
            scale = (MIN_LONG_SIDE * s / long).min(fill);
        }
        let mut filter = Filter::Smooth;
        if long <= PIXEL_ART && scale >= 2.0 {
            scale = scale.floor();
            filter = Filter::Nearest;
        }
        Fit {
            width: ((w * scale).round() as i32).max(1),
            height: ((h * scale).round() as i32).max(1),
            filter,
        }
    }

    /// Content size for a document whose first page measures `page` points,
    /// beside a dock `dock` logical pixels wide: the whole page in view, at
    /// most twice its actual size, and the page and the dock within the
    /// bounds.
    pub fn fit_page(&self, page: (f64, f64), dock: f64) -> (i32, i32) {
        let (pw, ph) = (page.0.max(1.0), page.1.max(1.0));
        let (rw, rh) = self.room(dock);
        let actual = self.scale * 96.0 / 72.0;
        let scale = (rw / pw).min(rh / ph).min(2.0 * actual);
        ((pw * scale).round() as i32, (ph * scale).round() as i32)
    }

    /// Content of `width` × `height` logical pixels beside a dock `dock`
    /// logical pixels wide, scaled to the desktop and kept, with the dock,
    /// within the bounds.
    pub fn card(&self, width: f64, height: f64, dock: f64) -> (i32, i32) {
        let (rw, rh) = self.room(dock);
        (
            (width * self.scale).min(rw).round() as i32,
            (height * self.scale).min(rh).round() as i32,
        )
    }

    /// Where a window of `size` goes: centred on the monitor and on it.
    pub fn centre(&self, size: (i32, i32)) -> Rect {
        let m = self.monitor;
        let (width, height) = (size.0.min(m.width), size.1.min(m.height));
        Rect {
            x: m.x + (m.width - width) / 2,
            y: m.y + (m.height - height) / 2,
            width,
            height,
        }
    }

    /// A window of `size` centred on `point`, moved onto the monitor where
    /// it would leave it.
    pub fn around(&self, (x, y): (i32, i32), size: (i32, i32)) -> Rect {
        let m = self.monitor;
        let (width, height) = (size.0.clamp(1, m.width), size.1.clamp(1, m.height));
        Rect {
            x: (x - width / 2).clamp(m.x, m.x + m.width - width),
            y: (y - height / 2).clamp(m.y, m.y + m.height - height),
            width,
            height,
        }
    }
}

/// `frame` once the dock at its right has grown by `by` pixels, or shrunk
/// when `by` is negative: the window keeps its left edge, so the content
/// stays where it is, unless it would then cross the right edge of
/// `monitor`; it moves left just enough not to.
pub fn redock(frame: Rect, monitor: Rect, by: i32) -> Rect {
    let width = (frame.width + by).clamp(1, monitor.width);
    let right = monitor.x + monitor.width;
    Rect {
        x: frame.x.min(right - width).max(monitor.x),
        width,
        ..frame
    }
}

/// The monitor holding `point`, else the one nearest to it.
pub fn monitor_at(monitors: &[Rect], point: (i32, i32)) -> Option<Rect> {
    monitors
        .iter()
        .find(|m| m.contains(point))
        .or_else(|| monitors.iter().min_by_key(|m| m.distance_squared(point)))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    const UHD: Rect = Rect {
        x: 0,
        y: 0,
        width: 3840,
        height: 2160,
    };

    fn screen() -> Screen {
        Screen {
            monitor: UHD,
            scale: 163.0 / 96.0,
        }
    }

    fn size(fit: Fit) -> (i32, i32) {
        (fit.width, fit.height)
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn large_content_fills_the_bounds() {
        let s = screen();
        assert_eq!(size(s.fit_media((3840, 2160))), (3264, 1836));
        assert_eq!(size(s.fit_media((8000, 4500))), (3264, 1836));
        assert_eq!(size(s.fit_media((3024, 4032))), (1377, 1836));
        assert_eq!(size(s.fit_media((12000, 2000))), (3264, 544));
    }

    #[test]
    fn content_is_enlarged_by_the_desktop_scale() {
        let s = screen();
        assert_eq!(size(s.fit_media((1024, 768))), (1739, 1304));
        assert_eq!(size(s.fit_media((1280, 720))), (2173, 1223));
    }

    #[test]
    fn small_content_grows_up_to_twice_the_desktop_scale() {
        let s = screen();
        // Half the bounds, which is a little more than the desktop scale.
        assert_eq!(size(s.fit_media((640, 480))), (1224, 918));
        // Capped at twice the desktop scale.
        assert_eq!(size(s.fit_media((300, 200))), (1019, 679));
    }

    #[test]
    fn tiny_images_grow_pixel_for_pixel_without_filling_the_screen() {
        let s = screen();
        let icon = s.fit_media((16, 16));
        assert_eq!(icon.filter, Filter::Nearest);
        assert_eq!(icon.width % 16, 0);
        assert!((240..=480).contains(&icon.width), "{icon:?}");
        let sprite = s.fit_media((48, 32));
        assert_eq!(sprite.filter, Filter::Nearest);
        assert_eq!((sprite.width / 48, sprite.height / 32), (8, 8));
        assert_eq!(s.fit_media((256, 256)).filter, Filter::Smooth);
    }

    #[test]
    fn fits_keep_the_aspect_and_stay_within_the_bounds() {
        let s = screen();
        let (bw, bh) = s.bounds();
        for (w, h) in [
            (1, 1),
            (1, 5000),
            (5000, 1),
            (16, 9),
            (333, 777),
            (1920, 1080),
            (4000, 3000),
            (20000, 20000),
        ] {
            let fit = s.fit_media((w, h));
            assert!(fit.width as f64 <= bw.ceil() && fit.height as f64 <= bh.ceil());
            let error = fit.width as f64 * h as f64 - fit.height as f64 * w as f64;
            assert!(error.abs() <= w.max(h) as f64, "{w}x{h} -> {fit:?}");
        }
    }

    #[test]
    fn without_a_desktop_scale_photos_still_fit() {
        let s = Screen {
            monitor: rect(0, 0, 1920, 1080),
            scale: 1.0,
        };
        assert_eq!(size(s.fit_media((4000, 3000))), (1224, 918));
        assert_eq!(size(s.fit_media((800, 600))), (800, 600));
    }

    #[test]
    fn the_window_holds_the_content_and_its_dock() {
        let s = screen();
        assert_eq!(s.window((1000, 500), 0.0), (1000, 500));
        assert_eq!(s.window((1000, 500), 108.0), (1183, 500));
    }

    #[test]
    fn pages_show_whole_and_small_pages_are_not_blown_up() {
        let s = screen();
        assert_eq!(s.fit_page((595.0, 842.0), 0.0), (1297, 1836));
        assert_eq!(s.fit_page((842.0, 595.0), 0.0), (2598, 1836));
        // A business card at twice its actual size.
        assert_eq!(s.fit_page((252.0, 144.0), 0.0), (1141, 652));
    }

    #[test]
    fn a_docked_rail_widens_the_window_and_leaves_the_page_alone() {
        let s = screen();
        // The rail's 108 logical pixels are 183 on this screen.
        assert_eq!(s.pixels(108.0), 183);
        // A portrait page is as tall as the bounds either way: the window
        // takes the rail on, and turning the rail off takes it away again.
        let page = s.fit_page((595.0, 842.0), 108.0);
        assert_eq!(page, s.fit_page((595.0, 842.0), 0.0));
        assert_eq!(s.window(page, 108.0), (1480, 1836));
        assert_eq!(s.window(page, 0.0), (1297, 1836));
        // A page too wide for the rail beside it gives way, just enough.
        assert_eq!(s.fit_page((1600.0, 400.0), 0.0), (3264, 816));
        assert_eq!(s.fit_page((1600.0, 400.0), 108.0), (3081, 770));
        assert_eq!(s.window((3081, 770), 108.0), (3264, 770));
    }

    #[test]
    fn docked_windows_stay_within_the_bounds() {
        let s = screen();
        let (bw, bh) = s.bounds();
        let within = |(w, h): (i32, i32)| w as f64 <= bw.ceil() && h as f64 <= bh.ceil();
        for dock in [0.0, 108.0, 122.0] {
            for page in [
                (595.0, 842.0),
                (842.0, 595.0),
                (960.0, 540.0),
                (100.0, 9000.0),
                (9000.0, 100.0),
            ] {
                let content = s.fit_page(page, dock);
                assert!(within(s.window(content, dock)), "{page:?} {dock}");
            }
            for card in [(860.0, 964.0), (4000.0, 300.0), (300.0, 4000.0)] {
                let content = s.card(card.0, card.1, dock);
                assert!(within(s.window(content, dock)), "{card:?} {dock}");
            }
        }
    }

    #[test]
    fn cards_scale_with_the_desktop_and_stay_within_the_bounds() {
        let s = screen();
        assert_eq!(s.card(600.0, 360.0, 0.0), (1019, 611));
        assert_eq!(s.card(4000.0, 4000.0, 0.0), (3264, 1836));
        // Text as wide as the bounds makes room for its minimap.
        assert_eq!(s.card(4000.0, 300.0, 122.0), (3057, 509));
    }

    #[test]
    fn docking_keeps_the_content_in_place() {
        let frame = rect(1000, 500, 800, 600);
        let grown = redock(frame, UHD, 207);
        assert_eq!(grown, rect(1000, 500, 1007, 600));
        // Undocking gives the same window back.
        assert_eq!(redock(grown, UHD, -207), frame);
    }

    #[test]
    fn a_dock_at_the_monitors_edge_moves_the_window_left_just_enough() {
        let grown = redock(rect(3000, 500, 800, 600), UHD, 207);
        assert_eq!(grown, rect(2833, 500, 1007, 600));
        // Undocking leaves it where it moved to.
        assert_eq!(redock(grown, UHD, -207), rect(2833, 500, 800, 600));
        // On a monitor to the right of another.
        let right = rect(3840, 0, 3840, 2160);
        assert_eq!(
            redock(rect(7000, 100, 600, 400), right, 300),
            rect(6780, 100, 900, 400)
        );
        // Never wider than the monitor, nor off its left edge.
        assert_eq!(
            redock(rect(100, 0, 3800, 600), UHD, 207),
            rect(0, 0, 3840, 600)
        );
    }

    #[test]
    fn windows_are_centred_on_their_monitor() {
        let mut s = screen();
        assert_eq!(s.centre((1000, 500)), rect(1420, 830, 1000, 500));
        s.monitor = rect(3840, 200, 2560, 1440);
        assert_eq!(s.centre((1001, 501)), rect(4619, 669, 1001, 501));
        // Larger than the monitor: clamped onto it.
        assert_eq!(s.centre((9000, 9000)), s.monitor);
    }

    #[test]
    fn a_window_resized_around_its_centre_stays_on_the_monitor() {
        let s = screen();
        let shown = rect(1420, 830, 1000, 500);
        assert_eq!(
            s.around(shown.centre(), (600, 300)),
            rect(1620, 930, 600, 300)
        );
        assert_eq!(s.around(shown.centre(), (1000, 500)), shown);
        // Near a corner a larger window is pushed back onto the monitor.
        assert_eq!(s.around((3700, 100), (1000, 800)), rect(2840, 0, 1000, 800));
    }

    #[test]
    fn the_monitor_is_the_one_holding_the_point_or_the_nearest() {
        let left = UHD;
        let right = rect(3840, 0, 1920, 1080);
        let monitors = [left, right];
        assert_eq!(monitor_at(&monitors, (100, 100)), Some(left));
        assert_eq!(monitor_at(&monitors, (4000, 500)), Some(right));
        assert_eq!(monitor_at(&monitors, (5000, 2000)), Some(right));
        assert_eq!(monitor_at(&monitors, (-50, 3000)), Some(left));
        assert_eq!(monitor_at(&[], (0, 0)), None);
    }
}
