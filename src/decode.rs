//! Image decoding on a thread of its own: JPEG and still PNG in this
//! process (see [`crate::native`]), everything else through glycin's
//! sandboxed loaders.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gio::glib;
use gio::prelude::*;
pub use glycin::{MemoryFormat, MemoryFormatSelection};
pub use image::metadata::Orientation;

use crate::geometry::Screen;
use crate::native;

/// Decoded pixels.
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    pub format: MemoryFormat,
    pub pixels: glib::Bytes,
    /// How long an animation shows this frame.
    pub delay: Option<Duration>,
    /// What is still to be done to the pixels, best once they are shrunk:
    /// turned upright, and taken to sRGB from this ICC profile. glycin's
    /// frames come with both done.
    pub orientation: Orientation,
    pub icc: Option<Arc<[u8]>>,
}

#[derive(Debug)]
pub enum Event<T> {
    /// The upright size, known before any pixels.
    Size {
        width: u32,
        height: u32,
    },
    Frame(T),
    Failed(String),
}

/// A running decode. Animations keep producing frames, paced by their
/// delays, until this is dropped.
pub struct Decoder<T> {
    pub events: async_channel::Receiver<Event<T>>,
    cancellable: gio::Cancellable,
}

impl<T> Drop for Decoder<T> {
    fn drop(&mut self) {
        self.cancellable.cancel();
    }
}

/// Decodes the image at `uri`. Vector images are rendered at the size
/// `screen` gives them; everything else arrives at its own resolution, in
/// one of the `accepted` formats. `prepare` turns each frame into what the
/// UI shows, still on the decoder's thread.
pub fn start<T, F>(
    uri: String,
    screen: Option<Screen>,
    accepted: MemoryFormatSelection,
    prepare: F,
) -> Decoder<T>
where
    T: Send + 'static,
    F: FnMut(Frame) -> T + Send + 'static,
{
    let (sender, events) = async_channel::bounded(2);
    let cancellable = gio::Cancellable::new();
    let thread_cancellable = cancellable.clone();
    std::thread::Builder::new()
        .name("decode".into())
        .spawn(move || {
            async_io::block_on(async {
                let mut prepare = prepare;
                let mut sized = false;
                if here(&uri, screen, &thread_cancellable, &mut prepare, &sender, &mut sized).await
                {
                    return;
                }
                let decoded = run(
                    &uri,
                    screen,
                    accepted,
                    thread_cancellable,
                    prepare,
                    &sender,
                    sized,
                )
                .await;
                if let Err(error) = decoded {
                    let _ = sender.send(Event::Failed(error.to_string())).await;
                }
            })
        })
        .expect("spawning the decoder thread");
    Decoder {
        events,
        cancellable,
    }
}

/// Decodes a JPEG or a still PNG in this process; false when glycin is to,
/// `sized` saying whether the size has gone out already.
async fn here<T>(
    uri: &str,
    screen: Option<Screen>,
    cancellable: &gio::Cancellable,
    prepare: &mut impl FnMut(Frame) -> T,
    sender: &async_channel::Sender<Event<T>>,
    sized: &mut bool,
) -> bool {
    let Some(path) = gio::File::for_uri(uri).path() else {
        return false;
    };
    let Some(opened) = native::open(&path) else {
        return false;
    };
    log::info!(target: "silent::trace", "headers read here");
    let (width, height) = opened.upright();
    if sender.send(Event::Size { width, height }).await.is_err() {
        return true;
    }
    *sized = true;
    // As stored, the size it is shrunk to for the monitor, if it is.
    let at_least = screen.and_then(|screen| {
        let monitor = screen.monitor;
        let shrink = (f64::from(monitor.width) / f64::from(width))
            .min(f64::from(monitor.height) / f64::from(height));
        (shrink < 1.0).then(|| {
            let upright = (
                (f64::from(width) * shrink).round() as u32,
                (f64::from(height) * shrink).round() as u32,
            );
            if native::turns(opened.orientation) {
                (upright.1, upright.0)
            } else {
                upright
            }
        })
    });
    let (orientation, icc) = (opened.orientation, opened.icc.clone());
    let decoded = match opened.decode(at_least) {
        Ok(decoded) => decoded,
        Err(error) => {
            log::info!(target: "silent::trace", "decoding here failed: {error}");
            return false;
        }
    };
    if cancellable.is_cancelled() {
        return true;
    }
    let channels = if decoded.format == MemoryFormat::R8g8b8 { 3 } else { 4 };
    let frame = Frame {
        width: decoded.width,
        height: decoded.height,
        stride: decoded.width * channels,
        format: decoded.format,
        pixels: glib::Bytes::from_owned(decoded.pixels),
        delay: None,
        orientation,
        icc,
    };
    let _ = sender.send(Event::Frame(prepare(frame))).await;
    true
}

async fn run<T>(
    uri: &str,
    screen: Option<Screen>,
    accepted: MemoryFormatSelection,
    cancellable: gio::Cancellable,
    mut prepare: impl FnMut(Frame) -> T,
    sender: &async_channel::Sender<Event<T>>,
    sized: bool,
) -> Result<(), glycin::Error> {
    let mut loader = glycin::Loader::new(gio::File::for_uri(uri));
    loader
        .cancellable(cancellable)
        .main_context_selector(glycin::MainContextSelector::Managed)
        .accepted_memory_formats(accepted);
    let mut image = loader.load().await?;
    let details = image.details();
    let (width, height) = (details.width(), details.height());
    if !sized && sender.send(Event::Size { width, height }).await.is_err() {
        return Ok(());
    }

    let mut request = glycin::FrameRequest::new();
    if let Some(screen) = screen
        && image.mime_type().as_str().starts_with("image/svg")
    {
        let fit = screen.fit_media((width, height));
        request = request.scale(fit.width as u32, fit.height as u32);
    }
    let mut frame = image.specific_frame(request).await?;
    let mut due = Instant::now();
    loop {
        let delay = frame.delay().map(playable);
        let decoded = prepare(Frame {
            width: frame.width(),
            height: frame.height(),
            stride: frame.stride(),
            format: frame.memory_format(),
            pixels: frame.buf_bytes(),
            delay,
            orientation: Orientation::NoTransforms,
            icc: None,
        });
        if sender.send(Event::Frame(decoded)).await.is_err() {
            return Ok(());
        }
        let Some(delay) = delay else {
            return Ok(());
        };
        due += delay;
        frame = image.next_frame().await?;
        async_io::Timer::at(due).await;
    }
}

/// Browsers show frames that ask for less than 20 ms for 100 ms, and GIFs
/// are made to look right in browsers.
fn playable(delay: Duration) -> Duration {
    if delay < Duration::from_millis(20) {
        Duration::from_millis(100)
    } else {
        delay
    }
}
