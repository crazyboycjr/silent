//! The scrubber that shows on the right edge while Shift is held: where the
//! view is in a long document, and a thumb to drag through it.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;

use crate::theme::{Theme, progress};

#[derive(Default)]
pub struct Scrubber {
    /// Shift is down.
    pub held: bool,
    pub dragging: bool,
    /// The rail, as last laid out.
    rail: Rc<Cell<Bounds<Pixels>>>,
}

impl Scrubber {
    pub fn shown(&self) -> bool {
        self.held || self.dragging
    }

    /// The fraction that puts the thumb's middle at `y`.
    fn fraction(&self, y: Pixels) -> f32 {
        let rail = self.rail.get();
        let span = rail.size.height - px(THUMB);
        if span <= px(0.) {
            return 0.;
        }
        ((y - rail.top() - px(THUMB / 2.)) / span).clamp(0., 1.)
    }
}

/// A view that can be scrubbed through.
pub trait Scrubbed: 'static + Sized {
    fn scrubber(&mut self) -> &mut Scrubber;
    /// Moves the view to `fraction` of the way down.
    fn scrub_to(&mut self, fraction: f32, window: &mut Window, cx: &mut Context<Self>);
}

const INSET: f32 = 14.;
const THUMB: f32 = 32.;

/// The rail, the thumb at `fraction` and `label` beside it.
pub fn track<V: Scrubbed>(
    scrubber: &Scrubber,
    fraction: f32,
    label: String,
    theme: Theme,
    cx: &mut Context<V>,
) -> AnyElement {
    let rail = scrubber.rail.clone();
    let view = cx.entity().downgrade();
    let follow = canvas(
        move |bounds, _, _| rail.set(bounds),
        move |_, _, window, _| {
            // Listened to on every frame: a quick click can be released
            // before the frame after its press is drawn.
            let moved = view.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    let _ = moved.update(cx, |this, cx| {
                        if this.scrubber().dragging {
                            let fraction = this.scrubber().fraction(event.position.y);
                            this.scrub_to(fraction, window, cx);
                        }
                    });
                }
            });
            let released = view.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    let _ = released.update(cx, |this, cx| {
                        if std::mem::take(&mut this.scrubber().dragging) {
                            cx.notify();
                        }
                    });
                }
            });
        },
    )
    .absolute()
    .top(px(INSET))
    .bottom(px(INSET))
    .left_0()
    .right_0();
    div()
        .id("scrubber")
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(px(44.))
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this: &mut V, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let scrubber = this.scrubber();
                scrubber.dragging = true;
                let fraction = scrubber.fraction(event.position.y);
                this.scrub_to(fraction, window, cx);
                cx.notify();
            }),
        )
        .child(follow)
        .child(
            div()
                .absolute()
                .top(px(INSET))
                .bottom(px(INSET))
                .right(px(13.))
                .w(px(4.))
                .rounded(px(2.))
                .bg(theme.muted.opacity(0.45)),
        )
        .child(
            div()
                .absolute()
                .top(px(INSET))
                .bottom(px(INSET))
                .left_0()
                .right_0()
                // The thumb runs from the rail's top to its bottom, the label beside it.
                .child(
                    div()
                        .absolute()
                        .top(relative(fraction))
                        .mt(px(-THUMB * fraction))
                        .right(px(10.))
                        .w(px(10.))
                        .h(px(THUMB))
                        .rounded(px(5.))
                        .bg(progress()),
                )
                .child(
                    div()
                        .absolute()
                        .top(relative(fraction))
                        .mt(px(-THUMB * fraction + THUMB / 2. - 13.))
                        .right(px(30.))
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .bg(theme.strip)
                        .text_color(theme.strip_text)
                        .text_size(px(12.5))
                        .whitespace_nowrap()
                        .child(label),
                ),
        )
        .into_any_element()
}
