//! YUV plane data types and conversion utilities for dav1d decoded frames.
//!
//! Supports 8-bit and 10-bit YUV 4:2:0, 4:2:2, and 4:4:4 with
//! BT.601, BT.709, and BT.2020 color matrices.
//!
//! `YuvPlaneData` holds copied plane bytes for GPU upload. Platform backends
//! create R8 textures from each plane; the fragment shader does color conversion.

use super::dav1d_ffi::{DecodedPicture, Dav1dMatrixCoefficients, Dav1dPixelLayout};

/// Color matrix selector passed to the GPU shader as a uniform.
/// Matches the `yuv_type` uniform: 0.0 = BT.709, 1.0 = BT.601, 2.0 = BT.2020.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum YuvColorMatrix {
    BT709 = 0,
    BT601 = 1,
    BT2020 = 2,
}

impl YuvColorMatrix {
    pub fn as_f32(self) -> f32 {
        self as u8 as f32
    }

    pub fn from_dav1d(mc: Dav1dMatrixCoefficients) -> Self {
        match mc {
            Dav1dMatrixCoefficients::BT709 => YuvColorMatrix::BT709,
            Dav1dMatrixCoefficients::BT601
            | Dav1dMatrixCoefficients::BT470BG
            | Dav1dMatrixCoefficients::FCC => YuvColorMatrix::BT601,
            Dav1dMatrixCoefficients::BT2020_NCL
            | Dav1dMatrixCoefficients::BT2020_CL => YuvColorMatrix::BT2020,
            _ => YuvColorMatrix::BT709,
        }
    }
}

/// Subsampling layout for chroma planes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum YuvLayout {
    /// 4:2:0 — chroma is half width and half height of luma.
    I420,
    /// 4:2:2 — chroma is half width, same height as luma.
    I422,
    /// 4:4:4 — chroma is same size as luma.
    I444,
    /// Monochrome — no chroma planes.
    I400,
}

impl YuvLayout {
    pub fn from_dav1d(layout: Dav1dPixelLayout) -> Self {
        match layout {
            Dav1dPixelLayout::I400 => YuvLayout::I400,
            Dav1dPixelLayout::I420 => YuvLayout::I420,
            Dav1dPixelLayout::I422 => YuvLayout::I422,
            Dav1dPixelLayout::I444 => YuvLayout::I444,
        }
    }

    /// Chroma dimensions given luma width/height.
    pub fn chroma_size(self, luma_w: u32, luma_h: u32) -> (u32, u32) {
        match self {
            YuvLayout::I420 => ((luma_w + 1) / 2, (luma_h + 1) / 2),
            YuvLayout::I422 => ((luma_w + 1) / 2, luma_h),
            YuvLayout::I444 => (luma_w, luma_h),
            YuvLayout::I400 => (0, 0),
        }
    }
}

/// Extracted YUV plane data ready for GPU texture upload.
///
/// Each plane is tightly packed (stride == width) with 8-bit samples.
/// 10-bit sources are downshifted to 8 bits during extraction.
pub struct YuvPlaneData {
    /// Luma plane, dimensions `width × height`.
    pub y: Vec<u8>,
    /// Cb (U) chroma plane.
    pub u: Vec<u8>,
    /// Cr (V) chroma plane.
    pub v: Vec<u8>,
    /// Luma width in pixels.
    pub width: u32,
    /// Luma height in pixels.
    pub height: u32,
    /// Subsampling layout.
    pub layout: YuvLayout,
    /// Color matrix for shader conversion.
    pub matrix: YuvColorMatrix,
}

/// Extract Y, U, V planes from a decoded dav1d picture into tightly-packed
/// 8-bit buffers suitable for R8 texture upload.
pub fn extract_yuv_planes(pic: &DecodedPicture) -> YuvPlaneData {
    let w = pic.width();
    let h = pic.height();
    let bpc = pic.bpc();
    let layout = YuvLayout::from_dav1d(pic.layout());
    let matrix = YuvColorMatrix::from_dav1d(pic.matrix_coefficients());
    let (cw, ch) = layout.chroma_size(w, h);

    let y = extract_plane_8bit(pic.plane_y(), w as usize, h as usize, bpc);
    let u = if cw > 0 {
        extract_plane_8bit(pic.plane_u(), cw as usize, ch as usize, bpc)
    } else {
        Vec::new()
    };
    let v = if cw > 0 {
        extract_plane_8bit(pic.plane_v(), cw as usize, ch as usize, bpc)
    } else {
        Vec::new()
    };

    YuvPlaneData { y, u, v, width: w, height: h, layout, matrix }
}

/// Copy one plane from dav1d memory into a tightly packed Vec<u8>.
/// Handles stride padding and 10-bit→8-bit downshift.
fn extract_plane_8bit(plane: (*const u8, isize), w: usize, h: usize, bpc: i32) -> Vec<u8> {
    let (ptr, stride) = plane;
    let mut buf = vec![0u8; w * h];

    if bpc <= 8 {
        for row in 0..h {
            let src = unsafe { std::slice::from_raw_parts(ptr.offset(row as isize * stride), w) };
            buf[row * w..(row + 1) * w].copy_from_slice(src);
        }
    } else {
        // 10-bit or higher: data is in 16-bit words, shift down to 8-bit.
        let stride_px = stride / 2;
        let shift = (bpc - 8).max(0) as usize;
        for row in 0..h {
            let src = unsafe {
                std::slice::from_raw_parts(
                    (ptr as *const u16).offset(row as isize * stride_px),
                    w,
                )
            };
            let dst = &mut buf[row * w..(row + 1) * w];
            for col in 0..w {
                dst[col] = (src[col] >> shift) as u8;
            }
        }
    }

    buf
}

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
    let full_range = pic.is_full_range();

    rgba_buf.resize(w * h * 4, 255);

    if bpc <= 8 {
        convert_8bit(pic, w, h, layout, mc, full_range, rgba_buf);
    } else {
        convert_high_bitdepth(pic, w, h, layout, mc, full_range, bpc, rgba_buf);
    }

    (w as u32, h as u32)
}

#[inline(always)]
fn yuv_to_rgb_8(
    y: u8,
    u: u8,
    v: u8,
    mc: Dav1dMatrixCoefficients,
    full_range: bool,
) -> (u8, u8, u8) {
    if full_range {
        let y = y as i32;
        let u = u as i32 - 128;
        let v = v as i32 - 128;
        let (cr_r, cb_b, cr_g, cb_g) = match mc {
            Dav1dMatrixCoefficients::BT601
            | Dav1dMatrixCoefficients::BT470BG
            | Dav1dMatrixCoefficients::FCC => (1436, 1815, -731, -352),
            Dav1dMatrixCoefficients::BT2020_NCL | Dav1dMatrixCoefficients::BT2020_CL => {
                (1510, 1921, -585, -165)
            }
            _ => (1613, 1900, -479, -192), // BT.709 fallback
        };

        let r = y + ((cr_r * v + 512) >> 10);
        let g = y + ((cr_g * v + cb_g * u + 512) >> 10);
        let b = y + ((cb_b * u + 512) >> 10);
        (clamp8(r), clamp8(g), clamp8(b))
    } else {
        let mat = matrix_for(mc);
        let y = y as i32 - 16;
        let u = u as i32 - 128;
        let v = v as i32 - 128;
        let r = (mat.yr * y + mat.cr_r * v + 512) >> 10;
        let g = (mat.yg * y + mat.cr_g * v + mat.cb_g * u + 512) >> 10;
        let b = (mat.yb * y + mat.cb_b * u + 512) >> 10;
        (clamp8(r), clamp8(g), clamp8(b))
    }
}

fn convert_8bit(
    pic: &DecodedPicture,
    w: usize,
    h: usize,
    layout: Dav1dPixelLayout,
    mc: Dav1dMatrixCoefficients,
    full_range: bool,
    rgba: &mut [u8],
) {
    let (y_ptr, y_stride) = pic.plane_y();
    let (u_ptr, u_stride) = pic.plane_u();
    let (v_ptr, v_stride) = pic.plane_v();

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
                        v_ptr.offset(uv_row_idx as isize * v_stride),
                        (w + 1) / 2,
                    )
                };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let (r, g, b) = yuv_to_rgb_8(
                        y_row[col],
                        u_row[col / 2],
                        v_row[col / 2],
                        mc,
                        full_range,
                    );
                    let idx = dst_off + col * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
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
                    std::slice::from_raw_parts(v_ptr.offset(row as isize * v_stride), (w + 1) / 2)
                };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let (r, g, b) = yuv_to_rgb_8(
                        y_row[col],
                        u_row[col / 2],
                        v_row[col / 2],
                        mc,
                        full_range,
                    );
                    let idx = dst_off + col * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
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
                    unsafe { std::slice::from_raw_parts(v_ptr.offset(row as isize * v_stride), w) };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let (r, g, b) = yuv_to_rgb_8(y_row[col], u_row[col], v_row[col], mc, full_range);
                    let idx = dst_off + col * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = 255;
                }
            }
        }
        Dav1dPixelLayout::I400 => {
            for row in 0..h {
                let y_row =
                    unsafe { std::slice::from_raw_parts(y_ptr.offset(row as isize * y_stride), w) };
                let dst_off = row * w * 4;
                for col in 0..w {
                    let (r, g, b) = yuv_to_rgb_8(y_row[col], 128, 128, mc, full_range);
                    let idx = dst_off + col * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
}

fn convert_high_bitdepth(
    pic: &DecodedPicture,
    w: usize,
    h: usize,
    layout: Dav1dPixelLayout,
    mc: Dav1dMatrixCoefficients,
    full_range: bool,
    bpc: i32,
    rgba: &mut [u8],
) {
    let (y_ptr, y_stride) = pic.plane_y();
    let (u_ptr, u_stride) = pic.plane_u();
    let (v_ptr, v_stride) = pic.plane_v();

    let y_stride_px = y_stride / 2;
    let u_stride_px = u_stride / 2;
    let v_stride_px = v_stride / 2;
    let shift = (bpc - 8).max(0) as usize;

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
                        (v_ptr as *const u16).offset(uv_row_idx as isize * v_stride_px),
                        (w + 1) / 2,
                    )
                };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let y8 = (y_row[col] >> shift) as u8;
                    let u8 = (u_row[col / 2] >> shift) as u8;
                    let v8 = (v_row[col / 2] >> shift) as u8;
                    let (r, g, b) = yuv_to_rgb_8(y8, u8, v8, mc, full_range);
                    let idx = dst_off + col * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = 255;
                }
            }
        }
        Dav1dPixelLayout::I422 => {
            for row in 0..h {
                let y_row = unsafe {
                    std::slice::from_raw_parts(
                        (y_ptr as *const u16).offset(row as isize * y_stride_px),
                        w,
                    )
                };
                let u_row = unsafe {
                    std::slice::from_raw_parts(
                        (u_ptr as *const u16).offset(row as isize * u_stride_px),
                        (w + 1) / 2,
                    )
                };
                let v_row = unsafe {
                    std::slice::from_raw_parts(
                        (v_ptr as *const u16).offset(row as isize * v_stride_px),
                        (w + 1) / 2,
                    )
                };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let y8 = (y_row[col] >> shift) as u8;
                    let u8 = (u_row[col / 2] >> shift) as u8;
                    let v8 = (v_row[col / 2] >> shift) as u8;
                    let (r, g, b) = yuv_to_rgb_8(y8, u8, v8, mc, full_range);
                    let idx = dst_off + col * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = 255;
                }
            }
        }
        Dav1dPixelLayout::I444 => {
            for row in 0..h {
                let y_row = unsafe {
                    std::slice::from_raw_parts(
                        (y_ptr as *const u16).offset(row as isize * y_stride_px),
                        w,
                    )
                };
                let u_row = unsafe {
                    std::slice::from_raw_parts(
                        (u_ptr as *const u16).offset(row as isize * u_stride_px),
                        w,
                    )
                };
                let v_row = unsafe {
                    std::slice::from_raw_parts(
                        (v_ptr as *const u16).offset(row as isize * v_stride_px),
                        w,
                    )
                };

                let dst_off = row * w * 4;
                for col in 0..w {
                    let y8 = (y_row[col] >> shift) as u8;
                    let u8 = (u_row[col] >> shift) as u8;
                    let v8 = (v_row[col] >> shift) as u8;
                    let (r, g, b) = yuv_to_rgb_8(y8, u8, v8, mc, full_range);
                    let idx = dst_off + col * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = 255;
                }
            }
        }
        Dav1dPixelLayout::I400 => {
            for row in 0..h {
                let y_row = unsafe {
                    std::slice::from_raw_parts(
                        (y_ptr as *const u16).offset(row as isize * y_stride_px),
                        w,
                    )
                };
                let dst_off = row * w * 4;
                for col in 0..w {
                    let y8 = (y_row[col] >> shift) as u8;
                    let (r, g, b) = yuv_to_rgb_8(y8, 128, 128, mc, full_range);
                    let idx = dst_off + col * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
}
