//! RGB <-> NV12 converters (4:2:0) in any [`VideoColor`]'s matrix and range,
//! with the fixed-point coefficients of [`VideoColor::yuv_to_rgb_fixed`] /
//! [`VideoColor::rgb_to_yuv_fixed`] (BT.709 limited range is the classic
//! integer maths). Primaries and HDR transfers are not converted here: a
//! picture that needs them is drawn by the GPU pass (`mod.draw.VideoColor`).
//!
//! NV12 layout: `height` rows of Y (one byte per pixel) followed by
//! `height/2` rows of interleaved UV (one U,V byte pair per 2x2 pixel block).
//! All helpers require even `width`/`height` and tightly packed planes
//! (stride == width).

use crate::color::VideoColor;

/// Byte size of one tightly packed NV12 frame.
pub fn nv12_frame_size(width: u32, height: u32) -> usize {
    let w = width as usize;
    let h = height as usize;
    w * h + w * (h / 2)
}

#[inline]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Convert tightly packed RGB(x) pixels to NV12. `pixel_stride` is 3 for RGB
/// and 4 for RGBA (alpha ignored). `out` is resized to fit.
pub fn rgbx_to_nv12(rgb: &[u8], width: u32, height: u32, pixel_stride: usize, color: VideoColor, out: &mut Vec<u8>) {
    let k = color.rgb_to_yuv_fixed();
    let w = width as usize;
    let h = height as usize;
    debug_assert!(w % 2 == 0 && h % 2 == 0);
    debug_assert!(rgb.len() >= w * h * pixel_stride);
    out.clear();
    out.resize(nv12_frame_size(width, height), 0);
    let (y_plane, uv_plane) = out.split_at_mut(w * h);

    for by in 0..h / 2 {
        let uv_row = &mut uv_plane[by * w..by * w + w];
        for bx in 0..w / 2 {
            // 2x2 block: per-pixel Y, averaged chroma.
            let mut sum_u = 0i32;
            let mut sum_v = 0i32;
            for dy in 0..2 {
                let y = by * 2 + dy;
                for dx in 0..2 {
                    let x = bx * 2 + dx;
                    let p = (y * w + x) * pixel_stride;
                    let r = rgb[p] as i32;
                    let g = rgb[p + 1] as i32;
                    let b = rgb[p + 2] as i32;
                    let luma = ((k.ky[0] * r + k.ky[1] * g + k.ky[2] * b) >> 8) + k.y_offset;
                    y_plane[y * w + x] = clamp_u8(luma);
                    sum_u += (k.ku[0] * r + k.ku[1] * g + k.ku[2] * b) >> 8;
                    sum_v += (k.kv[0] * r + k.kv[1] * g + k.kv[2] * b) >> 8;
                }
            }
            uv_row[bx * 2] = clamp_u8((sum_u + 2) / 4 + 128);
            uv_row[bx * 2 + 1] = clamp_u8((sum_v + 2) / 4 + 128);
        }
    }
}

/// Convert tightly packed RGB8 (3 bytes/pixel) to NV12.
pub fn rgb8_to_nv12(rgb: &[u8], width: u32, height: u32, color: VideoColor, out: &mut Vec<u8>) {
    rgbx_to_nv12(rgb, width, height, 3, color, out);
}

/// Convert tightly packed RGBA8 (4 bytes/pixel, alpha ignored) to NV12.
pub fn rgba8_to_nv12(rgba: &[u8], width: u32, height: u32, color: VideoColor, out: &mut Vec<u8>) {
    rgbx_to_nv12(rgba, width, height, 4, color, out);
}

/// NV12 → packed BGRA u32 (`0xAARRGGBB`, i.e. b,g,r,a in memory), ONE
/// pass, no intermediate RGB plane. The VJ's reverse/scratch tier converts
/// hundreds of megabytes a second on giant screen-capture frames; the
/// two-pass rgb8-then-repack shape it used before doubled the memory
/// traffic and cost roughly half of its per-frame budget.
pub fn nv12_to_bgra_u32(nv12: &[u8], width: u32, height: u32, color: VideoColor, out: &mut Vec<u32>) {
    let f = color.yuv_to_rgb_fixed();
    let k = f.k;
    let w = width as usize;
    let h = height as usize;
    debug_assert!(w % 2 == 0 && h % 2 == 0);
    debug_assert!(nv12.len() >= nv12_frame_size(width, height));
    out.clear();
    out.resize(w * h, 0);
    let (y_plane, uv_plane) = nv12.split_at(w * h);
    for y in 0..h {
        let uv_row = &uv_plane[(y / 2) * w..(y / 2) * w + w];
        let y_row = &y_plane[y * w..y * w + w];
        let out_row = &mut out[y * w..y * w + w];
        for x in 0..w {
            let c = y_row[x] as i32 - f.y_offset;
            let d = uv_row[(x / 2) * 2] as i32 - 128;
            let e = uv_row[(x / 2) * 2 + 1] as i32 - 128;
            let r = (k[0][0] * c + k[0][1] * d + k[0][2] * e + 128) >> 8;
            let g = (k[1][0] * c + k[1][1] * d + k[1][2] * e + 128) >> 8;
            let b = (k[2][0] * c + k[2][1] * d + k[2][2] * e + 128) >> 8;
            out_row[x] = 0xff00_0000
                | ((clamp_u8(r) as u32) << 16)
                | ((clamp_u8(g) as u32) << 8)
                | clamp_u8(b) as u32;
        }
    }
}

/// Convert tightly packed NV12 to tightly packed RGB8. `out` is resized to
/// `width*height*3`.
pub fn nv12_to_rgb8(nv12: &[u8], width: u32, height: u32, color: VideoColor, out: &mut Vec<u8>) {
    let f = color.yuv_to_rgb_fixed();
    let k = f.k;
    let w = width as usize;
    let h = height as usize;
    debug_assert!(w % 2 == 0 && h % 2 == 0);
    debug_assert!(nv12.len() >= nv12_frame_size(width, height));
    out.clear();
    out.resize(w * h * 3, 0);
    let (y_plane, uv_plane) = nv12.split_at(w * h);

    for y in 0..h {
        let uv_row = &uv_plane[(y / 2) * w..(y / 2) * w + w];
        for x in 0..w {
            let c = y_plane[y * w + x] as i32 - f.y_offset;
            let d = uv_row[(x / 2) * 2] as i32 - 128;
            let e = uv_row[(x / 2) * 2 + 1] as i32 - 128;
            let r = (k[0][0] * c + k[0][1] * d + k[0][2] * e + 128) >> 8;
            let g = (k[1][0] * c + k[1][1] * d + k[1][2] * e + 128) >> 8;
            let b = (k[2][0] * c + k[2][1] * d + k[2][2] * e + 128) >> 8;
            let p = (y * w + x) * 3;
            out[p] = clamp_u8(r);
            out[p + 1] = clamp_u8(g);
            out[p + 2] = clamp_u8(b);
        }
    }
}
