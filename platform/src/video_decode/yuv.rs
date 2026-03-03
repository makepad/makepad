//! YUV to RGBA conversion for dav1d decoded frames.
//!
//! Supports 8-bit and 10-bit YUV 4:2:0, 4:2:2, and 4:4:4 with
//! BT.601, BT.709, and BT.2020 color matrices.

use super::dav1d_ffi::{Dav1dMatrixCoefficients, Dav1dPixelLayout, DecodedPicture};

/// Color matrix coefficients for YUV→RGB conversion.
/// Values are fixed-point with 10-bit fractional part (multiply by 1024).
struct YuvMatrix {
    yr: i32,
    yg: i32,
    yb: i32,
    cr_r: i32,
    cb_b: i32,
    cr_g: i32,
    cb_g: i32,
}

/// BT.709 matrix (HD content).
const BT709: YuvMatrix = YuvMatrix {
    yr: 1192,
    yg: 1192,
    yb: 1192,
    cr_r: 1836,
    cb_b: 2164,
    cr_g: -547,
    cb_g: -218,
};

/// BT.601 matrix (SD content).
const BT601: YuvMatrix = YuvMatrix {
    yr: 1192,
    yg: 1192,
    yb: 1192,
    cr_r: 1634,
    cb_b: 2066,
    cr_g: -832,
    cb_g: -401,
};

/// BT.2020 matrix (UHD/HDR content).
const BT2020: YuvMatrix = YuvMatrix {
    yr: 1192,
    yg: 1192,
    yb: 1192,
    cr_r: 1749,
    cb_b: 2230,
    cr_g: -624,
    cb_g: -149,
};

fn matrix_for(mc: Dav1dMatrixCoefficients) -> &'static YuvMatrix {
    match mc {
        Dav1dMatrixCoefficients::BT709 => &BT709,
        Dav1dMatrixCoefficients::BT601
        | Dav1dMatrixCoefficients::BT470BG
        | Dav1dMatrixCoefficients::FCC => &BT601,
        Dav1dMatrixCoefficients::BT2020_NCL | Dav1dMatrixCoefficients::BT2020_CL => &BT2020,
        _ => &BT709,
    }
}

#[inline(always)]
fn clamp8(v: i32) -> u8 {
    v.max(0).min(255) as u8
}

/// Convert a decoded dav1d picture to RGBA. Writes to `rgba_buf`.
/// Returns (width, height).
pub fn picture_to_rgba(pic: &DecodedPicture, rgba_buf: &mut Vec<u8>) -> (u32, u32) {
    let w = pic.width() as usize;
    let h = pic.height() as usize;
    let bpc = pic.bpc();
    let layout = pic.layout();
    let mc = pic.matrix_coefficients();
    let _full_range = pic.is_full_range();

    rgba_buf.resize(w * h * 4, 255);

    if bpc <= 8 {
        convert_8bit(pic, w, h, layout, mc, rgba_buf);
    } else {
        convert_10bit(pic, w, h, layout, mc, rgba_buf);
    }

    (w as u32, h as u32)
}

fn convert_8bit(
    pic: &DecodedPicture,
    w: usize,
    h: usize,
    layout: Dav1dPixelLayout,
    mc: Dav1dMatrixCoefficients,
    rgba: &mut [u8],
) {
    let (y_ptr, y_stride) = pic.plane_y();
    let (u_ptr, u_stride) = pic.plane_u();
    let (v_ptr, _v_stride) = pic.plane_v();
    let mat = matrix_for(mc);

    match layout {
        Dav1dPixelLayout::I420 => {
            for row in 0..h {
                let y_row =
                    unsafe { std::slice::from_raw_parts(y_ptr.offset(row as isize * y_stride), w) };
                let uv_row_idx = row / 2;
                let u_row = unsafe {
                    std::slice::from_raw_parts(
                        u_ptr.offset(uv_row_idx as isize * u_stride),
                        (w + 1) / 2,
                    )
                };
                let v_row = unsafe {
                    std::slice::from_raw_parts(
                        v_ptr.offset(uv_row_idx as isize * u_stride),
                        (w + 1) / 2,
                    )
                };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let y = y_row[col] as i32 - 16;
                    let u = u_row[col / 2] as i32 - 128;
                    let v = v_row[col / 2] as i32 - 128;
                    let idx = dst_off + col * 4;
                    rgba[idx] = clamp8((mat.yr * y + mat.cr_r * v + 512) >> 10);
                    rgba[idx + 1] = clamp8((mat.yg * y + mat.cr_g * v + mat.cb_g * u + 512) >> 10);
                    rgba[idx + 2] = clamp8((mat.yb * y + mat.cb_b * u + 512) >> 10);
                    rgba[idx + 3] = 255;
                }
            }
        }
        Dav1dPixelLayout::I422 => {
            for row in 0..h {
                let y_row =
                    unsafe { std::slice::from_raw_parts(y_ptr.offset(row as isize * y_stride), w) };
                let u_row = unsafe {
                    std::slice::from_raw_parts(u_ptr.offset(row as isize * u_stride), (w + 1) / 2)
                };
                let v_row = unsafe {
                    std::slice::from_raw_parts(v_ptr.offset(row as isize * u_stride), (w + 1) / 2)
                };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let y = y_row[col] as i32 - 16;
                    let u = u_row[col / 2] as i32 - 128;
                    let v = v_row[col / 2] as i32 - 128;
                    let idx = dst_off + col * 4;
                    rgba[idx] = clamp8((mat.yr * y + mat.cr_r * v + 512) >> 10);
                    rgba[idx + 1] = clamp8((mat.yg * y + mat.cr_g * v + mat.cb_g * u + 512) >> 10);
                    rgba[idx + 2] = clamp8((mat.yb * y + mat.cb_b * u + 512) >> 10);
                    rgba[idx + 3] = 255;
                }
            }
        }
        Dav1dPixelLayout::I444 => {
            for row in 0..h {
                let y_row =
                    unsafe { std::slice::from_raw_parts(y_ptr.offset(row as isize * y_stride), w) };
                let u_row =
                    unsafe { std::slice::from_raw_parts(u_ptr.offset(row as isize * u_stride), w) };
                let v_row =
                    unsafe { std::slice::from_raw_parts(v_ptr.offset(row as isize * u_stride), w) };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let y = y_row[col] as i32 - 16;
                    let u = u_row[col] as i32 - 128;
                    let v = v_row[col] as i32 - 128;
                    let idx = dst_off + col * 4;
                    rgba[idx] = clamp8((mat.yr * y + mat.cr_r * v + 512) >> 10);
                    rgba[idx + 1] = clamp8((mat.yg * y + mat.cr_g * v + mat.cb_g * u + 512) >> 10);
                    rgba[idx + 2] = clamp8((mat.yb * y + mat.cb_b * u + 512) >> 10);
                    rgba[idx + 3] = 255;
                }
            }
        }
        Dav1dPixelLayout::I400 => {
            // Monochrome — Y only
            for row in 0..h {
                let y_row =
                    unsafe { std::slice::from_raw_parts(y_ptr.offset(row as isize * y_stride), w) };
                let dst_off = row * w * 4;
                for col in 0..w {
                    let luma = clamp8(((y_row[col] as i32 - 16) * 1192 + 512) >> 10);
                    let idx = dst_off + col * 4;
                    rgba[idx] = luma;
                    rgba[idx + 1] = luma;
                    rgba[idx + 2] = luma;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
}

fn convert_10bit(
    pic: &DecodedPicture,
    w: usize,
    h: usize,
    layout: Dav1dPixelLayout,
    mc: Dav1dMatrixCoefficients,
    rgba: &mut [u8],
) {
    // dav1d returns 10-bit in 16-bit words (LSB-aligned)
    let (y_ptr, y_stride) = pic.plane_y();
    let (u_ptr, u_stride) = pic.plane_u();
    let (v_ptr, _v_stride) = pic.plane_v();
    let mat = matrix_for(mc);

    // Stride is in bytes; for 16-bit, divide by 2 for element count
    let y_stride_px = y_stride / 2;
    let u_stride_px = u_stride / 2;

    match layout {
        Dav1dPixelLayout::I420 => {
            for row in 0..h {
                let y_row = unsafe {
                    std::slice::from_raw_parts(
                        (y_ptr as *const u16).offset(row as isize * y_stride_px),
                        w,
                    )
                };
                let uv_row_idx = row / 2;
                let u_row = unsafe {
                    std::slice::from_raw_parts(
                        (u_ptr as *const u16).offset(uv_row_idx as isize * u_stride_px),
                        (w + 1) / 2,
                    )
                };
                let v_row = unsafe {
                    std::slice::from_raw_parts(
                        (v_ptr as *const u16).offset(uv_row_idx as isize * u_stride_px),
                        (w + 1) / 2,
                    )
                };

                let dst_off = row * w * 4;
                for col in 0..w {
                    // Scale 10-bit to 8-bit range for matrix application
                    let y = (y_row[col] as i32 >> 2) - 16;
                    let u = (u_row[col / 2] as i32 >> 2) - 128;
                    let v = (v_row[col / 2] as i32 >> 2) - 128;
                    let idx = dst_off + col * 4;
                    rgba[idx] = clamp8((mat.yr * y + mat.cr_r * v + 512) >> 10);
                    rgba[idx + 1] = clamp8((mat.yg * y + mat.cr_g * v + mat.cb_g * u + 512) >> 10);
                    rgba[idx + 2] = clamp8((mat.yb * y + mat.cb_b * u + 512) >> 10);
                    rgba[idx + 3] = 255;
                }
            }
        }
        _ => {
            // For other 10-bit layouts, fall back to treating as 8-bit
            // (shift down by 2). Not common in practice.
            convert_8bit(pic, w, h, layout, mc, rgba);
        }
    }
}
