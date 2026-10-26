//! Video and audio, with silent's own minimal controls: a playbar over the
//! bottom of the window with the seek track, the times and the volume.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use gpui_video_player::Video;
use gstreamer as gst;
use gstreamer::prelude::*;
use silent::format;
use silent::scrim::{Rgb, Scrim};
use smallvec::SmallVec;
use yuv::{YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix, yuv_nv12_to_bgra};

use crate::content::{Audio, clock};
use crate::strip;
use crate::theme::{Theme, accent, progress};

/// Volume steps, in percent of a cubic scale, as the ear hears loudness.
const VOLUME_STEP: i32 = 5;
/// How often the frames under the playbar are looked at for its colours.
const SAMPLE: Duration = Duration::from_millis(300);
/// The volume's slider.
const LEVEL: f32 = 56.;

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

/// What a playbar needs of a player.
pub trait Player: 'static + Sized {
    fn position(&self) -> Duration;
    fn duration(&self) -> Option<Duration>;
    fn seek_to(&mut self, target: Duration, accurate: bool);
    /// The linear volume, 1 at full.
    fn volume(&self) -> f64;
    fn set_volume(&self, volume: f64);
    fn muted(&self) -> bool;
    fn set_muted(&self, muted: bool);
    fn bar(&mut self) -> &mut Bar;
}

/// A playbar's own state: drags on the track and on the volume, and where
/// both were laid out.
#[derive(Default)]
pub struct Bar {
    /// Where a drag on the track has got to.
    dragging: Option<Duration>,
    leveling: bool,
    track: Rc<Cell<Bounds<Pixels>>>,
    level: Rc<Cell<Bounds<Pixels>>>,
}

/// A playbar's parts, for the strip to lay out: the seek track, the times
/// and the volume.
pub struct Playbar {
    pub track: AnyElement,
    pub times: String,
    pub volume: AnyElement,
}

fn fraction_at(bounds: Bounds<Pixels>, x: Pixels) -> f32 {
    if bounds.size.width > px(0.) {
        ((x - bounds.left()) / bounds.size.width).clamp(0., 1.)
    } else {
        0.
    }
}

fn drag_to<P: Player>(player: &mut P, x: Pixels, cx: &mut Context<P>) {
    let track = player.bar().track.get();
    let target = player
        .duration()
        .unwrap_or_default()
        .mul_f32(fraction_at(track, x));
    player.bar().dragging = Some(target);
    player.seek_to(target, false);
    cx.notify();
}

fn level_to<P: Player>(player: &mut P, x: Pixels, cx: &mut Context<P>) {
    let level = fraction_at(player.bar().level.get(), x);
    player.set_volume(f64::from(level).powi(3));
    player.set_muted(level == 0.);
    cx.notify();
}

/// The playbar of `player`, drawn in `ink`. Only a shown one, `live`,
/// takes the pointer. Drags are followed on the whole window, so that they
/// carry on past the bar.
pub fn playbar<P: Player>(player: &mut P, live: bool, ink: Hsla, cx: &mut Context<P>) -> Playbar {
    let duration = player.duration().unwrap_or_default();
    let position = player.bar().dragging.unwrap_or_else(|| player.position());
    let played = if duration.is_zero() {
        0.
    } else {
        (position.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
    };
    let (muted, volume) = (player.muted(), player.volume());
    let level = if muted { 0. } else { volume.cbrt() as f32 };
    let bar = player.bar();
    let (track, levels) = (bar.track.clone(), bar.level.clone());
    let view = cx.entity().downgrade();
    let follow = canvas(
        move |bounds, _, _| track.set(bounds),
        move |_, _, window, _| {
            if !live {
                return;
            }
            let moved = view.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble {
                    let _ = moved.update(cx, |this, cx| {
                        if this.bar().dragging.is_some() {
                            drag_to(this, event.position.x, cx);
                        } else if this.bar().leveling {
                            level_to(this, event.position.x, cx);
                        }
                    });
                }
            });
            let released = view.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    let _ = released.update(cx, |this, cx| {
                        this.bar().leveling = false;
                        if let Some(target) = this.bar().dragging.take() {
                            this.seek_to(target, true);
                            cx.notify();
                        }
                    });
                }
            });
        },
    )
    .absolute()
    .size_full();
    let track = div()
        .id("seek")
        .w_full()
        .h(px(strip::TRACK))
        .relative()
        .flex()
        .items_center()
        .when(live, |track| {
            track.cursor(CursorStyle::PointingHand).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    drag_to(this, event.position.x, cx);
                }),
            )
        })
        .child(follow)
        .child(
            div()
                .w_full()
                .h(px(4.))
                .rounded(px(2.))
                .bg(ink.opacity(0.3))
                .child(
                    div()
                        .h_full()
                        .w(relative(played))
                        .rounded(px(2.))
                        .bg(progress()),
                ),
        )
        .child(
            div()
                .absolute()
                .left(relative(played))
                .ml(px(-5.))
                .size(px(10.))
                .rounded(px(5.))
                .bg(ink),
        )
        .into_any_element();
    let icon = if muted || level == 0. {
        IconName::VolumeX
    } else if level < 0.5 {
        IconName::Volume1
    } else {
        IconName::Volume2
    };
    let slider = div()
        .id("level")
        .w(px(LEVEL))
        .h(px(16.))
        .relative()
        .flex()
        .items_center()
        .when(live, |slider| {
            slider.cursor(CursorStyle::PointingHand).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.bar().leveling = true;
                    level_to(this, event.position.x, cx);
                }),
            )
        })
        .child(
            canvas(move |bounds, _, _| levels.set(bounds), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .child(
            div()
                .w_full()
                .h(px(3.))
                .rounded(px(2.))
                .bg(ink.opacity(0.3))
                .child(div().h_full().w(relative(level)).rounded(px(2.)).bg(ink)),
        );
    let volume = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(7.))
        .child(
            div()
                .id("mute")
                .when(live, |button| {
                    button.cursor(CursorStyle::PointingHand).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.set_muted(!this.muted());
                            cx.notify();
                        }),
                    )
                })
                .child(svg().path(icon.path()).size(px(16.)).text_color(ink)),
        )
        .child(slider)
        .into_any_element();
    let times = match player.duration() {
        Some(duration) => format!(
            "{} / {}",
            format::duration(position),
            format::duration(duration)
        ),
        None => format::duration(position),
    };
    Playbar {
        track,
        times,
        volume,
    }
}

/// A playing video. Frames are converted on a thread of their own, only
/// when the player has a new one, so the UI thread only swaps images.
pub struct VideoPane {
    video: Video,
    frame: Option<Arc<RenderImage>>,
    _feed: Task<()>,
    bar: Bar,
    /// The playbar's veil, and when the frames were last looked at for it.
    veil: Option<(Instant, Scrim)>,
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
            bar: Bar::default(),
            veil: None,
        }
    }

    /// The veil for a playbar over the bottom `band` of the picture in a
    /// window `area`, from the frames under it every few hundred
    /// milliseconds.
    pub fn scrim(&mut self, area: Size<Pixels>, band: f32) -> Scrim {
        let stale = self.veil.is_none_or(|(at, _)| at.elapsed() > SAMPLE);
        if stale && let Some(frame) = &self.frame {
            let under = strip::under_image(frame, area, band, Rgb::hex(0x000000));
            let scrim = strip::follow(&under, self.veil.map(|(_, scrim)| scrim));
            self.veil = Some((Instant::now(), scrim));
        }
        self.veil.map_or(
            Scrim {
                dark: true,
                alpha: 0.6,
            },
            |(_, scrim)| scrim,
        )
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
}

impl Player for VideoPane {
    fn position(&self) -> Duration {
        self.video.position()
    }

    fn duration(&self) -> Option<Duration> {
        Some(self.video.duration()).filter(|d| !d.is_zero())
    }

    fn seek_to(&mut self, target: Duration, accurate: bool) {
        let _ = self.video.seek(target, accurate);
    }

    fn volume(&self) -> f64 {
        self.video.volume()
    }

    fn set_volume(&self, volume: f64) {
        self.video.set_volume(volume);
    }

    fn muted(&self) -> bool {
        self.video.muted()
    }

    fn set_muted(&self, muted: bool) {
        self.video.set_muted(muted);
    }

    fn bar(&mut self) -> &mut Bar {
        &mut self.bar
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

/// A playing sound, shown as a card with its cover and tags.
pub struct AudioPane {
    audio: Audio,
    name: SharedString,
    bar: Bar,
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
            bar: Bar::default(),
            _tick: tick,
        }
    }

    fn playing(&self) -> bool {
        self.audio.playbin.current_state() == gst::State::Playing
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
        self.set_muted(!self.muted());
    }

    /// A step louder or quieter; louder also unmutes. Returns the percent.
    pub fn louder(&self, louder: bool) -> i32 {
        let volume = stepped(self.volume(), louder);
        self.set_volume(volume);
        if louder {
            self.set_muted(false);
        }
        percent(volume)
    }

    pub fn loudness(&self) -> Loudness {
        Loudness {
            volume: self.volume(),
            muted: self.muted(),
        }
    }

    /// Back or on by `by`, to the sample or to the nearest keyframe.
    pub fn seek(&mut self, forward: bool, by: Duration, accurate: bool) {
        let position = self.position();
        let target = if forward {
            position + by
        } else {
            position.saturating_sub(by)
        };
        let target = self.duration().map_or(target, |d| target.min(d));
        self.seek_to(target, accurate);
    }

    /// Whether it is paused or muted, for the strip.
    pub fn flags(&self) -> impl Iterator<Item = &'static str> {
        [(!self.playing(), "paused"), (self.muted(), "muted")]
            .into_iter()
            .filter_map(|(on, word)| on.then_some(word))
    }
}

impl Player for AudioPane {
    fn position(&self) -> Duration {
        self.audio
            .playbin
            .query_position::<gst::ClockTime>()
            .map_or(Duration::ZERO, |t| Duration::from_nanos(t.nseconds()))
    }

    fn duration(&self) -> Option<Duration> {
        self.audio
            .playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| Duration::from_nanos(t.nseconds()))
            .or(self.audio.duration)
            .filter(|d| !d.is_zero())
    }

    fn seek_to(&mut self, target: Duration, accurate: bool) {
        let how = if accurate {
            gst::SeekFlags::ACCURATE
        } else {
            gst::SeekFlags::KEY_UNIT
        };
        let _ = self
            .audio
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | how, clock(target));
    }

    fn volume(&self) -> f64 {
        self.audio.playbin.property::<f64>("volume")
    }

    fn set_volume(&self, volume: f64) {
        self.audio.playbin.set_property("volume", volume);
    }

    fn muted(&self) -> bool {
        self.audio.playbin.property::<bool>("mute")
    }

    fn set_muted(&self, muted: bool) {
        self.audio.playbin.set_property("mute", muted);
    }

    fn bar(&mut self) -> &mut Bar {
        &mut self.bar
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
        let progress = match self.duration() {
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
        // The playbar holds the times and the controls; the card keeps a
        // quiet line of how far it has played.
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(28.))
            .px(px(36.))
            .pb(px(strip::TRACK))
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
                            .h(px(3.))
                            .w_full()
                            .rounded(px(2.))
                            .bg(theme.muted.opacity(0.22))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(progress))
                                    .rounded(px(2.))
                                    .bg(color),
                            ),
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
