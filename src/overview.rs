//! The overview of a long view: a minimap of text, a rail of page
//! thumbnails, or a miniature of a document's column. Text shows its minimap
//! over its right edge while it scrolls, fading 1.5 s after, or docked
//! beside it once M has pinned it; pages and documents keep theirs docked
//! until M takes it away. A docked overview widens the window rather than
//! covering the content. Without one, a slim scrollbar shows while the view
//! scrolls. Pressing or dragging on any of them moves the view.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use silent::memory;

use crate::theme::{Theme, progress};

/// How long the overview stays after the view stops moving.
const LINGER: Duration = Duration::from_millis(1500);
const FADE: Duration = Duration::from_millis(180);
/// The scrollbar's inset from the window's edges, and its thumb's least height.
const INSET: f32 = 12.;
const MIN_THUMB: f64 = 28.;
const BAR_WIDTH: f32 = 22.;

/// Which overview a view has, and so what M cycles through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Text, code and hexdumps: shown while scrolling, docked, or off.
    Minimap,
    /// PDFs, slides and documents: docked or off.
    Rail,
}

impl Family {
    fn key(self) -> &'static str {
        match self {
            Family::Minimap => "minimap",
            Family::Rail => "rail",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Over the view's edge while it scrolls.
    #[default]
    Auto,
    /// Docked beside the view, always shown.
    On,
    /// The slim scrollbar instead, while the view scrolls.
    Off,
}

impl Mode {
    pub fn next(self, family: Family) -> Self {
        match (family, self) {
            (Family::Minimap, Mode::Auto) => Mode::On,
            (Family::Minimap, Mode::On) => Mode::Off,
            (Family::Minimap, Mode::Off) => Mode::Auto,
            (Family::Rail, Mode::Off) => Mode::On,
            (Family::Rail, _) => Mode::Off,
        }
    }

    /// The mode chosen last for `family`, kept until the next reboot.
    pub fn load(family: Family) -> Self {
        match memory::setting(family.key()).as_deref() {
            Some("on") => Mode::On,
            Some("off") => Mode::Off,
            _ if family == Family::Rail => Mode::On,
            _ => Mode::Auto,
        }
    }

    pub fn save(self, family: Family) {
        let name = match self {
            Mode::Auto => "auto",
            Mode::On => "on",
            Mode::Off => "off",
        };
        memory::set_setting(family.key(), name);
    }

    /// `Minimap: while moving`, for the info line, in the words of the card
    /// of keys.
    pub fn describe(self, what: &str) -> String {
        match self {
            Mode::Auto => format!("{what}: while moving"),
            Mode::On => format!("{what}: always"),
            Mode::Off => format!("{what}: never"),
        }
    }
}

/// A view `view` units tall on content `content` units tall, scrolled down
/// to `top`, overviewed at `scale` pixels a unit on a track `track` pixels
/// tall. The thumb stands for the part in view. When the overview is taller
/// than the track it scrolls along, so that what is under the thumb is
/// what is in view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub content: f64,
    pub view: f64,
    pub top: f64,
    pub scale: f64,
    pub track: f64,
    pub min_thumb: f64,
}

impl Geometry {
    pub fn new(content: f64, view: f64, top: f64, scale: f64, track: f64) -> Self {
        Self {
            content,
            view,
            top,
            scale,
            track: track.max(1.),
            min_thumb: 0.,
        }
    }

    /// The same view on a plain scrollbar: the whole content on the track.
    pub fn bar(self) -> Self {
        Self {
            scale: self.track / self.content.max(1e-9),
            min_thumb: MIN_THUMB,
            ..self
        }
    }

    pub fn scrollable(&self) -> bool {
        self.content - self.view > 1e-6
    }

    fn max_top(&self) -> f64 {
        (self.content - self.view).max(0.)
    }

    /// How far down the content the view is, from 0 to 1.
    pub fn fraction(&self) -> f64 {
        match self.max_top() {
            max if max > 0. => (self.top / max).clamp(0., 1.),
            _ => 0.,
        }
    }

    /// The part of the track the overview fills.
    pub fn span(&self) -> f64 {
        (self.content * self.scale).min(self.track)
    }

    pub fn thumb_height(&self) -> f64 {
        (self.view * self.scale)
            .max(self.min_thumb)
            .min(self.span())
    }

    pub fn thumb_top(&self) -> f64 {
        self.fraction() * (self.span() - self.thumb_height())
    }

    /// How far the overview itself is scrolled, in pixels.
    pub fn shift(&self) -> f64 {
        self.fraction() * (self.content * self.scale - self.span()).max(0.)
    }

    /// The top that puts the thumb's top `y` pixels down the track.
    pub fn top_for_thumb(&self, y: f64) -> f64 {
        let room = self.span() - self.thumb_height();
        if room <= 0. {
            return 0.;
        }
        (y / room).clamp(0., 1.) * self.max_top()
    }

    /// The top that centres the view on what the overview shows `y` pixels
    /// down the track.
    pub fn top_centred_at(&self, y: f64) -> f64 {
        let unit = (y + self.shift()) / self.scale;
        (unit - self.view / 2.).clamp(0., self.max_top())
    }
}

/// The overview's state in a view that has one.
#[derive(Default)]
pub struct Overview {
    pub mode: Mode,
    /// Docked beside the view now. It follows the mode once the window has
    /// made room, which the preview sees to.
    pub docked: bool,
    /// Recently scrolled, hovered or dragged.
    up: bool,
    /// Drawn still while it fades out.
    fading: bool,
    /// Counts showings and hidings, so that each fade is a new animation.
    epoch: usize,
    hovered: bool,
    /// Where on the thumb a drag holds it, in pixels from its top.
    grip: Option<f64>,
    /// The track as last laid out.
    track: Rc<Cell<Bounds<Pixels>>>,
    timer: Option<Task<()>>,
}

/// What shows now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Nothing,
    /// The minimap over the view's edge.
    Overlay,
    /// The minimap, the rail or the miniature beside the view.
    Docked,
    Bar,
}

impl Overview {
    pub fn new(mode: Mode, docked: bool) -> Self {
        Self {
            mode,
            docked,
            ..Self::default()
        }
    }

    pub fn shown(&self, scrollable: bool) -> Shown {
        let up = self.up || self.fading || self.grip.is_some();
        match self.mode {
            _ if self.docked => Shown::Docked,
            _ if !scrollable => Shown::Nothing,
            Mode::Auto if up => Shown::Overlay,
            // Pinned without a dock, as for a page that turned out longer
            // than it looked, the bar shows where the view is.
            Mode::On | Mode::Off if up => Shown::Bar,
            _ => Shown::Nothing,
        }
    }

    /// Whether the pointer holds or points at the scrollbar, so that the
    /// strip says where the view is.
    pub fn scrubbing(&self) -> bool {
        !self.docked && self.mode != Mode::Auto && (self.hovered || self.grip.is_some())
    }

    /// Fades the element in or out with the overview; docked, it just shows.
    fn fade(&self, element: Stateful<Div>) -> AnyElement {
        if self.docked {
            return element.into_any_element();
        }
        let up = self.up || self.grip.is_some();
        element
            .with_animation(
                ("overview-fade", self.epoch),
                Animation::new(FADE).with_easing(ease_in_out),
                move |element, t| element.opacity(if up { t } else { 1. - t }),
            )
            .into_any_element()
    }
}

/// A view with an overview.
pub trait Overviewed: 'static + Sized {
    fn overview(&mut self) -> &mut Overview;
    /// The view's geometry on a track `track` pixels tall, at the scale of
    /// its own overview.
    fn geometry(&self, track: f64) -> Geometry;
    /// Moves the view's top to `top`, in the geometry's units.
    fn scroll_to(&mut self, top: f64, window: &mut Window, cx: &mut Context<Self>);
    /// A drag on the overview begins or ends.
    fn on_drag(&mut self, _held: bool) {}
}

/// Shows the overview, for as long as the view keeps moving and 1.5 s more.
pub fn reveal<V: Overviewed>(view: &mut V, cx: &mut Context<V>) {
    let overview = view.overview();
    if !overview.up {
        overview.up = true;
        overview.epoch += 1;
    }
    overview.fading = false;
    linger(view, cx);
    cx.notify();
}

/// Hides the overview 1.5 s from now, unless the pointer is on it then.
fn linger<V: Overviewed>(view: &mut V, cx: &mut Context<V>) {
    let timer = cx.spawn(async move |this, cx| {
        cx.background_executor().timer(LINGER).await;
        let hidden = this.update(cx, |this, cx| {
            let overview = this.overview();
            if overview.hovered || overview.grip.is_some() || !overview.up {
                return false;
            }
            overview.up = false;
            overview.fading = true;
            overview.epoch += 1;
            cx.notify();
            true
        });
        if !matches!(hidden, Ok(true)) {
            return;
        }
        cx.background_executor()
            .timer(FADE + Duration::from_millis(40))
            .await;
        let _ = this.update(cx, |this, cx| {
            let overview = this.overview();
            if !overview.up {
                overview.fading = false;
                cx.notify();
            }
        });
    });
    view.overview().timer = Some(timer);
}

fn press<V: Overviewed>(
    view: &mut V,
    bar: bool,
    y: Pixels,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    let track = view.overview().track.get();
    let y = f64::from(y - track.top());
    let geometry = |view: &V| {
        let g = view.geometry(f64::from(track.size.height));
        if bar { g.bar() } else { g }
    };
    view.on_drag(true);
    let g = geometry(view);
    let (top, height) = (g.thumb_top(), g.thumb_height());
    let grip = if (top..=top + height).contains(&y) {
        y - top
    } else if bar {
        // The thumb's middle comes to the pointer.
        view.scroll_to(g.top_for_thumb(y - height / 2.), window, cx);
        height / 2.
    } else {
        // What was pressed on comes to the middle of the view; dragging on
        // carries the thumb from where that put it.
        view.scroll_to(g.top_centred_at(y), window, cx);
        y - geometry(view).thumb_top()
    };
    view.overview().grip = Some(grip);
    cx.notify();
}

fn drag<V: Overviewed>(
    view: &mut V,
    bar: bool,
    y: Pixels,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    let overview = view.overview();
    let Some(grip) = overview.grip else {
        return;
    };
    let track = overview.track.get();
    let g = view.geometry(f64::from(track.size.height));
    let g = if bar { g.bar() } else { g };
    let y = f64::from(y - track.top());
    view.scroll_to(g.top_for_thumb(y - grip), window, cx);
    cx.notify();
}

fn release<V: Overviewed>(view: &mut V, cx: &mut Context<V>) {
    if view.overview().grip.take().is_some() {
        view.on_drag(false);
        linger(view, cx);
        cx.notify();
    }
}

/// The frame every overview shares: the track's place, and the pointer on
/// it. Mouse moves and releases are listened to on the whole window, so
/// that a drag carries on outside the track.
fn frame<V: Overviewed>(
    overview: &Overview,
    id: &'static str,
    bar: bool,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let track = overview.track.clone();
    let entity = cx.entity().downgrade();
    let follow = canvas(
        move |bounds, _, _| track.set(bounds),
        move |_, _, window, _| {
            let moved = entity.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    let _ =
                        moved.update(cx, |this, cx| drag(this, bar, event.position.y, window, cx));
                }
            });
            let released = entity.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    let _ = released.update(cx, |this, cx| release(this, cx));
                }
            });
        },
    )
    .absolute()
    .size_full();
    let element = div().id(id).absolute().child(follow);
    if !(overview.docked || overview.up || overview.grip.is_some()) {
        // Fading out: drawn, but no longer in the pointer's way.
        return element;
    }
    element
        .block_mouse_except_scroll()
        .cursor(CursorStyle::Arrow)
        .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
        .on_hover(cx.listener(|this: &mut V, hovered: &bool, _, cx| {
            let overview = this.overview();
            overview.hovered = *hovered;
            if !*hovered {
                linger(this, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this: &mut V, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                press(this, bar, event.position.y, window, cx);
            }),
        )
}

/// The minimap, the rail or the miniature: filling its dock, or `width`
/// wide over the view's right edge. `content` draws what the track shows:
/// the overview from the geometry's shift down.
pub fn panel<V: Overviewed>(
    overview: &Overview,
    g: Geometry,
    width: f32,
    content: AnyElement,
    theme: Theme,
    cx: &mut Context<V>,
) -> AnyElement {
    let thumb = div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(g.thumb_top() as f32))
        .h(px(g.thumb_height() as f32))
        .bg(progress().opacity(if overview.grip.is_some() { 0.24 } else { 0.14 }))
        .border_y_1()
        .border_color(progress().opacity(0.7));
    let element = frame(overview, "overview", false, cx)
        .top_0()
        .bottom_0()
        .right_0()
        .when(overview.docked, |panel| panel.left_0())
        .when(!overview.docked, |panel| {
            panel
                .w(px(width))
                .bg(theme.surface.opacity(0.9))
                .border_l_1()
                .border_color(theme.muted.opacity(0.25))
        })
        .overflow_hidden()
        .child(content)
        .child(thumb);
    overview.fade(element)
}

/// The slim scrollbar over the view's right edge. Where the view is shows
/// in the strip while the scrollbar is held or pointed at.
pub fn scrollbar<V: Overviewed>(
    overview: &Overview,
    g: Geometry,
    theme: Theme,
    cx: &mut Context<V>,
) -> AnyElement {
    let g = g.bar();
    let active = overview.grip.is_some() || overview.hovered;
    let thumb = div()
        .absolute()
        .top(px(g.thumb_top() as f32))
        .h(px(g.thumb_height() as f32))
        .right(px(7.))
        .w(px(if active { 8. } else { 6. }))
        .rounded(px(4.))
        .bg(if active {
            progress()
        } else {
            theme.muted.opacity(0.6)
        });
    let element = frame(overview, "scrollbar", true, cx)
        .top(px(INSET))
        .bottom(px(INSET))
        .right_0()
        .w(px(BAR_WIDTH))
        .child(thumb);
    overview.fade(element)
}

/// The dock beside a view, `width` wide, on `ground`, with what its
/// overview shows; `rule` sets it off from the view.
pub fn dock(
    width: f32,
    ground: Hsla,
    rule: Option<Hsla>,
    shown: Option<AnyElement>,
) -> Stateful<Div> {
    div()
        .id("dock")
        .flex_none()
        .relative()
        .w(px(width))
        .h_full()
        .bg(ground)
        .when_some(rule, |dock, rule| dock.border_l_1().border_color(rule))
        .children(shown)
}

/// The track a scrollbar has in a view `height` pixels tall.
pub fn bar_track(height: f32) -> f64 {
    f64::from((height - 2. * INSET).max(1.))
}

/// The window's area left to a view beside a dock `dock` wide.
pub fn beside(window: &Window, dock: f32) -> Size<Pixels> {
    let viewport = window.viewport_size();
    size((viewport.width - px(dock)).max(px(1.)), viewport.height)
}

/// Where a view stands with its dock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Docking {
    /// The dock's width, in logical pixels.
    pub width: f32,
    pub docked: bool,
    /// The mode pins the overview, and the view is long enough for it.
    pub wanted: bool,
}

#[cfg(test)]
mod tests {
    use super::{Family, Geometry, MIN_THUMB, Mode, Overview, Shown};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn a_short_overview_maps_straight_onto_the_view() {
        // 300 lines at 2 px on an 800 px track; 50 lines in view from line 100.
        let g = Geometry::new(300., 50., 100., 2., 800.);
        assert!(close(g.span(), 600.));
        assert!(close(g.shift(), 0.));
        assert!(close(g.thumb_top(), 200.));
        assert!(close(g.thumb_height(), 100.));
        assert!(close(g.top_for_thumb(200.), 100.));
        // A press at 300 px is line 150: the view centres on it.
        assert!(close(g.top_centred_at(300.), 125.));
    }

    #[test]
    fn a_long_overview_scrolls_with_the_view() {
        // 99,000 lines; the track shows 400 of them.
        let g = Geometry::new(99_000., 50., 49_475., 2., 800.);
        assert!(close(g.fraction(), 0.5));
        assert!(close(g.thumb_top(), 350.));
        // What is under the thumb is what is in view.
        let under = (g.thumb_top() + g.shift()) / g.scale;
        assert!(close(under, g.top));
        // Dragging the thumb to the bottom reaches the end.
        assert!(close(g.top_for_thumb(700.), 98_950.));
        assert!(close(g.top_for_thumb(-5.), 0.));
    }

    #[test]
    fn the_scrollbar_keeps_a_thumb_to_hold() {
        let g = Geometry::new(1_000_000., 50., 0., 2., 800.).bar();
        assert!(close(g.span(), 800.));
        assert!(close(g.shift(), 0.));
        assert!(close(g.thumb_height(), MIN_THUMB));
        assert!(close(g.top_for_thumb(800. - MIN_THUMB), 999_950.));
    }

    #[test]
    fn content_that_fits_needs_no_overview() {
        assert!(!Geometry::new(40., 50., 0., 2., 800.).scrollable());
        assert!(Geometry::new(51., 50., 0., 2., 800.).scrollable());
        assert!(close(
            Geometry::new(40., 50., 0., 2., 800.).top_for_thumb(10.),
            0.
        ));
    }

    #[test]
    fn m_cycles_three_modes_in_text_and_two_in_pages() {
        assert_eq!(Mode::Auto.next(Family::Minimap), Mode::On);
        assert_eq!(Mode::On.next(Family::Minimap), Mode::Off);
        assert_eq!(Mode::Off.next(Family::Minimap), Mode::Auto);
        assert_eq!(Mode::On.next(Family::Rail), Mode::Off);
        assert_eq!(Mode::Off.next(Family::Rail), Mode::On);
        // An auto left from before pages lost it docks them.
        assert_eq!(Mode::Auto.next(Family::Rail), Mode::Off);
    }

    #[test]
    fn the_minimap_shows_while_scrolling_and_docked_overviews_always() {
        let mut overview = Overview::new(Mode::Auto, false);
        assert_eq!(overview.shown(true), Shown::Nothing);
        overview.up = true;
        assert_eq!(overview.shown(true), Shown::Overlay);
        assert_eq!(overview.shown(false), Shown::Nothing);
        overview.mode = Mode::Off;
        assert_eq!(overview.shown(true), Shown::Bar);
        // Pinned: docked, resting or not, long or short.
        let mut docked = Overview::new(Mode::On, true);
        assert_eq!(docked.shown(true), Shown::Docked);
        assert_eq!(docked.shown(false), Shown::Docked);
        // Pinned while the window has not made room yet: the bar stands in.
        docked.docked = false;
        assert_eq!(docked.shown(true), Shown::Nothing);
        docked.up = true;
        assert_eq!(docked.shown(true), Shown::Bar);
    }
}
