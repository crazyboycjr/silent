//! Image decoding through glycin's sandboxed loaders, on a thread of its own.

use std::time::{Duration, Instant};

use gio::glib;
use gio::prelude::*;
pub use glycin::{MemoryFormat, MemoryFormatSelection};

use crate::geometry::Screen;

/// Decoded pixels, upright (EXIF orientation applied).
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
}

#[derive(Debug)]
pub enum Event {
    /// The upright size, known before any pixels.
    Size {
        width: u32,
        height: u32,
    },
    Frame(Frame),
    Failed(String),
}

/// A running decode. Animations keep producing frames, paced by their
/// delays, until this is dropped.
pub struct Decoder {
    pub events: async_channel::Receiver<Event>,
    cancellable: gio::Cancellable,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.cancellable.cancel();
    }
}

/// Decodes the image at `uri`. Vector images are rendered at the size
/// `screen` gives them; everything else arrives at its own resolution, in
/// one of the `accepted` formats.
pub fn start(uri: String, screen: Option<Screen>, accepted: MemoryFormatSelection) -> Decoder {
    let (sender, events) = async_channel::bounded(2);
    let cancellable = gio::Cancellable::new();
    let thread_cancellable = cancellable.clone();
    std::thread::Builder::new()
        .name("decode".into())
        .spawn(move || {
            async_io::block_on(async {
                let decoded = run(&uri, screen, accepted, thread_cancellable, &sender).await;
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

async fn run(
    uri: &str,
    screen: Option<Screen>,
    accepted: MemoryFormatSelection,
    cancellable: gio::Cancellable,
    sender: &async_channel::Sender<Event>,
) -> Result<(), glycin::Error> {
    let mut loader = glycin::Loader::new(gio::File::for_uri(uri));
    loader
        .cancellable(cancellable)
        .main_context_selector(glycin::MainContextSelector::Managed)
        .accepted_memory_formats(accepted);
    let mut image = loader.load().await?;
    let details = image.details();
    let (width, height) = (details.width(), details.height());
    if sender.send(Event::Size { width, height }).await.is_err() {
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
        let decoded = Frame {
            width: frame.width(),
            height: frame.height(),
            stride: frame.stride(),
            format: frame.memory_format(),
            pixels: frame.buf_bytes(),
            delay,
        };
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
