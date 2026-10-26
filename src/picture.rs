//! Turning decoded frames into images GPUI can draw.

use std::sync::Arc;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use gpui_kit::RenderImage;
use silent::decode::{self, Decoder, Frame, MemoryFormat, MemoryFormatSelection, Orientation};
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
/// Resizing happens in the decoder's own layout; what the frame still
/// needs, its colours taken to sRGB and the picture turned upright, follows
/// on the smaller image, and the swizzle to GPUI's straight BGRA comes last.
fn prepare(frame: Frame, screen: &Screen) -> Prepared {
    crate::trace::mark("frame decoded");
    let turns = silent::native::turns(frame.orientation);
    let stored = (frame.width, frame.height);
    let (width, height) = if turns { (stored.1, stored.0) } else { stored };
    let monitor = screen.monitor;
    let shrink = (monitor.width as f64 / width as f64).min(monitor.height as f64 / height as f64);
    let fit = screen.fit_media((width, height));
    let (pixels, size) = if shrink < 1.0 {
        let upright = (
            ((width as f64 * shrink).round() as u32).max(1),
            ((height as f64 * shrink).round() as u32).max(1),
        );
        let to = if turns { (upright.1, upright.0) } else { upright };
        (
            resize(&frame, to, ResizeAlg::Convolution(FilterType::Bilinear)),
            to,
        )
    } else if fit.filter == Filter::Nearest {
        let factor = (fit.width as u32 / width).max(1);
        let to = (stored.0 * factor, stored.1 * factor);
        (resize(&frame, to, ResizeAlg::Nearest), to)
    } else {
        (tight(&frame), stored)
    };
    crate::trace::mark("frame resized");
    let pixels = to_srgb(pixels, frame.format, frame.icc.as_deref());
    let (pixels, size) = upright(pixels, size, frame.format, frame.orientation);
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

/// The pixels in sRGB, from the colours of `icc`; a profile that calls itself
/// sRGB, as most cameras' do, is taken at its word.
fn to_srgb(mut pixels: Vec<u8>, format: MemoryFormat, icc: Option<&[u8]>) -> Vec<u8> {
    let layout = match format {
        MemoryFormat::R8g8b8 => moxcms::Layout::Rgb,
        MemoryFormat::R8g8b8a8 => moxcms::Layout::Rgba,
        _ => return pixels,
    };
    let Some(profile) = icc.and_then(|icc| moxcms::ColorProfile::new_from_slice(icc).ok()) else {
        return pixels;
    };
    let named = |text: &moxcms::ProfileText| match text {
        moxcms::ProfileText::PlainString(name) => name.contains("sRGB"),
        moxcms::ProfileText::Localizable(names) => names.iter().any(|n| n.value.contains("sRGB")),
        moxcms::ProfileText::Description(name) => name.ascii_string.contains("sRGB"),
    };
    if profile.description.as_ref().is_some_and(named) {
        return pixels;
    }
    let Ok(transform) = profile.create_in_place_transform_8bit(
        layout,
        &moxcms::ColorProfile::new_srgb(),
        moxcms::TransformOptions::default(),
    ) else {
        return pixels;
    };
    let pixel = channels(format);
    shared(pixels.len() / pixel, |threads| {
        let chunk = (pixels.len() / pixel).div_ceil(threads) * pixel;
        std::thread::scope(|scope| {
            for part in pixels.chunks_mut(chunk.max(pixel)) {
                let transform = &transform;
                scope.spawn(move || {
                    let _ = transform.transform(part);
                });
            }
        });
    });
    pixels
}

/// The picture turned upright, and its size then.
fn upright(
    pixels: Vec<u8>,
    size: (u32, u32),
    format: MemoryFormat,
    orientation: Orientation,
) -> (Vec<u8>, (u32, u32)) {
    if orientation == Orientation::NoTransforms {
        return (pixels, size);
    }
    let mut picture = match channels(format) {
        3 => image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(size.0, size.1, pixels).expect("pixels match the size"),
        ),
        _ => image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(size.0, size.1, pixels).expect("pixels match the size"),
        ),
    };
    picture.apply_orientation(orientation);
    let size = (picture.width(), picture.height());
    (picture.into_bytes(), size)
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

/// GPUI's straight BGRA, the work shared among threads for a large picture.
fn to_bgra(pixels: Vec<u8>, format: MemoryFormat) -> Vec<u8> {
    match format {
        MemoryFormat::B8g8r8a8 => pixels,
        MemoryFormat::R8g8b8a8 => {
            let mut pixels = pixels;
            shared(pixels.len() / 4, |threads| {
                let chunk = (pixels.len() / 4).div_ceil(threads) * 4;
                std::thread::scope(|scope| {
                    for part in pixels.chunks_mut(chunk.max(4)) {
                        scope.spawn(move || {
                            part.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
                        });
                    }
                });
            });
            pixels
        }
        _ => {
            let rgb = format == MemoryFormat::R8g8b8;
            let mut bgra = vec![0u8; pixels.len() / 3 * 4];
            shared(pixels.len() / 3, |threads| {
                let per = (pixels.len() / 3).div_ceil(threads).max(1);
                std::thread::scope(|scope| {
                    for (from, to) in pixels.chunks(per * 3).zip(bgra.chunks_mut(per * 4)) {
                        scope.spawn(move || {
                            let to = to.as_chunks_mut::<4>().0;
                            for (&[a, g, c], out) in from.as_chunks::<3>().0.iter().zip(to) {
                                *out = if rgb { [c, g, a, 255] } else { [a, g, c, 255] };
                            }
                        });
                    }
                });
            });
            bgra
        }
    }
}

/// Runs `work` with as many threads as a picture of `pixels` pixels is worth:
/// one below a million, which the threads would only slow.
fn shared(pixels: usize, work: impl FnOnce(usize)) {
    let threads = if pixels < 1 << 20 {
        1
    } else {
        std::thread::available_parallelism().map_or(1, |n| n.get().min(8))
    };
    work(threads);
}

#[cfg(test)]
mod tests {
    use silent::decode::{MemoryFormat, Orientation};

    use super::{to_bgra, to_srgb, upright};

    #[test]
    fn a_display_p3_picture_is_taken_to_srgb() {
        let pixels = [200u8, 100, 50].repeat(16);
        let p3 = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let srgb = to_srgb(pixels.clone(), MemoryFormat::R8g8b8, Some(&p3));
        assert_ne!(srgb, pixels);
        // Display P3's orange is more saturated than sRGB's, and stays so.
        assert!(srgb[0] > pixels[0] && srgb[2] < pixels[2], "{:?}", &srgb[..3]);
    }

    #[test]
    fn a_profile_that_names_itself_srgb_leaves_the_pixels_alone() {
        let pixels = [200u8, 100, 50, 255].repeat(16);
        let mut profile = moxcms::ColorProfile::new_display_p3();
        profile.description = Some(moxcms::ProfileText::PlainString("sRGB IEC61966-2.1".into()));
        let named = profile.encode().unwrap();
        assert_eq!(to_srgb(pixels.clone(), MemoryFormat::R8g8b8a8, Some(&named)), pixels);
        assert_eq!(to_srgb(pixels.clone(), MemoryFormat::R8g8b8a8, None), pixels);
    }

    #[test]
    fn a_picture_turned_a_quarter_comes_upright() {
        // Left red, right blue; turned clockwise, red is on top.
        let pixels = vec![255, 0, 0, 0, 0, 255];
        let (turned, size) = upright(pixels, (2, 1), MemoryFormat::R8g8b8, Orientation::Rotate90);
        assert_eq!(size, (1, 2));
        assert_eq!(turned, vec![255, 0, 0, 0, 0, 255]);
        let (flipped, _) = upright(vec![255, 0, 0, 0, 0, 255], (2, 1), MemoryFormat::R8g8b8, Orientation::FlipHorizontal);
        assert_eq!(flipped, vec![0, 0, 255, 255, 0, 0]);
    }

    #[test]
    fn a_large_picture_swizzles_as_a_small_one() {
        // Over a million pixels, the work is shared among threads.
        let pixels: Vec<u8> = (0..(1 << 20) * 3 + 9).map(|i| (i % 251) as u8).collect();
        let bgra = to_bgra(pixels.clone(), MemoryFormat::R8g8b8);
        assert_eq!(bgra.len(), pixels.len() / 3 * 4);
        for (rgb, out) in pixels.chunks(3).zip(bgra.chunks(4)).step_by(9973) {
            assert_eq!(out, [rgb[2], rgb[1], rgb[0], 255]);
        }
        let rgba: Vec<u8> = (0..(1 << 20) * 4).map(|i| (i % 253) as u8).collect();
        let swapped = to_bgra(rgba.clone(), MemoryFormat::R8g8b8a8);
        for (a, b) in rgba.chunks(4).zip(swapped.chunks(4)).step_by(10007) {
            assert_eq!(b, [a[2], a[1], a[0], a[3]]);
        }
    }
}
