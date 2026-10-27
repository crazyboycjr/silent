//! JPEG and still PNG pictures decoded in this process, in pure Rust,
//! rather than in glycin's sandboxed loader, whose process takes some 35 ms
//! more to start and to hand the pixels over: JPEG by libjpeg-turbo-rs, a
//! port of libjpeg-turbo, PNG by the png crate `image` builds on. A JPEG two
//! or more times the size it is shown at is decoded at a half, a quarter or
//! an eighth of its size, as libjpeg-turbo scales in its inverse DCT. The
//! picture is turned upright, and taken to sRGB from its ICC profile, by
//! whoever draws it, after shrinking it. Everything else, and these when
//! they fail here, go to glycin, which keeps its sandbox for every other
//! format.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use glycin::MemoryFormat;
use image::ImageDecoder;
use image::metadata::Orientation;

/// Larger pictures go to glycin, whose loader runs with a memory limit.
const MAX_PIXELS: u64 = 120_000_000;

/// A picture whose headers have been read.
pub struct Opened {
    picture: Picture,
    /// As stored, before it is turned upright.
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,
    /// A PNG's; a JPEG's comes with its pixels.
    icc: Option<Arc<[u8]>>,
}

enum Picture {
    Jpeg(Vec<u8>),
    Png(Box<image::codecs::png::PngDecoder<Cursor<Vec<u8>>>>),
}

/// Decoded pixels, as stored.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub format: MemoryFormat,
    pub pixels: Vec<u8>,
    pub icc: Option<Arc<[u8]>>,
}

/// Reads the headers of the JPEG or still PNG at `path`; anything else, or a
/// picture these decoders cannot read, is `None`.
pub fn open(path: &Path) -> Option<Opened> {
    let bytes = std::fs::read(path).ok()?;
    let opened = if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        let info = libjpeg_turbo_rs::probe(&bytes).ok()?;
        Opened {
            width: u32::try_from(info.width).ok()?,
            height: u32::try_from(info.height).ok()?,
            orientation: info
                .exif_orientation
                .and_then(Orientation::from_exif)
                .unwrap_or(Orientation::NoTransforms),
            icc: None,
            picture: Picture::Jpeg(bytes),
        }
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).ok()?;
        // Animations stay with glycin, which plays them.
        if decoder.is_apng().unwrap_or(true) {
            return None;
        }
        let (width, height) = decoder.dimensions();
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        let icc = decoder.icc_profile().ok().flatten().map(Arc::from);
        Opened {
            width,
            height,
            orientation,
            icc,
            picture: Picture::Png(Box::new(decoder)),
        }
    } else {
        return None;
    };
    (u64::from(opened.width) * u64::from(opened.height) <= MAX_PIXELS).then_some(opened)
}

impl Opened {
    /// The size once turned upright.
    pub fn upright(&self) -> (u32, u32) {
        if turns(self.orientation) {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// Decodes the pixels, no smaller than `at_least` as stored when that
    /// lets a JPEG be decoded at a fraction of its size.
    pub fn decode(self, at_least: Option<(u32, u32)>) -> Result<Pixels, String> {
        match self.picture {
            Picture::Jpeg(bytes) => {
                let mut decoder =
                    libjpeg_turbo_rs::Decoder::new(&bytes).map_err(|e| e.to_string())?;
                decoder.set_output_format(libjpeg_turbo_rs::PixelFormat::Rgb);
                // A damaged stream shows what it has, grey where it fails.
                decoder.set_lenient(true);
                if let Some(at_least) = at_least
                    && let Some(by) = eighths(self.width, self.height, at_least)
                {
                    decoder.set_scale(libjpeg_turbo_rs::ScalingFactor::new(1, by));
                }
                let image = decoder.decode_image().map_err(|e| e.to_string())?;
                if image.data.len() != image.width * image.height * 3 {
                    return Err("not RGB".into());
                }
                Ok(Pixels {
                    width: image.width as u32,
                    height: image.height as u32,
                    format: MemoryFormat::R8g8b8,
                    pixels: image.data,
                    icc: image.icc_profile.map(Arc::from),
                })
            }
            Picture::Png(decoder) => png(*decoder, self.icc),
        }
    }
}

/// Whether `orientation` turns the picture a quarter, swapping its sides.
pub fn turns(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// What a `width` by `height` JPEG is divided by as it is decoded: a half,
/// a quarter or an eighth, the smallest fraction that still covers
/// `at_least`. The fractions between, 5/8 to 7/8, are slower than the
/// whole.
fn eighths(width: u32, height: u32, at_least: (u32, u32)) -> Option<u32> {
    [8, 4, 2]
        .into_iter()
        .find(|&by| width.div_ceil(by) >= at_least.0 && height.div_ceil(by) >= at_least.1)
}

fn png(
    decoder: image::codecs::png::PngDecoder<Cursor<Vec<u8>>>,
    icc: Option<Arc<[u8]>>,
) -> Result<Pixels, String> {
    use image::ColorType;
    let (width, height) = decoder.dimensions();
    let color = decoder.color_type();
    let mut raw = vec![0u8; usize::try_from(decoder.total_bytes()).map_err(|e| e.to_string())?];
    decoder.read_image(&mut raw).map_err(|e| e.to_string())?;
    // Sixteen bits a channel keep their high byte, as glycin's eight-bit
    // formats would have them; grey becomes RGB.
    let (pixels, format) = match color {
        ColorType::Rgb8 => (raw, MemoryFormat::R8g8b8),
        ColorType::Rgba8 => (raw, MemoryFormat::R8g8b8a8),
        ColorType::L8 => (
            raw.iter().flat_map(|&l| [l, l, l]).collect(),
            MemoryFormat::R8g8b8,
        ),
        ColorType::La8 => (
            raw.as_chunks::<2>()
                .0
                .iter()
                .flat_map(|&[l, a]| [l, l, l, a])
                .collect(),
            MemoryFormat::R8g8b8a8,
        ),
        ColorType::Rgb16 => (high_bytes(&raw), MemoryFormat::R8g8b8),
        ColorType::Rgba16 => (high_bytes(&raw), MemoryFormat::R8g8b8a8),
        ColorType::L16 => (
            high_bytes(&raw).iter().flat_map(|&l| [l, l, l]).collect(),
            MemoryFormat::R8g8b8,
        ),
        ColorType::La16 => (
            high_bytes(&raw)
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|&[l, a]| [l, l, l, a])
                .collect(),
            MemoryFormat::R8g8b8a8,
        ),
        _ => return Err(format!("{color:?}")),
    };
    Ok(Pixels {
        width,
        height,
        format,
        pixels,
        icc,
    })
}

/// The high byte of each native-endian 16-bit sample.
fn high_bytes(raw: &[u8]) -> Vec<u8> {
    raw.as_chunks::<2>()
        .0
        .iter()
        .map(|&sample| (u16::from_ne_bytes(sample) >> 8) as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::metadata::Orientation;
    use image::{ImageEncoder, ImageFormat};

    use super::{eighths, open};

    #[test]
    fn a_jpeg_is_decoded_at_the_smallest_fraction_that_covers_the_screen() {
        // 8000 by 1500, shown at 3840 by 720: a half.
        assert_eq!(eighths(8000, 1500, (3840, 720)), Some(2));
        // 4032 by 3024 at 2880 by 2160: no fraction offered covers it.
        assert_eq!(eighths(4032, 3024, (2880, 2160)), None);
        // Odd sizes round up, as libjpeg-turbo's scaling does: 1001 by 751.
        assert_eq!(eighths(8001, 6001, (1000, 750)), Some(8));
    }

    /// A file in a directory of its own, removed with it.
    struct Written(std::path::PathBuf);

    impl Written {
        fn new(name: &str, bytes: &[u8]) -> Self {
            let dir =
                std::env::temp_dir().join(format!("silent-native-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            Written(path)
        }
    }

    impl Drop for Written {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn gradient(width: u32, height: u32) -> image::RgbImage {
        image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 4) as u8, 128])
        })
    }

    fn jpeg(picture: &image::RgbImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(picture.clone())
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    /// The JPEG with an Exif segment saying `orientation`, after its SOI.
    fn with_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = b"MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        tiff.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0, 0, 0, 1]);
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut segment = b"Exif\0\0".to_vec();
        segment.extend_from_slice(&tiff);
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xff, 0xe1]);
        out.extend_from_slice(&((segment.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&segment);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn a_jpeg_turned_by_its_exif_is_upright_in_size_and_stored_in_pixels() {
        let file = Written::new("turned.jpg", &with_orientation(&jpeg(&gradient(64, 32)), 6));
        let opened = open(&file.0).expect("a JPEG");
        assert_eq!(opened.orientation, Orientation::Rotate90);
        assert_eq!(opened.upright(), (32, 64));
        let pixels = opened.decode(None).unwrap();
        assert_eq!((pixels.width, pixels.height), (64, 32));
        assert_eq!(pixels.pixels.len(), 64 * 32 * 3);
    }

    /// libjpeg-turbo and zune-jpeg, which `image` uses, round their inverse
    /// DCTs and chroma upsampling apart: the pixels agree within a few levels.
    #[test]
    fn a_jpeg_decodes_as_image_decodes_it_within_rounding() {
        let bytes = jpeg(&gradient(48, 40));
        let file = Written::new("plain.jpg", &bytes);
        let ours = open(&file.0).unwrap().decode(None).unwrap();
        let theirs = image::load_from_memory(&bytes)
            .unwrap()
            .into_rgb8()
            .into_raw();
        assert_eq!(ours.pixels.len(), theirs.len());
        let most = ours
            .pixels
            .iter()
            .zip(&theirs)
            .map(|(a, b)| a.abs_diff(*b))
            .max();
        assert!(most.unwrap() <= 16, "{most:?}");
    }

    #[test]
    fn a_large_jpeg_is_decoded_at_a_fraction() {
        let file = Written::new("large.jpg", &jpeg(&gradient(64, 48)));
        let pixels = open(&file.0).unwrap().decode(Some((30, 20))).unwrap();
        assert_eq!((pixels.width, pixels.height), (32, 24));
        assert_eq!(pixels.pixels.len(), 32 * 24 * 3);
    }

    #[test]
    fn a_sixteen_bit_grey_png_keeps_its_high_bytes_as_rgb() {
        let grey = image::ImageBuffer::<image::Luma<u16>, _>::from_fn(4, 2, |x, _| {
            image::Luma([x as u16 * 0x1234])
        });
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                bytemuck_bytes(grey.as_raw()).as_slice(),
                4,
                2,
                image::ExtendedColorType::L16,
            )
            .unwrap();
        let file = Written::new("grey.png", &bytes);
        let pixels = open(&file.0).unwrap().decode(None).unwrap();
        assert_eq!(pixels.format, glycin::MemoryFormat::R8g8b8);
        assert_eq!(
            &pixels.pixels[..12],
            &[
                0, 0, 0, 0x12, 0x12, 0x12, 0x24, 0x24, 0x24, 0x36, 0x36, 0x36
            ]
        );
    }

    /// Native-endian bytes of 16-bit samples, as `write_image` takes them.
    fn bytemuck_bytes(samples: &[u16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_ne_bytes()).collect()
    }

    #[test]
    fn an_animated_png_is_left_to_glycin() {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(gradient(8, 8))
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        // An acTL chunk after IHDR makes it an APNG.
        let ihdr_end = 8 + 4 + 4 + 13 + 4;
        let mut chunk = b"acTL".to_vec();
        chunk.extend_from_slice(&1u32.to_be_bytes());
        chunk.extend_from_slice(&0u32.to_be_bytes());
        let mut apng = bytes[..ihdr_end].to_vec();
        apng.extend_from_slice(&8u32.to_be_bytes());
        apng.extend_from_slice(&chunk);
        apng.extend_from_slice(&crc32(&chunk).to_be_bytes());
        apng.extend_from_slice(&bytes[ihdr_end..]);
        let file = Written::new("animated.png", &apng);
        assert!(open(&file.0).is_none());
        let still = Written::new("still.png", &bytes);
        assert!(open(&still.0).is_some());
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
