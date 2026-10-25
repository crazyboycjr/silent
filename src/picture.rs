//! Turning decoded frames into images GPUI can draw.

use std::sync::Arc;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use gpui_kit::RenderImage;
use silent::decode::{self, Decoder, Frame, MemoryFormat, MemoryFormatSelection};
use silent::geometry::{Filter, Screen};
use smallvec::SmallVec;

/// A frame ready for the screen.
pub struct Prepared {
    pub image: Arc<RenderImage>,
}

pub fn decode(uri: &str, screen: Screen) -> Decoder<Prepared> {
    let accepted = MemoryFormatSelection::R8g8b8
        | MemoryFormatSelection::B8g8r8
        | MemoryFormatSelection::R8g8b8a8
        | MemoryFormatSelection::B8g8r8a8;
    decode::start(uri.to_owned(), Some(screen), accepted, move |frame| {
        prepare(frame, &screen)
    })
}

/// Runs on the decoder's thread. GPUI samples images bilinearly without
/// mipmaps, so anything larger than the monitor is shrunk here with a filter
/// as wide as the reduction, and tiny images are enlarged pixel for pixel.
/// Resizing happens in the decoder's own layout and the swizzle to GPUI's
/// straight BGRA comes last, on the smaller image.
fn prepare(frame: Frame, screen: &Screen) -> Prepared {
    crate::trace::mark("frame decoded");
    let (width, height) = (frame.width, frame.height);
    let monitor = screen.monitor;
    let shrink = (monitor.width as f64 / width as f64).min(monitor.height as f64 / height as f64);
    let fit = screen.fit_media((width, height));
    let (pixels, size) = if shrink < 1.0 {
        let to = (
            ((width as f64 * shrink).round() as u32).max(1),
            ((height as f64 * shrink).round() as u32).max(1),
        );
        (
            resize(&frame, to, ResizeAlg::Convolution(FilterType::Bilinear)),
            to,
        )
    } else if fit.filter == Filter::Nearest {
        let factor = (fit.width as u32 / width).max(1);
        let to = (width * factor, height * factor);
        (resize(&frame, to, ResizeAlg::Nearest), to)
    } else {
        (tight(&frame), (width, height))
    };
    crate::trace::mark("frame resized");
    let bgra = to_bgra(pixels, frame.format);
    crate::trace::mark("frame prepared");
    let buffer = image::RgbaImage::from_raw(size.0, size.1, bgra).expect("pixels match the size");
    Prepared {
        image: Arc::new(RenderImage::new(SmallVec::from_elem(
            image::Frame::new(buffer),
            1,
        ))),
    }
}

fn channels(format: MemoryFormat) -> usize {
    match format {
        MemoryFormat::R8g8b8 | MemoryFormat::B8g8r8 => 3,
        _ => 4,
    }
}

fn tight(frame: &Frame) -> Vec<u8> {
    let row = frame.width as usize * channels(frame.format);
    let stride = frame.stride as usize;
    if stride == row {
        return frame.pixels.to_vec();
    }
    frame
        .pixels
        .chunks(stride)
        .flat_map(|line| &line[..row])
        .copied()
        .collect()
}

fn resize(frame: &Frame, to: (u32, u32), algorithm: ResizeAlg) -> Vec<u8> {
    let channels = channels(frame.format);
    let pixel_type = if channels == 3 {
        PixelType::U8x3
    } else {
        PixelType::U8x4
    };
    let copy;
    let pixels: &[u8] = if frame.stride as usize == frame.width as usize * channels {
        &frame.pixels
    } else {
        copy = tight(frame);
        &copy
    };
    let source = ImageRef::new(frame.width, frame.height, pixels, pixel_type)
        .expect("a tight buffer of the frame's size");
    let mut target = Image::new(to.0, to.1, pixel_type);
    let options = ResizeOptions::new()
        .resize_alg(algorithm)
        .use_alpha(channels == 4);
    Resizer::new()
        .resize(&source, &mut target, &options)
        .expect("resizing between images of one pixel type");
    target.into_vec()
}

fn to_bgra(pixels: Vec<u8>, format: MemoryFormat) -> Vec<u8> {
    match format {
        MemoryFormat::B8g8r8a8 => pixels,
        MemoryFormat::R8g8b8a8 => {
            let mut pixels = pixels;
            pixels
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .for_each(|p| p.swap(0, 2));
            pixels
        }
        MemoryFormat::R8g8b8 => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [b, g, r, 255])
            .collect(),
        _ => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[b, g, r]| [b, g, r, 255])
            .collect(),
    }
}
