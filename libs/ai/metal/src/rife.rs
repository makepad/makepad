//! Practical-RIFE v4.26 ops for the macOS Metal path.
//!
//! Fills the exact op set `rife_model.rs` needs and CUDA provides on the
//! fleet (`libs/ai/cuda/kernels/rife.cu`), in the HOST-BACKED style of this
//! crate's `GpuTensor`: the convolutions — over 90 % of the FLOPs — ride
//! the ggml Metal GEMM through `shim::try_matmul_nt_f32` via a strided
//! im2col, and the cheap elementwise/gather ops (warp, PReLU tail, pixel
//! shuffle, merge) run banded on the CPU, which on unified memory is the
//! same RAM the GPU reads. Semantics are transcribed 1:1 from the portable
//! reference in `makepad-ai-rife::rife_cpu` — that file stays the parity
//! oracle, and the op-level tests over there compare THIS module against
//! it on random tensors.

use crate::gpu_types::GpuTensor;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

/// MAKEPAD_RIFE_PROF=1: accumulate wall time per op family and print at
/// every `prof_dump` (the bench calls it once per pair batch).
static PROF: [AtomicU64; 6] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
const PROF_NAMES: [&str; 6] = ["im2col", "gemm", "deconv_prep", "warp", "resize_etc", "elementwise"];

fn prof_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RIFE_PROF").is_some())
}

pub(crate) fn prof_add(slot: usize, t0: std::time::Instant) {
    if prof_on() {
        PROF[slot].fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

pub fn prof_dump() {
    if !prof_on() {
        return;
    }
    for (name, cell) in PROF_NAMES.iter().zip(PROF.iter()) {
        let ms = cell.swap(0, Ordering::Relaxed) as f64 / 1e6;
        eprintln!("rife prof {name}: {ms:.1} ms");
    }
}

fn tensor(rows: usize, cols: usize, data: Vec<f32>) -> GpuTensor {
    GpuTensor {
        rows,
        cols,
        data: RefCell::new(data),
        u32s: RefCell::new(Vec::new()),
        id: std::cell::Cell::new(crate::gpu_types::fresh_tensor_id()),
    }
}

fn data(t: &GpuTensor) -> Result<std::cell::Ref<'_, Vec<f32>>, String> {
    t.data
        .try_borrow()
        .map_err(|_| "metal GpuTensor already borrowed".to_string())
}

/// Run `work(band_start_row, band)` over row bands of `out` on every core.
/// Same discipline as the video-flow estimator: each band writes only its
/// own rows, so the result is independent of the band count.
fn par_bands<T: Send>(out: &mut [T], row_len: usize, work: impl Fn(usize, &mut [T]) + Sync) {
    let rows = if row_len == 0 { 0 } else { out.len() / row_len };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(rows.max(1));
    // Scoped threads cost real spawn/join time; below ~1M elements the
    // churn eats the win (RIFE issues a hundred small ops per pair).
    if threads <= 1 || rows < 8 || out.len() < 1_000_000 {
        work(0, out);
        return;
    }
    let band_rows = (rows + threads - 1) / threads;
    std::thread::scope(|scope| {
        let mut y0 = 0usize;
        for chunk in out.chunks_mut(band_rows * row_len) {
            let start = y0;
            let work = &work;
            scope.spawn(move || work(start, chunk));
            y0 += band_rows;
        }
    });
}


/// `C[m,n] = A[m,k] * B[k,n]` (both row-major, no transpose) — the shape
/// the K-major im2col produces.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn matmul_nn_blas(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Vec<f32> {
    extern "C" {
        fn cblas_sgemm(
            order: i32,
            trans_a: i32,
            trans_b: i32,
            m: i32,
            n: i32,
            k: i32,
            alpha: f32,
            a: *const f32,
            lda: i32,
            b: *const f32,
            ldb: i32,
            beta: f32,
            c: *mut f32,
            ldc: i32,
        );
    }
    const ROW_MAJOR: i32 = 101;
    const NO_TRANS: i32 = 111;
    let mut out = vec![0.0f32; m * n];
    unsafe {
        cblas_sgemm(
            ROW_MAJOR,
            NO_TRANS,
            NO_TRANS,
            m as i32,
            n as i32,
            k as i32,
            1.0,
            a.as_ptr(),
            k as i32,
            b.as_ptr(),
            n as i32,
            0.0,
            out.as_mut_ptr(),
            n as i32,
        );
    }
    out
}

/// CPU fallback for the NN shape.
#[allow(dead_code)]
fn matmul_nn_cpu(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; m * n];
    par_bands(&mut out, n, |m0, band| {
        for (row_index, row) in band.chunks_mut(n).enumerate() {
            let arow = &a[(m0 + row_index) * k..(m0 + row_index + 1) * k];
            for (ki, av) in arow.iter().enumerate() {
                let brow = &b[ki * n..(ki + 1) * n];
                for (slot, bv) in row.iter_mut().zip(brow.iter()) {
                    *slot += av * bv;
                }
            }
        }
    });
    out
}

/// Naive CPU GEMM fallback (`C[m,n] = A[m,k] * B[n,k]^T`) for when the
/// Metal shim declines (tiny sizes / MAKEPAD_DIFFUSION_CPU).
#[allow(dead_code)]
fn matmul_nt_cpu(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; m * n];
    par_bands(&mut out, n, |m0, band| {
        for (row_index, row) in band.chunks_mut(n).enumerate() {
            let arow = &a[(m0 + row_index) * k..(m0 + row_index + 1) * k];
            for (col, slot) in row.iter_mut().enumerate() {
                let brow = &bt[col * k..(col + 1) * k];
                let mut sum = 0.0f32;
                for i in 0..k {
                    sum += arow[i] * brow[i];
                }
                *slot = sum;
            }
        }
    });
    out
}

/// `nn.Conv2d(in, out, k, stride, pad)` — im2col into `[N, K]` position-
/// major, then one GEMM against the `[out_c, K]` weights.
#[allow(clippy::too_many_arguments)]
pub fn conv2d_planar_strided(
    x: &GpuTensor,
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
    weights: &[f32],
    bias: &[f32],
    out_channels: usize,
    kw: usize,
    kh: usize,
    pad_x: usize,
    pad_y: usize,
    stride_x: usize,
    stride_y: usize,
) -> Result<GpuTensor, String> {
    let in_channels = x.rows;
    let in_plane = in_width * in_height;
    let xd = data(x)?;
    if xd.len() < in_channels * in_plane {
        return Err("rife conv: input shorter than its extent".to_string());
    }
    if bias.len() != out_channels {
        return Err("rife conv: bias length mismatch".to_string());
    }
    let k = in_channels * kh * kw;
    if weights.len() != out_channels * k {
        return Err("rife conv: weight length mismatch".to_string());
    }
    let n = out_width * out_height;
    let xd: &[f32] = &xd;
    let prof_t0 = std::time::Instant::now();
    // K-MAJOR im2col: cols[k][n] — each (ic, ky, kx) row over the output
    // positions. Stride-1 interior segments are straight memcpys of the
    // source row; the position-major layout scattered every write behind
    // a k-stride and was the whole profile.
    let mut cols = vec![0.0f32; k * n];
    par_bands(&mut cols, n, |k0, band| {
        for (ki, row) in band.chunks_mut(n).enumerate() {
            let kidx = k0 + ki;
            let kx = kidx % kw;
            let ky = (kidx / kw) % kh;
            let ic = kidx / (kw * kh);
            let base = ic * in_plane;
            for oy in 0..out_height {
                let iy = (oy * stride_y + ky) as isize - pad_y as isize;
                let dst = &mut row[oy * out_width..(oy + 1) * out_width];
                if iy < 0 || iy >= in_height as isize {
                    dst.fill(0.0);
                    continue;
                }
                let src_row = base + iy as usize * in_width;
                if stride_x == 1 {
                    // Contiguous run with edge fills.
                    let x_first = kx as isize - pad_x as isize;
                    let lead = (-x_first).clamp(0, out_width as isize) as usize;
                    let x_last = x_first + out_width as isize - 1;
                    let trail =
                        (x_last - (in_width as isize - 1)).clamp(0, out_width as isize)
                            as usize;
                    let mid = out_width - lead - trail;
                    dst[..lead].fill(0.0);
                    if mid > 0 {
                        let s = (src_row as isize + x_first + lead as isize) as usize;
                        dst[lead..lead + mid].copy_from_slice(&xd[s..s + mid]);
                    }
                    dst[out_width - trail..].fill(0.0);
                } else {
                    for (ox, slot) in dst.iter_mut().enumerate() {
                        let ix = (ox * stride_x + kx) as isize - pad_x as isize;
                        *slot = if ix < 0 || ix >= in_width as isize {
                            0.0
                        } else {
                            xd[(src_row as isize + ix) as usize]
                        };
                    }
                }
            }
        }
    });
    prof_add(0, prof_t0);
    let prof_t0 = std::time::Instant::now();
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let mut out = matmul_nn_blas(weights, &cols, out_channels, k, n);
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let mut out = matmul_nn_cpu(weights, &cols, out_channels, k, n);
    prof_add(1, prof_t0);
    for (oc, row) in out.chunks_mut(n).enumerate() {
        let b = bias[oc];
        for v in row.iter_mut() {
            *v += b;
        }
    }
    Ok(tensor(out_channels, n, out))
}

/// `nn.ConvTranspose2d(in, out, k, stride, pad)` — expressed as a stride-1
/// convolution of the zero-stuffed input with the spatially flipped,
/// channel-swapped weights, so it rides the same GEMM.
#[allow(clippy::too_many_arguments)]
pub fn conv_transpose2d(
    x: &GpuTensor,
    in_width: usize,
    in_height: usize,
    weights: &[f32],
    bias: &[f32],
    out_channels: usize,
    kw: usize,
    kh: usize,
    pad: usize,
    stride: usize,
) -> Result<GpuTensor, String> {
    let in_channels = x.rows;
    let in_plane = in_width * in_height;
    if stride == 0 || kw <= pad || kh <= pad {
        return Err("rife deconv: bad geometry".to_string());
    }
    if weights.len() != in_channels * out_channels * kh * kw {
        return Err("rife deconv: weight length mismatch".to_string());
    }
    // Zero-stuffed input: value (iy, ix) lands at (iy*stride, ix*stride).
    let prof_t0 = std::time::Instant::now();
    let sw = (in_width - 1) * stride + 1;
    let sh = (in_height - 1) * stride + 1;
    let xd = data(x)?;
    let mut stuffed = vec![0.0f32; in_channels * sw * sh];
    for ic in 0..in_channels {
        for iy in 0..in_height {
            let src = ic * in_plane + iy * in_width;
            let dst = ic * sw * sh + (iy * stride) * sw;
            for ix in 0..in_width {
                stuffed[dst + ix * stride] = xd[src + ix];
            }
        }
    }
    drop(xd);
    // Flipped weights: w'[oc][ic][ky][kx] = w[ic][oc][kh-1-ky][kw-1-kx].
    let mut flipped = vec![0.0f32; out_channels * in_channels * kh * kw];
    for ic in 0..in_channels {
        for oc in 0..out_channels {
            for ky in 0..kh {
                for kx in 0..kw {
                    let src = ((ic * out_channels + oc) * kh + (kh - 1 - ky)) * kw
                        + (kw - 1 - kx);
                    let dst = ((oc * in_channels + ic) * kh + ky) * kw + kx;
                    flipped[dst] = weights[src];
                }
            }
        }
    }
    let out_w = (in_width - 1) * stride + kw - 2 * pad;
    let out_h = (in_height - 1) * stride + kh - 2 * pad;
    prof_add(2, prof_t0);
    let stuffed_tensor = tensor(in_channels, sw * sh, stuffed);
    conv2d_planar_strided(
        &stuffed_tensor,
        sw,
        sh,
        out_w,
        out_h,
        &flipped,
        bias,
        out_channels,
        kw,
        kh,
        kw - 1 - pad,
        kh - 1 - pad,
        1,
        1,
    )
}

/// RIFE backward warp: sample every channel of `x` at `(px + fx, py + fy)`,
/// bilinear, border-clamped (the reference's exact arithmetic).
pub fn warp(
    x: &GpuTensor,
    flow: &GpuTensor,
    width: usize,
    height: usize,
) -> Result<GpuTensor, String> {
    let plane = width * height;
    let channels = x.rows;
    let xd = data(x)?;
    let fd = data(flow)?;
    if xd.len() < channels * plane || fd.len() < 2 * plane || flow.rows < 2 {
        return Err("rife warp: extent mismatch".to_string());
    }
    let xd: &[f32] = &xd;
    let fd: &[f32] = &fd;
    let prof_t0 = std::time::Instant::now();
    let max_x = (width - 1) as f32;
    let max_y = (height - 1) as f32;
    let mut out = vec![0.0f32; channels * plane];
    // Precompute the sample geometry once per pixel; apply to all channels.
    let mut geom = vec![(0usize, 0usize, 0usize, 0usize, 0.0f32, 0.0f32); plane];
    par_bands(&mut geom, width, |py0, band| {
        for (row_index, row) in band.chunks_mut(width).enumerate() {
            let py = py0 + row_index;
            for (px, slot) in row.iter_mut().enumerate() {
                let index = py * width + px;
                let fx = (px as f32 + fd[index]).clamp(0.0, max_x);
                let fy = (py as f32 + fd[plane + index]).clamp(0.0, max_y);
                let x0 = fx.floor() as usize;
                let y0 = fy.floor() as usize;
                let x1 = (x0 + 1).min(width - 1);
                let y1 = (y0 + 1).min(height - 1);
                *slot = (x0, y0, x1, y1, fx - x0 as f32, fy - y0 as f32);
            }
        }
    });
    par_bands(&mut out, plane, |c0, band| {
        for (ci, channel) in band.chunks_mut(plane).enumerate() {
            let src = &xd[(c0 + ci) * plane..(c0 + ci + 1) * plane];
            for (index, slot) in channel.iter_mut().enumerate() {
                let (x0, y0, x1, y1, lx, ly) = geom[index];
                let top = src[y0 * width + x0] * (1.0 - lx) + src[y0 * width + x1] * lx;
                let bot = src[y1 * width + x0] * (1.0 - lx) + src[y1 * width + x1] * lx;
                *slot = top * (1.0 - ly) + bot * ly;
            }
        }
    });
    prof_add(3, prof_t0);
    Ok(tensor(channels, plane, out))
}

/// RIFE's ResConv tail: `LeakyReLU(conv * beta_c + residual, slope)`.
pub fn res_conv(
    conv: &GpuTensor,
    residual: &GpuTensor,
    beta: &[f32],
    slope: f32,
) -> Result<GpuTensor, String> {
    let channels = conv.rows;
    let plane = conv.cols;
    if beta.len() != channels {
        return Err("rife res_conv: beta length mismatch".to_string());
    }
    let cd = data(conv)?;
    let rd = data(residual)?;
    if rd.len() < channels * plane {
        return Err("rife res_conv: residual shorter than conv".to_string());
    }
    let cd: &[f32] = &cd;
    let rd: &[f32] = &rd;
    let prof_t0 = std::time::Instant::now();
    let mut out = vec![0.0f32; channels * plane];
    par_bands(&mut out, plane, |c0, band| {
        for (ci, channel) in band.chunks_mut(plane).enumerate() {
            let c = c0 + ci;
            let b = beta[c];
            let base = c * plane;
            for (i, slot) in channel.iter_mut().enumerate() {
                let value = cd[base + i] * b + rd[base + i];
                *slot = if value < 0.0 { value * slope } else { value };
            }
        }
    });
    prof_add(5, prof_t0);
    Ok(tensor(channels, plane, out))
}

pub fn lrelu(x: &GpuTensor, slope: f32) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let out = xd
        .iter()
        .map(|v| if *v < 0.0 { *v * slope } else { *v })
        .collect();
    Ok(tensor(x.rows, x.cols, out))
}

pub fn scale(x: &GpuTensor, factor: f32) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    Ok(tensor(x.rows, x.cols, xd.iter().map(|v| *v * factor).collect()))
}

pub fn fill(rows: usize, cols: usize, value: f32) -> Result<GpuTensor, String> {
    Ok(tensor(rows, cols, vec![value; rows * cols]))
}

/// `nn.PixelShuffle(scale)` over `[out_c * scale^2, in_plane]`, plus a per-
/// output-channel bias (the device signature carries one; the model passes
/// zeros).
pub fn pixel_shuffle_planar(
    x: &GpuTensor,
    in_width: usize,
    in_height: usize,
    out_channels: usize,
    scale_factor: usize,
    bias: &[f32],
) -> Result<GpuTensor, String> {
    if scale_factor == 0 || x.rows != out_channels * scale_factor * scale_factor {
        return Err("rife pixel_shuffle: channels not divisible by scale^2".to_string());
    }
    if bias.len() != out_channels {
        return Err("rife pixel_shuffle: bias length mismatch".to_string());
    }
    let in_plane = in_width * in_height;
    let (out_w, out_h) = (in_width * scale_factor, in_height * scale_factor);
    let out_plane = out_w * out_h;
    let xd = data(x)?;
    let xd: &[f32] = &xd;
    let prof_t0 = std::time::Instant::now();
    let mut out = vec![0.0f32; out_channels * out_plane];
    par_bands(&mut out, out_plane, |c0, band| {
        for (ci, channel) in band.chunks_mut(out_plane).enumerate() {
            let c = c0 + ci;
            let b = bias[c];
            for oy in 0..out_h {
                let (iy, ky) = (oy / scale_factor, oy % scale_factor);
                for ox in 0..out_w {
                    let (ix, kx) = (ox / scale_factor, ox % scale_factor);
                    let feature = (c * scale_factor + ky) * scale_factor + kx;
                    channel[oy * out_w + ox] =
                        xd[feature * in_plane + iy * in_width + ix] + b;
                }
            }
        }
    });
    prof_add(4, prof_t0);
    Ok(tensor(out_channels, out_plane, out))
}

/// Final merge: sigmoid the mask, blend the two warped RGB planes, crop the
/// padding, and quantize to interleaved RGB8.
#[allow(clippy::too_many_arguments)]
pub fn merge_rgb8(
    warped0: &GpuTensor,
    warped1: &GpuTensor,
    mask: &GpuTensor,
    padded_width: usize,
    padded_height: usize,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, String> {
    let plane = padded_width * padded_height;
    let w0 = data(warped0)?;
    let w1 = data(warped1)?;
    let md = data(mask)?;
    if w0.len() < 3 * plane || w1.len() < 3 * plane || md.len() < plane {
        return Err("rife merge: extent mismatch".to_string());
    }
    let mut out = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let src = y * padded_width + x;
            let m = 1.0 / (1.0 + (-md[src]).exp());
            let dst = (y * width + x) * 3;
            for c in 0..3 {
                let value = w0[c * plane + src] * m + w1[c * plane + src] * (1.0 - m);
                out[dst + c] = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    Ok(out)
}
