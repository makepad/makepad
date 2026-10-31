//! Preview pictures for published media: the card for an image, the poster of
//! a video, the plain tile for everything else.

use super::results::{ResultMedia, ResultThumb};
use makepad_ai_hub::testpattern;
use makepad_zune_jpeg::JpegDecoder;
use makepad_zune_png::PngDecoder;
use makepad_zune_png::makepad_zune_core::{
    bytestream::ZCursor,
    colorspace::ColorSpace,
    options::DecoderOptions,
    result::DecodingResult,
};

/// Decode and shrink an image to a card. The source stays untouched. A card
/// keeps the picture's aspect with its short side at 256 and its long side at
/// most 1024 (see [`thumbnail_layout`]), so browsing never allocates a
/// full-size texture per row.
pub fn image_thumbnail(bytes: &[u8], media: ResultMedia) -> Result<ResultThumb, String> {
    let (pixels, width, height, colorspace) = match media {
        ResultMedia::Png => {
            let options = DecoderOptions::default()
                .set_max_width(4096)
                .set_max_height(4096)
                .png_set_strip_to_8bit(true);
            let mut decoder = PngDecoder::new_with_options(ZCursor::new(bytes), options);
            decoder.decode_headers().map_err(|error| format!("decode PNG thumbnail: {error}"))?;
            let (width, height) = decoder.dimensions().ok_or("PNG thumbnail dimensions missing")?;
            let colorspace = decoder.colorspace().ok_or("PNG thumbnail colorspace missing")?;
            let pixels = match decoder.decode().map_err(|error| format!("decode PNG thumbnail: {error}"))? {
                DecodingResult::U8(pixels) => pixels,
                _ => return Err("PNG thumbnail did not decode to 8-bit pixels".into()),
            };
            (pixels, width, height, colorspace)
        }
        ResultMedia::Jpeg => {
            let options = DecoderOptions::default()
                .set_max_width(4096)
                .set_max_height(4096)
                .jpeg_set_out_colorspace(ColorSpace::RGB);
            let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
            decoder.decode_headers().map_err(|error| format!("decode JPEG thumbnail: {error}"))?;
            let (width, height) = decoder.dimensions().ok_or("JPEG thumbnail dimensions missing")?;
            let pixels = decoder.decode().map_err(|error| format!("decode JPEG thumbnail: {error}"))?;
            (pixels, width, height, ColorSpace::RGB)
        }
        _ => return Err("image thumbnail requires PNG or JPEG".into()),
    };
    let rgba = rgba_pixels(&pixels, colorspace)?;
    let layout = thumbnail_layout(width, height);
    let (out_width, out_height) = (layout.width, layout.height);
    let mut out = [39u8, 45, 55, 255].repeat(out_width * out_height);
    for y in 0..layout.picture.1 {
        let source_y = y * height / layout.picture.1;
        for x in 0..layout.picture.0 {
            let source_x = x * width / layout.picture.0;
            let source = (source_y * width + source_x) * 4;
            let target = ((layout.at.1 + y) * out_width + layout.at.0 + x) * 4;
            out[target..target + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    let png = testpattern::encode_png_rgba(&out, out_width, out_height)
        .map_err(|error| format!("encode image thumbnail: {error}"))?;
    Ok(ResultThumb { png, width: out_width as u32, height: out_height as u32 })
}

const THUMBNAIL_SHORT: usize = 256;
const THUMBNAIL_LONG_MAX: usize = 1024;

/// Where a picture goes in its card: the card's size, and the scaled
/// picture's size and offset in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThumbnailLayout {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) picture: (usize, usize),
    pub(crate) at: (usize, usize),
}

/// The card for a `width`×`height` picture. Up to 4:1 it is the picture
/// itself, scaled so its short side is 256 (16:9 is 455×256, 9:16 256×455).
/// Past that the picture is fitted to a 1024 long side and centred on a
/// band 256 high (or wide), so the card never falls under the catalog's
/// 256 px minimum and never needs more than 1024.
pub(crate) fn thumbnail_layout(width: usize, height: usize) -> ThumbnailLayout {
    let (width, height) = (width.max(1), height.max(1));
    let (long, short, wide) = if width >= height { (width, height, true) } else { (height, width, false) };
    let scaled_long = (long * THUMBNAIL_SHORT).div_ceil(short);
    let (picture_long, picture_short) = if scaled_long <= THUMBNAIL_LONG_MAX {
        (scaled_long, THUMBNAIL_SHORT)
    } else {
        (THUMBNAIL_LONG_MAX, (short * THUMBNAIL_LONG_MAX / long).max(1))
    };
    let offset = (THUMBNAIL_SHORT - picture_short) / 2;
    if wide {
        ThumbnailLayout { width: picture_long, height: THUMBNAIL_SHORT, picture: (picture_long, picture_short), at: (0, offset) }
    } else {
        ThumbnailLayout { width: THUMBNAIL_SHORT, height: picture_long, picture: (picture_short, picture_long), at: (offset, 0) }
    }
}

fn rgba_pixels(pixels: &[u8], colorspace: ColorSpace) -> Result<Vec<u8>, String> {
    let components = colorspace.num_components();
    if components == 0 || pixels.len() % components != 0 {
        return Err("decoded image has invalid channel data".into());
    }
    let mut rgba = Vec::with_capacity(pixels.len() / components * 4);
    for chunk in pixels.chunks_exact(components) {
        let (r, g, b, a) = match colorspace {
            ColorSpace::RGB | ColorSpace::YCbCr => (chunk[0], chunk[1], chunk[2], 255),
            ColorSpace::RGBA => (chunk[0], chunk[1], chunk[2], chunk[3]),
            ColorSpace::BGR => (chunk[2], chunk[1], chunk[0], 255),
            ColorSpace::BGRA => (chunk[2], chunk[1], chunk[0], chunk[3]),
            ColorSpace::Luma => (chunk[0], chunk[0], chunk[0], 255),
            ColorSpace::LumaA => (chunk[0], chunk[0], chunk[0], chunk[1]),
            ColorSpace::ARGB => (chunk[1], chunk[2], chunk[3], chunk[0]),
            _ => return Err("unsupported image colorspace".into()),
        };
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    Ok(rgba)
}

pub fn video_thumbnail(bytes: &[u8]) -> Result<ResultThumb, String> {
    let frame = makepad_video::decode_first_frame_from_bytes(bytes)
        .map_err(|error| format!("decode first video frame: {error}"))?;
    let rgb = frame.to_rgb8();
    let (width, height) = (frame.width as usize, frame.height as usize);
    if width == 0 || height == 0 || rgb.len() != width.saturating_mul(height).saturating_mul(3) {
        return Err("video poster has invalid dimensions".into());
    }
    // Fill the square library tile with a centered crop. The full video
    // remains available in the viewer at its original aspect ratio.
    const SIDE: usize = 256;
    let crop = width.min(height);
    let (left, top) = ((width - crop) / 2, (height - crop) / 2);
    let mut rgba = vec![255; SIDE * SIDE * 4];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let source = ((top + y * crop / SIDE) * width + left + x * crop / SIDE) * 3;
            let target = (y * SIDE + x) * 4;
            rgba[target..target + 3].copy_from_slice(&rgb[source..source + 3]);
        }
    }
    let png = testpattern::encode_png_rgba(&rgba, SIDE, SIDE)
        .map_err(|error| format!("encode video poster: {error}"))?;
    Ok(ResultThumb { png, width: SIDE as u32, height: SIDE as u32 })
}

pub fn placeholder_thumbnail() -> Result<ResultThumb, String> {
    let mut pixels = vec![0u8; 256 * 256 * 4];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[39, 45, 55, 255]);
    }
    let bytes = testpattern::encode_png_rgba(&pixels, 256, 256)
        .map_err(|error| format!("build Publish thumbnail: {error}"))?;
    Ok(ResultThumb { png: bytes, width: 256, height: 256 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn video_posters_decode_h264_and_hevc_without_replacing_the_original() {
        use makepad_video::{VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions};
        let dir = std::env::temp_dir().join(format!("flow-video-poster-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        for codec in [VideoFileCodec::H264, VideoFileCodec::H265] {
            let mut posters = Vec::new();
            for (index, color) in [[210u8, 30, 60], [20u8, 90, 220]].into_iter().enumerate() {
                let path = dir.join(format!("{codec:?}-{index}.mp4"));
                let mut encoder = VideoFileEncoder::new(path.to_str().unwrap(), VideoFileEncoderOptions {
                    codec, width: 128, height: 64, fps_num: 24, fps_den: 1,
                    video_bitrate_bps: 1_000_000, audio: None, keyframe_only: true, ..Default::default()
                }).unwrap();
                encoder.push_frame_rgb8(&color.repeat(128 * 64), None).unwrap();
                encoder.finish().unwrap();
                let original = std::fs::read(&path).unwrap();
                let poster = video_thumbnail(&original).unwrap();
                assert_eq!((poster.width, poster.height), (256, 256));
                assert_ne!(poster.png, placeholder_thumbnail().unwrap().png);
                assert_eq!(std::fs::read(&path).unwrap(), original);
                posters.push(poster.png);
            }
            assert_ne!(posters[0], posters[1], "poster must reflect the source frame's pixels");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Every aspect gets a card the catalog accepts (256..=4096 a side,
    /// what a store's catalog checks) that keeps the picture's
    /// aspect, letterboxed only past 4:1.
    #[test]
    fn thumbnails_fit_the_catalog_bounds_at_any_aspect() {
        let cases = [
            ((1024, 1024), (256, 256), (256, 256)),
            ((1920, 1088), (452, 256), (452, 256)),
            ((1024, 576), (456, 256), (456, 256)),
            ((1088, 1920), (256, 452), (256, 452)),
            ((576, 1024), (256, 456), (256, 456)),
            ((4096, 64), (1024, 256), (1024, 16)),
            ((64, 4096), (256, 1024), (16, 1024)),
            ((2048, 512), (1024, 256), (1024, 256)),
            ((100, 30), (854, 256), (854, 256)),
            ((1, 1), (256, 256), (256, 256)),
        ];
        for ((width, height), card, picture) in cases {
            let layout = thumbnail_layout(width, height);
            assert_eq!(((layout.width, layout.height), layout.picture), (card, picture), "{width}x{height}");
            assert!((256..=4096).contains(&layout.width) && (256..=4096).contains(&layout.height));
            assert!(layout.at.0 + layout.picture.0 <= layout.width && layout.at.1 + layout.picture.1 <= layout.height);
        }
    }
}
