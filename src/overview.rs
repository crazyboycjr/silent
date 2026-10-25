//! The overview at the right edge of a long view: a minimap of text, a rail
//! of page thumbnails, or a slim scrollbar once M has turned those off. It
//! shows while the view scrolls and fades 1.5 s after, unless M has pinned
//! it; it stays while the pointer is on it, and pressing or dragging on it
//! moves the view.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

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

/// What M cycles through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Shown while the view scrolls.
    #[default]
    Auto,
    /// Shown whenever there is more than a screen.
    On,
    /// The slim scrollbar instead, while the view scrolls.
    Off,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Auto => Mode::On,
            Mode::On => Mode::Off,
            Mode::Off => Mode::Auto,
        }
    }

    /// The mode chosen last for `what`, kept until the next reboot.
    pub fn load(what: &str) -> Self {
        match memory::setting(what).as_deref() {
            Some("on") => Mode::On,
            Some("off") => Mode::Off,
            _ => Mode::Auto,
        }
    }

    pub fn save(self, what: &str) {
        let name = match self {
            Mode::Auto => "auto",
            Mode::On => "on",
            Mode::Off => "off",
        };
        memory::set_setting(what, name);
    }

    /// `Minimap while scrolling`, for the strip.
    pub fn describe(self, what: &str) -> String {
        match self {
            Mode::Auto => format!("{what} while scrolling"),
            Mode::On => format!("{what} on"),
            Mode::Off => format!("{what} off"),
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

/// What shows at the right edge now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Nothing,
    /// The minimap or the rail.
    Panel,
    Bar,
}

impl Overview {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn shown(&self, scrollable: bool) -> Shown {
        let up = self.up || self.fading || self.grip.is_some();
        match self.mode {
            _ if !scrollable => Shown::Nothing,
            Mode::On => Shown::Panel,
            Mode::Auto if up => Shown::Panel,
            Mode::Off if up => Shown::Bar,
            _ => Shown::Nothing,
        }
    }

    /// Fades the element in or out with the overview; pinned, it just shows.
    fn fade(&self, element: Stateful<Div>) -> AnyElement {
        if self.mode == Mode::On {
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

/// The frame both kinds of overview share: the track's place, and the
/// pointer on it. Mouse moves and releases are listened to on the whole
/// window, so that a drag carries on outside the track.
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
    let live = overview.up || overview.grip.is_some() || overview.mode == Mode::On;
    let element = div()
        .id(id)
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .child(follow);
    if !live {
        // Fading out: drawn, but no longer in the pointer's way.
        return element;
    }
    element
        .block_mouse_except_scroll()
        .cursor(CursorStyle::Arrow)
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

/// The minimap or the rail, `width` pixels wide. `content` draws what the
/// track shows: the overview from the geometry's shift down.
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
        .w(px(width))
        .bg(theme.surface.opacity(0.9))
        .border_l_1()
        .border_color(theme.muted.opacity(0.25))
        .overflow_hidden()
        .child(content)
        .child(thumb);
    overview.fade(element)
}

/// The slim scrollbar, with `label` beside its thumb while it is held or
/// pointed at.
pub fn scrollbar<V: Overviewed>(
    overview: &Overview,
    g: Geometry,
    label: String,
    theme: Theme,
    cx: &mut Context<V>,
) -> AnyElement {
    let g = g.bar();
    let active = overview.grip.is_some() || overview.hovered;
    let (top, height) = (g.thumb_top() as f32, g.thumb_height() as f32);
    let thumb = div()
        .absolute()
        .top(px(top))
        .h(px(height))
        .right(px(7.))
        .w(px(if active { 8. } else { 6. }))
        .rounded(px(4.))
        .bg(if active {
            progress()
        } else {
            theme.muted.opacity(0.6)
        });
    let tag = active.then(|| {
        div()
            .absolute()
            .top(px(top + height / 2. - 13.))
            .right(px(BAR_WIDTH + 4.))
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(theme.strip)
            .text_color(theme.strip_text)
            .text_size(px(12.5))
            .whitespace_nowrap()
            .child(label)
    });
    let element = frame(overview, "scrollbar", true, cx)
        .top(px(INSET))
        .bottom(px(INSET))
        .w(px(BAR_WIDTH))
        .child(thumb)
        .children(tag);
    // The label hangs outside the track, to the left.
    overview.fade(element)
}

/// The track a scrollbar has in a view `height` pixels tall.
pub fn bar_track(height: f32) -> f64 {
    f64::from((height - 2. * INSET).max(1.))
}

#[cfg(test)]
mod tests {
    use super::{Geometry, MIN_THUMB, Mode, Overview, Shown};

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
    fn m_cycles_through_the_modes() {
        assert_eq!(Mode::Auto.next(), Mode::On);
        assert_eq!(Mode::On.next(), Mode::Off);
        assert_eq!(Mode::Off.next(), Mode::Auto);
        let mut overview = Overview::new(Mode::Auto);
        assert_eq!(overview.shown(true), Shown::Nothing);
        overview.up = true;
        assert_eq!(overview.shown(true), Shown::Panel);
        assert_eq!(overview.shown(false), Shown::Nothing);
        overview.mode = Mode::Off;
        assert_eq!(overview.shown(true), Shown::Bar);
        overview.up = false;
        overview.mode = Mode::On;
        assert_eq!(overview.shown(true), Shown::Panel);
    }
}
