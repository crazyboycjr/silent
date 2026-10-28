//! Turning decoded frames into images GPUI can draw.

use std::sync::Arc;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use gpui_kit::RenderImage;
use silent::decode::{
    self, Decoder, Frame, MemoryFormat, MemoryFormatSelection, Orientation, Wanted,
};
use silent::geometry::Filter;
use smallvec::SmallVec;

/// A frame ready for the screen.
pub struct Prepared {
    pub image: Arc<RenderImage>,
    /// One of an animation's frames, which come on.
    pub animated: bool,
}

/// Pictures up to this many pixels on their long side are icons or pixel
/// art, enlarged pixel for pixel, as the window's geometry has it.
const PIXEL_ART: u32 = 64;

/// Decodes `uri` as large as `wanted`, turned `turns` quarters clockwise
/// by r.
pub fn decode(uri: &str, wanted: Wanted, turns: u8) -> Decoder<Prepared> {
    let accepted = MemoryFormatSelection::R8g8b8
        | MemoryFormatSelection::B8g8r8
        | MemoryFormatSelection::R8g8b8a8
        | MemoryFormatSelection::B8g8r8a8;
    decode::start(uri.to_owned(), Some(wanted), accepted, move |frame| {
        prepare(frame, &wanted, turns)
    })
}

/// The orientation of `turns` quarter turns clockwise.
pub fn quarter(turns: u8) -> Orientation {
    match turns % 4 {
        1 => Orientation::Rotate90,
        2 => Orientation::Rotate180,
        3 => Orientation::Rotate270,
        _ => Orientation::NoTransforms,
    }
}

/// `size` once turned `turns` quarters.
pub fn turned(size: (u32, u32), turns: u8) -> (u32, u32) {
    if turns % 2 == 1 {
        (size.1, size.0)
    } else {
        size
    }
}

/// A picture drawn already, such as a PDF page, turned `turns` quarters
/// clockwise; its first frame only.
pub fn turn(image: Arc<RenderImage>, turns: u8) -> Arc<RenderImage> {
    if turns % 4 == 0 {
        return image;
    }
    let size = image.size(0);
    let Some(bytes) = image.as_bytes(0) else {
        return image;
    };
    let size = (size.width.0 as u32, size.height.0 as u32);
    let (pixels, size) = upright(bytes.to_vec(), size, MemoryFormat::B8g8r8a8, quarter(turns));
    let buffer = image::RgbaImage::from_raw(size.0, size.1, pixels).expect("pixels match the size");
    Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(buffer),
        1,
    )))
}

/// Runs on the decoder's thread. GPUI samples images bilinearly without
/// mipmaps, so anything larger than it is wanted, the monitor or a zoom's
/// size, is shrunk here with a filter as wide as the reduction, and tiny
/// images are enlarged pixel for pixel. Resizing happens in the decoder's
/// own layout; what the frame still needs, its colours taken to sRGB and
/// the picture turned upright, follows on the smaller image, and the
/// swizzle to GPUI's straight BGRA comes last.
fn prepare(frame: Frame, wanted: &Wanted, turned_by_r: u8) -> Prepared {
    crate::trace::mark("frame decoded");
    let animated = frame.delay.is_some();
    let turns = silent::native::turns(frame.orientation);
    let stored = (frame.width, frame.height);
    let (width, height) = if turns { (stored.1, stored.0) } else { stored };
    // As shown: upright, and then as r turned it.
    let shown = turned((width, height), turned_by_r);
    let (bound, drawn) = wanted.bound(shown);
    let shrink = (f64::from(bound.0) / f64::from(shown.0.max(1)))
        .min(f64::from(bound.1) / f64::from(shown.1.max(1)));
    // Enlarged a whole number of times, as the screen's fit has it or to
    // fill a zoom.
    let nearest = match wanted {
        Wanted::Screen(screen) => {
            let fit = screen.fit_media(shown);
            (fit.filter == Filter::Nearest).then(|| (fit.width as u32 / shown.0).max(1))
        }
        Wanted::Within(..) => {
            let times = drawn.0 / shown.0.max(1);
            (shown.0.max(shown.1) <= PIXEL_ART && times >= 2).then_some(times)
        }
    };
    let (pixels, size) = if shrink < 1.0 {
        let upright = (
            ((width as f64 * shrink).round() as u32).max(1),
            ((height as f64 * shrink).round() as u32).max(1),
        );
        let to = if turns {
            (upright.1, upright.0)
        } else {
            upright
        };
        (
            resize(&frame, to, ResizeAlg::Convolution(FilterType::Bilinear)),
            to,
        )
    } else if let Some(factor) = nearest {
        let to = (stored.0 * factor, stored.1 * factor);
        (resize(&frame, to, ResizeAlg::Nearest), to)
    } else {
        (tight(&frame), stored)
    };
    crate::trace::mark("frame resized");
    let pixels = to_srgb(pixels, frame.format, frame.icc.as_deref());
    let (pixels, size) = upright(pixels, size, frame.format, frame.orientation);
    let (pixels, size) = upright(pixels, size, frame.format, quarter(turned_by_r));
    let bgra = to_bgra(pixels, frame.format);
    crate::trace::mark("frame prepared");
    let buffer = image::RgbaImage::from_raw(size.0, size.1, bgra).expect("pixels match the size");
    Prepared {
        image: Arc::new(RenderImage::new(SmallVec::from_elem(
            image::Frame::new(buffer),
            1,
        ))),
        animated,
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
                            part.as_chunks_mut::<4>()
                                .0
                                .iter_mut()
                                .for_each(|p| p.swap(0, 2));
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
        assert!(
            srgb[0] > pixels[0] && srgb[2] < pixels[2],
            "{:?}",
            &srgb[..3]
        );
    }

    #[test]
    fn a_profile_that_names_itself_srgb_leaves_the_pixels_alone() {
        let pixels = [200u8, 100, 50, 255].repeat(16);
        let mut profile = moxcms::ColorProfile::new_display_p3();
        profile.description = Some(moxcms::ProfileText::PlainString("sRGB IEC61966-2.1".into()));
        let named = profile.encode().unwrap();
        assert_eq!(
            to_srgb(pixels.clone(), MemoryFormat::R8g8b8a8, Some(&named)),
            pixels
        );
        assert_eq!(
            to_srgb(pixels.clone(), MemoryFormat::R8g8b8a8, None),
            pixels
        );
    }

    #[test]
    fn a_picture_turned_a_quarter_comes_upright() {
        // Left red, right blue; turned clockwise, red is on top.
        let pixels = vec![255, 0, 0, 0, 0, 255];
        let (turned, size) = upright(pixels, (2, 1), MemoryFormat::R8g8b8, Orientation::Rotate90);
        assert_eq!(size, (1, 2));
        assert_eq!(turned, vec![255, 0, 0, 0, 0, 255]);
        let (flipped, _) = upright(
            vec![255, 0, 0, 0, 0, 255],
            (2, 1),
            MemoryFormat::R8g8b8,
            Orientation::FlipHorizontal,
        );
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
