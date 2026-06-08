use crate::makepad_platform::*;
use makepad_gif::{ColorOutput, DecodeOptions, DisposalMethod};
use makepad_webp::WebPDecoder;
use makepad_zune_bmp::BmpDecoder;
use makepad_zune_jpeg::JpegDecoder;
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::{post_process_image, PngDecoder};
use makepad_zune_qoi::QoiDecoder;
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

pub use makepad_gif::DecodingError as GifDecodeErrors;
pub use makepad_webp::DecodingError as WebpDecodeErrors;
pub use makepad_zune_bmp::BmpDecoderErrors;
pub use makepad_zune_jpeg::errors::DecodeErrors as JpgDecodeErrors;
pub use makepad_zune_png::error::PngDecodeErrors;
pub use makepad_zune_qoi::QoiErrors;

#[derive(Debug, Default, Clone)]
pub struct ImageBuffer {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
    pub animation: Option<TextureAnimation>,
}

impl ImageBuffer {
    pub fn new(in_data: &[u8], width: usize, height: usize) -> Result<ImageBuffer, ImageError> {
        let pixels = width * height;
        if pixels == 0 {
            return Ok(ImageBuffer {
                width,
                height,
                data: Vec::new(),
                animation: None,
            });
        }
        let mut out = Vec::with_capacity(pixels);
        match in_data.len() / pixels {
            4 => {
                for rgba in in_data.chunks_exact(4).take(pixels) {
                    let r = rgba[0];
                    let g = rgba[1];
                    let b = rgba[2];
                    let a = rgba[3];
                    out.push(
                        ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
                    );
                }
            }
            3 => {
                for rgb in in_data.chunks_exact(3).take(pixels) {
                    let r = rgb[0];
                    let g = rgb[1];
                    let b = rgb[2];
                    out.push(0xff000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32));
                }
            }
            2 => {
                for ra in in_data.chunks_exact(2).take(pixels) {
                    let r = ra[0];
                    let a = ra[1];
                    out.push(
                        ((a as u32) << 24) | ((r as u32) << 16) | ((r as u32) << 8) | (r as u32),
                    );
                }
            }
            1 => {
                for r in in_data.iter().copied().take(pixels) {
                    out.push(
                        (0xff_u32 << 24) | ((r as u32) << 16) | ((r as u32) << 8) | (r as u32),
                    );
                }
            }
            unsupported => return Err(ImageError::InvalidPixelAlignment(unsupported)),
        }
        Ok(ImageBuffer {
            width,
            height,
            data: out,
            animation: None,
        })
    }

    pub fn into_new_texture(self, cx: &mut Cx) -> Texture {
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 {
                width: self.width,
                height: self.height,
                data: Some(self.data),
                updated: TextureUpdated::Full,
            },
        );
        texture.set_animation(cx, self.animation);
        texture
    }

    pub fn from_png(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        // 16-bit PNGs (e.g. HDR iPhone screenshots) decode as u16 and fail our u8 path; strip to 8-bit.
        let options = DecoderOptions::default().png_set_strip_to_8bit(true);
        let mut decoder = PngDecoder::new_with_options(cursor, options);
        decoder.decode_headers()?;

        if decoder.is_animated() {
            return Self::decode_animated_png(&mut decoder);
        }

        let image = decoder.decode()?;
        let decoded_data =
            image
                .u8()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to decode PNG image data as a slice of u8 bytes",
                )))?;
        let (width, height) =
            decoder
                .dimensions()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get PNG image dimensions",
                )))?;
        Self::new(&decoded_data, width, height)
    }

    fn decode_animated_png<T: makepad_zune_png::makepad_zune_core::bytestream::ZByteReaderTrait>(
        decoder: &mut PngDecoder<T>,
    ) -> Result<ImageBuffer, ImageError> {
        let colorspace =
            decoder
                .colorspace()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get animated PNG colorspace",
                )))?;
        let (width, height) =
            decoder
                .dimensions()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get animated PNG image dimensions",
                )))?;
        let actl_info =
            decoder
                .actl_info()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get animated PNG actl info",
                )))?;

        let num_components = colorspace.num_components();
        let mut output = vec![0; width * height * num_components];
        let fits_horizontal = Cx::max_texture_width() / width;
        let total_width = fits_horizontal * width;
        let total_height = ((actl_info.num_frames as usize / fits_horizontal) + 1) * height;
        let mut final_buffer = ImageBuffer::default();
        final_buffer.data.resize(total_width * total_height, 0);
        final_buffer.width = total_width;
        final_buffer.height = total_height;
        let mut cx = 0;
        let mut cy = 0;
        final_buffer.animation = Some(TextureAnimation {
            width,
            height,
            num_frames: actl_info.num_frames as usize,
            frame_delays: Vec::new(),
        });
        let mut previous_frame = None;
        while decoder.more_frames() {
            decoder.decode_headers()?;
            let frame = decoder.frame_info().expect("to have already been decoded");
            let pix = decoder.decode_raw()?;
            let info =
                decoder
                    .info()
                    .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                        "Failed to get animated PNG image info",
                    )))?;
            post_process_image(
                info,
                colorspace,
                &frame,
                &pix,
                previous_frame.as_deref(),
                &mut output,
                None,
            )?;
            previous_frame = Some(pix);
            match num_components {
                4 => {
                    for y in 0..height {
                        for x in 0..width {
                            let r = output[y * width * 4 + x * 4];
                            let g = output[y * width * 4 + x * 4 + 1];
                            let b = output[y * width * 4 + x * 4 + 2];
                            let a = output[y * width * 4 + x * 4 + 3];
                            final_buffer.data[(y + cy) * total_width + (x + cx)] = ((a as u32)
                                << 24)
                                | ((r as u32) << 16)
                                | ((g as u32) << 8)
                                | (b as u32);
                        }
                    }
                }
                3 => {
                    for y in 0..height {
                        for x in 0..width {
                            let r = output[y * width * 3 + x * 3];
                            let g = output[y * width * 3 + x * 3 + 1];
                            let b = output[y * width * 3 + x * 3 + 2];
                            final_buffer.data[(y + cy) * total_width + (x + cx)] =
                                0xff000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                        }
                    }
                }
                _ => return Err(ImageError::InvalidPixelAlignment(num_components)),
            }
            cx += width;
            if cx >= total_width {
                cy += height;
                cx = 0;
            }
        }
        Ok(final_buffer)
    }

    pub fn from_webp(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = std::io::Cursor::new(data);
        let mut decoder =
            WebPDecoder::new(std::io::BufReader::new(cursor)).map_err(ImageError::WebpDecode)?;
        let (width, height) = decoder.dimensions();
        let (width, height) = (width as usize, height as usize);
        let buf_size = decoder
            .output_buffer_size()
            .ok_or(ImageError::WebpDecode(WebpDecodeErrors::ImageTooLarge))?;

        if !decoder.is_animated() {
            let mut buf = vec![0u8; buf_size];
            decoder.read_image(&mut buf).map_err(ImageError::WebpDecode)?;
            return Self::new(&buf, width, height);
        }

        // Animated WebP: the decoder composites each frame onto its own canvas,
        // so we just collect each composited frame (as RGBA) and its delay, then
        // pack them into an animation atlas like GIF/APNG.
        let has_alpha = decoder.has_alpha();
        decoder.reset_animation();
        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut frame_delays: Vec<f64> = Vec::new();
        loop {
            let mut buf = vec![0u8; buf_size];
            match decoder.read_frame(&mut buf) {
                Ok(delay_ms) => {
                    frame_delays
                        .push(if delay_ms == 0 { 0.1 } else { f64::from(delay_ms) * 0.001 });
                    frames.push(if has_alpha { buf } else { rgb_to_rgba(&buf) });
                }
                Err(WebpDecodeErrors::NoMoreFrames) => break,
                Err(err) => return Err(ImageError::WebpDecode(err)),
            }
        }

        if frames.len() >= 2 {
            Ok(Self::pack_animation_atlas(frames, frame_delays, width, height))
        } else if let Some(frame) = frames.first() {
            Self::new(frame, width, height)
        } else {
            Self::new(&vec![0u8; width * height * 4], width, height)
        }
    }

    pub fn from_gif(data: &[u8]) -> Result<Self, ImageError> {
        let mut options = DecodeOptions::new();
        options.set_color_output(ColorOutput::RGBA);
        let mut decoder = options
            .read_info(std::io::Cursor::new(data))
            .map_err(ImageError::GifDecode)?;
        let width = decoder.width() as usize;
        let height = decoder.height() as usize;
        let mut frames = Vec::new();
        let mut frame_delays = Vec::new();
        let mut canvas = vec![0u8; width * height * 4];

        while let Some(frame) = decoder.read_next_frame().map_err(ImageError::GifDecode)? {
            let delay = if frame.delay == 0 {
                0.1
            } else {
                f64::from(frame.delay) * 0.01
            };
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

        if frames.len() <= 1 {
            let rgba = frames.first().map(Vec::as_slice).unwrap_or(&canvas);
            return Self::new(rgba, width, height);
        }
        Ok(Self::pack_animation_atlas(frames, frame_delays, width, height))
    }

    /// Packs multi-frame animation `frames` (each a full-canvas RGBA buffer of
    /// `width`x`height`) into a single horizontal-grid atlas texture carrying the
    /// per-frame `frame_delays` as a [`TextureAnimation`].
    fn pack_animation_atlas(
        frames: Vec<Vec<u8>>,
        frame_delays: Vec<f64>,
        width: usize,
        height: usize,
    ) -> ImageBuffer {
        let fits_horizontal = Cx::max_texture_width() / width;
        let total_width = fits_horizontal * width;
        let total_height = ((frames.len() / fits_horizontal) + 1) * height;
        let mut final_buffer = ImageBuffer::default();
        final_buffer.data.resize(total_width * total_height, 0);
        final_buffer.width = total_width;
        final_buffer.height = total_height;
        final_buffer.animation = Some(TextureAnimation {
            width,
            height,
            num_frames: frames.len(),
            frame_delays,
        });
        let mut cx = 0;
        let mut cy = 0;
        for frame in frames {
            for y in 0..height {
                for x in 0..width {
                    let src = (y * width + x) * 4;
                    let r = frame[src];
                    let g = frame[src + 1];
                    let b = frame[src + 2];
                    let a = frame[src + 3];
                    final_buffer.data[(y + cy) * total_width + (x + cx)] =
                        ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                }
            }
            cx += width;
            if cx >= total_width {
                cy += height;
                cx = 0;
            }
        }
        final_buffer
    }

    pub fn from_jpg(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        let mut decoder = JpegDecoder::new(cursor);
        match decoder.decode() {
            Ok(data) => {
                let info =
                    decoder
                        .info()
                        .ok_or(ImageError::JpgDecode(JpgDecodeErrors::FormatStatic(
                            "Failed to decode JPG image info",
                        )))?;
                ImageBuffer::new(&data, info.width as usize, info.height as usize)
            }
            Err(err) => Err(ImageError::JpgDecode(err)),
        }
    }

    pub fn from_bmp(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        let mut decoder = BmpDecoder::new(cursor);
        decoder.decode_headers().map_err(ImageError::BmpDecode)?;
        let (width, height) =
            decoder
                .dimensions()
                .ok_or(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic(
                    "Failed to get BMP image dimensions",
                )))?;
        let pixels = decoder.decode().map_err(ImageError::BmpDecode)?;
        Self::new(&pixels, width, height)
    }

    pub fn from_qoi(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        let mut decoder = QoiDecoder::new(cursor);
        decoder.decode_headers().map_err(ImageError::QoiDecode)?;
        let (width, height) =
            decoder
                .get_dimensions()
                .ok_or(ImageError::QoiDecode(QoiErrors::GenericStatic(
                    "Failed to get QOI image dimensions",
                )))?;
        let pixels = decoder.decode().map_err(ImageError::QoiDecode)?;
        Self::new(&pixels, width, height)
    }

    pub fn from_ico(data: &[u8]) -> Result<Self, ImageError> {
        let (offset, len, height) = ico_best_entry(data).ok_or(ImageError::UnsupportedFormat)?;
        let payload = &data[offset..offset + len];
        if detect_image_format(payload) == Some("png") {
            ImageBuffer::from_png(payload)
        } else {
            ImageBuffer::from_bmp(&ico_dib_to_bmp(payload, height)?)
        }
    }
}

pub enum ImageCacheEntry {
    Loaded(Texture),
    Loading(usize, usize),
}

#[derive(Debug)]
pub struct AsyncImageLoad {
    pub image_path: PathBuf,
    pub result: RefCell<Option<Result<ImageBuffer, ImageError>>>,
}

pub struct ImageCache {
    pub map: HashMap<PathBuf, ImageCacheEntry>,
    pub thread_pool: Option<TagThreadPool<PathBuf>>,
    pub pending_http_requests: HashMap<LiveId, PathBuf>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            thread_pool: None,
            pending_http_requests: HashMap::new(),
        }
    }
}

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
    UnsupportedFormat,
    Http(String),
}

pub enum AsyncLoadResult {
    Loading(usize, usize),
    Loaded,
}

impl Error for ImageError {}

impl From<PngDecodeErrors> for ImageError {
    fn from(value: PngDecodeErrors) -> Self {
        Self::PngDecode(value)
    }
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

fn image_decode_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("MAKEPAD_GLTF_TEX_DEBUG")
            .map(|value| value != "0")
            .unwrap_or(false)
    })
}

#[inline]
fn decode_timing_start() -> Option<Instant> {
    if !image_decode_debug_enabled() {
        return None;
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(Instant::now())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn headless_mode_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MAKEPAD")
            .map(|value| value.eq_ignore_ascii_case("headless"))
            .unwrap_or(false)
    })
}

// Pick the largest image entry from an ICO directory, returning its payload's
// (offset, length, height). Height comes from the directory entry since the
// embedded DIB doubles its own height to cover a trailing AND mask.
fn ico_best_entry(data: &[u8]) -> Option<(usize, usize, u32)> {
    if data.len() < 6 {
        return None;
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    let mut best: Option<(usize, usize, u32, u32)> = None; // offset, len, height, score
    for i in 0..count {
        let e = 6 + i * 16;
        if e + 16 > data.len() {
            break;
        }
        let width = if data[e] == 0 { 256 } else { data[e] as u32 };
        let height = if data[e + 1] == 0 { 256 } else { data[e + 1] as u32 };
        let bit_count = u16::from_le_bytes([data[e + 6], data[e + 7]]) as u32;
        let len =
            u32::from_le_bytes([data[e + 8], data[e + 9], data[e + 10], data[e + 11]]) as usize;
        let offset =
            u32::from_le_bytes([data[e + 12], data[e + 13], data[e + 14], data[e + 15]]) as usize;
        if len == 0 || offset.saturating_add(len) > data.len() {
            continue;
        }
        let score = width * height * 64 + bit_count;
        if best.is_none_or(|(.., s)| score > s) {
            best = Some((offset, len, height, score));
        }
    }
    best.map(|(offset, len, height, _)| (offset, len, height))
}

// Wrap an ICO's bare DIB (a BITMAPINFOHEADER and pixels with no BMP file header)
// into a standalone BMP the BMP decoder can read. The DIB doubles its height for
// a trailing 1bpp AND mask we skip, so restore the real height from the directory.
fn ico_dib_to_bmp(dib: &[u8], real_height: u32) -> Result<Vec<u8>, ImageError> {
    if dib.len() < 40 {
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic(
            "ICO DIB header too small",
        )));
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]) as u32;
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    let mut clr_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;
    if bit_count <= 8 && clr_used == 0 {
        clr_used = 1usize << bit_count;
    }
    let palette_bytes = if bit_count <= 8 { clr_used * 4 } else { 0 };
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
    let data_offset = 14 + header_size + mask_bytes + palette_bytes;
    let file_size = 14 + dib.len();

    let mut out = Vec::with_capacity(file_size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(data_offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    out[14 + 8..14 + 12].copy_from_slice(&(real_height as i32).to_le_bytes());
    Ok(out)
}

fn detect_image_format(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        Some("png")
    } else if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        Some("jpg")
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else if data.len() >= 6
        && data[0] == 0x47
        && data[1] == 0x49
        && data[2] == 0x46
        && data[3] == 0x38
        && (data[4] == 0x37 || data[4] == 0x39)
        && data[5] == 0x61
    {
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

fn detect_image_format_from_path_and_data(image_path: &Path, data: &[u8]) -> Option<&'static str> {
    // Prefer magic-byte detection over file extensions so in-memory/binary
    // resources decode correctly even when their synthetic path has no extension.
    if let Some(format) = detect_image_format(data) {
        return Some(format);
    }

    // Keep extension fallback for edge cases where headers are unavailable.
    let ext = image_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase());
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

/// Decodes an image of any format makepad supports into an [`ImageBuffer`],
/// auto-detecting the format from the encoded `data`'s magic bytes.
pub fn decode_image_from_data(data: &[u8]) -> Result<ImageBuffer, ImageError> {
    decode_image_buffer(Path::new(""), data)
}

/// Returns true if `data` looks like an SVG document (vs. a raster image).
///
/// SVG is a vector format with no magic bytes, so this sniffs for an `<svg>`
/// root element, optionally preceded by an XML prolog, DOCTYPE, or comment.
pub fn looks_like_svg(data: &[u8]) -> bool {
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data); // strip UTF-8 BOM
    let Ok(text) = std::str::from_utf8(data) else {
        return false;
    };
    let head = text.trim_start();
    head.starts_with("<svg")
        || ((head.starts_with("<?xml")
            || head.starts_with("<!DOCTYPE")
            || head.starts_with("<!--"))
            && text.contains("<svg"))
}

/// Expands tightly-packed RGB pixels to RGBA with full opacity.
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
    }
    out
}

fn decode_image_buffer(image_path: &Path, data: &[u8]) -> Result<ImageBuffer, ImageError> {
    let format = detect_image_format_from_path_and_data(image_path, data)
        .ok_or(ImageError::UnsupportedFormat)?;
    match format {
        "jpg" => ImageBuffer::from_jpg(data),
        "png" => ImageBuffer::from_png(data),
        "webp" => ImageBuffer::from_webp(data),
        "gif" => ImageBuffer::from_gif(data),
        "bmp" => ImageBuffer::from_bmp(data),
        "qoi" => ImageBuffer::from_qoi(data),
        "ico" => ImageBuffer::from_ico(data),
        _ => Err(ImageError::UnsupportedFormat),
    }
}

/// Returns the `(width, height)` in pixels of an encoded image, auto-detecting
/// the format from the data's magic bytes (falling back to the path's extension).
/// Reads only headers for most formats; does not fully decode.
pub fn image_size_by_data(data: &[u8], image_path: &Path) -> Result<(usize, usize), ImageError> {
    let format = detect_image_format_from_path_and_data(image_path, data)
        .ok_or(ImageError::UnsupportedFormat)?;
    match format {
        "jpg" => {
            let cursor = ZCursor::new(data);
            let mut decoder = JpegDecoder::new(cursor);
            decoder.decode_headers().map_err(ImageError::JpgDecode)?;
            let image_info = decoder.info().ok_or({
                ImageError::JpgDecode(JpgDecodeErrors::FormatStatic(
                    "Failed to get JPG image info after decoding headers",
                ))
            })?;
            Ok((image_info.width as usize, image_info.height as usize))
        }
        "png" => {
            let cursor = ZCursor::new(data);
            let mut decoder = PngDecoder::new(cursor);
            decoder.decode_headers()?;
            let (width, height) = decoder.dimensions().ok_or(ImageError::PngDecode(
                PngDecodeErrors::GenericStatic("Failed to get PNG image dimensions"),
            ))?;
            Ok((width, height))
        }
        "webp" => {
            let cursor = std::io::Cursor::new(data);
            let decoder = WebPDecoder::new(std::io::BufReader::new(cursor))
                .map_err(ImageError::WebpDecode)?;
            let (width, height) = decoder.dimensions();
            Ok((width as usize, height as usize))
        }
        "gif" => {
            let image = ImageBuffer::from_gif(data)?;
            Ok((
                image
                    .animation
                    .as_ref()
                    .map(|a| a.width)
                    .unwrap_or(image.width),
                image
                    .animation
                    .as_ref()
                    .map(|a| a.height)
                    .unwrap_or(image.height),
            ))
        }
        "bmp" => {
            let cursor = ZCursor::new(data);
            let mut decoder = BmpDecoder::new(cursor);
            decoder.decode_headers().map_err(ImageError::BmpDecode)?;
            decoder.dimensions().ok_or(ImageError::BmpDecode(
                BmpDecoderErrors::GenericStatic("Failed to get BMP image dimensions"),
            ))
        }
        "qoi" => {
            let cursor = ZCursor::new(data);
            let mut decoder = QoiDecoder::new(cursor);
            decoder.decode_headers().map_err(ImageError::QoiDecode)?;
            decoder.get_dimensions().ok_or(ImageError::QoiDecode(
                QoiErrors::GenericStatic("Failed to get QOI image dimensions"),
            ))
        }
        "ico" => {
            let image = ImageBuffer::from_ico(data)?;
            Ok((image.width, image.height))
        }
        _ => Err(ImageError::UnsupportedFormat),
    }
}

fn ensure_image_cache_inner(cx: &mut Cx) {
    if !cx.has_global::<ImageCache>() {
        cx.set_global(ImageCache::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_gif::{Encoder, Frame};
    use std::borrow::Cow;

    fn single_frame_gif() -> Vec<u8> {
        let palette = [0x00, 0x00, 0x00, 0xff, 0x00, 0x00];
        let pixels = [0, 1, 1, 0];
        let mut data = Vec::new();
        {
            let mut encoder = Encoder::new(&mut data, 2, 2, &palette).unwrap();
            let mut frame = Frame::default();
            frame.width = 2;
            frame.height = 2;
            frame.buffer = Cow::Borrowed(&pixels);
            encoder.write_frame(&frame).unwrap();
        }
        data
    }

    fn animated_gif() -> Vec<u8> {
        let palette = [
            0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0xff,
        ];
        let frames = [[0, 1, 1, 0], [1, 2, 2, 1], [2, 3, 3, 2], [3, 0, 0, 3]];
        let mut data = Vec::new();
        {
            let mut encoder = Encoder::new(&mut data, 2, 2, &palette).unwrap();
            for pixels in frames {
                let mut frame = Frame::default();
                frame.width = 2;
                frame.height = 2;
                frame.delay = 5;
                frame.buffer = Cow::Borrowed(&pixels);
                encoder.write_frame(&frame).unwrap();
            }
        }
        data
    }

    fn zero_delay_gif() -> Vec<u8> {
        let palette = [0x00, 0x00, 0x00, 0xff, 0x00, 0x00];
        let frames = [[0, 1, 1, 0], [1, 0, 0, 1]];
        let mut data = Vec::new();
        {
            let mut encoder = Encoder::new(&mut data, 2, 2, &palette).unwrap();
            for pixels in frames {
                let mut frame = Frame::default();
                frame.width = 2;
                frame.height = 2;
                frame.delay = 0;
                frame.buffer = Cow::Borrowed(&pixels);
                encoder.write_frame(&frame).unwrap();
            }
        }
        data
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    fn adler32(bytes: &[u8]) -> u32 {
        let mut a = 1u32;
        let mut b = 0u32;
        for byte in bytes {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn zlib_stored(bytes: &[u8]) -> Vec<u8> {
        let len = bytes.len() as u16;
        let mut out = vec![0x78, 0x01, 0x01];
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(&adler32(bytes).to_be_bytes());
        out
    }

    fn push_chunk(png: &mut Vec<u8>, name: &[u8; 4], payload: &[u8]) {
        png.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        png.extend_from_slice(name);
        png.extend_from_slice(payload);
        let mut crc_data = Vec::with_capacity(name.len() + payload.len());
        crc_data.extend_from_slice(name);
        crc_data.extend_from_slice(payload);
        png.extend_from_slice(&crc32(&crc_data).to_be_bytes());
    }

    fn rgba_frame(color: [u8; 4]) -> Vec<u8> {
        let mut data = Vec::new();
        for _ in 0..2 {
            data.push(0);
            data.extend_from_slice(&color);
            data.extend_from_slice(&color);
        }
        data
    }

    fn fctl(seq: u32, delay_num: u16) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&seq.to_be_bytes());
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&delay_num.to_be_bytes());
        payload.extend_from_slice(&100u16.to_be_bytes());
        payload.push(0);
        payload.push(0);
        payload
    }

    fn animated_png() -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        push_chunk(&mut png, b"IHDR", &ihdr);

        let mut actl = Vec::new();
        actl.extend_from_slice(&4u32.to_be_bytes());
        actl.extend_from_slice(&0u32.to_be_bytes());
        push_chunk(&mut png, b"acTL", &actl);

        let colors = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
        ];
        push_chunk(&mut png, b"fcTL", &fctl(0, 5));
        push_chunk(&mut png, b"IDAT", &zlib_stored(&rgba_frame(colors[0])));
        let mut seq = 1;
        for color in colors.iter().skip(1) {
            push_chunk(&mut png, b"fcTL", &fctl(seq, 5));
            seq += 1;
            let mut fdat = Vec::new();
            fdat.extend_from_slice(&seq.to_be_bytes());
            fdat.extend_from_slice(&zlib_stored(&rgba_frame(*color)));
            push_chunk(&mut png, b"fdAT", &fdat);
            seq += 1;
        }
        push_chunk(&mut png, b"IEND", &[]);
        png
    }

    #[test]
    fn test_detect_image_format_recognises_gif89a() {
        assert_eq!(detect_image_format(b"GIF89a"), Some("gif"));
    }

    #[test]
    fn test_detect_image_format_recognises_gif87a() {
        assert_eq!(detect_image_format(b"GIF87a"), Some("gif"));
    }

    #[test]
    fn test_detect_image_format_still_recognises_png_after_gif_branch() {
        assert_eq!(detect_image_format(b"\x89PNG\r\n\x1a\n"), Some("png"));
    }

    #[test]
    fn test_detect_image_format_from_path_and_data_falls_back_to_gif_extension() {
        assert_eq!(
            detect_image_format_from_path_and_data(Path::new("sticker.gif"), &[]),
            Some("gif")
        );
    }

    #[test]
    fn test_from_gif_decodes_single_frame() {
        let image = ImageBuffer::from_gif(&single_frame_gif()).unwrap();
        assert_eq!(image.width, 2);
        assert_eq!(image.height, 2);
        assert!(image.animation.is_none());
    }

    #[test]
    fn test_from_gif_packs_animated_frames_into_atlas() {
        let image = ImageBuffer::from_gif(&animated_gif()).unwrap();
        let animation = image.animation.as_ref().unwrap();
        assert_eq!(animation.width, 2);
        assert_eq!(animation.height, 2);
        assert_eq!(animation.num_frames, 4);
        assert_eq!(animation.frame_delays.len(), 4);
        assert!(animation
            .frame_delays
            .iter()
            .all(|delay| (*delay - 0.05).abs() < f64::EPSILON));
        assert!(image.width >= 2 * 4);
        assert!(image.height >= 2);
    }

    #[test]
    fn test_from_gif_single_frame_has_no_frame_delays() {
        let image = ImageBuffer::from_gif(&single_frame_gif()).unwrap();
        assert!(image.animation.is_none());
    }

    #[test]
    fn test_from_gif_zero_delay_normalised_to_100ms() {
        let image = ImageBuffer::from_gif(&zero_delay_gif()).unwrap();
        let animation = image.animation.as_ref().unwrap();
        assert_eq!(animation.frame_delays, vec![0.1, 0.1]);
    }

    #[test]
    fn test_from_png_animated_does_not_populate_frame_delays() {
        let image = ImageBuffer::from_png(&animated_png()).unwrap();
        let animation = image.animation.as_ref().unwrap();
        assert!(animation.frame_delays.is_empty());
    }

    #[test]
    fn test_from_gif_rejects_truncated_data() {
        assert!(matches!(
            ImageBuffer::from_gif(&[0x47, 0x49, 0x46, 0x38]),
            Err(ImageError::GifDecode(_))
        ));
    }

    #[test]
    fn test_decode_image_buffer_rejects_random_bytes_as_unsupported() {
        assert!(matches!(
            decode_image_buffer(Path::new("sticker"), &[0; 16]),
            Err(ImageError::UnsupportedFormat)
        ));
    }

    #[test]
    fn test_makepad_gif_is_only_a_dependency_of_makepad_draw() {
        let lock_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("Cargo.lock");
        let lock = std::fs::read_to_string(lock_path).unwrap();
        let consumers: Vec<&str> = lock
            .split("[[package]]")
            .filter(|block| block.contains("\"makepad-gif\""))
            .filter_map(|block| {
                block
                    .lines()
                    .find_map(|line| line.strip_prefix("name = \"")?.strip_suffix('"'))
            })
            .filter(|name| *name != "makepad-gif")
            .collect();
        assert_eq!(consumers, ["makepad-draw"]);
    }

    fn static_png_2x2(color: [u8; 4]) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        push_chunk(&mut png, b"IHDR", &ihdr);
        push_chunk(&mut png, b"IDAT", &zlib_stored(&rgba_frame(color)));
        push_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn bmp_2x2_24bit(r: u8, g: u8, b: u8) -> Vec<u8> {
        // Two bottom-up BGR rows, each padded to a 4-byte boundary.
        let pixels = [b, g, r, b, g, r, 0, 0, b, g, r, b, g, r, 0, 0];
        let mut info = Vec::new();
        info.extend_from_slice(&40u32.to_le_bytes()); // header size
        info.extend_from_slice(&2i32.to_le_bytes()); // width
        info.extend_from_slice(&2i32.to_le_bytes()); // height
        info.extend_from_slice(&1u16.to_le_bytes()); // planes
        info.extend_from_slice(&24u16.to_le_bytes()); // bpp
        info.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        info.extend_from_slice(&(pixels.len() as u32).to_le_bytes()); // image size
        info.extend_from_slice(&[0u8; 16]); // ppm x/y, clr used/important
        let offset = 14 + info.len();
        let file_size = offset + pixels.len();
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes()); // reserved
        bmp.extend_from_slice(&(offset as u32).to_le_bytes());
        bmp.extend_from_slice(&info);
        bmp.extend_from_slice(&pixels);
        bmp
    }

    fn qoi_solid_2x2(r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(b"qoif");
        data.extend_from_slice(&2u32.to_be_bytes()); // width
        data.extend_from_slice(&2u32.to_be_bytes()); // height
        data.push(4); // channels (RGBA)
        data.push(0); // colorspace (sRGB)
        data.extend_from_slice(&[0xFE, r, g, b]); // QOI_OP_RGB, alpha stays 255
        data.push(0xC0 | 2); // QOI_OP_RUN covering the remaining 3 pixels
        data.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]); // end marker
        data
    }

    fn ico_dib_2x2_32bit(r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes()); // header size
        dib.extend_from_slice(&2i32.to_le_bytes()); // width
        dib.extend_from_slice(&4i32.to_le_bytes()); // doubled height (color rows + AND mask)
        dib.extend_from_slice(&1u16.to_le_bytes()); // planes
        dib.extend_from_slice(&32u16.to_le_bytes()); // bpp
        dib.extend_from_slice(&[0u8; 24]); // compression, sizes, ppm, clr counts
        for _ in 0..4 {
            dib.extend_from_slice(&[b, g, r, 255]); // XOR bitmap, BGRA
        }
        dib.extend_from_slice(&[0u8; 8]); // AND mask, fully opaque
        dib
    }

    fn ico_wrap(payload: &[u8], width: u8, height: u8, bit_count: u16) -> Vec<u8> {
        let mut ico = Vec::new();
        ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // type = icon
        ico.extend_from_slice(&1u16.to_le_bytes()); // count
        ico.push(width);
        ico.push(height);
        ico.push(0); // color count
        ico.push(0); // reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // planes
        ico.extend_from_slice(&bit_count.to_le_bytes());
        ico.extend_from_slice(&(payload.len() as u32).to_le_bytes()); // bytes in resource
        ico.extend_from_slice(&22u32.to_le_bytes()); // offset (6 + 16)
        ico.extend_from_slice(payload);
        ico
    }

    #[test]
    fn test_detect_image_format_recognises_bmp_qoi_ico() {
        assert_eq!(detect_image_format(b"BM\x00\x00"), Some("bmp"));
        assert_eq!(detect_image_format(b"qoif"), Some("qoi"));
        assert_eq!(detect_image_format(&[0, 0, 1, 0]), Some("ico"));
    }

    #[test]
    fn test_from_bmp_decodes_24bit_in_rgb_order() {
        let image = ImageBuffer::from_bmp(&bmp_2x2_24bit(10, 20, 30)).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.data, vec![0xFF0A_141E; 4]);
    }

    #[test]
    fn test_from_qoi_decodes_solid() {
        let image = ImageBuffer::from_qoi(&qoi_solid_2x2(10, 20, 30)).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.data, vec![0xFF0A_141E; 4]);
    }

    #[test]
    fn test_from_ico_decodes_embedded_png() {
        let ico = ico_wrap(&static_png_2x2([10, 20, 30, 255]), 2, 2, 32);
        let image = ImageBuffer::from_ico(&ico).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.data[0] & 0x00FF_FFFF, 0x000A_141E);
    }

    #[test]
    fn test_from_ico_decodes_embedded_bmp_dib() {
        let ico = ico_wrap(&ico_dib_2x2_32bit(10, 20, 30), 2, 2, 32);
        let image = ImageBuffer::from_ico(&ico).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.data[0] & 0x00FF_FFFF, 0x000A_141E);
    }

    #[test]
    fn test_decode_image_buffer_dispatches_new_formats() {
        for data in [
            bmp_2x2_24bit(10, 20, 30),
            qoi_solid_2x2(10, 20, 30),
            ico_wrap(&ico_dib_2x2_32bit(10, 20, 30), 2, 2, 32),
        ] {
            let image = decode_image_buffer(Path::new("img"), &data).unwrap();
            assert_eq!((image.width, image.height), (2, 2));
        }
    }

    #[test]
    fn test_decode_image_from_data_auto_detects_without_path() {
        for data in [
            bmp_2x2_24bit(10, 20, 30),
            qoi_solid_2x2(10, 20, 30),
            ico_wrap(&static_png_2x2([10, 20, 30, 255]), 2, 2, 32),
        ] {
            let image = decode_image_from_data(&data).unwrap();
            assert_eq!((image.width, image.height), (2, 2));
        }
        assert!(matches!(
            decode_image_from_data(&[0; 16]),
            Err(ImageError::UnsupportedFormat)
        ));
    }
}

fn ensure_thread_pool(cx: &mut Cx) {
    ensure_image_cache_inner(cx);
    if cx.get_global::<ImageCache>().thread_pool.is_none() {
        let threads = cx.cpu_cores().max(3) - 2;
        cx.get_global::<ImageCache>().thread_pool = Some(TagThreadPool::new(cx, threads));
    }
}

fn spawn_decode_job(cx: &mut Cx, image_path: PathBuf, data: Arc<Vec<u8>>) {
    ensure_thread_pool(cx);
    let image_size_bytes = data.len();
    cx.get_global::<ImageCache>()
        .thread_pool
        .as_mut()
        .unwrap()
        .execute_rev(image_path, move |image_path| {
            let start = decode_timing_start();
            if image_decode_debug_enabled() {
                log!(
                    "ImageCache: decode_start key={} bytes={}",
                    image_path.display(),
                    image_size_bytes
                );
            }
            let result = decode_image_buffer(&image_path, &data);
            if image_decode_debug_enabled() {
                let status = match &result {
                    Ok(buffer) => format!("ok {}x{}", buffer.width, buffer.height),
                    Err(err) => format!("err {err}"),
                };
                if let Some(start) = start {
                    log!(
                        "ImageCache: decode_done key={} elapsed_ms={:.1} {}",
                        image_path.display(),
                        start.elapsed().as_secs_f64() * 1000.0,
                        status
                    );
                } else {
                    log!(
                        "ImageCache: decode_done key={} {}",
                        image_path.display(),
                        status
                    );
                }
            }
            Cx::post_action(AsyncImageLoad {
                image_path,
                result: RefCell::new(Some(result)),
            });
        });
}

pub fn ensure_image_cache(cx: &mut Cx) {
    ensure_image_cache_inner(cx);
}

pub fn process_async_image_load(
    cx: &mut Cx,
    image_path: &Path,
    result: Result<ImageBuffer, ImageError>,
) {
    ensure_image_cache_inner(cx);
    if let Ok(data) = result {
        let width = data.width;
        let height = data.height;
        let upload_start = decode_timing_start();
        let texture = data.into_new_texture(cx);
        if image_decode_debug_enabled() {
            if let Some(upload_start) = upload_start {
                log!(
                    "ImageCache: gpu_commit key={} elapsed_ms={:.1} size={}x{}",
                    image_path.display(),
                    upload_start.elapsed().as_secs_f64() * 1000.0,
                    width,
                    height
                );
            } else {
                log!(
                    "ImageCache: gpu_commit key={} size={}x{}",
                    image_path.display(),
                    width,
                    height
                );
            }
        }
        cx.get_global::<ImageCache>()
            .map
            .insert(image_path.into(), ImageCacheEntry::Loaded(texture));
    } else {
        if image_decode_debug_enabled() {
            log!(
                "ImageCache: gpu_commit key={} skipped (decode error)",
                image_path.display()
            );
        }
        cx.get_global::<ImageCache>().map.remove(image_path);
    }
}

pub fn load_image_from_cache(cx: &mut Cx, image_path: &Path) -> Option<Texture> {
    ensure_image_cache_inner(cx);
    match cx.get_global::<ImageCache>().map.get(image_path) {
        Some(ImageCacheEntry::Loaded(texture)) => Some(texture.clone()),
        _ => None,
    }
}

pub fn load_image_from_data_async(
    cx: &mut Cx,
    image_path: &Path,
    data: Arc<Vec<u8>>,
) -> Result<AsyncLoadResult, ImageError> {
    ensure_image_cache_inner(cx);
    match cx.get_global::<ImageCache>().map.get(image_path) {
        Some(ImageCacheEntry::Loaded(_)) => return Ok(AsyncLoadResult::Loaded),
        Some(ImageCacheEntry::Loading(w, h)) => return Ok(AsyncLoadResult::Loading(*w, *h)),
        None => {}
    }

    // On wasm, decode synchronously on the UI thread since thread pools
    // are not reliably available. Also decode synchronously for headless
    // single-frame runs so textured output is available in the first emitted PNG.
    #[cfg(target_arch = "wasm32")]
    let force_sync = true;
    #[cfg(not(target_arch = "wasm32"))]
    let force_sync = headless_mode_enabled();

    if force_sync {
        let image = decode_image_buffer(image_path, &data)?;
        let texture = image.into_new_texture(cx);
        cx.get_global::<ImageCache>()
            .map
            .insert(image_path.into(), ImageCacheEntry::Loaded(texture));
        return Ok(AsyncLoadResult::Loaded);
    }

    let (w, h) = image_size_by_data(&data, image_path)?;
    if image_decode_debug_enabled() {
        log!(
            "ImageCache: queue_decode key={} bytes={} size={}x{}",
            image_path.display(),
            data.len(),
            w,
            h
        );
    }
    cx.get_global::<ImageCache>()
        .map
        .insert(image_path.into(), ImageCacheEntry::Loading(w, h));
    spawn_decode_job(cx, image_path.to_path_buf(), data);
    Ok(AsyncLoadResult::Loading(w, h))
}

pub fn load_image_file_by_path_async(
    cx: &mut Cx,
    image_path: &Path,
) -> Result<AsyncLoadResult, ImageError> {
    ensure_image_cache_inner(cx);
    match cx.get_global::<ImageCache>().map.get(image_path) {
        Some(ImageCacheEntry::Loaded(_)) => Ok(AsyncLoadResult::Loaded),
        Some(ImageCacheEntry::Loading(w, h)) => Ok(AsyncLoadResult::Loading(*w, *h)),
        None => match std::fs::read(image_path) {
            Ok(data) => load_image_from_data_async(cx, image_path, Arc::new(data)),
            Err(_) => Err(ImageError::PathNotFound(image_path.into())),
        },
    }
}

pub fn load_image_http_by_url_async(cx: &mut Cx, url: &str) -> Result<AsyncLoadResult, ImageError> {
    ensure_image_cache_inner(cx);
    let image_path = PathBuf::from(url);
    match cx.get_global::<ImageCache>().map.get(&image_path) {
        Some(ImageCacheEntry::Loaded(_)) => return Ok(AsyncLoadResult::Loaded),
        Some(ImageCacheEntry::Loading(w, h)) => return Ok(AsyncLoadResult::Loading(*w, *h)),
        None => {}
    }

    let request_id = LiveId::unique();
    cx.get_global::<ImageCache>()
        .map
        .insert(image_path.clone(), ImageCacheEntry::Loading(1, 1));
    cx.get_global::<ImageCache>()
        .pending_http_requests
        .insert(request_id, image_path);
    cx.http_request(
        request_id,
        HttpRequest::new(url.to_string(), HttpMethod::GET),
    );
    Ok(AsyncLoadResult::Loading(1, 1))
}

pub fn handle_image_cache_network_responses(cx: &mut Cx, e: &NetworkResponsesEvent) {
    if !cx.has_global::<ImageCache>() {
        return;
    }

    let mut decode_queue = Vec::<(PathBuf, Arc<Vec<u8>>)>::with_capacity(e.len());

    {
        let cache = cx.get_global::<ImageCache>();
        for response in e {
            match response {
                NetworkResponse::HttpError { request_id, error } => {
                    let Some(image_path) = cache.pending_http_requests.remove(request_id) else {
                        continue;
                    };
                    error!(
                        "image http request failed for {:?}: {}",
                        image_path, error.message
                    );
                    cache.map.remove(&image_path);
                }
                NetworkResponse::HttpResponse {
                    request_id,
                    response,
                }
                | NetworkResponse::HttpStreamComplete {
                    request_id,
                    response,
                } => {
                    let Some(image_path) = cache.pending_http_requests.remove(request_id) else {
                        continue;
                    };
                    if !(200..300).contains(&response.status_code) {
                        cache.map.remove(&image_path);
                        continue;
                    }
                    if let Some(body) = &response.body {
                        cache.map.remove(&image_path);
                        decode_queue.push((image_path, Arc::new(body.clone())));
                    } else {
                        cache.map.remove(&image_path);
                    }
                }
                NetworkResponse::HttpProgress { .. }
                | NetworkResponse::HttpStreamChunk { .. }
                | NetworkResponse::WsOpened { .. }
                | NetworkResponse::WsMessage { .. }
                | NetworkResponse::WsClosed { .. }
                | NetworkResponse::WsError { .. } => {}
            }
        }
    }

    for (image_path, data) in decode_queue {
        let _ = load_image_from_data_async(cx, &image_path, data);
    }
}

pub trait ImageCacheImpl {
    fn get_texture(&self, id: usize) -> &Option<Texture>;
    fn set_texture(&mut self, texture: Option<Texture>, id: usize);

    fn lazy_create_image_cache(&mut self, cx: &mut Cx) {
        ensure_image_cache(cx);
    }

    fn load_png_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_png(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn load_jpg_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_jpg(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn load_bmp_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_bmp(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn load_qoi_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_qoi(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn load_ico_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_ico(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn load_gif_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_gif(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn load_webp_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = ImageBuffer::from_webp(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn load_image_from_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
    ) -> Result<(), ImageError> {
        let image = decode_image_from_data(data)?;
        self.set_texture(Some(image.into_new_texture(cx)), id);
        Ok(())
    }

    fn process_async_image_load(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        result: Result<ImageBuffer, ImageError>,
    ) -> bool {
        process_async_image_load(cx, image_path, result);
        false
    }

    fn load_image_from_cache(&mut self, cx: &mut Cx, image_path: &Path, id: usize) -> bool {
        if let Some(texture) = load_image_from_cache(cx, image_path) {
            self.set_texture(Some(texture), id);
            true
        } else {
            false
        }
    }

    fn load_image_from_data_async_impl(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        data: Arc<Vec<u8>>,
        id: usize,
    ) -> Result<AsyncLoadResult, ImageError> {
        let result = load_image_from_data_async(cx, image_path, data)?;
        if matches!(result, AsyncLoadResult::Loaded) {
            let _ = self.load_image_from_cache(cx, image_path, id);
        }
        Ok(result)
    }

    fn load_image_file_by_path_async_impl(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        id: usize,
    ) -> Result<AsyncLoadResult, ImageError> {
        let result = load_image_file_by_path_async(cx, image_path)?;
        if matches!(result, AsyncLoadResult::Loaded) {
            let _ = self.load_image_from_cache(cx, image_path, id);
        }
        Ok(result)
    }

    fn load_image_http_by_url_async_impl(
        &mut self,
        cx: &mut Cx,
        url: &str,
        id: usize,
    ) -> Result<AsyncLoadResult, ImageError> {
        let result = load_image_http_by_url_async(cx, url)?;
        if matches!(result, AsyncLoadResult::Loaded) {
            let image_path = PathBuf::from(url);
            let _ = self.load_image_from_cache(cx, &image_path, id);
        }
        Ok(result)
    }

    fn load_image_file_by_path_and_data(
        &mut self,
        cx: &mut Cx,
        data: &[u8],
        id: usize,
        image_path: &Path,
    ) -> Result<(), ImageError> {
        let image = decode_image_buffer(image_path, data)?;
        let texture = image.into_new_texture(cx);
        ensure_image_cache(cx);
        cx.get_global::<ImageCache>()
            .map
            .insert(image_path.into(), ImageCacheEntry::Loaded(texture.clone()));
        self.set_texture(Some(texture), id);
        Ok(())
    }

    fn load_image_file_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        id: usize,
    ) -> Result<(), ImageError> {
        if let Some(texture) = load_image_from_cache(cx, image_path) {
            self.set_texture(Some(texture), id);
            return Ok(());
        }
        let data =
            std::fs::read(image_path).map_err(|_| ImageError::PathNotFound(image_path.into()))?;
        self.load_image_file_by_path_and_data(cx, &data, id, image_path)
    }

    fn load_image_dep_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &str,
        id: usize,
    ) -> Result<(), ImageError> {
        let p_image_path = Path::new(image_path);
        if let Some(texture) = load_image_from_cache(cx, p_image_path) {
            self.set_texture(Some(texture), id);
            return Ok(());
        }
        match cx.take_dependency(image_path) {
            Ok(data) => self.load_image_file_by_path_and_data(cx, &data, id, p_image_path),
            Err(_) => Err(ImageError::PathNotFound(image_path.into())),
        }
    }
}
