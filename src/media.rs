//! Video and audio, with silent's own minimal controls.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_video_player::Video;
use gstreamer as gst;
use gstreamer::prelude::*;
use silent::format;
use smallvec::SmallVec;
use yuv::{YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix, yuv_nv12_to_bgra};

use crate::content::{Audio, clock};
use crate::theme::{Theme, accent, progress};

/// Volume steps, in percent of a cubic scale, as the ear hears loudness.
const VOLUME_STEP: i32 = 5;

/// The volume and mute that carry from one sound or video to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// The player's linear volume, 1 at full.
    pub volume: f64,
    pub muted: bool,
}

impl Default for Loudness {
    fn default() -> Self {
        Self {
            volume: 1.,
            muted: false,
        }
    }
}

/// The percent a linear volume reads as.
pub fn percent(volume: f64) -> i32 {
    (volume.max(0.).cbrt() * 100.).round() as i32
}

/// A step louder or quieter, as a linear volume.
fn stepped(volume: f64, louder: bool) -> f64 {
    let step = if louder { VOLUME_STEP } else { -VOLUME_STEP };
    let percent = (percent(volume) + step).clamp(0, 100);
    (f64::from(percent) / 100.).powi(3)
}

/// A playing video. Frames are converted on a thread of their own, only
/// when the player has a new one, so the UI thread only swaps images. Its
/// seek bar is drawn in the row below the picture.
pub struct VideoPane {
    video: Video,
    frame: Option<Arc<RenderImage>>,
    _feed: Task<()>,
    /// Where a drag on the seek bar has got to.
    dragging: Option<Duration>,
    /// The seek bar's track as last laid out.
    track: Rc<Cell<Bounds<Pixels>>>,
}

impl VideoPane {
    pub fn new(
        video: Video,
        loudness: Loudness,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        video.set_volume(loudness.volume);
        video.set_muted(loudness.muted);
        let (sender, frames) = async_channel::bounded(1);
        let source = video.clone();
        std::thread::Builder::new()
            .name("frames".into())
            .spawn(move || convert(source, sender))
            .expect("spawning the frame converter");
        let feed = cx.spawn_in(window, async move |this, cx| {
            while let Ok(image) = frames.recv().await {
                let shown = this.update_in(cx, |this, window, cx| {
                    if let Some(old) = this.frame.replace(image) {
                        cx.drop_image(old, Some(window));
                    }
                    cx.notify();
                });
                if shown.is_err() {
                    break;
                }
            }
        });
        Self {
            video,
            frame: None,
            _feed: feed,
            dragging: None,
            track: Rc::default(),
        }
    }

    pub fn toggle_pause(&self) {
        self.video.set_paused(!self.video.paused());
    }

    pub fn toggle_mute(&self) {
        self.video.set_muted(!self.video.muted());
    }

    /// Back or on by `by`, to the frame or to the nearest keyframe.
    pub fn seek(&mut self, forward: bool, by: Duration, accurate: bool, cx: &mut Context<Self>) {
        let position = self.video.position();
        let target = if forward {
            (position + by).min(self.video.duration())
        } else {
            position.saturating_sub(by)
        };
        let _ = self.video.seek(target, accurate);
        cx.notify();
    }

    /// A step louder or quieter; louder also unmutes. Returns the percent.
    pub fn louder(&self, louder: bool) -> i32 {
        let volume = stepped(self.video.volume(), louder);
        self.video.set_volume(volume);
        if louder {
            self.video.set_muted(false);
        }
        percent(volume)
    }

    pub fn loudness(&self) -> Loudness {
        Loudness {
            volume: self.video.volume(),
            muted: self.video.muted(),
        }
    }

    pub fn position(&self) -> Duration {
        self.video.position()
    }

    pub fn duration(&self) -> Option<Duration> {
        Some(self.video.duration()).filter(|d| !d.is_zero())
    }

    /// Whether it is paused or muted, for the strip.
    pub fn flags(&self) -> impl Iterator<Item = &'static str> {
        [
            (self.video.paused(), "paused"),
            (self.video.muted(), "muted"),
        ]
        .into_iter()
        .filter_map(|(on, word)| on.then_some(word))
    }

    pub fn ready(&self) -> bool {
        self.frame.is_some()
    }

    /// The time under `x` on the bar's track.
    fn time_at(&self, x: Pixels) -> Duration {
        let track = self.track.get();
        let fraction = if track.size.width > px(0.) {
            ((x - track.left()) / track.size.width).clamp(0., 1.)
        } else {
            0.
        };
        self.video.duration().mul_f32(fraction)
    }

    fn drag_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let target = self.time_at(x);
        self.dragging = Some(target);
        let _ = self.video.seek(target, false);
        cx.notify();
    }

    fn drop_at(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.dragging.take() {
            let _ = self.video.seek(target, true);
            cx.notify();
        }
    }

    /// The seek bar for the row below the picture: the time, the track and
    /// the length. Only a bar in a shown row takes the pointer.
    pub fn seek_bar(&self, live: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = Theme::of(WindowAppearance::Dark);
        let duration = self.video.duration();
        let position = self.dragging.unwrap_or_else(|| self.video.position());
        let fraction = if duration.is_zero() {
            0.
        } else {
            (position.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
        };
        let track = self.track.clone();
        let view = cx.entity().downgrade();
        let follow = canvas(
            move |bounds, _, _| track.set(bounds),
            move |_, _, window, _| {
                if !live {
                    return;
                }
                // Listened to on every frame: a quick click can be released
                // before the frame after its press is drawn.
                let moved = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = moved.update(cx, |this, cx| {
                            if this.dragging.is_some() {
                                this.drag_to(event.position.x, cx);
                            }
                        });
                    }
                });
                let released = view.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        let _ = released.update(cx, |this, cx| this.drop_at(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let time = |t: Duration| {
            div()
                .flex_none()
                .min_w(px(44.))
                .text_color(theme.strip_text)
                .child(format::duration(t))
        };
        div()
            .id("seek-bar")
            .flex_1()
            .min_w(px(160.))
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .when(live, |bar| {
                bar.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        // A little either side of the track seeks too.
                        let track = this.track.get();
                        let x = event.position.x;
                        if x > track.left() - px(12.) && x < track.right() + px(12.) {
                            this.drag_to(x, cx);
                        }
                    }),
                )
            })
            .child(time(position))
            .child(
                div()
                    .flex_1()
                    .h(px(24.))
                    .relative()
                    .flex()
                    .items_center()
                    .when(live, |track| track.cursor(CursorStyle::PointingHand))
                    .child(follow)
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(theme.strip_text.opacity(0.28))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .rounded(px(2.))
                                    .bg(progress()),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(fraction))
                            .ml(px(-6.))
                            .size(px(12.))
                            .rounded(px(6.))
                            .bg(theme.strip_text),
                    ),
            )
            .child(time(duration).flex().justify_end())
    }
}

impl Render for VideoPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let frame = self.frame.clone().map(|image| {
            img(ImageSource::Render(image))
                .size_full()
                .object_fit(ObjectFit::Contain)
        });
        div().size_full().bg(rgb(0x000000)).children(frame)
    }
}

/// Runs until the pane is dropped. The player marks each new frame; this
/// polls the mark, which costs nothing between frames.
fn convert(video: Video, sender: async_channel::Sender<Arc<RenderImage>>) {
    while !sender.is_closed() {
        if !video.take_frame_ready() {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        let Some((nv12, width, height)) = video.current_frame_data() else {
            continue;
        };
        let Some(image) = to_bgra(&nv12, width, height) else {
            continue;
        };
        if sender.send_blocking(image).is_err() {
            break;
        }
    }
}

/// Video is almost always limited range, BT.709 for HD and BT.601 below.
fn to_bgra(nv12: &[u8], width: u32, height: u32) -> Option<Arc<RenderImage>> {
    let luma = width as usize * height as usize;
    if nv12.len() < luma + luma / 2 {
        return None;
    }
    let planes = YuvBiPlanarImage {
        y_plane: &nv12[..luma],
        y_stride: width,
        uv_plane: &nv12[luma..luma + luma / 2],
        uv_stride: width,
        width,
        height,
    };
    let matrix = if height >= 720 {
        YuvStandardMatrix::Bt709
    } else {
        YuvStandardMatrix::Bt601
    };
    let mut bgra = vec![0u8; luma * 4];
    yuv_nv12_to_bgra(
        &planes,
        &mut bgra,
        width * 4,
        YuvRange::Limited,
        matrix,
        YuvConversionMode::Balanced,
    )
    .ok()?;
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(buffer),
        1,
    ))))
}

fn status(position: Duration, duration: Option<Duration>, paused: bool, muted: bool) -> String {
    let mut text = format::duration(position);
    if let Some(duration) = duration.filter(|d| !d.is_zero()) {
        text = format!("{text} / {}", format::duration(duration));
    }
    if paused {
        text.push_str(" · paused");
    }
    if muted {
        text.push_str(" · muted");
    }
    text
}

/// A playing sound, shown as a card with its cover and tags.
pub struct AudioPane {
    audio: Audio,
    name: SharedString,
    _tick: Task<()>,
}

impl AudioPane {
    pub fn new(
        audio: Audio,
        name: SharedString,
        loudness: Loudness,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        audio.playbin.set_property("volume", loudness.volume);
        audio.playbin.set_property("mute", loudness.muted);
        let _ = audio.playbin.set_state(gst::State::Playing);
        let tick = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            audio,
            name,
            _tick: tick,
        }
    }

    fn playing(&self) -> bool {
        self.audio.playbin.current_state() == gst::State::Playing
    }

    pub fn position(&self) -> Duration {
        self.audio
            .playbin
            .query_position::<gst::ClockTime>()
            .map_or(Duration::ZERO, |t| Duration::from_nanos(t.nseconds()))
    }

    pub fn duration(&self) -> Option<Duration> {
        self.audio
            .playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| Duration::from_nanos(t.nseconds()))
            .or(self.audio.duration)
    }

    fn muted(&self) -> bool {
        self.audio.playbin.property::<bool>("mute")
    }

    pub fn toggle_pause(&self) {
        let next = if self.playing() {
            gst::State::Paused
        } else {
            gst::State::Playing
        };
        let _ = self.audio.playbin.set_state(next);
    }

    pub fn toggle_mute(&self) {
        self.audio.playbin.set_property("mute", !self.muted());
    }

    /// A step louder or quieter; louder also unmutes. Returns the percent.
    pub fn louder(&self, louder: bool) -> i32 {
        let volume = stepped(self.audio.playbin.property::<f64>("volume"), louder);
        self.audio.playbin.set_property("volume", volume);
        if louder {
            self.audio.playbin.set_property("mute", false);
        }
        percent(volume)
    }

    pub fn loudness(&self) -> Loudness {
        Loudness {
            volume: self.audio.playbin.property::<f64>("volume"),
            muted: self.muted(),
        }
    }

    /// Back or on by `by`, to the sample or to the nearest keyframe.
    pub fn seek(&self, forward: bool, by: Duration, accurate: bool) {
        let how = if accurate {
            gst::SeekFlags::ACCURATE
        } else {
            gst::SeekFlags::KEY_UNIT
        };
        let position = self.position();
        let target = if forward {
            position + by
        } else {
            position.saturating_sub(by)
        };
        let target = self.duration().map_or(target, |d| target.min(d));
        let _ = self
            .audio
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | how, clock(target));
    }

    pub fn status(&self) -> String {
        status(
            self.position(),
            self.duration(),
            !self.playing(),
            self.muted(),
        )
    }
}

impl Drop for AudioPane {
    fn drop(&mut self) {
        let _ = self.audio.playbin.set_state(gst::State::Null);
    }
}

impl Render for AudioPane {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window.appearance());
        let color = accent(silent::detect::Kind::Audio);
        let cover = match &self.audio.cover {
            Some(cover) => img(ImageSource::Render(cover.clone()))
                .size(px(188.))
                .rounded(px(10.))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div()
                .size(px(188.))
                .rounded(px(10.))
                .bg(color.opacity(0.16))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(54.))
                .text_color(color)
                .child("♪")
                .into_any_element(),
        };
        let progress = match self.duration().filter(|d| !d.is_zero()) {
            Some(duration) => {
                (self.position().as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
            }
            None => 0.,
        };
        let title = self
            .audio
            .title
            .clone()
            .unwrap_or_else(|| self.name.to_string());
        let detail = |value: &Option<String>| {
            value
                .clone()
                .map(|v| div().text_color(theme.muted).truncate().child(v))
        };
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(28.))
            .px(px(36.))
            .bg(theme.surface)
            .text_size(px(13.5))
            .child(div().flex_none().child(cover))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(19.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .line_clamp(2)
                            .child(title),
                    )
                    .children(detail(&self.audio.artist))
                    .children(detail(&self.audio.album))
                    .child(
                        div()
                            .mt(px(16.))
                            .h(px(4.))
                            .w_full()
                            .rounded(px(2.))
                            .bg(theme.muted.opacity(0.25))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(progress))
                                    .rounded(px(2.))
                                    .bg(color),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .text_color(theme.muted)
                            .child(self.status()),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{percent, stepped};

    #[test]
    fn volume_steps_are_even_to_the_ear() {
        assert_eq!(percent(1.), 100);
        let quieter = stepped(1., false);
        assert!((quieter - 0.95f64.powi(3)).abs() < 1e-9);
        assert_eq!(percent(quieter), 95);
        assert_eq!(percent(stepped(quieter, true)), 100);
        // The ends hold.
        assert_eq!(stepped(1., true), 1.);
        assert_eq!(stepped(0., false), 0.);
        // A volume set elsewhere joins the steps.
        assert_eq!(percent(stepped(0.5, true)), 84);
    }
}
