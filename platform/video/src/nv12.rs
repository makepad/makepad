//! RGB <-> NV12 converters (BT.709, limited/video range, 4:2:0).
//!
//! NV12 layout: `height` rows of Y (one byte per pixel) followed by
//! `height/2` rows of interleaved UV (one U,V byte pair per 2x2 pixel block).
//! All helpers require even `width`/`height` and tightly packed planes
//! (stride == width).

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
pub fn rgbx_to_nv12(rgb: &[u8], width: u32, height: u32, pixel_stride: usize, out: &mut Vec<u8>) {
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
                    // BT.709 limited range.
                    let luma = ((47 * r + 157 * g + 16 * b) >> 8) + 16;
                    y_plane[y * w + x] = clamp_u8(luma);
                    sum_u += (-26 * r - 87 * g + 112 * b) >> 8;
                    sum_v += (112 * r - 102 * g - 10 * b) >> 8;
                }
            }
            uv_row[bx * 2] = clamp_u8((sum_u + 2) / 4 + 128);
            uv_row[bx * 2 + 1] = clamp_u8((sum_v + 2) / 4 + 128);
        }
    }
}

/// Convert tightly packed RGB8 (3 bytes/pixel) to NV12.
pub fn rgb8_to_nv12(rgb: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
    rgbx_to_nv12(rgb, width, height, 3, out);
}

/// Convert tightly packed RGBA8 (4 bytes/pixel, alpha ignored) to NV12.
pub fn rgba8_to_nv12(rgba: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
    rgbx_to_nv12(rgba, width, height, 4, out);
}

/// Convert tightly packed NV12 to tightly packed RGB8 (BT.709). `out` is
/// resized to `width*height*3`.
pub fn nv12_to_rgb8(nv12: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
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
            let c = y_plane[y * w + x] as i32 - 16;
            let d = uv_row[(x / 2) * 2] as i32 - 128;
            let e = uv_row[(x / 2) * 2 + 1] as i32 - 128;
            // BT.709 limited range.
            let r = (298 * c + 459 * e + 128) >> 8;
            let g = (298 * c - 55 * d - 136 * e + 128) >> 8;
            let b = (298 * c + 541 * d + 128) >> 8;
            let p = (y * w + x) * 3;
            out[p] = clamp_u8(r);
            out[p + 1] = clamp_u8(g);
            out[p + 2] = clamp_u8(b);
        }
    }
}
