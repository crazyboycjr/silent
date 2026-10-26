//! How big a preview window is and where it goes.

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

    /// Window size for a document whose first page measures `page` points:
    /// the whole page in view, at most twice its actual size.
    pub fn fit_page(&self, page: (f64, f64)) -> (i32, i32) {
        let (pw, ph) = (page.0.max(1.0), page.1.max(1.0));
        let (bw, bh) = self.bounds();
        let actual = self.scale * 96.0 / 72.0;
        let scale = (bw / pw).min(bh / ph).min(2.0 * actual);
        ((pw * scale).round() as i32, (ph * scale).round() as i32)
    }

    /// A card of `width` × `height` logical pixels, scaled to the desktop
    /// and kept within the bounds.
    pub fn card(&self, width: f64, height: f64) -> (i32, i32) {
        let (bw, bh) = self.bounds();
        (
            (width * self.scale).min(bw).round() as i32,
            (height * self.scale).min(bh).round() as i32,
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
            monitor: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            scale: 1.0,
        };
        assert_eq!(size(s.fit_media((4000, 3000))), (1224, 918));
        assert_eq!(size(s.fit_media((800, 600))), (800, 600));
    }

    #[test]
    fn pages_show_whole_and_small_pages_are_not_blown_up() {
        let s = screen();
        assert_eq!(s.fit_page((595.0, 842.0)), (1297, 1836));
        assert_eq!(s.fit_page((842.0, 595.0)), (2598, 1836));
        // A business card at twice its actual size.
        assert_eq!(s.fit_page((252.0, 144.0)), (1141, 652));
    }

    #[test]
    fn cards_scale_with_the_desktop_and_stay_within_the_bounds() {
        let s = screen();
        assert_eq!(s.card(600.0, 360.0), (1019, 611));
        assert_eq!(s.card(4000.0, 4000.0), (3264, 1836));
    }

    #[test]
    fn windows_are_centred_on_their_monitor() {
        let mut s = screen();
        assert_eq!(
            s.centre((1000, 500)),
            Rect {
                x: 1420,
                y: 830,
                width: 1000,
                height: 500
            }
        );
        s.monitor = Rect {
            x: 3840,
            y: 200,
            width: 2560,
            height: 1440,
        };
        assert_eq!(
            s.centre((1001, 501)),
            Rect {
                x: 4619,
                y: 669,
                width: 1001,
                height: 501
            }
        );
        // Larger than the monitor: clamped onto it.
        assert_eq!(s.centre((9000, 9000)), s.monitor);
    }

    #[test]
    fn the_monitor_is_the_one_holding_the_point_or_the_nearest() {
        let left = UHD;
        let right = Rect {
            x: 3840,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let monitors = [left, right];
        assert_eq!(monitor_at(&monitors, (100, 100)), Some(left));
        assert_eq!(monitor_at(&monitors, (4000, 500)), Some(right));
        assert_eq!(monitor_at(&monitors, (5000, 2000)), Some(right));
        assert_eq!(monitor_at(&monitors, (-50, 3000)), Some(left));
        assert_eq!(monitor_at(&[], (0, 0)), None);
    }
}
