//! Real-weight UNet2p5D graph prefix: `conv_in`, timestep embed, the first
//! down block's ResNets / downsample, and (via [`crate::unet_attn`]) the first
//! Transformer2D without 2.5D extras.

use crate::cuda_unet::f16_bytes;
use crate::torch_bin::{self, TorchDtype};
use makepad_ai_common::backend::cuda::{
    gpu_add, gpu_add_rows_broadcast, gpu_concat_cols, gpu_concat_rows, gpu_conv2d_nchw_cached,
    gpu_conv2d_nchw_packed, gpu_conv2d_planar_cached, gpu_conv2d_planar_strided, gpu_device_available,
    gpu_download, gpu_group_norm_planar, gpu_linear_f32_resident, gpu_linear_nt_cached,
    gpu_linear_nt_cached_f16, gpu_to_f16, gpu_to_f32,
    gpu_group_norm_nchw, gpu_nchw_add_channel, gpu_nchw_add_channel_inplace, gpu_nchw_to_planar,
    gpu_paint_group_norm_batched,
    gpu_planar_to_nchw, gpu_silu, gpu_slice_cols, gpu_slice_rows,
    gpu_upload, gpu_upsample_nearest2x,
    GpuLinearPart, GpuTensor,
};
use makepad_ai_common::quant::GGML_TYPE_F16;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub(crate) const NS: &str = "paint-unet";
pub(crate) const GN_EPS: f32 = 1e-5;
pub(crate) const GN_GROUPS: usize = 32;
pub(crate) const ATTN_GN_EPS: f32 = 1e-6;
pub(crate) const LN_EPS: f32 = 1e-5;

pub(crate) struct W {
    pub data: Vec<f32>,
    pub shape: Vec<usize>,
}

/// Packed contiguous NCHW `[N, C*H*W]` (rows=N, cols=C*H*W). Official
/// `rearrange("b n_pbr n c h w -> (b n_pbr n) c h w")` as a single tensor.
pub(crate) struct BatchAct {
    pub t: GpuTensor,
    pub n: usize,
    pub c: usize,
    pub w: usize,
    pub h: usize,
}

impl BatchAct {
    pub(crate) fn plane(&self) -> usize {
        self.w * self.h
    }

    pub(crate) fn stacked_h(&self) -> usize {
        self.n * self.h
    }

    pub(crate) fn expect(
        &self,
        channels: usize,
        width: usize,
        height: usize,
        ctx: &str,
    ) -> Result<(), String> {
        let want = channels * width * height;
        if self.w != width
            || self.h != height
            || self.c != channels
            || self.t.rows() != self.n
            || self.t.cols() != want
        {
            return Err(format!(
                "{ctx} NCHW wants n={} C={channels} {width}x{height}, got {}x{}",
                self.n,
                self.t.rows(),
                self.t.cols(),
            ));
        }
        Ok(())
    }

    pub(crate) fn clone_act(&self) -> Result<Self, String> {
        Ok(Self {
            t: gpu_slice_rows(&self.t, 0, self.t.rows())?,
            n: self.n,
            c: self.c,
            w: self.w,
            h: self.h,
        })
    }
}

/// Pack N planar `[C, HW]` host images into one `[C, N*HW]` buffer.
pub(crate) fn pack_planar_host(xs: &[&[f32]], channels: usize, plane: usize) -> Result<Vec<f32>, String> {
    let n = xs.len();
    let mut out = vec![0.0f32; channels * n * plane];
    for (i, x) in xs.iter().enumerate() {
        if x.len() != channels * plane {
            return Err(format!(
                "pack_planar_host[{i}] len {} vs {channels}x{plane}",
                x.len()
            ));
        }
        for c in 0..channels {
            let dst = c * n * plane + i * plane;
            let src = c * plane;
            out[dst..dst + plane].copy_from_slice(&x[src..src + plane]);
        }
    }
    Ok(out)
}

/// Split one `[C, N*HW]` host buffer back into N planar images.
pub(crate) fn unpack_planar_host(
    flat: &[f32],
    channels: usize,
    n: usize,
    plane: usize,
) -> Result<Vec<Vec<f32>>, String> {
    if flat.len() != channels * n * plane {
        return Err(format!(
            "unpack_planar_host len {} vs {channels}x{n}x{plane}",
            flat.len()
        ));
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut v = vec![0.0f32; channels * plane];
        for c in 0..channels {
            let src = c * n * plane + i * plane;
            v[c * plane..(c + 1) * plane].copy_from_slice(&flat[src..src + plane]);
        }
        out.push(v);
    }
    Ok(out)
}

struct CachedLinear {
    weight: GpuTensor,
    bias: Option<GpuTensor>,
    f16_bytes: Vec<u8>,
    bias_host: Vec<f32>,
    n: usize,
}

pub struct UnetFirst {
    w: HashMap<String, W>,
    linear_gpu: RefCell<HashMap<String, CachedLinear>>,
    /// Step-invariant encoder K/V (`{prefix}|{tag}`). Same GEMM, once per job.
    enc_kv: RefCell<HashMap<String, (GpuTensor, GpuTensor)>>,
    enc_lin: RefCell<HashMap<String, GpuTensor>>,
}

/// Pin the slow fp32 tap path. Inference binaries leave GEMM/im2col/fp16-attn
/// at their crate defaults (on). Canaries set `MAKEPAD_PBR_TAP_PARITY=1`.
pub fn maybe_pin_tap_parity() {
    if std::env::var("MAKEPAD_PBR_TAP_PARITY").as_deref() == Ok("1") {
        std::env::set_var("FLUX_VAE_CONV_GEMM", "0");
        std::env::set_var("FLUX_VAE_CONV_IM2COL", "0");
        std::env::set_var("FLUX_ATTN_F16", "0");
    }
}

/// Inference extras use fp16 GEMM + hd=64 flash. Tap canaries stay fp32.
pub(crate) fn paint_fast() -> bool {
    std::env::var("MAKEPAD_PBR_TAP_PARITY").as_deref() != Ok("1")
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 1) as u32;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let man = (bits & 0x3ff) as u32;
    let out = if exp == 0 {
        if man == 0 {
            sign << 31
        } else {
            let mut m = man;
            let mut e = 127 - 15 + 1;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (sign << 31) | (e << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xff << 23) | (man << 13)
    } else {
        (sign << 31) | ((exp + (127 - 15)) << 23) | (man << 13)
    };
    f32::from_bits(out)
}

const PREFIXES: &[&str] = &[
    "unet.conv_in.",
    "unet.time_embedding.",
    "unet.down_blocks.",
    "unet.mid_block.",
    "unet.learned_text_clip_albedo",
    "unet.learned_text_clip_mr",
    "unet.learned_text_clip_ref",
    "unet.up_blocks.",
    "unet.conv_norm_out.",
    "unet.conv_out.",
    "unet_dual.conv_in.",
    "unet_dual.time_embedding.",
    "unet_dual.down_blocks.",
    "unet_dual.mid_block.",
    "unet_dual.up_blocks.",
    "unet_dual.conv_norm_out.",
    "unet_dual.conv_out.",
];

impl UnetFirst {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !gpu_device_available() {
            return Err("CUDA unavailable".into());
        }
        maybe_pin_tap_parity();
        let mut file = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
        let index = torch_bin::read_index_from(&mut file).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        let mut w = HashMap::new();
        for record in &index.tensors {
            if !PREFIXES.iter().any(|p| record.name.starts_with(p)) {
                continue;
            }
            index
                .read_tensor_into(&mut file, record, &mut raw)
                .map_err(|e| e.to_string())?;
            let data = match record.dtype {
                TorchDtype::F32 => raw
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect(),
                TorchDtype::F16 => raw
                    .chunks_exact(2)
                    .map(|b| half_to_f32(u16::from_le_bytes([b[0], b[1]])))
                    .collect(),
                other => return Err(format!("{} {other:?}", record.name)),
            };
            w.insert(
                record.name.clone(),
                W {
                    data,
                    shape: record.shape.clone(),
                },
            );
        }
        if !w.contains_key("unet.conv_in.weight") {
            return Err(format!(
                "unet.conv_in.weight missing; have {}",
                w.len()
            ));
        }
        Ok(Self {
            w,
            linear_gpu: RefCell::new(HashMap::new()),
            enc_kv: RefCell::new(HashMap::new()),
            enc_lin: RefCell::new(HashMap::new()),
        })
    }

    fn cached_linear(&self, prefix: &str, bias: bool) -> Result<(), String> {
        if self.linear_gpu.borrow().contains_key(prefix) {
            return Ok(());
        }
        let weight = self.get(&format!("{prefix}.weight"))?;
        if weight.shape.len() != 2 {
            return Err(format!("{prefix} weight shape {:?}", weight.shape));
        }
        let w = gpu_upload(&weight.data, weight.shape[0], weight.shape[1])?;
        let (b, bias_host) = if bias {
            let bias = self.get(&format!("{prefix}.bias"))?;
            (
                Some(gpu_upload(&bias.data, 1, bias.data.len())?),
                bias.data.clone(),
            )
        } else {
            (None, Vec::new())
        };
        self.linear_gpu.borrow_mut().insert(
            prefix.to_string(),
            CachedLinear {
                weight: w,
                bias: b,
                f16_bytes: f16_bytes(&weight.data),
                bias_host,
                n: weight.shape[0],
            },
        );
        Ok(())
    }

    pub(crate) fn linear_cached(
        &self,
        x: &GpuTensor,
        prefix: &str,
        bias: bool,
    ) -> Result<GpuTensor, String> {
        self.cached_linear(prefix, bias)?;
        let cache = self.linear_gpu.borrow();
        let lin = cache
            .get(prefix)
            .ok_or_else(|| format!("linear cache miss {prefix}"))?;
        if paint_fast() && x.is_half() {
            gpu_linear_nt_cached_f16(
                x,
                NS,
                &[GpuLinearPart {
                    bt_ggml_type: GGML_TYPE_F16,
                    n: lin.n,
                    cache_key: prefix,
                    bytes: &lin.f16_bytes,
                }],
                &lin.bias_host,
            )
        } else if paint_fast() {
            gpu_linear_nt_cached(
                x,
                NS,
                &[GpuLinearPart {
                    bt_ggml_type: GGML_TYPE_F16,
                    n: lin.n,
                    cache_key: prefix,
                    bytes: &lin.f16_bytes,
                }],
                &lin.bias_host,
            )
        } else {
            gpu_linear_f32_resident(x, &lin.weight, lin.bias.as_ref())
        }
    }

    /// One GEMM for bias-free Q/K/V, then column splits. Official fused qkv.
    pub(crate) fn linear_qkv(
        &self,
        x: &GpuTensor,
        q: &str,
        k: &str,
        v: &str,
    ) -> Result<(GpuTensor, GpuTensor, GpuTensor), String> {
        self.cached_linear(q, false)?;
        self.cached_linear(k, false)?;
        self.cached_linear(v, false)?;
        let cache = self.linear_gpu.borrow();
        let lq = cache.get(q).ok_or_else(|| format!("linear cache miss {q}"))?;
        let lk = cache.get(k).ok_or_else(|| format!("linear cache miss {k}"))?;
        let lv = cache.get(v).ok_or_else(|| format!("linear cache miss {v}"))?;
        if lq.n != lk.n || lk.n != lv.n {
            return Err(format!("qkv n {} {} {}", lq.n, lk.n, lv.n));
        }
        let hidden = lq.n;
        let qkv = if paint_fast() && x.is_half() {
            gpu_linear_nt_cached_f16(
                x,
                NS,
                &[
                    GpuLinearPart {
                        bt_ggml_type: GGML_TYPE_F16,
                        n: lq.n,
                        cache_key: q,
                        bytes: &lq.f16_bytes,
                    },
                    GpuLinearPart {
                        bt_ggml_type: GGML_TYPE_F16,
                        n: lk.n,
                        cache_key: k,
                        bytes: &lk.f16_bytes,
                    },
                    GpuLinearPart {
                        bt_ggml_type: GGML_TYPE_F16,
                        n: lv.n,
                        cache_key: v,
                        bytes: &lv.f16_bytes,
                    },
                ],
                &[],
            )?
        } else if paint_fast() {
            gpu_linear_nt_cached(
                x,
                NS,
                &[
                    GpuLinearPart {
                        bt_ggml_type: GGML_TYPE_F16,
                        n: lq.n,
                        cache_key: q,
                        bytes: &lq.f16_bytes,
                    },
                    GpuLinearPart {
                        bt_ggml_type: GGML_TYPE_F16,
                        n: lk.n,
                        cache_key: k,
                        bytes: &lk.f16_bytes,
                    },
                    GpuLinearPart {
                        bt_ggml_type: GGML_TYPE_F16,
                        n: lv.n,
                        cache_key: v,
                        bytes: &lv.f16_bytes,
                    },
                ],
                &[],
            )?
        } else {
            return Err("linear_qkv is inference-only".into());
        };
        drop(cache);
        let qt = gpu_slice_cols(&qkv, 0, hidden)?;
        let kt = gpu_slice_cols(&qkv, hidden, hidden)?;
        let vt = gpu_slice_cols(&qkv, hidden * 2, hidden)?;
        Ok((qt, kt, vt))
    }

    /// Encoder K/V for a job-constant token pack. Numerically the same GEMM
    /// as `linear_tokens`; reused across the 15 DDIM steps.
    pub(crate) fn encoder_kv(
        &self,
        enc: &GpuTensor,
        prefix: &str,
        tag: &str,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        let key = format!("{prefix}|{tag}");
        if let Some((k, v)) = self.enc_kv.borrow().get(&key) {
            return Ok((
                gpu_slice_rows(k, 0, k.rows())?,
                gpu_slice_rows(v, 0, v.rows())?,
            ));
        }
        let k = self.linear_tokens(enc, &format!("{prefix}.to_k"), false)?;
        let v = self.linear_tokens(enc, &format!("{prefix}.to_v"), false)?;
        self.enc_kv
            .borrow_mut()
            .insert(key, (gpu_slice_rows(&k, 0, k.rows())?, gpu_slice_rows(&v, 0, v.rows())?));
        Ok((k, v))
    }

    pub(crate) fn cache_kv(
        &self,
        key: &str,
        build: impl FnOnce() -> Result<(GpuTensor, GpuTensor), String>,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        if let Some((k, v)) = self.enc_kv.borrow().get(key) {
            return Ok((
                gpu_slice_rows(k, 0, k.rows())?,
                gpu_slice_rows(v, 0, v.rows())?,
            ));
        }
        let (k, v) = build()?;
        self.enc_kv.borrow_mut().insert(
            key.to_string(),
            (
                gpu_slice_rows(&k, 0, k.rows())?,
                gpu_slice_rows(&v, 0, v.rows())?,
            ),
        );
        Ok((k, v))
    }

    pub(crate) fn encoder_lin(
        &self,
        enc: &GpuTensor,
        weight: &str,
        tag: &str,
    ) -> Result<GpuTensor, String> {
        if let Some(t) = self.enc_lin.borrow().get(tag) {
            return gpu_slice_rows(t, 0, t.rows());
        }
        let t = self.linear_tokens(enc, weight, false)?;
        self.enc_lin
            .borrow_mut()
            .insert(tag.to_string(), gpu_slice_rows(&t, 0, t.rows())?);
        Ok(t)
    }

    /// Drop job-scoped caches (reference/DINO/text K/V and encoder packs).
    /// `linear_gpu` stays: it holds weights, which are job-invariant. Must be
    /// called at the start of every job on a warm-kept UnetFirst — the maps
    /// are keyed by layer name only, so a second job would silently reuse the
    /// first job's tokens (or shape-mismatch on an n_views change).
    pub(crate) fn begin_job(&self) {
        self.enc_kv.borrow_mut().clear();
        self.enc_lin.borrow_mut().clear();
    }

    pub(crate) fn get(&self, name: &str) -> Result<&W, String> {
        self.w
            .get(name)
            .ok_or_else(|| format!("missing {name}"))
    }

    /// Vertical stack with `2*pad` zero rows between images so a same-pad
    /// `kw x kw` conv cannot bleed across batch items.
    fn stack_sep(
        xs: &[GpuTensor],
        width: usize,
        height: usize,
        pad: usize,
    ) -> Result<(GpuTensor, usize), String> {
        if xs.is_empty() {
            return Err("stack_sep empty".into());
        }
        let channels = xs[0].rows();
        let plane = width * height;
        for x in xs {
            if x.rows() != channels || x.cols() != plane {
                return Err(format!(
                    "stack_sep expected {channels}x{plane}, got {}x{}",
                    x.rows(),
                    x.cols()
                ));
            }
        }
        if xs.len() == 1 || pad == 0 {
            if xs.len() == 1 {
                return Ok((gpu_slice_cols(&xs[0], 0, plane)?, height));
            }
            let refs: Vec<&GpuTensor> = xs.iter().collect();
            return Ok((gpu_concat_cols(&refs)?, xs.len() * height));
        }
        let sep = 2 * pad;
        let zeros = gpu_upload(&vec![0.0f32; channels * sep * width], channels, sep * width)?;
        let mut parts: Vec<&GpuTensor> = Vec::with_capacity(xs.len() * 2 - 1);
        for (i, x) in xs.iter().enumerate() {
            parts.push(x);
            if i + 1 < xs.len() {
                parts.push(&zeros);
            }
        }
        let stacked = gpu_concat_cols(&parts)?;
        let stacked_h = xs.len() * height + (xs.len() - 1) * sep;
        Ok((stacked, stacked_h))
    }

    fn unstack_sep(
        y: &GpuTensor,
        n: usize,
        width: usize,
        height: usize,
        pad: usize,
    ) -> Result<Vec<GpuTensor>, String> {
        let plane = width * height;
        if n == 1 || pad == 0 {
            if n == 1 {
                return Ok(vec![gpu_slice_cols(y, 0, plane)?]);
            }
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                out.push(gpu_slice_cols(y, i * plane, plane)?);
            }
            return Ok(out);
        }
        let stride = (height + 2 * pad) * width;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(gpu_slice_cols(y, i * stride, plane)?);
        }
        Ok(out)
    }

    pub(crate) fn conv(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
        kw: usize,
        pad: usize,
    ) -> Result<GpuTensor, String> {
        let weight = self.get(&format!("{prefix}.weight"))?;
        let bias = self.get(&format!("{prefix}.bias"))?;
        gpu_conv2d_planar_cached(
            x,
            width,
            height,
            NS,
            prefix,
            &weight.data,
            &bias.data,
            weight.shape[0],
            kw,
            kw,
            pad,
            pad,
        )
    }

    pub(crate) fn conv_n(
        &self,
        xs: &[GpuTensor],
        width: usize,
        height: usize,
        prefix: &str,
        kw: usize,
        pad: usize,
    ) -> Result<Vec<GpuTensor>, String> {
        if xs.is_empty() {
            return Ok(Vec::new());
        }
        if xs.len() == 1 {
            return Ok(vec![self.conv(&xs[0], width, height, prefix, kw, pad)?]);
        }
        let (stacked, stacked_h) = Self::stack_sep(xs, width, height, pad)?;
        let y = self.conv(&stacked, width, stacked_h, prefix, kw, pad)?;
        Self::unstack_sep(&y, xs.len(), width, height, pad)
    }

    fn gn_silu_n(
        &self,
        xs: &[GpuTensor],
        width: usize,
        height: usize,
        prefix: &str,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Result<Vec<GpuTensor>, String> {
        if xs.is_empty() {
            return Ok(Vec::new());
        }
        let plane = width * height;
        if paint_fast() && xs.len() > 1 {
            let refs: Vec<&GpuTensor> = xs.iter().collect();
            let stacked = gpu_concat_cols(&refs)?;
            let gn = gpu_paint_group_norm_batched(
                &stacked,
                width,
                height,
                xs.len(),
                GN_GROUPS,
                NS,
                prefix,
                gamma,
                beta,
                eps,
            )?;
            let y = gpu_silu(&gn)?;
            return (0..xs.len())
                .map(|i| gpu_slice_cols(&y, i * plane, plane))
                .collect();
        }
        xs.iter()
            .map(|x| {
                let y = gpu_group_norm_planar(
                    x,
                    width,
                    height,
                    GN_GROUPS,
                    NS,
                    prefix,
                    gamma,
                    beta,
                    eps,
                )?;
                gpu_silu(&y)
            })
            .collect()
    }

    pub fn resnet_n(
        &self,
        xs: &[GpuTensor],
        width: usize,
        height: usize,
        temb: &[f32],
        prefix: &str,
        channels: usize,
    ) -> Result<Vec<GpuTensor>, String> {
        if xs.is_empty() {
            return Ok(Vec::new());
        }
        let plane = width * height;
        for x in xs {
            if x.rows() != channels || x.cols() != plane {
                return Err(format!(
                    "{prefix} batch expects {channels}x{width}x{height}, got {}x{}",
                    x.rows(),
                    x.cols()
                ));
            }
        }
        let n1w = self.get(&format!("{prefix}.norm1.weight"))?;
        let n1b = self.get(&format!("{prefix}.norm1.bias"))?;
        let n1 = self.gn_silu_n(
            xs,
            width,
            height,
            &format!("{prefix}.norm1"),
            &n1w.data,
            &n1b.data,
            GN_EPS,
        )?;
        let y = self.conv_n(&n1, width, height, &format!("{prefix}.conv1"), 3, 1)?;
        let cout = self.get(&format!("{prefix}.conv1.weight"))?.shape[0];
        let temb_g = if paint_fast() {
            let temb_in = gpu_upload(temb, 1, temb.len())?;
            let temb_act = gpu_silu(&temb_in)?;
            self.linear_cached(&temb_act, &format!("{prefix}.time_emb_proj"), true)?
        } else {
            let temb_act: Vec<f32> = temb.iter().map(|v| v / (1.0 + (-v).exp())).collect();
            let temb_proj = self.linear(&temb_act, &format!("{prefix}.time_emb_proj"))?;
            gpu_upload(&temb_proj, cout, 1)?
        };
        let n2w = self.get(&format!("{prefix}.norm2.weight"))?;
        let n2b = self.get(&format!("{prefix}.norm2.bias"))?;
        let has_skip = self.w.contains_key(&format!("{prefix}.conv_shortcut.weight"));
        let skips = if has_skip {
            Some(self.conv_n(xs, width, height, &format!("{prefix}.conv_shortcut"), 1, 0)?)
        } else {
            None
        };
        let y2 = if paint_fast() && y.len() > 1 {
            let refs: Vec<&GpuTensor> = y.iter().collect();
            let stacked = gpu_concat_cols(&refs)?;
            let stacked = gpu_add_rows_broadcast(&stacked, &temb_g)?;
            let gn = gpu_paint_group_norm_batched(
                &stacked,
                width,
                height,
                y.len(),
                GN_GROUPS,
                NS,
                &format!("{prefix}.norm2"),
                &n2w.data,
                &n2b.data,
                GN_EPS,
            )?;
            let stacked = gpu_silu(&gn)?;
            (0..y.len())
                .map(|i| gpu_slice_cols(&stacked, i * plane, plane))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let mut y2 = Vec::with_capacity(y.len());
            for yi in y.into_iter() {
                let yi = gpu_add_rows_broadcast(&yi, &temb_g)?;
                let yi = gpu_group_norm_planar(
                    &yi,
                    width,
                    height,
                    GN_GROUPS,
                    NS,
                    &format!("{prefix}.norm2"),
                    &n2w.data,
                    &n2b.data,
                    GN_EPS,
                )?;
                y2.push(gpu_silu(&yi)?);
            }
            y2
        };
        let y2 = self.conv_n(&y2, width, height, &format!("{prefix}.conv2"), 3, 1)?;
        let mut out = Vec::with_capacity(y2.len());
        for (i, yi) in y2.into_iter().enumerate() {
            let added = if let Some(sk) = &skips {
                gpu_add(&yi, &sk[i])?
            } else {
                gpu_add(&yi, &xs[i])?
            };
            out.push(added);
        }
        Ok(out)
    }

    /// GroupNorm + SiLU on packed NCHW (no planar permute).
    pub(crate) fn gn_silu_batch(
        &self,
        x: &BatchAct,
        prefix: &str,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Result<BatchAct, String> {
        let gn = gpu_group_norm_nchw(
            &x.t,
            x.c,
            x.w,
            x.h,
            GN_GROUPS,
            NS,
            prefix,
            gamma,
            beta,
            eps,
        )?;
        Ok(BatchAct {
            t: gpu_silu(&gn)?,
            n: x.n,
            c: x.c,
            w: x.w,
            h: x.h,
        })
    }

    /// Same-pad or strided conv on packed NCHW `[N, C*H*W]`.
    pub(crate) fn conv_batch(
        &self,
        x: &BatchAct,
        prefix: &str,
        kw: usize,
        pad: usize,
    ) -> Result<BatchAct, String> {
        self.conv_batch_ex(x, prefix, kw, pad, 1)
    }

    pub(crate) fn conv_batch_ex(
        &self,
        x: &BatchAct,
        prefix: &str,
        kw: usize,
        pad: usize,
        stride: usize,
    ) -> Result<BatchAct, String> {
        let weight = self.get(&format!("{prefix}.weight"))?;
        let bias = self.get(&format!("{prefix}.bias"))?;
        let cout = weight.shape[0];
        let out_w = if stride == 1 { x.w } else { x.w / stride };
        let out_h = if stride == 1 { x.h } else { x.h / stride };
        let y = gpu_conv2d_nchw_packed(
            &x.t,
            x.c,
            x.w,
            x.h,
            out_w,
            out_h,
            NS,
            prefix,
            &weight.data,
            &bias.data,
            cout,
            kw,
            kw,
            pad,
            pad,
            stride,
            stride,
        )?;
        Ok(BatchAct {
            t: y,
            n: x.n,
            c: cout,
            w: out_w,
            h: out_h,
        })
    }

    pub(crate) fn silu_temb(&self, temb: &[f32]) -> Result<GpuTensor, String> {
        gpu_silu(&gpu_upload(temb, 1, temb.len())?)
    }

    /// Official ResNet on one stacked `[C, n*H*W]` tensor: GN, conv, time
    /// linear, residual. Inference-only (`paint_fast`); tap canaries keep
    /// [`Self::resnet_n`].
    pub(crate) fn resnet_batch(
        &self,
        x: &BatchAct,
        temb: &[f32],
        prefix: &str,
        channels: usize,
    ) -> Result<BatchAct, String> {
        self.resnet_batch_act(x, &self.silu_temb(temb)?, prefix, channels)
    }

    pub(crate) fn resnet_batch_act(
        &self,
        x: &BatchAct,
        temb_act: &GpuTensor,
        prefix: &str,
        channels: usize,
    ) -> Result<BatchAct, String> {
        x.expect(channels, x.w, x.h, prefix)?;
        let n1w = self.get(&format!("{prefix}.norm1.weight"))?;
        let n1b = self.get(&format!("{prefix}.norm1.bias"))?;
        let y = self.gn_silu_batch(x, &format!("{prefix}.norm1"), &n1w.data, &n1b.data, GN_EPS)?;
        let y = self.conv_batch(&y, &format!("{prefix}.conv1"), 3, 1)?;
        let cout = self.get(&format!("{prefix}.conv1.weight"))?.shape[0];
        let temb_g = self.linear_cached(temb_act, &format!("{prefix}.time_emb_proj"), true)?;
        if temb_g.rows() * temb_g.cols() != cout {
            return Err(format!("{prefix} temb {cout} vs {}x{}", temb_g.rows(), temb_g.cols()));
        }
        let y = BatchAct {
            t: if y.t.is_half() {
                gpu_nchw_add_channel(&y.t, &temb_g, cout, y.w, y.h)?
            } else {
                gpu_nchw_add_channel_inplace(&y.t, &temb_g, cout, y.w, y.h)?
            },
            n: y.n,
            c: y.c,
            w: y.w,
            h: y.h,
        };
        let n2w = self.get(&format!("{prefix}.norm2.weight"))?;
        let n2b = self.get(&format!("{prefix}.norm2.bias"))?;
        let y = self.gn_silu_batch(&y, &format!("{prefix}.norm2"), &n2w.data, &n2b.data, GN_EPS)?;
        let y = self.conv_batch(&y, &format!("{prefix}.conv2"), 3, 1)?;
        let skip = if self.w.contains_key(&format!("{prefix}.conv_shortcut.weight")) {
            self.conv_batch(x, &format!("{prefix}.conv_shortcut"), 1, 0)?
        } else {
            x.clone_act()?
        };
        if skip.t.rows() != y.t.rows() || skip.t.cols() != y.t.cols() {
            return Err(format!(
                "{prefix} residual {}x{} vs {}x{}",
                y.t.rows(),
                y.t.cols(),
                skip.t.rows(),
                skip.t.cols()
            ));
        }
        Ok(BatchAct {
            t: gpu_add(&y.t, &skip.t)?,
            n: y.n,
            c: y.c,
            w: y.w,
            h: y.h,
        })
    }

    pub(crate) fn downsample_batch(
        &self,
        x: &BatchAct,
        prefix: &str,
        channels: usize,
    ) -> Result<BatchAct, String> {
        x.expect(channels, x.w, x.h, prefix)?;
        self.conv_batch_ex(x, prefix, 3, 1, 2)
    }

    pub(crate) fn upsample_batch(
        &self,
        x: &BatchAct,
        prefix: &str,
        channels: usize,
    ) -> Result<BatchAct, String> {
        x.expect(channels, x.w, x.h, prefix)?;
        let keep_half = x.t.is_half();
        let src = if keep_half {
            gpu_to_f32(&x.t)?
        } else {
            gpu_slice_rows(&x.t, 0, x.t.rows())?
        };
        let planar = gpu_nchw_to_planar(&src, x.c, x.w, x.h)?;
        let up = gpu_upsample_nearest2x(&planar, x.w, x.stacked_h())?;
        let mut nchw = gpu_planar_to_nchw(&up, x.n, x.w * 2, x.h * 2)?;
        if keep_half {
            nchw = gpu_to_f16(&nchw)?;
        }
        let y = BatchAct {
            t: nchw,
            n: x.n,
            c: x.c,
            w: x.w * 2,
            h: x.h * 2,
        };
        self.conv_batch(&y, prefix, 3, 1)
    }

    pub(crate) fn conv_in_batch(
        &self,
        x12s: &[&[f32]],
        width: usize,
        height: usize,
    ) -> Result<BatchAct, String> {
        let plane = width * height;
        let packed = pack_planar_host(x12s, 12, plane)?;
        let x = gpu_upload(&packed, 12, x12s.len() * plane)?;
        self.conv_in_stacked(&x, x12s.len(), width, height)
    }

    pub(crate) fn conv_in_stacked(
        &self,
        x12: &GpuTensor,
        n: usize,
        width: usize,
        height: usize,
    ) -> Result<BatchAct, String> {
        let planar = gpu_slice_rows(x12, 0, x12.rows())?;
        let x = BatchAct {
            t: gpu_planar_to_nchw(&planar, n, width, height)?,
            n,
            c: 12,
            w: width,
            h: height,
        };
        x.expect(12, width, height, "conv_in")?;
        // Official nn.Conv2d is fp16 NCHW. Stay HALF through the walk.
        let x = if paint_fast() {
            BatchAct {
                t: gpu_to_f16(&x.t)?,
                n: x.n,
                c: x.c,
                w: x.w,
                h: x.h,
            }
        } else {
            x
        };
        self.conv_batch(&x, "unet.conv_in", 3, 1)
    }

    pub(crate) fn conv_head_batch(&self, x: &BatchAct) -> Result<BatchAct, String> {
        x.expect(320, x.w, x.h, "conv_head")?;
        let nw = self.get("unet.conv_norm_out.weight")?;
        let nb = self.get("unet.conv_norm_out.bias")?;
        let y = self.gn_silu_batch(x, "unet.conv_norm_out", &nw.data, &nb.data, GN_EPS)?;
        let y = self.conv_batch(&y, "unet.conv_out", 3, 1)?;
        let y_t = if y.t.is_half() {
            gpu_to_f32(&y.t)?
        } else {
            gpu_slice_rows(&y.t, 0, y.t.rows())?
        };
        // DDIM is planar `[4, n*H*W]`.
        Ok(BatchAct {
            t: gpu_nchw_to_planar(&y_t, y.c, y.w, y.h)?,
            n: y.n,
            c: y.c,
            w: y.w,
            h: y.h,
        })
    }

    pub(crate) fn cat_skip_batch(hidden: &BatchAct, skip: &BatchAct) -> Result<BatchAct, String> {
        if hidden.n != skip.n || hidden.w != skip.w || hidden.h != skip.h {
            return Err(format!(
                "cat_skip_batch nwh {}x{}x{} vs {}x{}x{}",
                hidden.n, hidden.w, hidden.h, skip.n, skip.w, skip.h
            ));
        }
        Ok(BatchAct {
            t: gpu_concat_cols(&[&hidden.t, &skip.t])?,
            n: hidden.n,
            c: hidden.c + skip.c,
            w: hidden.w,
            h: hidden.h,
        })
    }

    pub(crate) fn linear(&self, x: &[f32], prefix: &str) -> Result<Vec<f32>, String> {
        let weight = self.get(&format!("{prefix}.weight"))?;
        let bias = self.get(&format!("{prefix}.bias"))?;
        let out_f = weight.shape[0];
        let in_f = weight.shape[1];
        if x.len() != in_f {
            return Err(format!("{prefix} in {} vs {}", x.len(), in_f));
        }
        let mut out = vec![0.0f32; out_f];
        for o in 0..out_f {
            let mut acc = bias.data[o];
            let row = &weight.data[o * in_f..(o + 1) * in_f];
            for i in 0..in_f {
                acc += x[i] * row[i];
            }
            out[o] = acc;
        }
        Ok(out)
    }

    pub fn timestep_embedding(&self, t: f32) -> Result<Vec<f32>, String> {
        self.timestep_embedding_named(t, "unet.time_embedding")
    }

    pub fn timestep_embedding_named(&self, t: f32, prefix: &str) -> Result<Vec<f32>, String> {
        // diffusers Timesteps: flip_sin_to_cos, freq_shift=0, dim=320
        let half = 160usize;
        let emb_scale = (10000f32).ln() / half as f32;
        let mut freqs = vec![0.0f32; half];
        for i in 0..half {
            freqs[i] = (-(i as f32) * emb_scale).exp();
        }
        let mut sin = vec![0.0f32; half];
        let mut cos = vec![0.0f32; half];
        for i in 0..half {
            let a = t * freqs[i];
            sin[i] = a.sin();
            cos[i] = a.cos();
        }
        // flip_sin_to_cos
        let mut proj = Vec::with_capacity(320);
        proj.extend_from_slice(&cos);
        proj.extend_from_slice(&sin);
        let h = self.linear(&proj, &format!("{prefix}.linear_1"))?;
        let h: Vec<f32> = h.iter().map(|v| v / (1.0 + (-v).exp())).collect();
        self.linear(&h, &format!("{prefix}.linear_2"))
    }

    pub fn learned_text_clip_albedo(&self) -> Result<Vec<f32>, String> {
        Ok(self.get("unet.learned_text_clip_albedo")?.data.clone())
    }

    pub fn learned_text_clip_mr(&self) -> Result<Vec<f32>, String> {
        Ok(self.get("unet.learned_text_clip_mr")?.data.clone())
    }

    pub fn learned_text_clip_ref(&self) -> Result<Vec<f32>, String> {
        Ok(self.get("unet.learned_text_clip_ref")?.data.clone())
    }

    pub fn concat_planar(a: &[f32], b: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(a.len() + b.len());
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        out
    }

    pub fn upsample(
        &self,
        h: &[f32],
        width: usize,
        height: usize,
        prefix: &str,
        channels: usize,
    ) -> Result<(Vec<f32>, usize, usize), String> {
        let plane = width * height;
        if h.len() != channels * plane {
            return Err(format!(
                "{prefix} expects {channels}x{width}x{height}, got {}",
                h.len()
            ));
        }
        let x = gpu_upload(h, channels, plane)?;
        let (y, ow, oh) = self.upsample_gpu(&x, width, height, prefix, channels)?;
        Ok((gpu_download(&y)?, ow, oh))
    }

    pub fn upsample_gpu(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
        channels: usize,
    ) -> Result<(GpuTensor, usize, usize), String> {
        if x.rows() != channels || x.cols() != width * height {
            return Err(format!(
                "{prefix} gpu upsample expects {channels}x{} got {}x{}",
                width * height,
                x.rows(),
                x.cols()
            ));
        }
        let y = gpu_upsample_nearest2x(x, width, height)?;
        let ow = width * 2;
        let oh = height * 2;
        let y = self.conv(&y, ow, oh, prefix, 3, 1)?;
        Ok((y, ow, oh))
    }

    /// Final UNet head: GroupNorm32 + SiLU + 3x3 conv to 4-ch v-pred.
    pub fn conv_head(
        &self,
        h: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, String> {
        let plane = width * height;
        if h.len() != 320 * plane {
            return Err(format!(
                "conv_head expects 320x{width}x{height}, got {}",
                h.len()
            ));
        }
        let x = gpu_upload(h, 320, plane)?;
        let nw = self.get("unet.conv_norm_out.weight")?;
        let nb = self.get("unet.conv_norm_out.bias")?;
        let y = gpu_group_norm_planar(
            &x,
            width,
            height,
            GN_GROUPS,
            NS,
            "unet.conv_norm_out",
            &nw.data,
            &nb.data,
            GN_EPS,
        )?;
        let y = gpu_silu(&y)?;
        let y = self.conv(&y, width, height, "unet.conv_out", 3, 1)?;
        gpu_download(&y)
    }

    pub fn conv_head_gpu(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        let plane = width * height;
        if x.rows() != 320 || x.cols() != plane {
            return Err(format!(
                "conv_head gpu expects 320x{width}x{height}, got {}x{}",
                x.rows(),
                x.cols()
            ));
        }
        let nw = self.get("unet.conv_norm_out.weight")?;
        let nb = self.get("unet.conv_norm_out.bias")?;
        let y = gpu_group_norm_planar(
            x,
            width,
            height,
            GN_GROUPS,
            NS,
            "unet.conv_norm_out",
            &nw.data,
            &nb.data,
            GN_EPS,
        )?;
        let y = gpu_silu(&y)?;
        self.conv(&y, width, height, "unet.conv_out", 3, 1)
    }

    pub fn conv_in(&self, x12: &[f32], width: usize, height: usize) -> Result<Vec<f32>, String> {
        gpu_download(&self.conv_in_gpu(x12, width, height)?)
    }

    pub fn conv_in_gpu(&self, x12: &[f32], width: usize, height: usize) -> Result<GpuTensor, String> {
        if x12.len() != 12 * width * height {
            return Err("conv_in expects 12-ch planar".into());
        }
        let x = gpu_upload(x12, 12, width * height)?;
        self.conv(&x, width, height, "unet.conv_in", 3, 1)
    }

    /// Dual-stream 4-ch `unet_dual.conv_in` (reference latent, not the 12-ch pack).
    pub fn conv_in_dual(&self, x4: &[f32], width: usize, height: usize) -> Result<Vec<f32>, String> {
        if x4.len() != 4 * width * height {
            return Err("conv_in_dual expects 4-ch planar".into());
        }
        let x = gpu_upload(x4, 4, width * height)?;
        let y = self.conv(&x, width, height, "unet_dual.conv_in", 3, 1)?;
        gpu_download(&y)
    }

    pub fn resnet0(
        &self,
        h: &[f32],
        width: usize,
        height: usize,
        temb: &[f32],
    ) -> Result<Vec<f32>, String> {
        self.resnet(h, width, height, temb, "unet.down_blocks.0.resnets.0", 320)
    }

    pub fn resnet(
        &self,
        h: &[f32],
        width: usize,
        height: usize,
        temb: &[f32],
        prefix: &str,
        channels: usize,
    ) -> Result<Vec<f32>, String> {
        let plane = width * height;
        if h.len() != channels * plane {
            return Err(format!(
                "{prefix} expects {channels}x{width}x{height}, got {}",
                h.len()
            ));
        }
        let x = gpu_upload(h, channels, plane)?;
        gpu_download(&self.resnet_gpu(&x, width, height, temb, prefix, channels)?)
    }

    pub fn resnet_gpu(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        temb: &[f32],
        prefix: &str,
        channels: usize,
    ) -> Result<GpuTensor, String> {
        let plane = width * height;
        if x.rows() != channels || x.cols() != plane {
            return Err(format!(
                "{prefix} gpu expects {channels}x{width}x{height}, got {}x{}",
                x.rows(),
                x.cols()
            ));
        }
        let n1w = self.get(&format!("{prefix}.norm1.weight"))?;
        let n1b = self.get(&format!("{prefix}.norm1.bias"))?;
        let y = gpu_group_norm_planar(
            x,
            width,
            height,
            GN_GROUPS,
            NS,
            &format!("{prefix}.norm1"),
            &n1w.data,
            &n1b.data,
            GN_EPS,
        )?;
        let y = gpu_silu(&y)?;
        let y = self.conv(&y, width, height, &format!("{prefix}.conv1"), 3, 1)?;
        let cout = self.get(&format!("{prefix}.conv1.weight"))?.shape[0];
        let temb_act: Vec<f32> = temb.iter().map(|v| v / (1.0 + (-v).exp())).collect();
        let temb_proj = self.linear(&temb_act, &format!("{prefix}.time_emb_proj"))?;
        if temb_proj.len() != cout {
            return Err(format!("{prefix} temb {cout} vs {}", temb_proj.len()));
        }
        let temb_g = gpu_upload(&temb_proj, cout, 1)?;
        let y = gpu_add_rows_broadcast(&y, &temb_g)?;
        let n2w = self.get(&format!("{prefix}.norm2.weight"))?;
        let n2b = self.get(&format!("{prefix}.norm2.bias"))?;
        let y = gpu_group_norm_planar(
            &y,
            width,
            height,
            GN_GROUPS,
            NS,
            &format!("{prefix}.norm2"),
            &n2w.data,
            &n2b.data,
            GN_EPS,
        )?;
        let y = gpu_silu(&y)?;
        let y = self.conv(&y, width, height, &format!("{prefix}.conv2"), 3, 1)?;
        if self.w.contains_key(&format!("{prefix}.conv_shortcut.weight")) {
            let skip = self.conv(x, width, height, &format!("{prefix}.conv_shortcut"), 1, 0)?;
            gpu_add(&y, &skip)
        } else if y.rows() != x.rows() {
            Err(format!("{prefix} channel change without shortcut"))
        } else {
            gpu_add(&y, x)
        }
    }

    /// UNet Downsample2D: 3x3 stride-2, `downsample_padding=1`.
    pub fn downsample(
        &self,
        h: &[f32],
        width: usize,
        height: usize,
        prefix: &str,
        channels: usize,
    ) -> Result<(Vec<f32>, usize, usize), String> {
        let plane = width * height;
        if h.len() != channels * plane {
            return Err(format!(
                "{prefix} expects {channels}x{width}x{height}, got {}",
                h.len()
            ));
        }
        let x = gpu_upload(h, channels, plane)?;
        let (y, out_w, out_h) = self.downsample_gpu(&x, width, height, prefix, channels)?;
        Ok((gpu_download(&y)?, out_w, out_h))
    }

    pub fn downsample_gpu(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
        channels: usize,
    ) -> Result<(GpuTensor, usize, usize), String> {
        if x.rows() != channels || x.cols() != width * height {
            return Err(format!(
                "{prefix} gpu expects {channels}x{width}x{height}, got {}x{}",
                x.rows(),
                x.cols()
            ));
        }
        let weight = self.get(&format!("{prefix}.weight"))?;
        let bias = self.get(&format!("{prefix}.bias"))?;
        let oc = weight.shape[0];
        let out_w = width / 2;
        let out_h = height / 2;
        let y = gpu_conv2d_planar_strided(
            x,
            width,
            height,
            out_w,
            out_h,
            NS,
            prefix,
            &weight.data,
            &bias.data,
            oc,
            3,
            3,
            1,
            1,
            2,
            2,
        )?;
        Ok((y, out_w, out_h))
    }
}

pub fn arange_nchw_planar(channels: usize, h: usize, w: usize) -> Vec<f32> {
    let n = channels * h * w;
    (0..n).map(|i| i as f32 / n as f32).collect()
}
