//! How a picture sits in the window as Ctrl with + and - zoom it: how large
//! it is drawn, against its fit, and which point of it is in the middle of
//! the view. Larger than the view, it covers it and a drag moves it; smaller,
//! it stands in the middle.

use gpui_kit::{Bounds, Pixels, Point, Size, point, px, size};

pub struct Framing {
    /// 1 when the picture fits the view, as it opens.
    pub level: f32,
    /// The point of the picture in the middle of the view, as shares of its
    /// width and height.
    pub centre: (f32, f32),
    /// A drag under way: where it began, and the centre then.
    held: Option<(Point<Pixels>, (f32, f32))>,
}

impl Framing {
    pub fn new(level: f32, centre: (f32, f32)) -> Self {
        Self {
            level,
            centre,
            held: None,
        }
    }

    /// Where a picture of `shape`, its width and height, is drawn in a view
    /// of `area`.
    pub fn rect(&self, shape: (f32, f32), area: Size<Pixels>) -> Bounds<Pixels> {
        let (w, h) = drawn(shape, area, self.level);
        let (aw, ah) = (f32::from(area.width), f32::from(area.height));
        let place = |length: f32, view: f32, centre: f32| {
            if length <= view {
                (view - length) / 2.
            } else {
                (view / 2. - centre * length).clamp(view - length, 0.)
            }
        };
        Bounds::new(
            point(
                px(place(w, aw, self.centre.0)),
                px(place(h, ah, self.centre.1)),
            ),
            size(px(w), px(h)),
        )
    }

    /// Whether the picture is larger than the view either way, so that a
    /// drag moves it.
    pub fn movable(&self, shape: (f32, f32), area: Size<Pixels>) -> bool {
        let (w, h) = drawn(shape, area, self.level);
        w > f32::from(area.width) + 0.5 || h > f32::from(area.height) + 0.5
    }

    /// To `level`, keeping the point of the picture under `at`, from the
    /// view's top left, where it is; without one, the point in the middle.
    pub fn zoom(
        &mut self,
        level: f32,
        at: Option<Point<Pixels>>,
        shape: (f32, f32),
        area: Size<Pixels>,
    ) {
        if let Some(at) = at {
            let before = self.rect(shape, area);
            let share = |at: Pixels, start: Pixels, length: Pixels| {
                ((at - start) / length.max(px(1e-3))).clamp(0., 1.)
            };
            let x = share(at.x, before.left(), before.size.width);
            let y = share(at.y, before.top(), before.size.height);
            let (w, h) = drawn(shape, area, level);
            // The point at `at` is where the middle of the view would put
            // the centre, less the way from the centre to `at`.
            let (aw, ah) = (f32::from(area.width), f32::from(area.height));
            self.centre = (
                x + (aw / 2. - f32::from(at.x)) / w,
                y + (ah / 2. - f32::from(at.y)) / h,
            );
        }
        self.level = level;
        self.keep_within(shape, area);
    }

    /// A drag begins at `at`.
    pub fn hold(&mut self, at: Point<Pixels>) {
        self.held = Some((at, self.centre));
    }

    pub fn held(&self) -> bool {
        self.held.is_some()
    }

    pub fn release(&mut self) {
        self.held = None;
    }

    /// The pointer is at `at` while a drag holds the picture: it follows,
    /// as far as the picture's edges; a drag past an edge is not stored up.
    pub fn drag(&mut self, at: Point<Pixels>, shape: (f32, f32), area: Size<Pixels>) -> bool {
        let Some((from, centre)) = self.held else {
            return false;
        };
        let (w, h) = drawn(shape, area, self.level);
        self.centre = (
            centre.0 - f32::from(at.x - from.x) / w,
            centre.1 - f32::from(at.y - from.y) / h,
        );
        self.keep_within(shape, area);
        self.held = Some((at, self.centre));
        true
    }

    /// The centre where the picture still covers the view, or in the middle
    /// where it is smaller.
    fn keep_within(&mut self, shape: (f32, f32), area: Size<Pixels>) {
        let (w, h) = drawn(shape, area, self.level);
        let within = |centre: f32, length: f32, view: f32| {
            if length <= view {
                0.5
            } else {
                let half = view / 2. / length;
                centre.clamp(half, 1. - half)
            }
        };
        self.centre = (
            within(self.centre.0, w, f32::from(area.width)),
            within(self.centre.1, h, f32::from(area.height)),
        );
    }
}

/// The size a picture of `shape` is drawn at `level` in a view of `area`:
/// at 1, whole and as large as the view lets it be.
fn drawn(shape: (f32, f32), area: Size<Pixels>, level: f32) -> (f32, f32) {
    let (w, h) = (shape.0.max(1.), shape.1.max(1.));
    let fit = (f32::from(area.width) / w).min(f32::from(area.height) / h);
    (w * fit * level, h * fit * level)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Size<Pixels> = Size {
        width: px(800.),
        height: px(600.),
    };
    /// A picture of the window's shape, and a wide one.
    const SAME: (f32, f32) = (4000., 3000.);
    const WIDE: (f32, f32) = (1600., 600.);

    fn close(a: Pixels, b: f32) -> bool {
        (f32::from(a) - b).abs() < 0.01
    }

    #[test]
    fn at_fit_the_picture_fills_the_view_and_stays() {
        let framing = Framing::new(1., (0.5, 0.5));
        let rect = framing.rect(SAME, AREA);
        assert!(close(rect.left(), 0.) && close(rect.top(), 0.));
        assert!(close(rect.size.width, 800.) && close(rect.size.height, 600.));
        assert!(!framing.movable(SAME, AREA));
        // A wide one stands in the middle, top to bottom.
        let rect = framing.rect(WIDE, AREA);
        assert!(close(rect.top(), 150.) && close(rect.size.width, 800.));
    }

    #[test]
    fn a_zoom_keeps_the_point_under_the_pointer() {
        let mut framing = Framing::new(1., (0.5, 0.5));
        let at = point(px(200.), px(450.));
        framing.zoom(2., Some(at), SAME, AREA);
        let rect = framing.rect(SAME, AREA);
        // 200 across of 800 was a quarter of the picture; it still is.
        let x = (at.x - rect.left()) / rect.size.width;
        let y = (at.y - rect.top()) / rect.size.height;
        assert!(
            (x - 0.25).abs() < 1e-4 && (y - 0.75).abs() < 1e-4,
            "{x} {y}"
        );
        assert!(framing.movable(SAME, AREA));
        // And on again, from there.
        framing.zoom(3.3, Some(at), SAME, AREA);
        let rect = framing.rect(SAME, AREA);
        let x = (at.x - rect.left()) / rect.size.width;
        assert!((x - 0.25).abs() < 1e-4, "{x}");
    }

    #[test]
    fn a_key_zooms_about_the_middle_and_the_picture_covers_the_view() {
        let mut framing = Framing::new(1., (0.5, 0.5));
        framing.zoom(2., None, SAME, AREA);
        let rect = framing.rect(SAME, AREA);
        assert!(close(rect.left(), -400.) && close(rect.top(), -300.));
        // About a corner, the picture still reaches the view's edges.
        framing.zoom(1.1, Some(point(px(0.), px(0.))), SAME, AREA);
        framing.zoom(2., Some(point(px(799.), px(599.))), SAME, AREA);
        let rect = framing.rect(SAME, AREA);
        assert!(f32::from(rect.left()) <= 0. && f32::from(rect.right()) >= 800. - 0.01);
        assert!(f32::from(rect.top()) <= 0. && f32::from(rect.bottom()) >= 600. - 0.01);
        // Smaller than the view, it goes back to the middle.
        framing.zoom(0.5, Some(point(px(10.), px(10.))), SAME, AREA);
        let rect = framing.rect(SAME, AREA);
        assert!(close(rect.left(), 200.) && close(rect.top(), 150.));
        assert_eq!(framing.centre, (0.5, 0.5));
    }

    #[test]
    fn a_drag_moves_the_picture_as_far_as_its_edges() {
        let mut framing = Framing::new(2., (0.5, 0.5));
        framing.hold(point(px(400.), px(300.)));
        assert!(framing.drag(point(px(500.), px(250.)), SAME, AREA));
        let rect = framing.rect(SAME, AREA);
        assert!(close(rect.left(), -300.) && close(rect.top(), -350.));
        // Past its edge, it stops at it, and coming back moves it at once.
        framing.drag(point(px(2000.), px(300.)), SAME, AREA);
        assert!(close(framing.rect(SAME, AREA).left(), 0.));
        framing.drag(point(px(1900.), px(300.)), SAME, AREA);
        assert!(close(framing.rect(SAME, AREA).left(), -100.));
        framing.release();
        assert!(!framing.drag(point(px(0.), px(0.)), SAME, AREA));
        // Wider than the view but not taller: only across.
        let mut wide = Framing::new(1.5, (0.5, 0.5));
        wide.hold(point(px(0.), px(0.)));
        wide.drag(point(px(-100.), px(-100.)), WIDE, AREA);
        assert_eq!(wide.centre.1, 0.5);
        assert!(wide.centre.0 > 0.5);
    }
}
