//! Pictures decoded to RGBA: PNG (and animated PNG), JPEG, WebP (and animated WebP), GIF, BMP,
//! QOI and ICO, each still picture upright as its EXIF says, an animation as its composited
//! frames and their delays. No platform: the app's drawing (`makepad-draw`'s `ImageBuffer`) and
//! headless tools read pictures through this one decoder.

use makepad_gif::{ColorOutput, DecodeOptions, DisposalMethod};
use makepad_webp::WebPDecoder;
use makepad_zune_bmp::BmpDecoder;
use makepad_zune_jpeg::JpegDecoder;
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::{BlendOp, DisposeOp, PngDecoder};
use makepad_zune_qoi::QoiDecoder;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

pub use makepad_gif::DecodingError as GifDecodeErrors;
pub use makepad_webp::DecodingError as WebpDecodeErrors;
pub use makepad_zune_bmp::BmpDecoderErrors;
pub use makepad_zune_jpeg::errors::DecodeErrors as JpgDecodeErrors;
pub use makepad_zune_png::error::PngDecodeErrors;
pub use makepad_zune_qoi::QoiErrors;

/// The largest side a picture may have, pixels.
pub const MAX_IMAGE_DIMENSION: usize = 16384;
/// Hard upper bound on a decoded image or animation atlas in bytes. Browser
/// textures have a tighter admission limit; native retains the existing 1 GiB
/// cap (the RGBA size of a 16384x16384 image). Other 32-bit targets (e.g.,
/// armv7 Android) get 128 MiB (32 MP): decoding holds a few buffers of up to
/// this size at once, and several 1 GiB ones don't fit in a 32-bit address
/// space, so an oversized image would abort the app instead of failing.
pub const WEB_MAX_IMAGE_DECODED_BYTES: usize = 64 * 1024 * 1024;
pub const NATIVE_32BIT_MAX_IMAGE_DECODED_BYTES: usize = 128 * 1024 * 1024;
pub const NATIVE_MAX_IMAGE_DECODED_BYTES: usize = 1024 * 1024 * 1024;
pub const MAX_IMAGE_DECODED_BYTES: usize = if cfg!(target_arch = "wasm32") {
    WEB_MAX_IMAGE_DECODED_BYTES
} else if cfg!(target_pointer_width = "32") {
    NATIVE_32BIT_MAX_IMAGE_DECODED_BYTES
} else {
    NATIVE_MAX_IMAGE_DECODED_BYTES
};
pub const IMAGE_RGBA_BYTES_PER_PIXEL: usize = 4;
/// Hard upper bound on total pixels in a decoded buffer or animation atlas.
pub const MAX_IMAGE_PIXELS: usize = MAX_IMAGE_DECODED_BYTES / IMAGE_RGBA_BYTES_PER_PIXEL;
/// Hard upper bound on animation frames. Pixel caps alone do not account for
/// per-frame Vec overhead, which matters for malicious tiny-frame animations.
pub const MAX_IMAGE_FRAMES: usize = 4096;

#[derive(Debug)]
pub enum ImageError {
    EmptyData,
    InvalidPixelAlignment(usize),
    JpgDecode(JpgDecodeErrors),
    PathNotFound(PathBuf),
    PngDecode(PngDecodeErrors),
    GifDecode(GifDecodeErrors),
    WebpDecode(WebpDecodeErrors),
    BmpDecode(BmpDecoderErrors),
    QoiDecode(QoiErrors),
    DimensionsTooLarge { width: usize, height: usize },
    DataTooLarge { bytes: usize, limit: usize },
    UnsupportedFormat,
    Http(String),
    Worker(String),
}

impl Error for ImageError {}

impl From<PngDecodeErrors> for ImageError {
    fn from(value: PngDecodeErrors) -> Self {
        Self::PngDecode(value)
    }
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A decoded picture: its frames as RGBA bytes (not premultiplied, row by row from the top),
/// each `width` x `height`. A still picture has one frame; an animation's frames are composited
/// as browsers show them, each shown for its delay (seconds).
#[derive(Clone, Debug, Default)]
pub struct Decoded {
    pub width: usize,
    pub height: usize,
    pub frames: Vec<Vec<u8>>,
    pub delays: Vec<f64>,
}

impl Decoded {
    fn still(rgba: Vec<u8>, width: usize, height: usize) -> Decoded {
        Decoded { width, height, frames: vec![rgba], delays: Vec::new() }
    }

    /// An animation's frames, or the one picture.
    fn frames(frames: Vec<Vec<u8>>, delays: Vec<f64>, width: usize, height: usize) -> Decoded {
        match frames.len() {
            1 => Decoded { width, height, frames, delays: Vec::new() },
            _ => Decoded { width, height, frames, delays },
        }
    }

    /// The first frame (the picture, when it is still).
    pub fn into_first_frame(mut self) -> Vec<u8> {
        self.frames.swap_remove(0)
    }
}

fn decoder_options() -> DecoderOptions {
    DecoderOptions::default().set_max_width(MAX_IMAGE_DIMENSION).set_max_height(MAX_IMAGE_DIMENSION)
}

fn png_decoder_options() -> DecoderOptions {
    decoder_options().png_set_strip_to_8bit(true)
}

pub fn checked_rgba_byte_len(pixel_count: usize, max_decoded_bytes: usize) -> Option<usize> {
    pixel_count.checked_mul(IMAGE_RGBA_BYTES_PER_PIXEL).filter(|&bytes| bytes <= max_decoded_bytes)
}

/// Validates `width`/`height` against the supplied decoded-byte policy and
/// returns `width * height`, rejecting invalid dimensions before allocation.
pub fn checked_pixel_count_with_limit(width: usize, height: usize, max_decoded_bytes: usize) -> Result<usize, ImageError> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(ImageError::DimensionsTooLarge { width, height });
    }
    let pixels = width.checked_mul(height).ok_or(ImageError::DimensionsTooLarge { width, height })?;
    checked_rgba_byte_len(pixels, max_decoded_bytes).map(|_| pixels).ok_or(ImageError::DimensionsTooLarge { width, height })
}

pub fn checked_pixel_count(width: usize, height: usize) -> Result<usize, ImageError> {
    checked_pixel_count_with_limit(width, height, MAX_IMAGE_DECODED_BYTES)
}

/// `width` x `height` pixels of one to four bytes each (grey, grey and alpha, RGB, RGBA) as RGBA.
pub fn to_rgba(in_data: &[u8], width: usize, height: usize) -> Result<Vec<u8>, ImageError> {
    let pixels = checked_pixel_count(width, height)?;
    if in_data.len() % pixels != 0 {
        return Err(ImageError::InvalidPixelAlignment(in_data.len()));
    }
    let components = in_data.len() / pixels;
    let mut out = Vec::with_capacity(pixels * 4);
    match components {
        4 => out.extend_from_slice(&in_data[..pixels * 4]),
        3 => in_data.chunks_exact(3).take(pixels).for_each(|p| out.extend_from_slice(&[p[0], p[1], p[2], 0xff])),
        2 => in_data.chunks_exact(2).take(pixels).for_each(|p| out.extend_from_slice(&[p[0], p[0], p[0], p[1]])),
        1 => in_data.iter().take(pixels).for_each(|&v| out.extend_from_slice(&[v, v, v, 0xff])),
        unsupported => return Err(ImageError::InvalidPixelAlignment(unsupported)),
    }
    Ok(out)
}

/// The format `data` starts as: `png`, `jpg`, `webp`, `gif`, `bmp`, `qoi` or `ico`.
pub fn detect_image_format(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        Some("png")
    } else if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        Some("jpg")
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else if data.len() >= 6 && data[0] == 0x47 && data[1] == 0x49 && data[2] == 0x46 && data[3] == 0x38 && (data[4] == 0x37 || data[4] == 0x39) && data[5] == 0x61 {
        Some("gif")
    } else if data.len() >= 2 && data[0] == 0x42 && data[1] == 0x4D {
        Some("bmp")
    } else if data.len() >= 4 && &data[0..4] == b"qoif" {
        Some("qoi")
    } else if data.len() >= 4 && data[0] == 0 && data[1] == 0 && data[2] == 1 && data[3] == 0 {
        Some("ico")
    } else {
        None
    }
}

pub fn detect_image_format_from_path_and_data(image_path: &Path, data: &[u8]) -> Option<&'static str> {
    // Prefer magic-byte detection over file extensions so in-memory/binary
    // resources decode correctly even when their synthetic path has no extension.
    if let Some(format) = detect_image_format(data) {
        return Some(format);
    }

    // Keep extension fallback for edge cases where headers are unavailable.
    let ext = image_path.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase());
    match ext.as_deref() {
        Some("jpg") | Some("jpeg") => Some("jpg"),
        Some("png") => Some("png"),
        Some("webp") => Some("webp"),
        Some("gif") => Some("gif"),
        Some("bmp") => Some("bmp"),
        Some("qoi") => Some("qoi"),
        Some("ico") => Some("ico"),
        _ => None,
    }
}

/// Decodes a picture of any format read here, its format from its bytes (else `image_path`'s
/// extension).
pub fn decode(image_path: &Path, data: &[u8]) -> Result<Decoded, ImageError> {
    if data.len() > MAX_IMAGE_DECODED_BYTES {
        return Err(ImageError::DataTooLarge { bytes: data.len(), limit: MAX_IMAGE_DECODED_BYTES });
    }
    match detect_image_format_from_path_and_data(image_path, data).ok_or(ImageError::UnsupportedFormat)? {
        "jpg" => decode_jpg(data),
        "png" => decode_png(data),
        "webp" => decode_webp(data),
        "gif" => decode_gif(data),
        "bmp" => decode_bmp(data),
        "qoi" => decode_qoi(data),
        "ico" => decode_ico(data),
        _ => Err(ImageError::UnsupportedFormat),
    }
}

/// A picture's pixels as RGBA bytes, row by row from the top, upright: its width, its height and
/// its pixels. An animation gives its first frame.
pub fn decode_rgba(data: &[u8]) -> Result<(usize, usize, Vec<u8>), ImageError> {
    let decoded = decode(Path::new(""), data)?;
    let (width, height) = (decoded.width, decoded.height);
    Ok((width, height, decoded.into_first_frame()))
}

pub fn decode_png(data: &[u8]) -> Result<Decoded, ImageError> {
    let cursor = ZCursor::new(data);
    // 16-bit PNGs (e.g. 16-bit HDR PNG screenshots) decode as u16 and fail our u8 path; strip to 8-bit.
    let mut decoder = PngDecoder::new_with_options(cursor, png_decoder_options());
    decoder.decode_headers()?;
    let (width, height) = decoder.dimensions().ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic("Failed to get PNG image dimensions")))?;
    checked_pixel_count(width, height)?;

    if decoder.is_animated() {
        return decode_animated_png(&mut decoder);
    }

    let image = decoder.decode()?;
    let decoded_data = image.u8().ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic("Failed to decode PNG image data as a slice of u8 bytes")))?;
    let still = Decoded::still(to_rgba(&decoded_data, width, height)?, width, height);
    Ok(orient(still, decoder.info().and_then(|i| i.exif.as_deref())))
}

/// Decodes an animated PNG by drawing each frame onto a canvas the way browsers do
/// (blending it, then disposing of it).
fn decode_animated_png<T: makepad_zune_png::makepad_zune_core::bytestream::ZByteReaderTrait>(decoder: &mut PngDecoder<T>) -> Result<Decoded, ImageError> {
    let colorspace = decoder.colorspace().ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic("Failed to get animated PNG colorspace")))?;
    let (width, height) = decoder.dimensions().ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic("Failed to get animated PNG image dimensions")))?;
    let num_components = colorspace.num_components();
    let frame_pixels = checked_pixel_count(width, height)?;
    let max_frames = (MAX_IMAGE_PIXELS / frame_pixels).max(1).min(MAX_IMAGE_FRAMES);
    let mut canvas = vec![0u8; frame_pixels * IMAGE_RGBA_BYTES_PER_PIXEL];
    let mut frames = Vec::new();
    let mut frame_delays = Vec::new();

    while decoder.more_frames() {
        decoder.decode_headers()?;
        let frame = decoder.frame_info().ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic("Failed to get animated PNG frame info")))?;
        let pixels = decoder.decode_raw()?;
        // A default image that isn't part of the animation is only there for viewers without APNG support.
        if !frame.is_part_of_seq {
            continue;
        }
        if frames.len() >= max_frames {
            return Err(ImageError::DimensionsTooLarge { width, height });
        }
        if frame.width == 0 || frame.height == 0 || frame.x_offset + frame.width > width || frame.y_offset + frame.height > height || pixels.len() < frame.width * frame.height * num_components {
            return Err(ImageError::PngDecode(PngDecodeErrors::GenericStatic("Animated PNG frame doesn't fit within the image")));
        }
        let frame_rgba = match num_components {
            4 => pixels,
            3 => rgb_to_rgba(&pixels),
            2 => pixels.chunks_exact(2).flat_map(|la| [la[0], la[0], la[0], la[1]]).collect(),
            1 => pixels.iter().flat_map(|&l| [l, l, l, 0xFF]).collect(),
            unsupported => return Err(ImageError::InvalidPixelAlignment(unsupported)),
        };
        // A first frame that asks to be disposed to the "previous" frame is disposed to the background instead.
        let previous_canvas = (frame.dispose_op == DisposeOp::Previous && !frames.is_empty()).then(|| canvas.clone());
        for (y, src_row) in frame_rgba.chunks_exact(frame.width * 4).take(frame.height).enumerate() {
            let dst_start = ((frame.y_offset + y) * width + frame.x_offset) * 4;
            let dst_row = &mut canvas[dst_start..dst_start + frame.width * 4];
            match frame.blend_op {
                BlendOp::Source => dst_row.copy_from_slice(src_row),
                BlendOp::Over => {
                    for (dst, src) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
                        blend_rgba_over(dst, src);
                    }
                }
            }
        }
        frames.push(canvas.clone());
        // A delay denominator of 0 means hundredths of a second.
        let delay_denom = if frame.delay_denom == 0 { 100 } else { frame.delay_denom };
        frame_delays.push(f64::from(frame.delay_num) / f64::from(delay_denom));

        match (frame.dispose_op, previous_canvas) {
            (DisposeOp::None, _) => {}
            (DisposeOp::Previous, Some(previous_canvas)) => canvas = previous_canvas,
            (DisposeOp::Background | DisposeOp::Previous, _) => {
                for y in frame.y_offset..frame.y_offset + frame.height {
                    let row_start = (y * width + frame.x_offset) * 4;
                    canvas[row_start..row_start + frame.width * 4].fill(0);
                }
            }
        }
    }

    if frames.is_empty() {
        return Err(ImageError::PngDecode(PngDecodeErrors::GenericStatic("Animated PNG had no frames")));
    }
    Ok(Decoded::frames(frames, frame_delays, width, height))
}

pub fn decode_webp(data: &[u8]) -> Result<Decoded, ImageError> {
    let cursor = std::io::Cursor::new(data);
    let mut decoder = WebPDecoder::new(std::io::BufReader::new(cursor)).map_err(ImageError::WebpDecode)?;
    decoder.set_memory_limit(MAX_IMAGE_PIXELS * 4);
    let (width, height) = decoder.dimensions();
    let (width, height) = (width as usize, height as usize);
    let frame_pixels = checked_pixel_count(width, height)?;
    let buf_size = decoder.output_buffer_size().filter(|&len| len <= MAX_IMAGE_DECODED_BYTES).ok_or(ImageError::WebpDecode(WebpDecodeErrors::ImageTooLarge))?;

    if !decoder.is_animated() {
        let mut buf = vec![0u8; buf_size];
        decoder.read_image(&mut buf).map_err(ImageError::WebpDecode)?;
        let still = Decoded::still(to_rgba(&buf, width, height)?, width, height);
        let exif = decoder.exif_metadata().ok().flatten();
        return Ok(orient(still, exif.as_deref()));
    }

    // Animated WebP: the decoder composites each frame onto its own canvas,
    // so we just collect each composited frame (as RGBA) and its delay.
    let has_alpha = decoder.has_alpha();
    let max_frames = (MAX_IMAGE_PIXELS / frame_pixels).max(1).min(MAX_IMAGE_FRAMES);
    let num_frames = decoder.num_frames() as usize;
    if num_frames > max_frames {
        return Err(ImageError::DimensionsTooLarge { width, height });
    }
    // Browsers ignore the file's background color and clear disposed frames to transparent.
    // Without a background color, the decoder never clears them at all.
    decoder.set_background_color([0, 0, 0, 0]).map_err(ImageError::WebpDecode)?;
    decoder.reset_animation();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut frame_delays: Vec<f64> = Vec::new();
    for _ in 0..num_frames {
        let mut buf = vec![0u8; buf_size];
        match decoder.read_frame(&mut buf) {
            Ok(delay_ms) => {
                if frames.len() >= max_frames {
                    return Err(ImageError::DimensionsTooLarge { width, height });
                }
                frame_delays.push(if delay_ms == 0 { 0.1 } else { f64::from(delay_ms) * 0.001 });
                frames.push(if has_alpha { buf } else { rgb_to_rgba(&buf) });
            }
            Err(WebpDecodeErrors::NoMoreFrames) => break,
            Err(err) => return Err(ImageError::WebpDecode(err)),
        }
    }
    if frames.is_empty() {
        return Err(ImageError::WebpDecode(WebpDecodeErrors::NoMoreFrames));
    }
    Ok(Decoded::frames(frames, frame_delays, width, height))
}

pub fn decode_gif(data: &[u8]) -> Result<Decoded, ImageError> {
    let mut options = DecodeOptions::new();
    options.set_color_output(ColorOutput::RGBA);
    let mut decoder = options.read_info(std::io::Cursor::new(data)).map_err(ImageError::GifDecode)?;
    let width = decoder.width() as usize;
    let height = decoder.height() as usize;
    let frame_pixels = checked_pixel_count(width, height)?;
    let max_frames = (MAX_IMAGE_PIXELS / frame_pixels).max(1).min(MAX_IMAGE_FRAMES);
    let mut frames = Vec::new();
    let mut frame_delays = Vec::new();
    let mut canvas = vec![0u8; frame_pixels * IMAGE_RGBA_BYTES_PER_PIXEL];

    while let Some(frame) = decoder.read_next_frame().map_err(ImageError::GifDecode)? {
        if frames.len() >= max_frames {
            return Err(ImageError::DimensionsTooLarge { width, height });
        }
        let delay = if frame.delay == 0 { 0.1 } else { f64::from(frame.delay) * 0.01 };
        let restore = (frame.dispose == DisposalMethod::Previous).then(|| canvas.clone());
        let frame_left = frame.left as usize;
        let frame_top = frame.top as usize;
        let frame_width = frame.width as usize;
        let frame_height = frame.height as usize;
        for y in 0..frame_height {
            let dst_y = frame_top + y;
            if dst_y >= height {
                continue;
            }
            for x in 0..frame_width {
                let dst_x = frame_left + x;
                if dst_x >= width {
                    continue;
                }
                let src = (y * frame_width + x) * 4;
                let dst = (dst_y * width + dst_x) * 4;
                let rgba = &frame.buffer[src..src + 4];
                if rgba[3] != 0 {
                    canvas[dst..dst + 4].copy_from_slice(rgba);
                }
            }
        }

        frames.push(canvas.clone());
        frame_delays.push(delay);

        match frame.dispose {
            DisposalMethod::Background => {
                for y in 0..frame_height {
                    let dst_y = frame_top + y;
                    if dst_y >= height {
                        continue;
                    }
                    for x in 0..frame_width {
                        let dst_x = frame_left + x;
                        if dst_x >= width {
                            continue;
                        }
                        let dst = (dst_y * width + dst_x) * 4;
                        canvas[dst..dst + 4].fill(0);
                    }
                }
            }
            DisposalMethod::Previous => {
                if let Some(restore) = restore {
                    canvas = restore;
                }
            }
            _ => {}
        }
    }

    if frames.is_empty() {
        return Ok(Decoded::still(canvas, width, height));
    }
    Ok(Decoded::frames(frames, frame_delays, width, height))
}

pub fn decode_jpg(data: &[u8]) -> Result<Decoded, ImageError> {
    let cursor = ZCursor::new(data);
    let mut decoder = JpegDecoder::new_with_options(cursor, decoder_options());
    decoder.decode_headers().map_err(ImageError::JpgDecode)?;
    let (width, height) = decoder.dimensions().ok_or(ImageError::JpgDecode(JpgDecodeErrors::FormatStatic("Failed to decode JPG image dimensions")))?;
    checked_pixel_count(width, height)?;
    let pixels = decoder.decode().map_err(ImageError::JpgDecode)?;
    let still = Decoded::still(to_rgba(&pixels, width, height)?, width, height);
    Ok(orient(still, decoder.exif().map(Vec::as_slice)))
}

pub fn decode_bmp(data: &[u8]) -> Result<Decoded, ImageError> {
    let cursor = ZCursor::new(data);
    let mut decoder = BmpDecoder::new_with_options(cursor, decoder_options());
    decoder.decode_headers().map_err(ImageError::BmpDecode)?;
    let (width, height) = decoder.dimensions().ok_or(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic("Failed to get BMP image dimensions")))?;
    checked_pixel_count(width, height)?;
    let pixels = decoder.decode().map_err(ImageError::BmpDecode)?;
    Ok(Decoded::still(to_rgba(&pixels, width, height)?, width, height))
}

pub fn decode_qoi(data: &[u8]) -> Result<Decoded, ImageError> {
    let cursor = ZCursor::new(data);
    let mut decoder = QoiDecoder::new_with_options(cursor, decoder_options());
    decoder.decode_headers().map_err(ImageError::QoiDecode)?;
    let (width, height) = decoder.get_dimensions().ok_or(ImageError::QoiDecode(QoiErrors::GenericStatic("Failed to get QOI image dimensions")))?;
    checked_pixel_count(width, height)?;
    let pixels = decoder.decode().map_err(ImageError::QoiDecode)?;
    Ok(Decoded::still(to_rgba(&pixels, width, height)?, width, height))
}

pub fn decode_ico(data: &[u8]) -> Result<Decoded, ImageError> {
    let (offset, len, _width, height) = ico_best_entry(data).ok_or(ImageError::UnsupportedFormat)?;
    let payload = &data[offset..offset + len];
    if detect_image_format(payload) == Some("png") {
        decode_png(payload)
    } else {
        decode_bmp(&ico_dib_to_bmp(payload, height)?)
    }
}

// Pick the largest image entry from an ICO directory, returning its payload's
// (offset, length, height). Height comes from the directory entry since the
// embedded DIB doubles its own height to cover a trailing AND mask.
fn ico_best_entry(data: &[u8]) -> Option<(usize, usize, u32, u32)> {
    if data.len() < 6 {
        return None;
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    let mut best: Option<(usize, usize, u32, u32, u32)> = None; // offset, len, width, height, score
    for i in 0..count {
        let e = 6 + i * 16;
        if e + 16 > data.len() {
            break;
        }
        let width = if data[e] == 0 { 256 } else { data[e] as u32 };
        let height = if data[e + 1] == 0 { 256 } else { data[e + 1] as u32 };
        let bit_count = u16::from_le_bytes([data[e + 6], data[e + 7]]) as u32;
        let len = u32::from_le_bytes([data[e + 8], data[e + 9], data[e + 10], data[e + 11]]) as usize;
        let offset = u32::from_le_bytes([data[e + 12], data[e + 13], data[e + 14], data[e + 15]]) as usize;
        if len == 0 || offset.saturating_add(len) > data.len() {
            continue;
        }
        let score = width * height * 64 + bit_count;
        if best.is_none_or(|(.., s)| score > s) {
            best = Some((offset, len, width, height, score));
        }
    }
    best.map(|(offset, len, width, height, _)| (offset, len, width, height))
}

// Wrap an ICO's bare DIB (a BITMAPINFOHEADER and pixels with no BMP file header)
// into a standalone BMP the BMP decoder can read. The DIB doubles its height for
// a trailing 1bpp AND mask we skip, so restore the real height from the directory.
fn ico_dib_to_bmp(dib: &[u8], real_height: u32) -> Result<Vec<u8>, ImageError> {
    if dib.len() < 40 {
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic("ICO DIB header too small")));
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
    if header_size < 40 || header_size > dib.len() {
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic("ICO DIB header has invalid size")));
    }
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]) as u32;
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    let mut clr_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;
    if bit_count <= 8 && clr_used == 0 {
        clr_used = 1usize << bit_count;
    }
    if bit_count <= 8 && clr_used > (1usize << bit_count) {
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic("ICO DIB palette is too large")));
    }
    let palette_bytes = if bit_count <= 8 { clr_used.checked_mul(4).ok_or(ImageError::DimensionsTooLarge { width: dib.len(), height: 1 })? } else { 0 };
    // A plain BITMAPINFOHEADER stores its bitfield masks between the header and pixels.
    let mask_bytes = if header_size == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let data_offset = 14usize
        .checked_add(header_size)
        .and_then(|offset| offset.checked_add(mask_bytes))
        .and_then(|offset| offset.checked_add(palette_bytes))
        .ok_or(ImageError::DimensionsTooLarge { width: dib.len(), height: 1 })?;
    let file_size = 14usize.checked_add(dib.len()).filter(|&size| size <= u32::MAX as usize).ok_or(ImageError::DimensionsTooLarge { width: dib.len(), height: 1 })?;
    if data_offset > file_size {
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic("ICO DIB pixel data offset exceeds payload")));
    }

    let mut out = Vec::with_capacity(file_size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(data_offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    out[14 + 8..14 + 12].copy_from_slice(&(real_height as i32).to_le_bytes());
    Ok(out)
}

/// Expands tightly-packed RGB pixels to RGBA with full opacity.
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
    }
    out
}

/// Draws the RGBA pixel `src` over the RGBA pixel `dst`, neither of them premultiplied.
fn blend_rgba_over(dst: &mut [u8], src: &[u8]) {
    let src_alpha = u32::from(src[3]);
    if src_alpha == 0xFF {
        dst.copy_from_slice(src);
        return;
    }
    if src_alpha == 0 {
        return;
    }
    let dst_alpha = u32::from(dst[3]) * (0xFF - src_alpha) / 0xFF;
    let out_alpha = src_alpha + dst_alpha;
    for (dst_channel, &src_channel) in dst.iter_mut().zip(src).take(3) {
        let blended = u32::from(src_channel) * src_alpha + u32::from(*dst_channel) * dst_alpha;
        *dst_channel = ((blended + out_alpha / 2) / out_alpha) as u8;
    }
    dst[3] = out_alpha as u8;
}

/// Returns true if `data` is an animated GIF, WebP or PNG, by looking at its headers only.
pub fn is_animated_image(data: &[u8]) -> bool {
    match detect_image_format(data) {
        Some("gif") => gif_has_multiple_frames(data).unwrap_or(false),
        // An animated WebP starts with a VP8X chunk whose flags have the animation bit set.
        Some("webp") => data.get(12..16) == Some(&b"VP8X"[..]) && data.get(20).is_some_and(|flags| flags & 0x02 != 0),
        Some("png") => png_has_multiple_frames(data).unwrap_or(false),
        _ => false,
    }
}

/// Returns whether a GIF has at least two frames, skipping over all the blocks before its second one.
fn gif_has_multiple_frames(data: &[u8]) -> Option<bool> {
    let color_table_len = |flags: u8| if flags & 0x80 != 0 { 3 << ((flags & 0x07) + 1) } else { 0 };
    let skip_sub_blocks = |mut pos: usize| -> Option<usize> {
        loop {
            let len = *data.get(pos)? as usize;
            pos += 1 + len;
            if len == 0 {
                return Some(pos);
            }
        }
    };
    // The header and logical screen descriptor are 13 bytes, followed by an optional global color table.
    let mut pos = 13 + color_table_len(*data.get(10)?);
    let mut num_frames = 0;
    loop {
        match *data.get(pos)? {
            // An extension: its introducer, its label, then its data sub-blocks.
            0x21 => pos = skip_sub_blocks(pos + 2)?,
            // An image descriptor, an optional local color table, the LZW code size, then sub-blocks.
            0x2C => {
                num_frames += 1;
                if num_frames > 1 {
                    return Some(true);
                }
                pos = skip_sub_blocks(pos + 10 + color_table_len(*data.get(pos + 9)?) + 1)?;
            }
            _ => return Some(false),
        }
    }
}

/// Returns whether a PNG has an `acTL` chunk for more than one frame before its image data.
fn png_has_multiple_frames(data: &[u8]) -> Option<bool> {
    let mut pos = 8;
    loop {
        let len = u32::from_be_bytes(data.get(pos..pos + 4)?.try_into().ok()?) as usize;
        match data.get(pos + 4..pos + 8)? {
            b"acTL" => {
                let num_frames = u32::from_be_bytes(data.get(pos + 8..pos + 12)?.try_into().ok()?);
                return Some(num_frames > 1);
            }
            b"IDAT" | b"IEND" => return Some(false),
            _ => pos = pos.checked_add(12)?.checked_add(len)?,
        }
    }
}

/// Applies the EXIF orientation found in a still picture's raw `exif` TIFF block (as handed
/// back by the format decoders) so its pixels display upright.
fn orient(still: Decoded, exif: Option<&[u8]>) -> Decoded {
    apply_exif_orientation(still, exif.and_then(tiff_orientation).unwrap_or(1))
}

/// Returns the size of an image that's `width` x `height` once its EXIF orientation is applied.
pub fn get_oriented_size((width, height): (usize, usize), exif: Option<&[u8]>) -> (usize, usize) {
    match exif.and_then(tiff_orientation) {
        Some(5..=8) => (height, width),
        _ => (width, height),
    }
}

/// Reads the Orientation tag (0x0112) from a TIFF/EXIF block that begins at the
/// byte-order marker ("II" little-endian or "MM" big-endian). Returns the value
/// only when it's a valid 1-8.
pub fn tiff_orientation(tiff: &[u8]) -> Option<u16> {
    // Some sources (e.g. WebP EXIF chunks) keep the JPEG-style "Exif\0\0" prefix.
    let tiff = tiff.strip_prefix(b"Exif\0\0").unwrap_or(tiff);
    let little_endian = match tiff.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    // checked_add so an attacker-controlled near-usize::MAX offset can't overflow
    // (and panic on 32-bit builds) before the bounds check runs.
    let u16_at = |off: usize| -> Option<u16> {
        let b = tiff.get(off..off.checked_add(2)?)?;
        Some(if little_endian { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    };
    let u32_at = |off: usize| -> Option<u32> {
        let b = tiff.get(off..off.checked_add(4)?)?;
        Some(if little_endian { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) })
    };
    if u16_at(2)? != 0x002A {
        return None;
    }
    let ifd0 = u32_at(4)? as usize;
    let count = u16_at(ifd0)? as usize;
    for n in 0..count {
        let entry = ifd0 + 2 + n * 12;
        if u16_at(entry)? == 0x0112 {
            // Orientation is a SHORT: its value sits in the first 2 bytes of the
            // entry's value field (offset 8 within the 12-byte entry).
            let v = u16_at(entry + 8)?;
            return (1..=8).contains(&v).then_some(v);
        }
    }
    None
}

/// Rotates/flips a still picture per its EXIF `orientation` (1-8) so it displays upright.
/// Orientation 1 (or anything out of range, or an animation) is returned unchanged.
pub fn apply_exif_orientation(still: Decoded, orientation: u16) -> Decoded {
    if orientation <= 1 || orientation > 8 || still.frames.len() != 1 {
        return still;
    }
    let (w, h) = (still.width, still.height);
    let src = &still.frames[0];
    if src.len() < w * h * 4 {
        return still;
    }
    // Orientations 5-8 are 90°/diagonal turns, which swap width and height.
    let swap = matches!(orientation, 5..=8);
    let (dw, dh) = if swap { (h, w) } else { (w, h) };
    let mut dst = vec![0u8; dw * dh * 4];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = match orientation {
                2 => (w - 1 - x, y),         // mirror horizontal
                3 => (w - 1 - x, h - 1 - y), // rotate 180°
                4 => (x, h - 1 - y),         // mirror vertical
                5 => (y, x),                 // transpose
                6 => (h - 1 - y, x),         // rotate 90° CW
                7 => (h - 1 - y, w - 1 - x), // transverse
                8 => (y, w - 1 - x),         // rotate 90° CCW
                _ => (x, y),
            };
            let (d, s) = ((dy * dw + dx) * 4, (y * w + x) * 4);
            dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    Decoded::still(dst, dw, dh)
}

/// Returns the `(width, height)` in pixels of an encoded image as it's shown (after its EXIF orientation),
/// auto-detecting the format from the data's magic bytes (falling back to the path's extension).
/// Reads only headers for most formats; does not fully decode.
pub fn image_size_by_data(data: &[u8], image_path: &Path) -> Result<(usize, usize), ImageError> {
    let format = detect_image_format_from_path_and_data(image_path, data).ok_or(ImageError::UnsupportedFormat)?;
    match format {
        "jpg" => {
            let cursor = ZCursor::new(data);
            let mut decoder = JpegDecoder::new_with_options(cursor, decoder_options());
            decoder.decode_headers().map_err(ImageError::JpgDecode)?;
            let (width, height) = decoder.dimensions().ok_or(ImageError::JpgDecode(JpgDecodeErrors::FormatStatic("Failed to get JPG image dimensions after decoding headers")))?;
            checked_pixel_count(width, height)?;
            Ok(get_oriented_size((width, height), decoder.exif().map(Vec::as_slice)))
        }
        "png" => {
            let cursor = ZCursor::new(data);
            let mut decoder = PngDecoder::new_with_options(cursor, png_decoder_options());
            decoder.decode_headers()?;
            let (width, height) = decoder.dimensions().ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic("Failed to get PNG image dimensions")))?;
            checked_pixel_count(width, height)?;
            // Animations are never rotated, see `apply_exif_orientation`.
            let exif = if decoder.is_animated() { None } else { decoder.info().and_then(|i| i.exif.as_deref()) };
            Ok(get_oriented_size((width, height), exif))
        }
        "webp" => {
            let cursor = std::io::Cursor::new(data);
            let mut decoder = WebPDecoder::new(std::io::BufReader::new(cursor)).map_err(ImageError::WebpDecode)?;
            let (width, height) = decoder.dimensions();
            let (width, height) = (width as usize, height as usize);
            checked_pixel_count(width, height)?;
            let exif = if decoder.is_animated() { None } else { decoder.exif_metadata().ok().flatten() };
            Ok(get_oriented_size((width, height), exif.as_deref()))
        }
        "gif" => {
            let decoder = DecodeOptions::new().read_info(std::io::Cursor::new(data)).map_err(ImageError::GifDecode)?;
            let width = decoder.width() as usize;
            let height = decoder.height() as usize;
            checked_pixel_count(width, height)?;
            Ok((width, height))
        }
        "bmp" => {
            let cursor = ZCursor::new(data);
            let mut decoder = BmpDecoder::new_with_options(cursor, decoder_options());
            decoder.decode_headers().map_err(ImageError::BmpDecode)?;
            let (width, height) = decoder.dimensions().ok_or(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic("Failed to get BMP image dimensions")))?;
            checked_pixel_count(width, height)?;
            Ok((width, height))
        }
        "qoi" => {
            let cursor = ZCursor::new(data);
            let mut decoder = QoiDecoder::new_with_options(cursor, decoder_options());
            decoder.decode_headers().map_err(ImageError::QoiDecode)?;
            let (width, height) = decoder.get_dimensions().ok_or(ImageError::QoiDecode(QoiErrors::GenericStatic("Failed to get QOI image dimensions")))?;
            checked_pixel_count(width, height)?;
            Ok((width, height))
        }
        "ico" => {
            let (_, _, width, height) = ico_best_entry(data).ok_or(ImageError::UnsupportedFormat)?;
            let (width, height) = (width as usize, height as usize);
            checked_pixel_count(width, height)?;
            Ok((width, height))
        }
        _ => Err(ImageError::UnsupportedFormat),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_gif::{Encoder, Frame};

    /// A 2x2 GIF of two frames: black and red, then red and green.
    fn two_frame_gif() -> Vec<u8> {
        let palette = [0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0xff, 0x00];
        let mut data = Vec::new();
        {
            let mut encoder = Encoder::new(&mut data, 2, 2, &palette).unwrap();
            for pixels in [[0u8, 1, 1, 0], [1, 2, 2, 1]] {
                let frame = Frame { width: 2, height: 2, delay: 5, buffer: pixels.to_vec().into(), ..Default::default() };
                encoder.write_frame(&frame).unwrap();
            }
        }
        data
    }

    #[test]
    fn an_animation_reads_as_its_first_frame() {
        let gif = two_frame_gif();
        let decoded = decode(Path::new(""), &gif).unwrap();
        assert_eq!((decoded.width, decoded.height, decoded.frames.len()), (2, 2, 2));
        let (w, h, rgba) = decode_rgba(&gif).unwrap();
        assert_eq!((w, h), (2, 2));
        let black = [0x00, 0x00, 0x00, 0xff];
        let red = [0xff, 0x00, 0x00, 0xff];
        assert_eq!(rgba, [black, red, red, black].concat());
    }

    /// One picture each as PNG, JPEG and WebP, its left half red and its right half blue: each
    /// reads at its size with its colours where they are (a JPEG's within its loss, away from
    /// the edge between them).
    #[test]
    fn png_jpeg_and_webp_each_read_with_their_pixels() {
        use makepad_zune_png::makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
        let (w, h) = (32usize, 16usize);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| if i % w < w / 2 { [200, 30, 40, 255] } else { [30, 60, 210, 255] }).collect();
        let mut png = Vec::new();
        makepad_zune_png::PngEncoder::new(&rgba, EncoderOptions::new(w, h, ColorSpace::RGBA, BitDepth::Eight)).encode(&mut png).unwrap();
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, 95).encode(&rgba, w as u16, h as u16, jpeg_encoder::ColorType::Rgba).unwrap();
        let mut webp = Vec::new();
        makepad_webp::WebPEncoder::new(&mut webp).encode(&rgba, w as u32, h as u32, makepad_webp::ColorType::Rgba8).unwrap();
        for (name, bytes, loss) in [("PNG", png, 0u8), ("JPEG", jpeg, 20), ("WebP", webp, 0)] {
            let (dw, dh, got) = decode_rgba(&bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!((dw, dh, got.len()), (w, h, w * h * 4), "{name}");
            for (i, (a, b)) in got.chunks_exact(4).zip(rgba.chunks_exact(4)).enumerate() {
                if loss > 0 && (w / 2 - 4..w / 2 + 4).contains(&(i % w)) {
                    continue;
                }
                assert!((0..3).all(|c| a[c].abs_diff(b[c]) <= loss) && a[3] == 255, "{name} at {}, {}: {a:?} for {b:?}", i % w, i / w);
            }
        }
    }

    #[test]
    fn what_is_not_a_picture_says_so() {
        assert!(matches!(decode_rgba(b"not a picture"), Err(ImageError::UnsupportedFormat)));
    }
}
