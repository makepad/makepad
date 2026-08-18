//! MiniMax-Music3 Flow-VAE / DAC vocoder. CPU f32 (430-latent clip is ~0.2s).
//! Matches `MiniMaxMusic3Vocoder`: fold 128-ch latents into two 64-ch streams,
//! upsample 8×8×4×2, tanh, reshape stereo.

use crate::music3::{
    MUSIC3_VAE_DEC_HIDDEN, MUSIC3_VAE_DEC_IN, MUSIC3_VAE_LATENT, MUSIC3_VAE_UPSAMPLE,
};
use crate::music3_weights::Music3Shards;
use crate::sa3::par_rows;
use crate::{DiffusionError, Result};
use std::path::Path;

struct Conv {
    w: Vec<f32>,
    b: Vec<f32>,
    out_ch: usize,
    in_ch: usize,
    k: usize,
    dilation: usize,
    padding: usize,
}

struct TConv {
    w: Vec<f32>,
    b: Vec<f32>,
    in_ch: usize,
    out_ch: usize,
    k: usize,
    stride: usize,
    padding: usize,
    /// Tap-packed `[s*out_ch, in_ch]` matrices for the GEMM path (built once
    /// at load): row `r*out_ch+o` = kernel tap `r` (lo) / `r+s` (hi).
    tap_lo: Vec<f32>,
    tap_hi: Vec<f32>,
}

fn tconv_tap_matrices(
    w: &[f32],
    in_ch: usize,
    out_ch: usize,
    k: usize,
    s: usize,
) -> (Vec<f32>, Vec<f32>) {
    if k != 2 * s {
        return (Vec::new(), Vec::new());
    }
    let n = s * out_ch;
    let mut lo = vec![0f32; n * in_ch];
    let mut hi = vec![0f32; n * in_ch];
    for c in 0..in_ch {
        for o in 0..out_ch {
            let w_row = &w[(c * out_ch + o) * k..(c * out_ch + o + 1) * k];
            for r in 0..s {
                lo[(r * out_ch + o) * in_ch + c] = w_row[r];
                hi[(r * out_ch + o) * in_ch + c] = w_row[r + s];
            }
        }
    }
    (lo, hi)
}

struct ResUnit {
    alpha1: Vec<f32>,
    conv1: Conv,
    alpha2: Vec<f32>,
    conv2: Conv,
}

struct DecBlock {
    alpha: Vec<f32>,
    up: TConv,
    units: [ResUnit; 3],
}

struct Plane {
    ch: usize,
    len: usize,
    data: Vec<f32>,
}

fn norm_weight_conv(g: &[f32], v: &[f32], out_ch: usize, per: usize) -> Vec<f32> {
    let mut w = vec![0f32; out_ch * per];
    for o in 0..out_ch {
        let row = &v[o * per..(o + 1) * per];
        let mut sum = 0f64;
        for value in row {
            sum += (*value as f64) * (*value as f64);
        }
        let scale = g[o] as f64 / sum.sqrt().max(1e-30);
        for (i, value) in row.iter().enumerate() {
            w[o * per + i] = (*value as f64 * scale) as f32;
        }
    }
    w
}

pub struct Music3Vocoder {
    dec_in: Conv,
    conv_in: Conv,
    blocks: Vec<DecBlock>,
    alpha_out: Vec<f32>,
    conv_out: Conv,
}

impl Music3Vocoder {
    pub fn load(model_dir: &Path) -> Result<Self> {
        let w = Music3Shards::load(model_dir.join("vocoder"))?;
        Self::load_with(|name| w.tensor_f32(name))
    }

    /// Same names as the official `MiniMaxMusic3Vocoder` / audio.cpp vocoder GGUF.
    pub fn load_with(mut tensor_f32: impl FnMut(&str) -> Result<Vec<f32>>) -> Result<Self> {
        fn wn(
            tensor_f32: &mut dyn FnMut(&str) -> Result<Vec<f32>>,
            prefix: &str,
            out_ch: usize,
            in_ch: usize,
            k: usize,
            dil: usize,
            pad: usize,
        ) -> Result<Conv> {
            let g = tensor_f32(&format!("{prefix}.weight_g"))?;
            let v = tensor_f32(&format!("{prefix}.weight_v"))?;
            let b = tensor_f32(&format!("{prefix}.bias"))?;
            if g.len() != out_ch || v.len() != out_ch * in_ch * k || b.len() != out_ch {
                return Err(DiffusionError::model(format!(
                    "vocoder {prefix} shape g={} v={} b={}",
                    g.len(),
                    v.len(),
                    b.len()
                )));
            }
            Ok(Conv {
                w: norm_weight_conv(&g, &v, out_ch, in_ch * k),
                b,
                out_ch,
                in_ch,
                k,
                dilation: dil,
                padding: pad,
            })
        }
        fn tconv(
            tensor_f32: &mut dyn FnMut(&str) -> Result<Vec<f32>>,
            prefix: &str,
            in_ch: usize,
            out_ch: usize,
            stride: usize,
        ) -> Result<TConv> {
            let k = 2 * stride;
            let g = tensor_f32(&format!("{prefix}.weight_g"))?;
            let v = tensor_f32(&format!("{prefix}.weight_v"))?;
            let b = tensor_f32(&format!("{prefix}.bias"))?;
            let w = norm_weight_conv(&g, &v, in_ch, out_ch * k);
            let (tap_lo, tap_hi) = tconv_tap_matrices(&w, in_ch, out_ch, k, stride);
            Ok(TConv {
                w,
                b,
                in_ch,
                out_ch,
                k,
                stride,
                padding: stride.div_ceil(2),
                tap_lo,
                tap_hi,
            })
        }
        let get = &mut tensor_f32;
        let half = MUSIC3_VAE_LATENT / 2;
        let dec_w = get("dec_in_proj.weight")?;
        let dec_b = get("dec_in_proj.bias")?;
        let dec_in = Conv {
            w: dec_w,
            b: dec_b,
            out_ch: MUSIC3_VAE_DEC_IN,
            in_ch: half,
            k: 1,
            dilation: 1,
            padding: 0,
        };
        let conv_in = wn(get, "conv_in", MUSIC3_VAE_DEC_HIDDEN, MUSIC3_VAE_DEC_IN, 7, 1, 3)?;
        let mut blocks = Vec::new();
        for (i, &stride) in MUSIC3_VAE_UPSAMPLE.iter().enumerate() {
            let in_ch = MUSIC3_VAE_DEC_HIDDEN >> i;
            let out_ch = MUSIC3_VAE_DEC_HIDDEN >> (i + 1);
            let p = format!("blocks.{i}");
            let unit = |get: &mut dyn FnMut(&str) -> Result<Vec<f32>>,
                        u: usize,
                        dil: usize|
             -> Result<ResUnit> {
                let up = format!("{p}.res_unit{u}");
                Ok(ResUnit {
                    alpha1: get(&format!("{up}.snake1.alpha"))?,
                    conv1: wn(get, &format!("{up}.conv1"), out_ch, out_ch, 7, dil, 3 * dil)?,
                    alpha2: get(&format!("{up}.snake2.alpha"))?,
                    conv2: wn(get, &format!("{up}.conv2"), out_ch, out_ch, 1, 1, 0)?,
                })
            };
            blocks.push(DecBlock {
                alpha: get(&format!("{p}.snake1.alpha"))?,
                up: tconv(get, &format!("{p}.conv_t1"), in_ch, out_ch, stride)?,
                units: [unit(get, 1, 1)?, unit(get, 2, 3)?, unit(get, 3, 9)?],
            });
        }
        Ok(Self {
            dec_in,
            conv_in,
            blocks,
            alpha_out: get("snake_out.alpha")?,
            conv_out: wn(
                get,
                "conv_out",
                1,
                MUSIC3_VAE_DEC_HIDDEN >> MUSIC3_VAE_UPSAMPLE.len(),
                7,
                1,
                3,
            )?,
        })
    }

    /// `latents` channel-major `[128, T]`. Returns stereo `[2, T*512]`.
    pub fn decode(&self, latents: &[f32], frames: usize) -> Result<Vec<f32>> {
        if latents.len() != MUSIC3_VAE_LATENT * frames {
            return Err(DiffusionError::model(format!(
                "vocoder latents {} expected {}",
                latents.len(),
                MUSIC3_VAE_LATENT * frames
            )));
        }
        let half = MUSIC3_VAE_LATENT / 2;
        let hop: usize = MUSIC3_VAE_UPSAMPLE.iter().product();
        let mut stereo = vec![0f32; 2 * frames * hop];
        let decode_ch = |ch: usize| -> Plane {
            let mut plane = Plane {
                ch: half,
                len: frames,
                data: latents[ch * half * frames..(ch + 1) * half * frames].to_vec(),
            };
            plane = conv1d(&plane, &self.dec_in);
            plane = conv1d(&plane, &self.conv_in);
            for block in &self.blocks {
                snake(&mut plane, &block.alpha);
                plane = conv_transpose1d(&plane, &block.up);
                for unit in &block.units {
                    let mut y = Plane {
                        ch: plane.ch,
                        len: plane.len,
                        data: plane.data.clone(),
                    };
                    snake(&mut y, &unit.alpha1);
                    y = conv1d(&y, &unit.conv1);
                    snake(&mut y, &unit.alpha2);
                    y = conv1d(&y, &unit.conv2);
                    for (x, yv) in plane.data.iter_mut().zip(y.data.iter()) {
                        *x += *yv;
                    }
                }
            }
            snake(&mut plane, &self.alpha_out);
            plane = conv1d(&plane, &self.conv_out);
            for v in &mut plane.data {
                *v = v.tanh();
            }
            plane
        };
        // Metal context is thread-local, so the two stereo streams can run together.
        std::thread::scope(|scope| {
            let right = scope.spawn(|| decode_ch(1));
            let left = decode_ch(0);
            let right = right.join().expect("vocoder right");
            let n = frames * hop;
            stereo[..n].copy_from_slice(&left.data[..n.min(left.data.len())]);
            stereo[n..].copy_from_slice(&right.data[..n.min(right.data.len())]);
        });
        eprintln!(
            "  voc-perf snake={:.2}s conv={:.2}s tconv={:.2}s (gemm={:.2}s cpu_gemm={:.2}s)",
            vperf::secs(&vperf::SNAKE),
            vperf::secs(&vperf::CONV),
            vperf::secs(&vperf::TCONV),
            vperf::secs(&vperf::TCONV_GEMM),
            vperf::secs(&vperf::TCONV_CPU),
        );
        Ok(stereo)
    }

    /// CUDA decode (time-major gemm taps + snake kernel). Same layout as
    /// [`Self::decode`].
    pub fn decode_cuda(&self, latents: &[f32], frames: usize) -> Result<Vec<f32>> {
        decode_cuda(self, latents, frames)
    }
}

/// Per-op wall-clock for one `decode`, printed as `voc-perf`.
mod vperf {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static SNAKE: AtomicU64 = AtomicU64::new(0);
    pub static CONV: AtomicU64 = AtomicU64::new(0);
    pub static TCONV: AtomicU64 = AtomicU64::new(0);
    pub static TCONV_GEMM: AtomicU64 = AtomicU64::new(0);
    pub static TCONV_CPU: AtomicU64 = AtomicU64::new(0);

    pub fn add(slot: &AtomicU64, t0: std::time::Instant) {
        slot.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    pub fn secs(slot: &AtomicU64) -> f32 {
        slot.load(Ordering::Relaxed) as f32 / 1e9
    }
}

fn snake(p: &mut Plane, alpha: &[f32]) {
    let t0 = std::time::Instant::now();
    let len = p.len;
    par_rows(&mut p.data, len, &|ch, row| {
        let a = alpha[ch];
        let inv = 1.0 / (a + 1e-9);
        for v in row.iter_mut() {
            let s = (a * *v).sin();
            *v += inv * s * s;
        }
    });
    vperf::add(&vperf::SNAKE, t0);
}

fn conv1d(input: &Plane, conv: &Conv) -> Plane {
    let t0 = std::time::Instant::now();
    let len = input.len;
    let span = (conv.k - 1) * conv.dilation;
    let out_len = len + 2 * conv.padding - span;
    if let Some(plane) = conv1d_metal(input, conv, out_len) {
        vperf::add(&vperf::CONV, t0);
        return plane;
    }
    let mut out = vec![0f32; conv.out_ch * out_len];
    conv1d_cpu(input, conv, out_len, &mut out);
    vperf::add(&vperf::CONV, t0);
    Plane {
        ch: conv.out_ch,
        len: out_len,
        data: out,
    }
}

fn conv1d_metal(input: &Plane, conv: &Conv, out_len: usize) -> Option<Plane> {
    let k_col = conv.in_ch.saturating_mul(conv.k);
    let macs = out_len.saturating_mul(k_col).saturating_mul(conv.out_ch);
    if macs < 4 * 1024 * 1024 || out_len == 0 || k_col == 0 {
        return None;
    }
    // Chunk bounds the im2col buffer (col ≤ ~44MB at 16Ki rows × widest
    // k_col); fewer chunks = fewer submits + W re-uploads. Chunking cannot
    // change results: every output row's dot is computed identically.
    let chunk = (16 * 4096).min(out_len);
    let mut tm = vec![0f32; out_len * conv.out_ch];
    let dil = conv.dilation as isize;
    let pad = conv.padding as isize;
    let t_in = input.len as isize;
    let mut col = vec![0f32; chunk * k_col];
    for t0 in (0..out_len).step_by(chunk) {
        let take = (out_len - t0).min(chunk);
        let col = &mut col[..take * k_col];
        par_rows(col, k_col, &|ti, row| {
            let t = (t0 + ti) as isize;
            for c in 0..conv.in_ch {
                let in_row = &input.data[c * input.len..(c + 1) * input.len];
                for tap in 0..conv.k {
                    let src = t + tap as isize * dil - pad;
                    row[c * conv.k + tap] = if src >= 0 && src < t_in {
                        in_row[src as usize]
                    } else {
                        0.0
                    };
                }
            }
        });
        let y = crate::metal_accel::linear_nt(
            &col,
            &conv.w,
            Some(&conv.b),
            take,
            k_col,
            conv.out_ch,
        )?;
        tm[t0 * conv.out_ch..(t0 + take) * conv.out_ch].copy_from_slice(&y);
    }
    let mut out = vec![0f32; conv.out_ch * out_len];
    par_rows(&mut out, out_len, &|o, row| {
        for (t, slot) in row.iter_mut().enumerate() {
            *slot = tm[t * conv.out_ch + o];
        }
    });
    Some(Plane {
        ch: conv.out_ch,
        len: out_len,
        data: out,
    })
}

fn conv1d_cpu(input: &Plane, conv: &Conv, out_len: usize, out: &mut [f32]) {
    par_rows(out, out_len, &|o, out_row| {
        conv1d_one_row(input, conv, out_len, o, out_row);
    });
}

fn conv1d_one_row(input: &Plane, conv: &Conv, out_len: usize, o: usize, out_row: &mut [f32]) {
    conv1d_one_row_range(input, conv, o, 0, out_len, out_row);
}

fn conv1d_one_row_range(
    input: &Plane,
    conv: &Conv,
    o: usize,
    t0: usize,
    t1: usize,
    out_row: &mut [f32],
) {
    let len = input.len;
    out_row.fill(conv.b[o]);
    let w_base = &conv.w[o * conv.in_ch * conv.k..(o + 1) * conv.in_ch * conv.k];
    for c in 0..conv.in_ch {
        let in_row = &input.data[c * len..(c + 1) * len];
        for tap in 0..conv.k {
            let w = w_base[c * conv.k + tap];
            if w == 0.0 {
                continue;
            }
            let off = tap as isize * conv.dilation as isize - conv.padding as isize;
            let t_start = ((-off).max(0) as usize).max(t0);
            let t_end = ((len as isize - off).min(t1 as isize)).max(0) as usize;
            if t_start >= t_end {
                continue;
            }
            let src = &in_row[(t_start as isize + off) as usize
                ..(t_start as isize + off) as usize + (t_end - t_start)];
            let dst = &mut out_row[t_start - t0..t_end - t0];
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                *d += w * *s;
            }
        }
    }
}

fn conv_transpose1d(input: &Plane, conv: &TConv) -> Plane {
    let t0 = std::time::Instant::now();
    let plane = if let Some(plane) = conv_transpose1d_taps(input, conv) {
        plane
    } else {
        conv_transpose1d_scatter(input, conv)
    };
    vperf::add(&vperf::TCONV, t0);
    plane
}

/// Same math as [`conv_transpose1d_scatter`] arranged as two dense GEMMs
/// (mirrors `dev_tconv` on the CUDA path). With `k == 2*stride`, output
/// sample `t` receives exactly two kernel taps: tap `r = (t+pad) % s` of
/// source `(t+pad)/s` and tap `r + s` of source `(t+pad)/s - 1`. Packing
/// taps `[0,s)` / `[s,2s)` into `[s*out_ch, in_ch]` matrices turns the
/// whole tconv into `X_tm · W^T` twice plus a bias-seeded interleave.
fn conv_transpose1d_taps(input: &Plane, conv: &TConv) -> Option<Plane> {
    let len = input.len;
    let s = conv.stride;
    if conv.k != 2 * s || len == 0 || conv.tap_lo.is_empty() {
        return None;
    }
    let out_len = (len - 1) * s + conv.k - 2 * conv.padding;
    let (in_ch, out_ch) = (conv.in_ch, conv.out_ch);
    let n = s * out_ch;
    let (w_lo, w_hi) = (&conv.tap_lo, &conv.tap_hi);
    // Channel-major plane -> time-major rows for the GEMM lhs.
    let mut x_tm = vec![0f32; len * in_ch];
    par_rows(&mut x_tm, in_ch, &|t, row| {
        for (c, slot) in row.iter_mut().enumerate() {
            *slot = input.data[c * len + t];
        }
    });
    let t0 = std::time::Instant::now();
    let (y_lo, y_hi) = tconv_tap_gemms(&x_tm, w_lo, w_hi, len, in_ch, n)?;
    vperf::add(&vperf::TCONV_GEMM, t0);
    // Bias-seeded interleave: out[o][t] = b[o] + lo[src][r*out_ch+o] + hi[src-1][...].
    let mut out = vec![0f32; out_ch * out_len];
    par_rows(&mut out, out_len, &|o, out_row| {
        let b = conv.b[o];
        for (t, slot) in out_row.iter_mut().enumerate() {
            let tp = t + conv.padding;
            let src = tp / s;
            let r = tp % s;
            let mut acc = b;
            if src < len {
                acc += y_lo[src * n + r * out_ch + o];
            }
            if src >= 1 && src - 1 < len {
                acc += y_hi[(src - 1) * n + r * out_ch + o];
            }
            *slot = acc;
        }
    });
    Some(Plane {
        ch: out_ch,
        len: out_len,
        data: out,
    })
}

/// `Y = X[m,k] · W[n,k]^T` for both tap matrices — one Metal submit with a
/// single lhs upload when available, else threaded CPU rows.
fn tconv_tap_gemms(
    x_tm: &[f32],
    w_lo: &[f32],
    w_hi: &[f32],
    m: usize,
    k: usize,
    n: usize,
) -> Option<(Vec<f32>, Vec<f32>)> {
    use makepad_ggml::backend::metal::{try_matmul_nt_ggml_bytes_multi, MatmulNtGgmlBytesMatrix};
    use makepad_ggml::tensor::TensorType;

    fn f32_bytes(v: &[f32]) -> &[u8] {
        unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4) }
    }

    if crate::metal_accel::gemm_enabled() && m * k * n >= 4 * 1024 * 1024 {
        let mats = [
            MatmulNtGgmlBytesMatrix {
                bt_bytes: f32_bytes(w_lo),
                bt_ggml_type: TensorType::F32.ggml_type(),
                n,
            },
            MatmulNtGgmlBytesMatrix {
                bt_bytes: f32_bytes(w_hi),
                bt_ggml_type: TensorType::F32.ggml_type(),
                n,
            },
        ];
        if let Some(mut ys) = try_matmul_nt_ggml_bytes_multi(x_tm, m, k, &mats) {
            if ys.len() == 2
                && ys.iter().all(|y| {
                    y.len() == m * n && y.iter().any(|v| v.is_finite() && *v != 0.0)
                })
            {
                let y_hi = ys.pop().expect("tconv hi");
                let y_lo = ys.pop().expect("tconv lo");
                return Some((y_lo, y_hi));
            }
        }
    }
    let t0 = std::time::Instant::now();
    let mut y_lo = vec![0f32; m * n];
    let mut y_hi = vec![0f32; m * n];
    for (w, y) in [(w_lo, &mut y_lo), (w_hi, &mut y_hi)] {
        par_rows(y, n, &|t, row| {
            let x_row = &x_tm[t * k..(t + 1) * k];
            for (j, slot) in row.iter_mut().enumerate() {
                let w_row = &w[j * k..(j + 1) * k];
                let mut acc = 0f32;
                for i in 0..k {
                    acc += x_row[i] * w_row[i];
                }
                *slot = acc;
            }
        });
    }
    vperf::add(&vperf::TCONV_CPU, t0);
    Some((y_lo, y_hi))
}

fn conv_transpose1d_scatter(input: &Plane, conv: &TConv) -> Plane {
    let len = input.len;
    let out_len = (len - 1) * conv.stride + conv.k - 2 * conv.padding;
    let mut out = vec![0f32; conv.out_ch * out_len];
    par_rows(&mut out, out_len, &|o, out_row| {
        out_row.fill(conv.b[o]);
        for c in 0..conv.in_ch {
            let in_row = &input.data[c * len..(c + 1) * len];
            let w_row = &conv.w[(c * conv.out_ch + o) * conv.k..(c * conv.out_ch + o + 1) * conv.k];
            for (tap, &w) in w_row.iter().enumerate() {
                if w == 0.0 {
                    continue;
                }
                let base = tap as isize - conv.padding as isize;
                let src_start = if base < 0 {
                    ((-base) as usize).div_ceil(conv.stride)
                } else {
                    0
                };
                let src_end = if base >= out_len as isize {
                    0
                } else {
                    (((out_len as isize - base - 1) / conv.stride as isize) as usize + 1).min(len)
                };
                let mut t = (src_start as isize * conv.stride as isize + base) as usize;
                for src in src_start..src_end {
                    out_row[t] += w * in_row[src];
                    t += conv.stride;
                }
            }
        }
    });
    Plane {
        ch: conv.out_ch,
        len: out_len,
        data: out,
    }
}

use crate::sa3::F16Weight;
use makepad_ggml::backend::cuda::{
    gpu_add, gpu_concat_rows, gpu_download, gpu_gather_rows_colblock, gpu_linear_nt_cached,
    gpu_slice_rows, gpu_snake_cols, gpu_upload, gpu_upload_u32, GpuTensor,
};

struct DevConv {
    taps: Vec<F16Weight>,
    bias: Vec<f32>,
    dilation: usize,
    padding: usize,
    in_ch: usize,
    out_ch: usize,
}

struct DevTConv {
    lo: F16Weight,
    hi: F16Weight,
    bias_tiled: Vec<f32>,
    stride: usize,
    padding: usize,
    out_ch: usize,
}

struct DevRes {
    a1: Vec<f32>,
    c1: DevConv,
    a2: Vec<f32>,
    c2: DevConv,
}

fn dev_conv(conv: &Conv, key: &str) -> DevConv {
    let (out_ch, in_ch, k) = (conv.out_ch, conv.in_ch, conv.k);
    let mut taps = Vec::with_capacity(k);
    for tap in 0..k {
        let mut w = vec![0f32; out_ch * in_ch];
        for o in 0..out_ch {
            for c in 0..in_ch {
                w[o * in_ch + c] = conv.w[(o * in_ch + c) * k + tap];
            }
        }
        taps.push(F16Weight::new(format!("{key}.t{tap}"), &w, out_ch, in_ch));
    }
    DevConv {
        taps,
        bias: conv.b.clone(),
        dilation: conv.dilation,
        padding: conv.padding,
        in_ch,
        out_ch,
    }
}

fn dev_tconv(conv: &TConv, key: &str) -> DevTConv {
    let (in_ch, out_ch, k, s) = (conv.in_ch, conv.out_ch, conv.k, conv.stride);
    let mut lo = vec![0f32; s * out_ch * in_ch];
    let mut hi = vec![0f32; s * out_ch * in_ch];
    for r in 0..s {
        for o in 0..out_ch {
            for c in 0..in_ch {
                lo[(r * out_ch + o) * in_ch + c] = conv.w[(c * out_ch + o) * k + r];
                hi[(r * out_ch + o) * in_ch + c] = conv.w[(c * out_ch + o) * k + r + s];
            }
        }
    }
    let mut bias_tiled = Vec::with_capacity(s * out_ch);
    for _ in 0..s {
        bias_tiled.extend_from_slice(&conv.b);
    }
    DevTConv {
        lo: F16Weight::new(format!("{key}.lo"), &lo, s * out_ch, in_ch),
        hi: F16Weight::new(format!("{key}.hi"), &hi, s * out_ch, in_ch),
        bias_tiled,
        stride: s,
        padding: conv.padding,
        out_ch,
    }
}

fn conv1d_dev(x: &GpuTensor, len: usize, conv: &DevConv) -> Result<GpuTensor> {
    let span = (conv.taps.len() - 1) * conv.dilation;
    let out_len = len + 2 * conv.padding - span;
    let padded = if conv.padding > 0 {
        let zeros = gpu_upload(&vec![0f32; conv.padding * conv.in_ch], conv.padding, conv.in_ch)
            .map_err(DiffusionError::model)?;
        let top = gpu_concat_rows(&zeros, x).map_err(DiffusionError::model)?;
        gpu_concat_rows(&top, &zeros).map_err(DiffusionError::model)?
    } else {
        gpu_slice_rows(x, 0, len).map_err(DiffusionError::model)?
    };
    let mut out: Option<GpuTensor> = None;
    for (tap, weight) in conv.taps.iter().enumerate() {
        let shifted =
            gpu_slice_rows(&padded, tap * conv.dilation, out_len).map_err(DiffusionError::model)?;
        let bias: &[f32] = if tap == 0 { &conv.bias } else { &[] };
        let y = gpu_linear_nt_cached(&shifted, "music3vae", &[weight.part()], bias)
            .map_err(DiffusionError::model)?;
        out = Some(match out {
            None => y,
            Some(acc) => gpu_add(&acc, &y).map_err(DiffusionError::model)?,
        });
    }
    Ok(out.expect("conv taps"))
}

fn tconv_dev(x: &GpuTensor, len: usize, conv: &DevTConv) -> Result<GpuTensor> {
    let s = conv.stride;
    let k = 2 * s;
    let out_len = (len - 1) * s + k - 2 * conv.padding;
    let y_hi_src = gpu_linear_nt_cached(x, "music3vae", &[conv.lo.part()], &conv.bias_tiled)
        .map_err(DiffusionError::model)?;
    let y_lo_src = gpu_linear_nt_cached(x, "music3vae", &[conv.hi.part()], &[])
        .map_err(DiffusionError::model)?;
    let mut row_hi = vec![0u32; out_len];
    let mut row_lo = vec![0u32; out_len];
    let mut blocks = vec![0u32; out_len];
    for t in 0..out_len {
        let tp = t + conv.padding;
        let src = (tp / s) as isize;
        row_hi[t] = if src < len as isize { src as u32 } else { u32::MAX };
        row_lo[t] = if src - 1 >= 0 && src - 1 < len as isize {
            (src - 1) as u32
        } else {
            u32::MAX
        };
        blocks[t] = (tp % s) as u32;
    }
    let row_hi = gpu_upload_u32(&row_hi).map_err(DiffusionError::model)?;
    let row_lo = gpu_upload_u32(&row_lo).map_err(DiffusionError::model)?;
    let blocks = gpu_upload_u32(&blocks).map_err(DiffusionError::model)?;
    let hi = gpu_gather_rows_colblock(&y_hi_src, &row_hi, Some(&blocks), conv.out_ch)
        .map_err(DiffusionError::model)?;
    let lo = gpu_gather_rows_colblock(&y_lo_src, &row_lo, Some(&blocks), conv.out_ch)
        .map_err(DiffusionError::model)?;
    gpu_add(&hi, &lo).map_err(DiffusionError::model)
}

fn snake_dev(x: &GpuTensor, alpha: &[f32]) -> Result<GpuTensor> {
    let alpha_dev = gpu_upload(alpha, 1, alpha.len()).map_err(DiffusionError::model)?;
    gpu_snake_cols(x, &alpha_dev).map_err(DiffusionError::model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(state: &mut u64) -> f32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*state >> 33) as f32 / (1u64 << 31) as f32) - 1.0
    }

    #[test]
    fn tconv_taps_matches_scatter_reference() {
        // Same stride/pad family as the real blocks ([8, 8, 4, 2], k = 2s,
        // pad = ceil(s/2)); sizes stay tiny so the GEMM path is pure CPU.
        for &(in_ch, out_ch, stride, len) in
            &[(6usize, 4usize, 8usize, 5usize), (5, 3, 4, 7), (4, 2, 2, 9), (3, 5, 8, 1)]
        {
            let k = 2 * stride;
            let mut state = 0x9E3779B97F4A7C15u64 ^ (in_ch * 31 + stride) as u64;
            let w: Vec<f32> = (0..in_ch * out_ch * k).map(|_| lcg(&mut state)).collect();
            let b: Vec<f32> = (0..out_ch).map(|_| lcg(&mut state)).collect();
            let x: Vec<f32> = (0..in_ch * len).map(|_| lcg(&mut state)).collect();
            let (tap_lo, tap_hi) = tconv_tap_matrices(&w, in_ch, out_ch, k, stride);
            let conv = TConv {
                w,
                b,
                in_ch,
                out_ch,
                k,
                stride,
                padding: stride.div_ceil(2),
                tap_lo,
                tap_hi,
            };
            let input = Plane {
                ch: in_ch,
                len,
                data: x,
            };
            let reference = conv_transpose1d_scatter(&input, &conv);
            let taps = conv_transpose1d_taps(&input, &conv).expect("taps path");
            assert_eq!(taps.ch, reference.ch);
            assert_eq!(taps.len, reference.len);
            assert_eq!(taps.data.len(), reference.data.len());
            for (i, (a, r)) in taps.data.iter().zip(reference.data.iter()).enumerate() {
                let tol = 1e-4 * r.abs().max(1.0);
                assert!(
                    (a - r).abs() <= tol,
                    "tconv mismatch s={stride} at {i}: taps={a} ref={r}"
                );
            }
        }
    }
}

fn decode_cuda(voc: &Music3Vocoder, latents: &[f32], frames: usize) -> Result<Vec<f32>> {
    if latents.len() != MUSIC3_VAE_LATENT * frames {
        return Err(DiffusionError::model("vocoder cuda latent size"));
    }
    let half = MUSIC3_VAE_LATENT / 2;
    let hop: usize = MUSIC3_VAE_UPSAMPLE.iter().product();
    let dec_in = F16Weight::new(
        "music3vae.decin",
        &voc.dec_in.w,
        voc.dec_in.out_ch,
        voc.dec_in.in_ch,
    );
    let conv_in = dev_conv(&voc.conv_in, "music3vae.in");
    let blocks: Vec<(Vec<f32>, DevTConv, [DevRes; 3])> = voc
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            (
                b.alpha.clone(),
                dev_tconv(&b.up, &format!("music3vae.b{i}.up")),
                [0, 1, 2].map(|u| DevRes {
                    a1: b.units[u].alpha1.clone(),
                    c1: dev_conv(&b.units[u].conv1, &format!("music3vae.b{i}.u{u}.c1")),
                    a2: b.units[u].alpha2.clone(),
                    c2: dev_conv(&b.units[u].conv2, &format!("music3vae.b{i}.u{u}.c2")),
                }),
            )
        })
        .collect();
    let conv_out = dev_conv(&voc.conv_out, "music3vae.out");
    let mut stereo = vec![0f32; 2 * frames * hop];
    for ch in 0..2 {
        let src = &latents[ch * half * frames..(ch + 1) * half * frames];
        let mut x_host = vec![0f32; frames * half];
        for c in 0..half {
            for t in 0..frames {
                x_host[t * half + c] = src[c * frames + t];
            }
        }
        let x = gpu_upload(&x_host, frames, half).map_err(DiffusionError::model)?;
        let x = gpu_linear_nt_cached(&x, "music3vae", &[dec_in.part()], &voc.dec_in.b)
            .map_err(DiffusionError::model)?;
        let mut plane = conv1d_dev(&x, frames, &conv_in)?;
        let mut len = frames;
        for (alpha, up, units) in &blocks {
            plane = snake_dev(&plane, alpha)?;
            plane = tconv_dev(&plane, len, up)?;
            len *= up.stride;
            for unit in units {
                let mut y = snake_dev(&plane, &unit.a1)?;
                y = conv1d_dev(&y, len, &unit.c1)?;
                y = snake_dev(&y, &unit.a2)?;
                y = conv1d_dev(&y, len, &unit.c2)?;
                plane = gpu_add(&plane, &y).map_err(DiffusionError::model)?;
            }
        }
        plane = snake_dev(&plane, &voc.alpha_out)?;
        plane = conv1d_dev(&plane, len, &conv_out)?;
        let host = gpu_download(&plane).map_err(DiffusionError::model)?;
        for (i, v) in host.iter().enumerate() {
            stereo[ch * frames * hop + i] = v.tanh();
        }
    }
    Ok(stereo)
}
