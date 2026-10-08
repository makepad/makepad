use crate::makepad_platform::*;
use makepad_gif::{ColorOutput, DecodeOptions, DisposalMethod};
use makepad_webp::WebPDecoder;
use makepad_zune_bmp::BmpDecoder;
use makepad_zune_jpeg::JpegDecoder;
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::{BlendOp, DisposeOp, PngDecoder};
use makepad_zune_qoi::QoiDecoder;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

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
    /// `Some(n)` when `data` holds the full concatenated mip chain (level 0 first,
    /// highest level `n`) instead of level 0 only. Set by
    /// [`ImageBuffer::build_cpu_mip_chain_if_uploaded`].
    max_level: Option<usize>,
    /// The size of the image (or of each animation frame) before it was shrunk, if it was.
    natural_size: Option<(usize, usize)>,
}

/// Alpha-weighted average of up to four `0xAARRGGBB` texels (premultiplying RGB by alpha so
/// transparent texels don't bleed their undefined color into the average).
fn box_average_bgra(samples: &[u32]) -> u32 {
    let (mut b, mut g, mut r, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
    for &p in samples {
        let pa = (p >> 24) & 0xFF;
        b += (p & 0xFF) * pa;
        g += ((p >> 8) & 0xFF) * pa;
        r += ((p >> 16) & 0xFF) * pa;
        a += pa;
        n += 1;
    }
    if a == 0 {
        return 0;
    }
    let out_a = a / n.max(1);
    (out_a << 24) | ((r / a) << 16) | ((g / a) << 8) | (b / a)
}

/// Build a full mip chain (level 0 down to 1x1) for a BGRA-`u32` image via an alpha-weighted
/// 2x2 box filter. Returns the concatenated level data (level 0 first) and the highest mip
/// level index. Only backends that upload the chain level by level read past level 0, so the
/// chain should only be built where [`backend_uploads_cpu_mip_chain`] is true.
fn generate_bgra_mip_chain(width: usize, height: usize, level0: Vec<u32>) -> (Vec<u32>, usize) {
    let mut out = level0;
    let (mut w, mut h) = (width, height);
    let mut level_start = 0usize;
    let mut max_level = 0usize;
    while w > 1 || h > 1 {
        let nw = (w / 2).max(1);
        let nh = (h / 2).max(1);
        let mut next = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            let sy0 = (y * 2).min(h - 1);
            let sy1 = (y * 2 + 1).min(h - 1);
            for x in 0..nw {
                let sx0 = (x * 2).min(w - 1);
                let sx1 = (x * 2 + 1).min(w - 1);
                let at = |sx: usize, sy: usize| out[level_start + sy * w + sx];
                next.push(box_average_bgra(&[
                    at(sx0, sy0),
                    at(sx1, sy0),
                    at(sx0, sy1),
                    at(sx1, sy1),
                ]));
            }
        }
        level_start = out.len();
        out.extend_from_slice(&next);
        w = nw;
        h = nh;
        max_level += 1;
    }
    (out, max_level)
}

/// Highest mip level index for a `width` x `height` image, matching the level count
/// `generate_bgra_mip_chain` emits. Needed even without CPU-built levels: OpenGL sets
/// it as `TEXTURE_MAX_LEVEL` while generating the chain itself on the GPU.
fn mip_chain_max_level(width: usize, height: usize) -> usize {
    width.max(height).max(1).ilog2() as usize
}

/// True when the active render backend uploads a CPU-built mip chain level by level
/// (Metal). OpenGL generates mips on the GPU from level 0 via `glGenerateMipmap`, and
/// the remaining backends upload level 0 only, so building the chain for them wastes
/// CPU time and memory.
fn backend_uploads_cpu_mip_chain() -> bool {
    cfg!(any(target_os = "macos", target_os = "ios", target_os = "tvos"))
}

/// Halves an image's size in both directions, rounding up.
fn halve_size((width, height): (usize, usize)) -> (usize, usize) {
    (width.div_ceil(2), height.div_ceil(2))
}

/// Returns how many times an image that's `natural_size` pixels big can be halved
/// and still have enough pixels to be drawn `drawn_size` pixels big.
fn count_halvings(natural_size: (usize, usize), drawn_size: (usize, usize)) -> usize {
    let mut size = natural_size;
    let mut halvings = 0;
    while size != (1, 1) && halve_size(size).0 >= drawn_size.0 && halve_size(size).1 >= drawn_size.1 {
        size = halve_size(size);
        halvings += 1;
    }
    halvings
}

/// Returns how many pixels an image that's `natural_size` pixels big gets decoded into
/// to be drawn `drawn_size` pixels big: halved as often as it can be, like a mip level.
fn get_decoded_size(natural_size: (usize, usize), drawn_size: (usize, usize)) -> (usize, usize) {
    (0..count_halvings(natural_size, drawn_size)).fold(natural_size, |size, _| halve_size(size))
}

/// Halves the `width` x `height` texels at the start of `src` (whose rows are `stride` texels
/// apart) into a new buffer, averaging each 2x2 block the same way the mip chain does.
fn halve_texels(src: &[u32], stride: usize, (width, height): (usize, usize)) -> Vec<u32> {
    let (half_width, half_height) = halve_size((width, height));
    let mut half = Vec::with_capacity(half_width * half_height);
    for y in 0..half_height {
        let row0 = y * 2 * stride;
        let row1 = (y * 2 + 1).min(height - 1) * stride;
        for x in 0..half_width {
            let (x0, x1) = (x * 2, (x * 2 + 1).min(width - 1));
            half.push(box_average_bgra(&[
                src[row0 + x0],
                src[row0 + x1],
                src[row1 + x0],
                src[row1 + x1],
            ]));
        }
    }
    half
}

/// Returns whether `texture` has enough pixels to be drawn `drawn_size` pixels big,
/// or to be drawn with all of the image's pixels if that's `None`.
pub fn has_enough_pixels(cx: &mut Cx, texture: &Texture, drawn_size: Option<(usize, usize)>) -> bool {
    let Some(natural_size) = texture.natural_size(cx) else {
        return true;
    };
    let Some(drawn_size) = drawn_size else {
        return false;
    };
    let decoded_size = match texture.animation(cx) {
        Some(animation) => (animation.width, animation.height),
        None => texture.get_format(cx).vec_width_height().unwrap_or_default(),
    };
    let needed_size = get_decoded_size(natural_size, drawn_size);
    decoded_size.0 >= needed_size.0 && decoded_size.1 >= needed_size.1
}

/// A decode that's on its way, with enough pixels for an image that's `natural_size` pixels big
/// to be drawn at `drawn_size`, or with all of its pixels if that's `None`.
#[derive(Clone, Copy)]
struct PendingDecode {
    natural_size: (usize, usize),
    drawn_size: Option<(usize, usize)>,
}

impl PendingDecode {
    /// Returns whether this decode will have enough pixels for the image to be drawn at `drawn_size` too.
    fn is_big_enough_for(&self, drawn_size: Option<(usize, usize)>) -> bool {
        match (self.drawn_size, drawn_size) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(own_drawn_size), Some(drawn_size)) => {
                let own_decoded_size = get_decoded_size(self.natural_size, own_drawn_size);
                let decoded_size = get_decoded_size(self.natural_size, drawn_size);
                own_decoded_size.0 >= decoded_size.0 && own_decoded_size.1 >= decoded_size.1
            }
        }
    }
}

/// Hard upper bound on a decoded image's width or height (per side). Matches
/// zune's default cap; lets us reject decompression-bomb headers before
/// allocating anything.
const MAX_IMAGE_DIMENSION: usize = 16384;
/// Hard upper bound on a decoded image or animation atlas in bytes. Browser
/// textures have a tighter admission limit; native retains the existing 1 GiB
/// cap (the RGBA size of a 16384x16384 image).
const WEB_MAX_IMAGE_DECODED_BYTES: usize = 64 * 1024 * 1024;
const NATIVE_MAX_IMAGE_DECODED_BYTES: usize = 1024 * 1024 * 1024;
const MAX_IMAGE_DECODED_BYTES: usize = if cfg!(target_arch = "wasm32") {
    WEB_MAX_IMAGE_DECODED_BYTES
} else {
    NATIVE_MAX_IMAGE_DECODED_BYTES
};
const IMAGE_RGBA_BYTES_PER_PIXEL: usize = std::mem::size_of::<u32>();
/// Hard upper bound on total pixels in a decoded buffer or animation atlas.
const MAX_IMAGE_PIXELS: usize = MAX_IMAGE_DECODED_BYTES / IMAGE_RGBA_BYTES_PER_PIXEL;
/// Hard upper bound on animation frames. Pixel caps alone do not account for
/// per-frame Vec overhead, which matters for malicious tiny-frame animations.
const MAX_IMAGE_FRAMES: usize = 4096;
const MAX_SVG_SNIFF_BYTES: usize = 4096;

fn decoder_options() -> DecoderOptions {
    DecoderOptions::default()
        .set_max_width(MAX_IMAGE_DIMENSION)
        .set_max_height(MAX_IMAGE_DIMENSION)
}

fn png_decoder_options() -> DecoderOptions {
    decoder_options().png_set_strip_to_8bit(true)
}

fn checked_rgba_byte_len(pixel_count: usize, max_decoded_bytes: usize) -> Option<usize> {
    pixel_count
        .checked_mul(IMAGE_RGBA_BYTES_PER_PIXEL)
        .filter(|&bytes| bytes <= max_decoded_bytes)
}

/// Validates `width`/`height` against the supplied decoded-byte policy and
/// returns `width * height`, rejecting invalid dimensions before allocation.
fn checked_pixel_count_with_limit(
    width: usize,
    height: usize,
    max_decoded_bytes: usize,
) -> Result<usize, ImageError> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(ImageError::DimensionsTooLarge { width, height });
    }
    let pixels = width
        .checked_mul(height)
        .ok_or(ImageError::DimensionsTooLarge { width, height })?;
    checked_rgba_byte_len(pixels, max_decoded_bytes)
        .map(|_| pixels)
        .ok_or(ImageError::DimensionsTooLarge { width, height })
}

fn checked_pixel_count(width: usize, height: usize) -> Result<usize, ImageError> {
    checked_pixel_count_with_limit(width, height, MAX_IMAGE_DECODED_BYTES)
}

/// Computes an animation atlas's `(total_width, total_height)` for `frame_count`
/// frames of `width`x`height`. Rejects zero-width frames (avoids divide-by-zero)
/// and atlases exceeding the decoded-byte policy (avoids overflow / OOM from a
/// malicious frame count).
fn animation_atlas_layout(
    frame_count: usize,
    width: usize,
    height: usize,
) -> Result<(usize, usize), ImageError> {
    animation_atlas_layout_with_limit(frame_count, width, height, MAX_IMAGE_DECODED_BYTES)
}

fn animation_atlas_layout_with_limit(
    frame_count: usize,
    width: usize,
    height: usize,
    max_decoded_bytes: usize,
) -> Result<(usize, usize), ImageError> {
    checked_pixel_count_with_limit(width, height, max_decoded_bytes)?;
    if frame_count == 0 || frame_count > MAX_IMAGE_FRAMES {
        return Err(ImageError::DimensionsTooLarge { width, height });
    }
    // Use as few rows as the frames fit in, then as few columns as fill those rows,
    // so the atlas (which gets uploaded all at once) is barely bigger than its frames.
    let max_columns = (Cx::max_texture_width() / width).max(1);
    let fits_horizontal = frame_count.div_ceil(frame_count.div_ceil(max_columns));
    let total_width = fits_horizontal * width;
    let rows = frame_count
        .checked_add(fits_horizontal - 1)
        .and_then(|count| count.checked_div(fits_horizontal))
        .ok_or(ImageError::DimensionsTooLarge { width, height })?;
    let total_height = rows
        .checked_mul(height)
        .ok_or(ImageError::DimensionsTooLarge { width, height })?;
    if total_width
        .checked_mul(total_height)
        .and_then(|pixels| checked_rgba_byte_len(pixels, max_decoded_bytes))
        .is_none()
    {
        return Err(ImageError::DimensionsTooLarge {
            width: total_width,
            height: total_height,
        });
    }
    Ok((total_width, total_height))
}

impl ImageBuffer {
    pub fn new(in_data: &[u8], width: usize, height: usize) -> Result<ImageBuffer, ImageError> {
        let pixels = checked_pixel_count(width, height)?;
        if in_data.len() % pixels != 0 {
            return Err(ImageError::InvalidPixelAlignment(in_data.len()));
        }
        let components = in_data.len() / pixels;
        if !(1..=4).contains(&components) {
            return Err(ImageError::InvalidPixelAlignment(components));
        }
        let mut out = Vec::with_capacity(pixels);
        match components {
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
            max_level: None,
            natural_size: None,
        })
    }

    /// Expands `data` from level 0 to the full concatenated mip chain when the current
    /// configuration actually uploads CPU-built mips: a static image, with mipmaps
    /// enabled, on a backend that reads levels past 0. Idempotent. Called from the
    /// decode worker so the box-filter cost stays off the UI thread.
    fn build_cpu_mip_chain_if_uploaded(&mut self) {
        if self.max_level.is_some()
            || self.animation.is_some()
            || !backend_uploads_cpu_mip_chain()
            || !image_cache_use_mipmaps()
        {
            return;
        }
        let level0 = std::mem::take(&mut self.data);
        let (data, max_level) = generate_bgra_mip_chain(self.width, self.height, level0);
        self.data = data;
        self.max_level = Some(max_level);
    }

    pub fn into_new_texture(mut self, cx: &mut Cx) -> Texture {
        // Mipmap static images (not animations — frames must not bleed across mip levels) to avoid aliasing when minified.
        let format = if self.animation.is_none() && image_cache_use_mipmaps() {
            self.build_cpu_mip_chain_if_uploaded();
            let max_level = self
                .max_level
                .unwrap_or_else(|| mip_chain_max_level(self.width, self.height));
            TextureFormat::VecMipBGRAu8_32 {
                width: self.width,
                height: self.height,
                data: Some(self.data),
                max_level: Some(max_level),
                wrap: TextureWrap::ClampToEdge,
                updated: TextureUpdated::Full,
            }
        } else {
            TextureFormat::VecBGRAu8_32 {
                width: self.width,
                height: self.height,
                data: Some(self.data),
                updated: TextureUpdated::Full,
            }
        };
        let texture = Texture::new_with_format(cx, format);
        texture.set_animation(cx, self.animation);
        texture.set_natural_size(cx, self.natural_size);
        texture
    }

    /// Always upload a mip chain (unlike [`into_new_texture`], which is gated
    /// by `MAKEPAD_IMAGE_MIPMAPS` / OpenGL-only). World / statue albedo uses
    /// this so distant tiling surfaces trilinear-filter instead of sparkling.
    pub fn into_new_mip_texture(self, cx: &mut Cx) -> Texture {
        self.into_new_mip_texture_wrap(cx, TextureWrap::ClampToEdge)
    }

    /// Same as [`into_new_mip_texture`], with REPEAT wrap so UVs outside 0..1
    /// tile. Pair with `sample_as_bgra_repeat` (not `fract(uv)`) so mip LOD
    /// stays valid across wrap seams.
    pub fn into_new_mip_repeat_texture(self, cx: &mut Cx) -> Texture {
        self.into_new_mip_texture_wrap(cx, TextureWrap::Repeat)
    }

    fn into_new_mip_texture_wrap(mut self, cx: &mut Cx, wrap: TextureWrap) -> Texture {
        if self.animation.is_some() {
            return self.into_new_texture(cx);
        }
        if self.max_level.is_none() && backend_uploads_cpu_mip_chain() {
            let level0 = std::mem::take(&mut self.data);
            let (data, max_level) = generate_bgra_mip_chain(self.width, self.height, level0);
            self.data = data;
            self.max_level = Some(max_level);
        }
        let max_level = self
            .max_level
            .unwrap_or_else(|| mip_chain_max_level(self.width, self.height));
        Texture::new_with_format(
            cx,
            TextureFormat::VecMipBGRAu8_32 {
                width: self.width,
                height: self.height,
                data: Some(self.data),
                max_level: Some(max_level),
                wrap,
                updated: TextureUpdated::Full,
            },
        )
    }

    pub fn from_png(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        // 16-bit PNGs (e.g. HDR iPhone screenshots) decode as u16 and fail our u8 path; strip to 8-bit.
        let mut decoder = PngDecoder::new_with_options(cursor, png_decoder_options());
        decoder.decode_headers()?;
        let (width, height) =
            decoder
                .dimensions()
                .ok_or(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Failed to get PNG image dimensions",
                )))?;
        checked_pixel_count(width, height)?;

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
        let buffer = Self::new(&decoded_data, width, height)?;
        Ok(orient(buffer, decoder.info().and_then(|i| i.exif.as_deref())))
    }

    /// Decodes an animated PNG by drawing each frame onto a canvas the way browsers do
    /// (blending it, then disposing of it), and packs those into an atlas like GIF/WebP.
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
        let num_components = colorspace.num_components();
        let frame_pixels = checked_pixel_count(width, height)?;
        let max_frames = (MAX_IMAGE_PIXELS / frame_pixels).max(1).min(MAX_IMAGE_FRAMES);
        let mut canvas = vec![0u8; frame_pixels * IMAGE_RGBA_BYTES_PER_PIXEL];
        let mut frames = Vec::new();
        let mut frame_delays = Vec::new();

        while decoder.more_frames() {
            decoder.decode_headers()?;
            let frame = decoder.frame_info().ok_or(ImageError::PngDecode(
                PngDecodeErrors::GenericStatic("Failed to get animated PNG frame info"),
            ))?;
            let pixels = decoder.decode_raw()?;
            // A default image that isn't part of the animation is only there for viewers without APNG support.
            if !frame.is_part_of_seq {
                continue;
            }
            if frames.len() >= max_frames {
                return Err(ImageError::DimensionsTooLarge { width, height });
            }
            if frame.width == 0
                || frame.height == 0
                || frame.x_offset + frame.width > width
                || frame.y_offset + frame.height > height
                || pixels.len() < frame.width * frame.height * num_components
            {
                return Err(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                    "Animated PNG frame doesn't fit within the image",
                )));
            }
            let frame_rgba = match num_components {
                4 => pixels,
                3 => rgb_to_rgba(&pixels),
                2 => pixels.chunks_exact(2).flat_map(|la| [la[0], la[0], la[0], la[1]]).collect(),
                1 => pixels.iter().flat_map(|&l| [l, l, l, 0xFF]).collect(),
                unsupported => return Err(ImageError::InvalidPixelAlignment(unsupported)),
            };
            // A first frame that asks to be disposed to the "previous" frame is disposed to the background instead.
            let previous_canvas = (frame.dispose_op == DisposeOp::Previous && !frames.is_empty())
                .then(|| canvas.clone());
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

        match frames.len() {
            0 => Err(ImageError::PngDecode(PngDecodeErrors::GenericStatic(
                "Animated PNG had no frames",
            ))),
            1 => Self::new(&frames[0], width, height),
            _ => Self::pack_animation_atlas(frames, frame_delays, width, height),
        }
    }

    pub fn from_webp(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = std::io::Cursor::new(data);
        let mut decoder =
            WebPDecoder::new(std::io::BufReader::new(cursor)).map_err(ImageError::WebpDecode)?;
        decoder.set_memory_limit(MAX_IMAGE_PIXELS * 4);
        let (width, height) = decoder.dimensions();
        let (width, height) = (width as usize, height as usize);
        let frame_pixels = checked_pixel_count(width, height)?;
        let buf_size = decoder
            .output_buffer_size()
            .filter(|&len| len <= MAX_IMAGE_DECODED_BYTES)
            .ok_or(ImageError::WebpDecode(WebpDecodeErrors::ImageTooLarge))?;

        if !decoder.is_animated() {
            let mut buf = vec![0u8; buf_size];
            decoder.read_image(&mut buf).map_err(ImageError::WebpDecode)?;
            let buffer = Self::new(&buf, width, height)?;
            let exif = decoder.exif_metadata().ok().flatten();
            return Ok(orient(buffer, exif.as_deref()));
        }

        // Animated WebP: the decoder composites each frame onto its own canvas,
        // so we just collect each composited frame (as RGBA) and its delay, then
        // pack them into an animation atlas like GIF/APNG.
        let has_alpha = decoder.has_alpha();
        let max_frames = (MAX_IMAGE_PIXELS / frame_pixels).max(1).min(MAX_IMAGE_FRAMES);
        let num_frames = decoder.num_frames() as usize;
        if num_frames > max_frames {
            return Err(ImageError::DimensionsTooLarge { width, height });
        }
        // Browsers ignore the file's background color and clear disposed frames to transparent.
        // Without a background color, the decoder never clears them at all.
        decoder
            .set_background_color([0, 0, 0, 0])
            .map_err(ImageError::WebpDecode)?;
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
                    frame_delays
                        .push(if delay_ms == 0 { 0.1 } else { f64::from(delay_ms) * 0.001 });
                    frames.push(if has_alpha { buf } else { rgb_to_rgba(&buf) });
                }
                Err(WebpDecodeErrors::NoMoreFrames) => break,
                Err(err) => return Err(ImageError::WebpDecode(err)),
            }
        }

        if frames.len() >= 2 {
            Self::pack_animation_atlas(frames, frame_delays, width, height)
        } else if let Some(frame) = frames.first() {
            Self::new(frame, width, height)
        } else {
            Err(ImageError::WebpDecode(WebpDecodeErrors::NoMoreFrames))
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
        let frame_pixels = checked_pixel_count(width, height)?;
        let max_frames = (MAX_IMAGE_PIXELS / frame_pixels).max(1).min(MAX_IMAGE_FRAMES);
        let mut frames = Vec::new();
        let mut frame_delays = Vec::new();
        let mut canvas = vec![0u8; frame_pixels * IMAGE_RGBA_BYTES_PER_PIXEL];

        while let Some(frame) = decoder.read_next_frame().map_err(ImageError::GifDecode)? {
            if frames.len() >= max_frames {
                return Err(ImageError::DimensionsTooLarge { width, height });
            }
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
        Self::pack_animation_atlas(frames, frame_delays, width, height)
    }

    /// Packs multi-frame animation `frames` (each a full-canvas RGBA buffer of
    /// `width`x`height`) into a single horizontal-grid atlas texture carrying the
    /// per-frame `frame_delays` as a [`TextureAnimation`].
    fn pack_animation_atlas(
        frames: Vec<Vec<u8>>,
        frame_delays: Vec<f64>,
        width: usize,
        height: usize,
    ) -> Result<ImageBuffer, ImageError> {
        let (total_width, total_height) = animation_atlas_layout(frames.len(), width, height)?;
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
        Ok(final_buffer)
    }

    /// Shrinks this image (or each frame of this animation) to as few pixels as it
    /// needs to be drawn `drawn_size` pixels big, by halving it like a mip level.
    fn shrink_to_drawn_size(self, drawn_size: (usize, usize)) -> Result<ImageBuffer, ImageError> {
        let frame_size = self.animation.as_ref()
            .map_or((self.width, self.height), |animation| (animation.width, animation.height));
        let halvings = count_halvings(frame_size, drawn_size);
        if halvings == 0 {
            return Ok(self);
        }
        let shrunk_size = get_decoded_size(frame_size, drawn_size);
        let shrink_frame = |first_texel: usize| {
            let mut texels = halve_texels(&self.data[first_texel..], self.width, frame_size);
            let mut size = halve_size(frame_size);
            for _ in 1..halvings {
                texels = halve_texels(&texels, size.0, size);
                size = halve_size(size);
            }
            texels
        };
        let Some(animation) = &self.animation else {
            return Ok(ImageBuffer {
                width: shrunk_size.0,
                height: shrunk_size.1,
                data: shrink_frame(0),
                animation: None,
                max_level: None,
                natural_size: Some(frame_size),
            });
        };

        // Pack the shrunk frames into a smaller atlas, in the same order.
        let (atlas_width, atlas_height) =
            animation_atlas_layout(animation.num_frames, shrunk_size.0, shrunk_size.1)?;
        let columns = self.width / frame_size.0;
        let shrunk_columns = atlas_width / shrunk_size.0;
        let mut data = vec![0; atlas_width * atlas_height];
        for frame in 0..animation.num_frames {
            let first_texel = (frame / columns) * frame_size.1 * self.width + (frame % columns) * frame_size.0;
            let shrunk_first_texel = (frame / shrunk_columns) * shrunk_size.1 * atlas_width
                + (frame % shrunk_columns) * shrunk_size.0;
            for (y, row) in shrink_frame(first_texel).chunks_exact(shrunk_size.0).enumerate() {
                let start = shrunk_first_texel + y * atlas_width;
                data[start..start + shrunk_size.0].copy_from_slice(row);
            }
        }
        Ok(ImageBuffer {
            width: atlas_width,
            height: atlas_height,
            data,
            animation: Some(TextureAnimation {
                width: shrunk_size.0,
                height: shrunk_size.1,
                ..animation.clone()
            }),
            max_level: None,
            natural_size: Some(frame_size),
        })
    }

    pub fn from_jpg(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        let mut decoder = JpegDecoder::new_with_options(cursor, decoder_options());
        decoder.decode_headers().map_err(ImageError::JpgDecode)?;
        let (width, height) = decoder.dimensions().ok_or(ImageError::JpgDecode(
            JpgDecodeErrors::FormatStatic("Failed to decode JPG image dimensions"),
        ))?;
        checked_pixel_count(width, height)?;
        let pixels = decoder.decode().map_err(ImageError::JpgDecode)?;
        let buffer = ImageBuffer::new(&pixels, width, height)?;
        Ok(orient(buffer, decoder.exif().map(Vec::as_slice)))
    }

    pub fn from_bmp(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        let mut decoder = BmpDecoder::new_with_options(cursor, decoder_options());
        decoder.decode_headers().map_err(ImageError::BmpDecode)?;
        let (width, height) =
            decoder
                .dimensions()
                .ok_or(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic(
                    "Failed to get BMP image dimensions",
                )))?;
        checked_pixel_count(width, height)?;
        let pixels = decoder.decode().map_err(ImageError::BmpDecode)?;
        Self::new(&pixels, width, height)
    }

    pub fn from_qoi(data: &[u8]) -> Result<Self, ImageError> {
        let cursor = ZCursor::new(data);
        let mut decoder = QoiDecoder::new_with_options(cursor, decoder_options());
        decoder.decode_headers().map_err(ImageError::QoiDecode)?;
        let (width, height) =
            decoder
                .get_dimensions()
                .ok_or(ImageError::QoiDecode(QoiErrors::GenericStatic(
                    "Failed to get QOI image dimensions",
                )))?;
        checked_pixel_count(width, height)?;
        let pixels = decoder.decode().map_err(ImageError::QoiDecode)?;
        Self::new(&pixels, width, height)
    }

    pub fn from_ico(data: &[u8]) -> Result<Self, ImageError> {
        let (offset, len, _width, height) =
            ico_best_entry(data).ok_or(ImageError::UnsupportedFormat)?;
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
    /// Decodes staged newest-first in front of the runtime pool's light
    /// lane; a re-request of the same path replaces the staged decode.
    pub decode_queue: TaskQueue<PathBuf>,
    pub pending_http_requests: HashMap<LiveId, PathBuf>,
    /// The decode that's on its way for each image being decoded,
    /// so a request it has enough pixels for just waits for it.
    pending_decodes: HashMap<PathBuf, PendingDecode>,
    /// Images evicted while a decode of them was on its way. That decode still gets cached when
    /// it lands (for anything waiting on it), and then evicted when the next decode lands.
    evicted_while_decoding: HashSet<PathBuf>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    /// Max distinct cached images before eviction kicks in. Each `Loaded` entry can hold a
    /// decoded GPU texture, so without a cap this `HashMap` grows for the process lifetime.
    const MAX_ENTRIES: usize = 512;

    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            decode_queue: TaskQueue::new(Lane::Light, MAX_POOL_WORKERS, 1024),
            pending_http_requests: HashMap::new(),
            pending_decodes: HashMap::new(),
            evicted_while_decoding: HashSet::new(),
        }
    }

    /// Insert a freshly-loaded texture, then bound the cache size if it has grown too large.
    pub fn insert_loaded(&mut self, image_path: PathBuf, texture: Texture) {
        self.map.insert(image_path, ImageCacheEntry::Loaded(texture));
        self.evict_loaded_if_oversized();
    }

    /// Forget one path, returning the entry it held (`None` if it held none).
    ///
    /// The size cap below counts ENTRIES, not bytes: 512 thumbnails and 512
    /// forty-megapixel photographs look the same to it, and the second costs
    /// gigabytes. An app that knows it is done with an image — a viewer whose
    /// panel just closed, say — hands it back here rather than waiting for a
    /// cap that may never be reached. Prefer [`evict_image_from_cache`],
    /// which also releases the pixels behind the entry.
    pub fn evict(&mut self, image_path: &Path) -> Option<ImageCacheEntry> {
        if self.pending_decodes.contains_key(image_path) {
            self.evicted_while_decoding.insert(image_path.into());
        }
        self.map.remove(image_path)
    }

    /// Drop `Loaded` entries once the cache exceeds its cap. This is safe because widgets keep
    /// their own clone of any texture they're currently displaying (via `set_texture`), so
    /// eviction never affects a visible image — a later *fresh* request for an evicted path
    /// simply re-loads it. In-flight `Loading` entries are preserved so decode work isn't
    /// orphaned. There is no per-entry access timestamp, so eviction order is unspecified; the
    /// cap is generous enough that this rarely triggers in practice.
    fn evict_loaded_if_oversized(&mut self) {
        if self.map.len() <= Self::MAX_ENTRIES {
            return;
        }
        let excess = self.map.len() - (Self::MAX_ENTRIES * 3 / 4);
        let to_remove: Vec<PathBuf> = self
            .map
            .iter()
            .filter(|(_, e)| matches!(e, ImageCacheEntry::Loaded(_)))
            .map(|(k, _)| k.clone())
            .take(excess)
            .collect();
        for k in to_remove {
            self.map.remove(&k);
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
    DimensionsTooLarge { width: usize, height: usize },
    DataTooLarge { bytes: usize, limit: usize },
    UnsupportedFormat,
    Http(String),
    Worker(String),
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
fn decode_timing_start() -> Option<f64> {
    if !image_decode_debug_enabled() {
        return None;
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(Cx::monotonic_now())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn gpusim_mode_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MAKEPAD")
            .map(|value| value.eq_ignore_ascii_case("gpusim"))
            .unwrap_or(false)
    })
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
        let len =
            u32::from_le_bytes([data[e + 8], data[e + 9], data[e + 10], data[e + 11]]) as usize;
        let offset =
            u32::from_le_bytes([data[e + 12], data[e + 13], data[e + 14], data[e + 15]]) as usize;
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
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic(
            "ICO DIB header too small",
        )));
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
    if header_size < 40 || header_size > dib.len() {
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic(
            "ICO DIB header has invalid size",
        )));
    }
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]) as u32;
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    let mut clr_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;
    if bit_count <= 8 && clr_used == 0 {
        clr_used = 1usize << bit_count;
    }
    if bit_count <= 8 && clr_used > (1usize << bit_count) {
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic(
            "ICO DIB palette is too large",
        )));
    }
    let palette_bytes = if bit_count <= 8 {
        clr_used
            .checked_mul(4)
            .ok_or(ImageError::DimensionsTooLarge {
                width: dib.len(),
                height: 1,
            })?
    } else {
        0
    };
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
        .ok_or(ImageError::DimensionsTooLarge {
            width: dib.len(),
            height: 1,
        })?;
    let file_size = 14usize
        .checked_add(dib.len())
        .filter(|&size| size <= u32::MAX as usize)
        .ok_or(ImageError::DimensionsTooLarge {
            width: dib.len(),
            height: 1,
        })?;
    if data_offset > file_size {
        return Err(ImageError::BmpDecode(BmpDecoderErrors::GenericStatic(
            "ICO DIB pixel data offset exceeds payload",
        )));
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
    decode_image_buffer(Path::new(""), data, None)
}

/// Returns true if `data` looks like an SVG document (vs. a raster image).
///
/// SVG is a vector format with no magic bytes, so this sniffs for an `<svg>`
/// root element, optionally preceded by an XML prolog, DOCTYPE, or comment.
pub fn looks_like_svg(data: &[u8]) -> bool {
    let data = &data[..data.len().min(MAX_SVG_SNIFF_BYTES)];
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

/// Decodes `data` with just enough pixels to be drawn `drawn_size` pixels big,
/// or with all of them if that's `None`.
fn decode_image_buffer(
    image_path: &Path,
    data: &[u8],
    drawn_size: Option<(usize, usize)>,
) -> Result<ImageBuffer, ImageError> {
    if data.len() > MAX_IMAGE_DECODED_BYTES {
        return Err(ImageError::DataTooLarge {
            bytes: data.len(),
            limit: MAX_IMAGE_DECODED_BYTES,
        });
    }
    let format = detect_image_format_from_path_and_data(image_path, data)
        .ok_or(ImageError::UnsupportedFormat)?;
    let buffer = match format {
        "jpg" => ImageBuffer::from_jpg(data),
        "png" => ImageBuffer::from_png(data),
        "webp" => ImageBuffer::from_webp(data),
        "gif" => ImageBuffer::from_gif(data),
        "bmp" => ImageBuffer::from_bmp(data),
        "qoi" => ImageBuffer::from_qoi(data),
        "ico" => ImageBuffer::from_ico(data),
        _ => Err(ImageError::UnsupportedFormat),
    }?;
    match drawn_size {
        Some(drawn_size) => buffer.shrink_to_drawn_size(drawn_size),
        None => Ok(buffer),
    }
}

/// Applies the EXIF orientation found in an already-decoded image's raw `exif`
/// TIFF block (as handed back by the format decoders) so the pixels display
/// upright. Returns the buffer unchanged when there's no usable orientation tag.
fn orient(buffer: ImageBuffer, exif: Option<&[u8]>) -> ImageBuffer {
    apply_exif_orientation(buffer, exif.and_then(tiff_orientation).unwrap_or(1))
}

/// Returns the size of an image that's `width` x `height` once its EXIF orientation is applied.
fn get_oriented_size((width, height): (usize, usize), exif: Option<&[u8]>) -> (usize, usize) {
    match exif.and_then(tiff_orientation) {
        Some(5..=8) => (height, width),
        _ => (width, height),
    }
}

/// Reads the Orientation tag (0x0112) from a TIFF/EXIF block that begins at the
/// byte-order marker ("II" little-endian or "MM" big-endian). Returns the value
/// only when it's a valid 1-8.
fn tiff_orientation(tiff: &[u8]) -> Option<u16> {
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
        Some(if little_endian {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |off: usize| -> Option<u32> {
        let b = tiff.get(off..off.checked_add(4)?)?;
        Some(if little_endian {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
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

/// Rotates/flips a decoded buffer per its EXIF `orientation` (1-8) so it displays
/// upright. Orientation 1 (or anything out of range, or an animation atlas) is
/// returned unchanged.
fn apply_exif_orientation(buf: ImageBuffer, orientation: u16) -> ImageBuffer {
    if orientation <= 1 || orientation > 8 || buf.animation.is_some() {
        return buf;
    }
    let (w, h) = (buf.width, buf.height);
    if buf.data.len() < w * h {
        return buf;
    }
    // Orientations 5-8 are 90°/diagonal turns, which swap width and height.
    let swap = matches!(orientation, 5..=8);
    let (dw, dh) = if swap { (h, w) } else { (w, h) };
    let mut dst = vec![0u32; dw * dh];
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
            dst[dy * dw + dx] = buf.data[y * w + x];
        }
    }
    ImageBuffer { width: dw, height: dh, data: dst, animation: buf.animation, max_level: None, natural_size: None }
}

/// Returns the `(width, height)` in pixels of an encoded image as it's shown (after its EXIF orientation),
/// auto-detecting the format from the data's magic bytes (falling back to the path's extension).
/// Reads only headers for most formats; does not fully decode.
pub fn image_size_by_data(data: &[u8], image_path: &Path) -> Result<(usize, usize), ImageError> {
    let format = detect_image_format_from_path_and_data(image_path, data)
        .ok_or(ImageError::UnsupportedFormat)?;
    match format {
        "jpg" => {
            let cursor = ZCursor::new(data);
            let mut decoder = JpegDecoder::new_with_options(cursor, decoder_options());
            decoder.decode_headers().map_err(ImageError::JpgDecode)?;
            let (width, height) = decoder.dimensions().ok_or({
                ImageError::JpgDecode(JpgDecodeErrors::FormatStatic(
                    "Failed to get JPG image dimensions after decoding headers",
                ))
            })?;
            checked_pixel_count(width, height)?;
            Ok(get_oriented_size((width, height), decoder.exif().map(Vec::as_slice)))
        }
        "png" => {
            let cursor = ZCursor::new(data);
            let mut decoder = PngDecoder::new_with_options(cursor, png_decoder_options());
            decoder.decode_headers()?;
            let (width, height) = decoder.dimensions().ok_or(ImageError::PngDecode(
                PngDecodeErrors::GenericStatic("Failed to get PNG image dimensions"),
            ))?;
            checked_pixel_count(width, height)?;
            // Animations are never rotated, see `apply_exif_orientation`.
            let exif = if decoder.is_animated() { None } else { decoder.info().and_then(|i| i.exif.as_deref()) };
            Ok(get_oriented_size((width, height), exif))
        }
        "webp" => {
            let cursor = std::io::Cursor::new(data);
            let mut decoder = WebPDecoder::new(std::io::BufReader::new(cursor))
                .map_err(ImageError::WebpDecode)?;
            let (width, height) = decoder.dimensions();
            let (width, height) = (width as usize, height as usize);
            checked_pixel_count(width, height)?;
            let exif = if decoder.is_animated() { None } else { decoder.exif_metadata().ok().flatten() };
            Ok(get_oriented_size((width, height), exif.as_deref()))
        }
        "gif" => {
            let decoder = DecodeOptions::new()
                .read_info(std::io::Cursor::new(data))
                .map_err(ImageError::GifDecode)?;
            let width = decoder.width() as usize;
            let height = decoder.height() as usize;
            checked_pixel_count(width, height)?;
            Ok((width, height))
        }
        "bmp" => {
            let cursor = ZCursor::new(data);
            let mut decoder = BmpDecoder::new_with_options(cursor, decoder_options());
            decoder.decode_headers().map_err(ImageError::BmpDecode)?;
            let (width, height) = decoder.dimensions().ok_or(ImageError::BmpDecode(
                BmpDecoderErrors::GenericStatic("Failed to get BMP image dimensions"),
            ))?;
            checked_pixel_count(width, height)?;
            Ok((width, height))
        }
        "qoi" => {
            let cursor = ZCursor::new(data);
            let mut decoder = QoiDecoder::new_with_options(cursor, decoder_options());
            decoder.decode_headers().map_err(ImageError::QoiDecode)?;
            let (width, height) = decoder.get_dimensions().ok_or(ImageError::QoiDecode(
                QoiErrors::GenericStatic("Failed to get QOI image dimensions"),
            ))?;
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

    #[test]
    fn web_decoded_image_policy_boundaries() {
        let boundary_pixels = 4096 * 4096;
        assert_eq!(
            checked_pixel_count_with_limit(4096, 4096, WEB_MAX_IMAGE_DECODED_BYTES).unwrap(),
            boundary_pixels
        );
        assert!(checked_rgba_byte_len(
            boundary_pixels + 1,
            WEB_MAX_IMAGE_DECODED_BYTES
        )
        .is_none());
        assert!(checked_rgba_byte_len(usize::MAX, WEB_MAX_IMAGE_DECODED_BYTES).is_none());
        assert_eq!(
            checked_pixel_count_with_limit(2048, 2560, WEB_MAX_IMAGE_DECODED_BYTES).unwrap(),
            2048 * 2560
        );

        assert!(animation_atlas_layout_with_limit(
            16,
            1024,
            1024,
            WEB_MAX_IMAGE_DECODED_BYTES
        )
        .is_ok());
        assert!(animation_atlas_layout_with_limit(
            17,
            1024,
            1024,
            WEB_MAX_IMAGE_DECODED_BYTES
        )
        .is_err());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_decoded_image_policy_remains_one_gibibyte() {
        assert_eq!(MAX_IMAGE_DECODED_BYTES, NATIVE_MAX_IMAGE_DECODED_BYTES);
    }

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
    fn test_mip_chain_max_level_matches_generated_chain() {
        for (w, h) in [(1, 1), (2, 1), (3, 2), (5, 9), (16, 16), (300, 200)] {
            let (_, max_level) = generate_bgra_mip_chain(w, h, vec![0u32; w * h]);
            assert_eq!(max_level, mip_chain_max_level(w, h), "dims {w}x{h}");
        }
    }

    #[test]
    fn test_mip_chain_is_longer_than_level0() {
        let (data, max_level) = generate_bgra_mip_chain(4, 4, vec![0xFF00_00FFu32; 16]);
        assert_eq!(max_level, 2);
        // 4x4 + 2x2 + 1x1
        assert_eq!(data.len(), 16 + 4 + 1);
    }

    #[test]
    fn evict_removes_one_path_and_leaves_the_rest() {
        // `Loading` entries need no Cx, and eviction is the same map
        // operation either way.
        let mut cache = ImageCache::new();
        for name in ["a.png", "b.png", "c.png"] {
            cache
                .map
                .insert(PathBuf::from(name), ImageCacheEntry::Loading(4, 4));
        }
        assert!(cache.evict(Path::new("b.png")).is_some());
        assert_eq!(cache.map.len(), 2);
        assert!(cache.map.contains_key(Path::new("a.png")));
        assert!(cache.map.contains_key(Path::new("c.png")));
        // Evicting the same path twice, or one nobody cached, is not an
        // error — a viewer hands its list back without bookkeeping.
        assert!(cache.evict(Path::new("b.png")).is_none());
        assert!(cache.evict(Path::new("never-loaded.png")).is_none());
        assert_eq!(cache.map.len(), 2);
    }

    #[test]
    fn the_entry_cap_still_only_bites_past_512() {
        // The global behaviour is unchanged by eviction: nothing is shed
        // until MAX_ENTRIES is exceeded, and then only `Loaded` entries.
        let mut cache = ImageCache::new();
        for i in 0..ImageCache::MAX_ENTRIES {
            cache
                .map
                .insert(PathBuf::from(format!("{i}.png")), ImageCacheEntry::Loading(1, 1));
        }
        cache.evict_loaded_if_oversized();
        assert_eq!(cache.map.len(), ImageCache::MAX_ENTRIES);
        // Over the cap, but every entry is an in-flight decode: dropping
        // those would orphan the work, so the map is left alone.
        cache
            .map
            .insert(PathBuf::from("one-too-many.png"), ImageCacheEntry::Loading(1, 1));
        cache.evict_loaded_if_oversized();
        assert_eq!(cache.map.len(), ImageCache::MAX_ENTRIES + 1);
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
        // Four frames fit in one row, so the atlas is no wider than that.
        assert_eq!((image.width, image.height), (2 * 4, 2));
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
    fn test_from_png_animated_reads_frame_delays() {
        let image = ImageBuffer::from_png(&animated_png()).unwrap();
        let animation = image.animation.as_ref().unwrap();
        assert_eq!(animation.num_frames, 4);
        assert_eq!(animation.frame_delays, vec![0.05; 4]);
    }

    #[test]
    fn test_animation_atlas_layout_splits_frames_evenly_across_rows() {
        let max_columns = Cx::max_texture_width() / 10;
        let layout = animation_atlas_layout(max_columns + 2, 10, 10).unwrap();
        assert_eq!(layout, ((max_columns + 2).div_ceil(2) * 10, 2 * 10));
    }

    #[test]
    fn test_is_animated_image() {
        assert!(is_animated_image(&animated_gif()));
        assert!(!is_animated_image(&single_frame_gif()));
        assert!(is_animated_image(&animated_png()));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        assert!(!is_animated_image(&webp));
        webp[20] = 0x02;
        assert!(is_animated_image(&webp));
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
            decode_image_buffer(Path::new("sticker"), &[0; 16], None),
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
            let image = decode_image_buffer(Path::new("img"), &data, None).unwrap();
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

    #[test]
    fn decoded_size_halves_while_it_still_has_enough_pixels() {
        assert_eq!(get_decoded_size((4032, 3024), (800, 600)), (1008, 756));
        assert_eq!(get_decoded_size((4032, 3024), (1009, 600)), (2016, 1512));
        assert_eq!(get_decoded_size((400, 300), (800, 600)), (400, 300));
        // Odd sizes round up, so no pixel row or column gets dropped.
        assert_eq!(get_decoded_size((5, 3), (1, 1)), (1, 1));
        assert_eq!(get_decoded_size((5, 3), (2, 2)), (3, 2));
        assert_eq!(get_decoded_size((1, 1), (1, 1)), (1, 1));
    }

    #[test]
    fn shrinking_averages_by_alpha_and_keeps_the_natural_size() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let clear = [0, 0, 0, 0];
        let rows = [[red, red, blue, blue], [red, red, blue, blue], [clear, red, clear, clear]];
        let data: Vec<u8> = rows.iter().flatten().flatten().copied().collect();
        let image = ImageBuffer::new(&data, 4, 3).unwrap().shrink_to_drawn_size((2, 2)).unwrap();
        assert_eq!((image.width, image.height, image.natural_size), (2, 2, Some((4, 3))));
        // A transparent pixel lowers the alpha but doesn't darken the color.
        assert_eq!(image.data, vec![0xffff0000, 0xff0000ff, 0x7fff0000, 0x00000000]);

        let image = ImageBuffer::new(&data, 4, 3).unwrap().shrink_to_drawn_size((3, 3)).unwrap();
        assert_eq!((image.width, image.height, image.natural_size), (4, 3, None));
    }

    #[test]
    fn shrinking_an_animation_shrinks_each_frame() {
        let image = decode_image_buffer(Path::new("a.gif"), &animated_gif(), Some((1, 1))).unwrap();
        let animation = image.animation.as_ref().unwrap();
        assert_eq!((animation.width, animation.height, animation.num_frames), (1, 1, 4));
        assert_eq!(animation.frame_delays, vec![0.05; 4]);
        assert_eq!(image.natural_size, Some((2, 2)));
        assert_eq!((image.width, image.height), (4, 1));
        // Each frame's one pixel averages two pixels of one color and two of the next one.
        assert_eq!(image.data, vec![0xff7f0000, 0xff7f7f00, 0xff007f7f, 0xff00007f]);
    }

    #[test]
    fn a_pending_decode_covers_requests_that_decode_to_the_same_size() {
        let pending_decode = PendingDecode { natural_size: (4032, 3024), drawn_size: Some((1000, 700)) };
        assert!(pending_decode.is_big_enough_for(Some((1005, 755))));
        assert!(!pending_decode.is_big_enough_for(Some((1009, 757))));
        assert!(!pending_decode.is_big_enough_for(None));
        let pending_decode = PendingDecode { natural_size: (4032, 3024), drawn_size: None };
        assert!(pending_decode.is_big_enough_for(Some((4000, 3000))));
        assert!(pending_decode.is_big_enough_for(None));
    }

    #[test]
    fn oriented_size_swaps_for_turning_exif_orientations() {
        let exif = |orientation: u8| {
            let mut tiff = b"II\x2a\0\x08\0\0\0\x01\0".to_vec();
            tiff.extend_from_slice(&[0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, orientation, 0, 0, 0]);
            tiff
        };
        assert_eq!(get_oriented_size((4, 3), Some(&exif(6))), (3, 4));
        assert_eq!(get_oriented_size((4, 3), Some(&exif(8))), (3, 4));
        assert_eq!(get_oriented_size((4, 3), Some(&exif(3))), (4, 3));
        assert_eq!(get_oriented_size((4, 3), None), (4, 3));
    }

    #[test]
    fn only_a_decode_with_enough_pixels_counts_as_cached() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        ensure_image_cache(&mut cx);
        let is_loaded = |result: Option<AsyncLoadResult>| matches!(result, Some(AsyncLoadResult::Loaded));
        let loading_size = |result: Option<AsyncLoadResult>| match result {
            Some(AsyncLoadResult::Loading(w, h)) => Some((w, h)),
            _ => None,
        };

        // An image that's still being fetched gets all of its pixels.
        let fetched = Path::new("https://example.com/photo.png");
        cx.get_global::<ImageCache>().map.insert(fetched.into(), ImageCacheEntry::Loading(1, 1));
        assert_eq!(loading_size(get_cached_load_result(&mut cx, fetched, None)), Some((1, 1)));

        let path = Path::new("photo.png");
        let mut shrunk = ImageBuffer::new(&[0; 2 * 2 * 4], 2, 2).unwrap();
        shrunk.natural_size = Some((8, 8));
        let texture = shrunk.into_new_texture(&mut cx);
        cx.get_global::<ImageCache>().insert_loaded(path.into(), texture);
        assert!(is_loaded(get_cached_load_result(&mut cx, path, Some((2, 2)))));
        assert!(get_cached_load_result(&mut cx, path, Some((3, 3))).is_none());
        assert!(get_cached_load_result(&mut cx, path, None).is_none());

        // A request that a decode on its way covers waits for it.
        cx.get_global::<ImageCache>()
            .pending_decodes
            .insert(path.into(), PendingDecode { natural_size: (8, 8), drawn_size: Some((4, 4)) });
        assert_eq!(loading_size(get_cached_load_result(&mut cx, path, Some((3, 3)))), Some((8, 8)));
        assert!(get_cached_load_result(&mut cx, path, Some((5, 5))).is_none());
    }

    #[test]
    fn an_image_evicted_while_decoding_goes_once_its_decode_lands() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        ensure_image_cache(&mut cx);
        let (path, other_path) = (Path::new("photo.png"), Path::new("other.png"));
        let decoded = || ImageBuffer::new(&[0; 2 * 2 * 4], 2, 2).unwrap();
        for path in [path, other_path] {
            let cache = cx.get_global::<ImageCache>();
            cache.map.insert(path.into(), ImageCacheEntry::Loading(2, 2));
            cache.pending_decodes.insert(path.into(), PendingDecode { natural_size: (2, 2), drawn_size: None });
        }
        assert!(evict_image_from_cache(&mut cx, path));
        // Its decode still lands in the cache, for anything that's waiting on it.
        process_async_image_load(&mut cx, path, Ok(decoded()));
        assert!(load_image_from_cache(&mut cx, path).is_some());
        process_async_image_load(&mut cx, other_path, Ok(decoded()));
        assert!(load_image_from_cache(&mut cx, path).is_none());
        assert!(load_image_from_cache(&mut cx, other_path).is_some());
    }

    #[test]
    fn a_cached_image_with_more_pixels_is_kept() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        ensure_image_cache(&mut cx);
        let path = Path::new("photo.png");
        let shrunk = |pixels: usize| {
            let mut image = ImageBuffer::new(&vec![0; pixels * pixels * 4], pixels, pixels).unwrap();
            image.natural_size = Some((8, 8));
            image
        };
        let cached_size = |cx: &mut Cx| {
            load_image_from_cache(cx, path).and_then(|texture| texture.get_format(cx).vec_width_height())
        };
        cx.get_global::<ImageCache>()
            .pending_decodes
            .insert(path.into(), PendingDecode { natural_size: (8, 8), drawn_size: Some((4, 4)) });
        process_async_image_load(&mut cx, path, Ok(shrunk(2)));
        assert_eq!(cached_size(&mut cx), Some((2, 2)));
        // A bigger decode is still on its way.
        assert!(is_decoding_image(&mut cx, path, Some((4, 4))));

        process_async_image_load(&mut cx, path, Ok(shrunk(4)));
        assert_eq!(cached_size(&mut cx), Some((4, 4)));
        assert!(!is_decoding_image(&mut cx, path, Some((4, 4))));
        let texture = load_image_from_cache(&mut cx, path).unwrap();
        assert!(has_enough_pixels(&mut cx, &texture, Some((4, 4))));
        assert!(!has_enough_pixels(&mut cx, &texture, Some((5, 5))));
        assert!(!has_enough_pixels(&mut cx, &texture, None));

        // One with fewer pixels that lands later doesn't replace it.
        process_async_image_load(&mut cx, path, Ok(shrunk(2)));
        assert_eq!(cached_size(&mut cx), Some((4, 4)));
        // A failed decode doesn't throw the cached one out either.
        process_async_image_load(&mut cx, path, Err(ImageError::EmptyData));
        assert_eq!(cached_size(&mut cx), Some((4, 4)));
    }
}

fn spawn_decode_job<D>(
    cx: &mut Cx,
    image_path: PathBuf,
    data: Arc<D>,
    drawn_size: Option<(usize, usize)>,
)
where
    D: AsRef<[u8]> + Send + Sync + ?Sized + 'static,
{
    ensure_image_cache_inner(cx);
    let image_size_bytes = (*data).as_ref().len();
    let task_key = image_path.clone();
    let job = move || {
            let start = decode_timing_start();
            if image_decode_debug_enabled() {
                log!(
                    "ImageCache: decode_start key={} bytes={}",
                    image_path.display(),
                    image_size_bytes
                );
            }
            let result = decode_image_buffer(&image_path, (*data).as_ref(), drawn_size).map(|mut buffer| {
                // Mip-chain generation belongs with the decode: doing it here keeps the
                // box-filter cost off the UI thread when the texture is committed.
                buffer.build_cpu_mip_chain_if_uploaded();
                buffer
            });
            if image_decode_debug_enabled() {
                let status = match &result {
                    Ok(buffer) => format!("ok {}x{}", buffer.width, buffer.height),
                    Err(err) => format!("err {err}"),
                };
                if let Some(start) = start {
                    log!(
                        "ImageCache: decode_done key={} elapsed_ms={:.1} {}",
                        image_path.display(),
                        (Cx::monotonic_now() - start) * 1000.0,
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
        };
    let pool = cx.task_pool();
    if pool.is_open() {
        let queue = &mut cx.get_global::<ImageCache>().decode_queue;
        queue.set_in_flight_limit(pool.worker_count());
        if let Err(error) = queue.push(&pool, task_key.clone(), true, QueueOrder::Lifo, job) {
            Cx::post_action(AsyncImageLoad {
                image_path: task_key,
                result: RefCell::new(Some(Err(ImageError::Worker(error.to_string())))),
            });
        }
    } else {
        // A non-threaded wasm build has an explicit serial path for this pure
        // CPU decode.
        job();
    }
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
    // A finished decode frees a slot: hand the next staged one over.
    let pool = cx.task_pool();
    cx.get_global::<ImageCache>().decode_queue.pump(&pool);
    // Images that were evicted while they were decoding go now if their decode has landed.
    let cache = cx.get_global::<ImageCache>();
    let landed_paths: Vec<PathBuf> = cache.evicted_while_decoding.iter()
        .filter(|path| !cache.pending_decodes.contains_key(path.as_path()))
        .cloned()
        .collect();
    for path in landed_paths {
        cx.get_global::<ImageCache>().evicted_while_decoding.remove(&path);
        evict_image_from_cache(cx, &path);
    }
    if let Ok(data) = result {
        let width = data.width;
        let height = data.height;
        let cached_texture = load_image_from_cache(cx, image_path);
        // A smaller decode that lands after a bigger one doesn't replace it.
        let cached_size = cached_texture.as_ref().and_then(|texture| texture.get_format(cx).vec_width_height());
        let texture = match cached_texture {
            Some(cached_texture) if cached_size.is_some_and(|(w, h)| w * h >= width * height) => cached_texture,
            _ => {
                let upload_start = decode_timing_start();
                let texture = data.into_new_texture(cx);
                if image_decode_debug_enabled() {
                    if let Some(upload_start) = upload_start {
                        log!(
                            "ImageCache: gpu_commit key={} elapsed_ms={:.1} size={}x{}",
                            image_path.display(),
                            (Cx::monotonic_now() - upload_start) * 1000.0,
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
                    .insert_loaded(image_path.into(), texture.clone());
                texture
            }
        };
        // It's done decoding, unless a decode with more pixels is still on its way.
        let pending_decode = cx.get_global::<ImageCache>().pending_decodes.get(image_path).copied();
        if pending_decode.is_some_and(|pending_decode| has_enough_pixels(cx, &texture, pending_decode.drawn_size)) {
            cx.get_global::<ImageCache>().pending_decodes.remove(image_path);
        }
    } else {
        if image_decode_debug_enabled() {
            log!(
                "ImageCache: gpu_commit key={} skipped (decode error)",
                image_path.display()
            );
        }
        let cache = cx.get_global::<ImageCache>();
        cache.pending_decodes.remove(image_path);
        // An image that's already decoded (with fewer pixels) stays cached.
        if matches!(cache.map.get(image_path), Some(ImageCacheEntry::Loading(..))) {
            cache.map.remove(image_path);
        }
    }
}

/// Hand one path's decoded image back: the cache forgets it AND the pixels
/// behind it are released. True when there was something to evict.
///
/// Dropping the cache's `Texture` handle alone is not enough. A `Texture` is a
/// refcounted slot in `Cx`'s texture pool; when the last handle goes the slot
/// joins the free list, but the `CxTexture` in it — pixel buffer and all —
/// stays put until some later allocation happens to reuse that slot. The
/// decoded buffer is the expensive half (a 24-megapixel photo is 96 MB of it,
/// kept as the upload source for the life of the slot), so this drops it
/// outright and lets the freed slot be reused for the rest.
///
/// Call it only for a path the caller loaded and no longer shows: a widget
/// still displaying that image holds its own clone of the `Texture`, which
/// would survive here with nothing left to re-upload from.
///
/// "Released" means released to the allocator, which is not the same as
/// released to the OS: on macOS a freed 96 MB block goes into libmalloc's
/// large cache and `ps rss` does not move (a plain `vec![0u32; 24_000_000]`
/// x5, touched then dropped, leaves RSS exactly where it was). What changes
/// is what the process is still *holding*: the next decode reuses those
/// pages instead of asking for more, so a viewer dialing through a folder
/// stops climbing.
///
/// Its GPU copy is released too, unless something else still has a handle to it.
pub fn evict_image_from_cache(cx: &mut Cx, image_path: &Path) -> bool {
    if !cx.has_global::<ImageCache>() {
        return false;
    }
    let Some(entry) = cx.get_global::<ImageCache>().evict(image_path) else {
        return false;
    };
    if let ImageCacheEntry::Loaded(texture) = entry {
        let is_last_handle = texture.readers() == 1;
        // The decoded pixels; the slot itself goes when `texture` drops.
        match texture.get_format(cx) {
            // Pixels that something showing them still has to upload are kept.
            TextureFormat::VecBGRAu8_32 { data, updated, .. }
            | TextureFormat::VecMipBGRAu8_32 { data, updated, .. }
                if is_last_handle || updated.is_empty() =>
            {
                *data = None;
            }
            _ => {}
        }
        if is_last_handle {
            texture.release(cx);
        }
    }
    true
}

pub fn load_image_from_cache(cx: &mut Cx, image_path: &Path) -> Option<Texture> {
    ensure_image_cache_inner(cx);
    match cx.get_global::<ImageCache>().map.get(image_path) {
        Some(ImageCacheEntry::Loaded(texture)) => Some(texture.clone()),
        _ => None,
    }
}

fn read_image_file_limited(image_path: &Path) -> Result<Vec<u8>, ImageError> {
    if let Ok(metadata) = std::fs::metadata(image_path) {
        if metadata.len() > MAX_IMAGE_DECODED_BYTES as u64 {
            return Err(ImageError::DataTooLarge {
                bytes: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
                limit: MAX_IMAGE_DECODED_BYTES,
            });
        }
    }
    let file =
        std::fs::File::open(image_path).map_err(|_| ImageError::PathNotFound(image_path.into()))?;
    let mut data = Vec::new();
    file.take((MAX_IMAGE_DECODED_BYTES as u64) + 1)
        .read_to_end(&mut data)
        .map_err(|_| ImageError::PathNotFound(image_path.into()))?;
    if data.len() > MAX_IMAGE_DECODED_BYTES {
        return Err(ImageError::DataTooLarge {
            bytes: data.len(),
            limit: MAX_IMAGE_DECODED_BYTES,
        });
    }
    Ok(data)
}

pub fn load_image_from_data_async<D>(
    cx: &mut Cx,
    image_path: &Path,
    data: Arc<D>,
) -> Result<AsyncLoadResult, ImageError>
where
    D: AsRef<[u8]> + Send + Sync + ?Sized + 'static,
{
    load_image_from_data_async_at_size(cx, image_path, data, None)
}

/// Like [`load_image_from_data_async`], but decodes the image with just enough pixels to be drawn
/// `drawn_size` pixels big (or all of them if `None`), unless it's cached or decoding with that many.
pub fn load_image_from_data_async_at_size<D>(
    cx: &mut Cx,
    image_path: &Path,
    data: Arc<D>,
    drawn_size: Option<(usize, usize)>,
) -> Result<AsyncLoadResult, ImageError>
where
    D: AsRef<[u8]> + Send + Sync + ?Sized + 'static,
{
    ensure_image_cache_inner(cx);
    // It's wanted again, so it stays cached once its decode lands.
    cx.get_global::<ImageCache>().evicted_while_decoding.remove(image_path);
    if let Some(result) = get_cached_load_result(cx, image_path, drawn_size) {
        return Ok(result);
    }
    let bytes: &[u8] = (*data).as_ref();
    // Decode it once with enough pixels for both this and the decode that's on its way.
    let pending_drawn_size = cx.get_global::<ImageCache>()
        .pending_decodes
        .get(image_path)
        .and_then(|pending_decode| pending_decode.drawn_size);
    let drawn_size = match (pending_drawn_size, drawn_size) {
        (Some(pending_drawn_size), Some(drawn_size)) => Some((
            pending_drawn_size.0.max(drawn_size.0),
            pending_drawn_size.1.max(drawn_size.1),
        )),
        _ => drawn_size,
    };
    if bytes.len() > MAX_IMAGE_DECODED_BYTES {
        return Err(ImageError::DataTooLarge {
            bytes: bytes.len(),
            limit: MAX_IMAGE_DECODED_BYTES,
        });
    }

    // On wasm, decode synchronously on the UI thread since thread pools
    // are not reliably available. Also decode synchronously for gpusim
    // single-frame runs so textured output is available in the first emitted PNG.
    #[cfg(target_arch = "wasm32")]
    let force_sync = true;
    #[cfg(not(target_arch = "wasm32"))]
    let force_sync = gpusim_mode_enabled();

    if force_sync {
        let image = decode_image_buffer(image_path, bytes, drawn_size)?;
        let texture = image.into_new_texture(cx);
        cx.get_global::<ImageCache>()
            .insert_loaded(image_path.into(), texture);
        return Ok(AsyncLoadResult::Loaded);
    }

    let (w, h) = image_size_by_data(bytes, image_path)?;
    if image_decode_debug_enabled() {
        log!(
            "ImageCache: queue_decode key={} bytes={} size={}x{}",
            image_path.display(),
            bytes.len(),
            w,
            h
        );
    }
    let cache = cx.get_global::<ImageCache>();
    cache.pending_decodes.insert(image_path.into(), PendingDecode { natural_size: (w, h), drawn_size });
    // An image that's cached with fewer pixels stays cached until this decode lands.
    cache.map.entry(image_path.into()).or_insert(ImageCacheEntry::Loading(w, h));
    spawn_decode_job(cx, image_path.to_path_buf(), data, drawn_size);
    Ok(AsyncLoadResult::Loading(w, h))
}

/// Returns how loading the image at `image_path` with enough pixels to be drawn at `drawn_size`
/// (or with all of them if `None`) goes if the cache already has it or is getting it.
fn get_cached_load_result(cx: &mut Cx, image_path: &Path, drawn_size: Option<(usize, usize)>) -> Option<AsyncLoadResult> {
    let cache = cx.get_global::<ImageCache>();
    let pending_decode = cache.pending_decodes.get(image_path).copied();
    let cached_texture = match cache.map.get(image_path) {
        Some(ImageCacheEntry::Loaded(texture)) => Some(texture.clone()),
        // It's still being fetched, so it'll be decoded with all of its pixels.
        Some(ImageCacheEntry::Loading(w, h)) if pending_decode.is_none() => {
            return Some(AsyncLoadResult::Loading(*w, *h));
        }
        _ => None,
    };
    if cached_texture.is_some_and(|texture| has_enough_pixels(cx, &texture, drawn_size)) {
        return Some(AsyncLoadResult::Loaded);
    }
    // A decode with enough pixels that's on its way just needs waiting for.
    let (w, h) = pending_decode
        .filter(|pending_decode| pending_decode.is_big_enough_for(drawn_size))?
        .natural_size;
    Some(AsyncLoadResult::Loading(w, h))
}

/// Returns whether the image at `image_path` is being decoded with enough pixels
/// to be drawn `drawn_size` pixels big, or with all of them if that's `None`.
pub fn is_decoding_image(cx: &mut Cx, image_path: &Path, drawn_size: Option<(usize, usize)>) -> bool {
    cx.has_global::<ImageCache>()
        && cx.get_global::<ImageCache>().pending_decodes.get(image_path)
            .is_some_and(|pending_decode| pending_decode.is_big_enough_for(drawn_size))
}

pub fn load_image_file_by_path_async(
    cx: &mut Cx,
    image_path: &Path,
) -> Result<AsyncLoadResult, ImageError> {
    ensure_image_cache_inner(cx);
    // It's wanted again, so it stays cached once its decode lands.
    cx.get_global::<ImageCache>().evicted_while_decoding.remove(image_path);
    if let Some(result) = get_cached_load_result(cx, image_path, None) {
        return Ok(result);
    }
    let data = read_image_file_limited(image_path)?;
    load_image_from_data_async(cx, image_path, Arc::new(data))
}

pub fn load_image_http_by_url_async(cx: &mut Cx, url: &str) -> Result<AsyncLoadResult, ImageError> {
    ensure_image_cache_inner(cx);
    let image_path = PathBuf::from(url);
    // It's wanted again, so it stays cached once its decode lands.
    cx.get_global::<ImageCache>().evicted_while_decoding.remove(&image_path);
    if let Some(result) = get_cached_load_result(cx, &image_path, None) {
        return Ok(result);
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
                        if body.len() > MAX_IMAGE_DECODED_BYTES {
                            error!(
                                "image http response too large for {:?}: {} bytes",
                                image_path,
                                body.len()
                            );
                            cache.map.remove(&image_path);
                            continue;
                        }
                        cache.map.remove(&image_path);
                        decode_queue.push((image_path, Arc::new(body.to_vec())));
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

    fn load_image_from_data_async_impl<D>(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        data: Arc<D>,
        id: usize,
    ) -> Result<AsyncLoadResult, ImageError>
    where
        D: AsRef<[u8]> + Send + Sync + ?Sized + 'static,
        Self: Sized,
    {
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
        let image = decode_image_buffer(image_path, data, None)?;
        let texture = image.into_new_texture(cx);
        ensure_image_cache(cx);
        cx.get_global::<ImageCache>()
            .insert_loaded(image_path.into(), texture.clone());
        self.set_texture(Some(texture), id);
        Ok(())
    }

    fn load_image_file_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &Path,
        id: usize,
    ) -> Result<(), ImageError> {
        if let Some(texture) = load_image_from_cache(cx, image_path).filter(|texture| has_enough_pixels(cx, texture, None)) {
            self.set_texture(Some(texture), id);
            return Ok(());
        }
        let data = read_image_file_limited(image_path)?;
        self.load_image_file_by_path_and_data(cx, &data, id, image_path)
    }

    fn load_image_dep_by_path(
        &mut self,
        cx: &mut Cx,
        image_path: &str,
        id: usize,
    ) -> Result<(), ImageError> {
        let p_image_path = Path::new(image_path);
        if let Some(texture) = load_image_from_cache(cx, p_image_path).filter(|texture| has_enough_pixels(cx, texture, None)) {
            self.set_texture(Some(texture), id);
            return Ok(());
        }
        match cx.take_dependency(image_path) {
            Ok(data) => self.load_image_file_by_path_and_data(cx, &data, id, p_image_path),
            Err(_) => Err(ImageError::PathNotFound(image_path.into())),
        }
    }
}
