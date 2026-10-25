//! Video and audio, with silent's own minimal controls.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use gpui_video_player::Video;
use gstreamer as gst;
use gstreamer::prelude::*;
use silent::format;
use smallvec::SmallVec;
use yuv::{YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix, yuv_nv12_to_bgra};

use crate::content::Audio;
use crate::theme::{Theme, accent};

const SEEK: Duration = Duration::from_secs(10);

/// A playing video. Frames are converted on a thread of their own, only
/// when the player has a new one, so the UI thread only swaps images.
pub struct VideoPane {
    video: Video,
    frame: Option<Arc<RenderImage>>,
    _feed: Task<()>,
}

impl VideoPane {
    pub fn new(video: Video, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        }
    }

    pub fn toggle_pause(&self) {
        self.video.set_paused(!self.video.paused());
    }

    pub fn toggle_mute(&self) {
        self.video.set_muted(!self.video.muted());
    }

    pub fn seek(&self, forward: bool) {
        let position = self.video.position();
        let target = if forward {
            (position + SEEK).min(self.video.duration())
        } else {
            position.saturating_sub(SEEK)
        };
        let _ = self.video.seek(target, false);
    }

    /// `0:12 / 0:42`, and whether it is paused or muted.
    pub fn status(&self) -> String {
        status(
            self.video.position(),
            Some(self.video.duration()),
            self.video.paused(),
            self.video.muted(),
        )
    }

    pub fn ready(&self) -> bool {
        self.frame.is_some()
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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

    pub fn seek(&self, forward: bool) {
        let position = self.position();
        let target = if forward {
            position + SEEK
        } else {
            position.saturating_sub(SEEK)
        };
        let target = self.duration().map_or(target, |d| target.min(d));
        let _ = self.audio.playbin.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
            gst::ClockTime::from_nseconds(target.as_nanos() as u64),
        );
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
