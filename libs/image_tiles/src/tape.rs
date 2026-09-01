//! The tile-tape engine: pixels in, hardware-HEVC tapes out, and back.
//!
//! Nothing here knows about downloads, databases or widgets — these are the
//! primitives both the baker and any viewer build on, and the same ones the
//! Source Library app runs its wall with:
//!
//! - NV12 [`Planes`] with BT.709 limited-range conversion, so a round trip
//!   through HEVC is colour-neutral.
//! - The slot pyramid: one picture fitted into a 128 px atlas slot and
//!   halved down to 8 px ([`build_pyramid`]), and the shard atlas geometry
//!   that packs 1024 slots into a 32x32 grid per level.
//! - One HEVC intra frame per file ([`write_frame`] / [`read_frame`]):
//!   hardware encoded, hardware decoded, written atomically. An atlas level
//!   is one such frame; a full-resolution picture is another.
//! - The BGRA mip chain a full-resolution frame becomes on its way to a
//!   `VecMipBGRAu8_32` texture ([`full_frame`]).
//!
//! No JPEGs or PNGs ever touch the disk: sources are decoded in memory and
//! kept only as hardware-decodable HEVC.

use makepad_video::{encode_intra_frame_mp4, nv12, VideoFileCodec, VideoFileEncoderOptions};
use makepad_video::VideoFileDecoder;
use makepad_widgets::ImageBuffer;
use std::path::Path;

/// Atlas slot side in pixels at level 0.
pub const SLOT: u32 = 128;
/// Slots per shard side: a shard is a GRID x GRID block of slots.
pub const GRID: u32 = 32;
/// Items per shard.
pub const SHARD_CAP: u32 = GRID * GRID;
/// Pyramid levels per shard: 128, 64, 32, 16, 8 px per slot.
pub const LEVELS: usize = 5;

/// HEVC intra quality for atlas pages / full pictures, bits per pixel per
/// frame. Shared so every writer fills byte-compatible tapes.
pub const PAGE_BPP: f64 = 1.2;
pub const FULL_BPP: f64 = 1.6;

/// The long sides a picture's on-disk zoom pyramid is cut to, largest first.
/// Below the smallest of these the atlas tapes already carry the picture.
pub const PYRAMID_LEVELS: [u32; 4] = [4096, 2048, 1024, 512];

/// The largest full-resolution frame kept on disk.
pub const FULL_MAX_PX: u32 = 8192;

pub fn page_size(level: usize) -> u32 {
    (GRID * SLOT) >> level
}

pub fn slot_size(level: usize) -> u32 {
    SLOT >> level
}

pub fn slot_origin(slot: u32, level: usize) -> (u32, u32) {
    let s = slot_size(level);
    ((slot % GRID) * s, (slot / GRID) * s)
}

/// NV12 planes: `y` is width*height, `uv` is interleaved CbCr at half
/// resolution (width * height/2 bytes). Dimensions are even.
#[derive(Clone, Debug, PartialEq)]
pub struct Planes {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub uv: Vec<u8>,
}

impl Planes {
    pub fn black(width: u32, height: u32) -> Planes {
        let (w, h) = (width as usize, height as usize);
        Planes { width, height, y: vec![0; w * h], uv: vec![128; w * h / 2] }
    }

    pub fn from_nv12(width: u32, height: u32, data: &[u8]) -> Result<Planes, String> {
        let (w, h) = (width as usize, height as usize);
        if data.len() < w * h * 3 / 2 {
            return Err(format!("nv12 frame too short: {} < {}", data.len(), w * h * 3 / 2));
        }
        Ok(Planes { width, height, y: data[..w * h].to_vec(), uv: data[w * h..w * h * 3 / 2].to_vec() })
    }

    pub fn to_nv12(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.y.len() + self.uv.len());
        out.extend_from_slice(&self.y);
        out.extend_from_slice(&self.uv);
        out
    }

    /// BT.709 limited range, the same math the tape decoder and the tile
    /// shader use, so a round trip through HEVC is colour-neutral.
    pub fn from_rgba(rgba: &[u8], width: u32, height: u32) -> Planes {
        debug_assert!(width % 2 == 0 && height % 2 == 0);
        let mut nv12 = Vec::new();
        nv12::rgbx_to_nv12(rgba, width, height, 4, &mut nv12);
        Planes::from_nv12(width, height, &nv12).expect("nv12 size")
    }

    pub fn to_rgba(&self) -> Vec<u8> {
        let mut bgra = Vec::new();
        nv12::nv12_to_bgra_u32(&self.to_nv12(), self.width, self.height, &mut bgra);
        let mut out = Vec::with_capacity(bgra.len() * 4);
        for p in bgra {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, 255]);
        }
        out
    }

    /// Copy `src` into this plane set at even luma coordinates.
    pub fn blit(&mut self, src: &Planes, x: u32, y: u32) {
        debug_assert!(x % 2 == 0 && y % 2 == 0);
        let (w, sw, sh) = (self.width as usize, src.width as usize, src.height as usize);
        if x as usize + sw > w || y as usize + sh > self.height as usize {
            return;
        }
        for row in 0..sh {
            let dst = (y as usize + row) * w + x as usize;
            self.y[dst..dst + sw].copy_from_slice(&src.y[row * sw..row * sw + sw]);
        }
        for row in 0..sh / 2 {
            let dst = (y as usize / 2 + row) * w + x as usize;
            self.uv[dst..dst + sw].copy_from_slice(&src.uv[row * sw..row * sw + sw]);
        }
    }

    /// Area-average downscale straight on the planes.
    pub fn downscale(&self, dw: u32, dh: u32) -> Planes {
        let dw = dw.max(2) & !1;
        let dh = dh.max(2) & !1;
        Planes {
            width: dw,
            height: dh,
            y: box_downscale(&self.y, self.width, self.height, 1, dw, dh),
            uv: box_downscale(&self.uv, self.width / 2, self.height / 2, 2, dw / 2, dh / 2),
        }
    }
}

/// Exact box filter: each destination pixel averages its source rectangle.
pub fn box_downscale(src: &[u8], sw: u32, sh: u32, channels: usize, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw * dh * channels];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for dy in 0..dh {
        let y0 = dy * sh / dh;
        let y1 = ((dy + 1) * sh / dh).max(y0 + 1).min(sh);
        for dx in 0..dw {
            let x0 = dx * sw / dw;
            let x1 = ((dx + 1) * sw / dw).max(x0 + 1).min(sw);
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            for c in 0..channels {
                let mut sum = 0u32;
                for y in y0..y1 {
                    let row = y * sw;
                    for x in x0..x1 {
                        sum += src[(row + x) * channels + c] as u32;
                    }
                }
                out[(dy * dw + dx) * channels + c] = ((sum + n / 2) / n) as u8;
            }
        }
    }
    out
}

/// Fit `w x h` inside `max x max` keeping aspect; even dimensions, >= 2.
pub fn fit_dims(w: u32, h: u32, max: u32) -> (u32, u32) {
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let scale = (max as f64 / w).min(max as f64 / h).min(1.0);
    let fw = ((w * scale).round() as u32).clamp(2, max) & !1;
    let fh = ((h * scale).round() as u32).clamp(2, max) & !1;
    (fw.max(2), fh.max(2))
}

/// One item's atlas slot at every level: the fitted picture sits at the
/// top-left of a black slot; `fit` is its level-0 size in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct TilePyramid {
    pub fit: (u32, u32),
    pub levels: Vec<Planes>,
}

/// Decode PNG/JPEG (or whatever else the magic bytes say) to an ImageBuffer.
pub fn decode_image(bytes: &[u8]) -> Result<ImageBuffer, String> {
    let is_png = bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    let is_jpg = bytes.starts_with(&[0xff, 0xd8]);
    let result = if is_png {
        ImageBuffer::from_png(bytes)
    } else if is_jpg {
        ImageBuffer::from_jpg(bytes)
    } else {
        ImageBuffer::from_jpg(bytes).or_else(|_| ImageBuffer::from_png(bytes))
    };
    result.map_err(|e| format!("{e:?}"))
}

pub fn image_to_rgba(img: &ImageBuffer) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len() * 4);
    for p in &img.data {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
    }
    out
}

/// Downscale a picture into its slot pyramid: level 0 is the picture fitted
/// into 128 px, every further level halves it.
pub fn build_pyramid(rgba: &[u8], width: u32, height: u32) -> TilePyramid {
    let fit = fit_dims(width, height, SLOT);
    let fitted = box_downscale(rgba, width, height, 4, fit.0, fit.1);
    let mut level0 = Planes::black(SLOT, SLOT);
    level0.blit(&Planes::from_rgba(&fitted, fit.0, fit.1), 0, 0);
    let mut levels = vec![level0];
    for level in 1..LEVELS {
        let prev = &levels[level - 1];
        levels.push(prev.downscale(slot_size(level), slot_size(level)));
    }
    TilePyramid { fit, levels }
}

/// The H265 options a given size and quality ask for; public because apps
/// with their own encode paths (clips, streams) size bitrates the same way.
pub fn encoder_options(width: u32, height: u32, fps: u32, bpp: f64, keyframe_only: bool) -> VideoFileEncoderOptions {
    let bitrate = (width as f64 * height as f64 * bpp * fps as f64).clamp(200_000.0, 800_000_000.0) as u32;
    VideoFileEncoderOptions {
        codec: VideoFileCodec::H265,
        width,
        height,
        fps_num: fps,
        fps_den: 1,
        video_bitrate_bps: bitrate,
        audio: None,
        keyframe_only,
    }
}

/// One HEVC intra frame in its own container, written atomically.
pub fn write_frame(path: &Path, planes: &Planes, bpp: f64) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;
    }
    let options = encoder_options(planes.width, planes.height, 30, bpp, true);
    // One still needs none of AVAssetWriter's machinery: the session encodes
    // it and we write the container ourselves.
    let mp4 = encode_intra_frame_mp4(
        &planes.to_nv12(),
        planes.width,
        planes.height,
        30,
        options.video_bitrate_bps,
        options.codec,
    )
    .map_err(|e| format!("encode: {e}"))?;
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &mp4).map_err(|e| format!("write tmp: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))
}

/// The first frame of a container, hardware decoded.
pub fn read_frame(path: &Path) -> Result<Planes, String> {
    let text = path.to_string_lossy().to_string();
    let mut dec = VideoFileDecoder::open(&text).map_err(|e| format!("open {text}: {e:?}"))?;
    let frame = dec
        .next_frame()
        .map_err(|e| format!("decode {text}: {e:?}"))?
        .ok_or_else(|| format!("no frame in {text}"))?;
    Planes::from_nv12(frame.width, frame.height, &frame.nv12)
}

/// A full-resolution picture as a BGRA mip chain, ready for a
/// `VecMipBGRAu8_32` texture: level 0 first, each level halving, so the GPU
/// samples it trilinearly and it stays sharp at any zoom.
pub struct FullFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u32>,
    pub max_level: usize,
}

/// Decoded NV12 -> BGRA with a full mip chain (2x2 box per level).
pub fn full_frame(planes: &Planes) -> FullFrame {
    let (w, h) = (planes.width as usize, planes.height as usize);
    let mut level0 = Vec::new();
    nv12::nv12_to_bgra_u32(&planes.to_nv12(), planes.width, planes.height, &mut level0);
    let mut chain = level0;
    let (mut lw, mut lh) = (w, h);
    let mut max_level = 0usize;
    let mut src_start = 0usize;
    while lw > 1 || lh > 1 {
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let src = &chain[src_start..src_start + lw * lh];
        let mut next = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            let y0 = (y * 2).min(lh - 1);
            let y1 = (y * 2 + 1).min(lh - 1);
            for x in 0..nw {
                let x0 = (x * 2).min(lw - 1);
                let x1 = (x * 2 + 1).min(lw - 1);
                let px = [src[y0 * lw + x0], src[y0 * lw + x1], src[y1 * lw + x0], src[y1 * lw + x1]];
                let mut acc = [0u32; 4];
                for p in px {
                    acc[0] += (p >> 24) & 0xff;
                    acc[1] += (p >> 16) & 0xff;
                    acc[2] += (p >> 8) & 0xff;
                    acc[3] += p & 0xff;
                }
                next.push(((acc[0] / 4) << 24) | ((acc[1] / 4) << 16) | ((acc[2] / 4) << 8) | (acc[3] / 4));
            }
        }
        src_start += lw * lh;
        chain.extend_from_slice(&next);
        lw = nw;
        lh = nh;
        max_level += 1;
    }
    FullFrame { width: planes.width, height: planes.height, bgra: chain, max_level }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry() {
        assert_eq!(page_size(0), 4096);
        assert_eq!(page_size(4), 256);
        assert_eq!(slot_size(4), 8);
        assert_eq!(slot_origin(0, 0), (0, 0));
        assert_eq!(slot_origin(33, 0), (128, 128));
        assert_eq!(slot_origin(33, 2), (32, 32));
        assert_eq!(slot_origin(1023, 4), (31 * 8, 31 * 8));
    }

    #[test]
    fn fits() {
        assert_eq!(fit_dims(1000, 500, 128), (128, 64));
        assert_eq!(fit_dims(500, 1000, 128), (64, 128));
        assert_eq!(fit_dims(60, 30, 128), (60, 30));
        assert_eq!(fit_dims(129, 129, 128), (128, 128));
        assert_eq!(fit_dims(1, 1, 128), (2, 2));
    }

    #[test]
    fn pyramid_shape() {
        let rgba = vec![200u8; 640 * 480 * 4];
        let p = build_pyramid(&rgba, 640, 480);
        assert_eq!(p.fit, (128, 96));
        assert_eq!(p.levels.len(), LEVELS);
        for (l, planes) in p.levels.iter().enumerate() {
            assert_eq!(planes.width, slot_size(l));
            assert_eq!(planes.height, slot_size(l));
        }
    }
}
