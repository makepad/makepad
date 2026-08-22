//! DINOv2-giant architecture + Hunyuan `Dino_v2` preprocess contract.
//!
//! Official wrapper (`hy3dpaint/hunyuanpaintpbr/unet/modules.py` @
//! 82920d643c0dc2f7bfd7255f45f62d386edfe60c) is:
//! `AutoImageProcessor.from_pretrained` + `AutoModel.from_pretrained`,
//! then `last_hidden_state` `[B, L, 1536]` is projected by
//! [`crate::dino_proj::DinoProj`] to `[B, L*4, 1024]` for `attn_dino`.
//!
//! `DinoVit::forward` runs the 40-layer ViT and returns `last_hidden_state`
//! `[257, 1536]` (224-crop: 16×16 patches + CLS). The parent projector is
//! [`crate::dino_proj::DinoProj`].

use crate::hunyuan;
use crate::test_backend::PbrError;

/// facebook/dinov2-giant @ the Hunyuan pin.
pub const REPO: &str = hunyuan::DINO_REPO;
pub const REVISION: &str = hunyuan::DINO_REVISION;

pub const HIDDEN: usize = 1536;
pub const DEPTH: usize = 40;
pub const HEADS: usize = 24;
pub const PATCH: usize = 14;
/// Model `image_size` in config.json. The AutoImageProcessor on the box
/// may still resize/crop differently — see [`processor_spatial`].
pub const MODEL_IMAGE_SIZE: usize = 518;
pub const LN_EPS: f32 = 1e-6;
pub const MLP_RATIO: usize = 4;
/// SwiGLU FFN (`use_swiglu_ffn: true` in the pinned config).
pub const USE_SWIGLU: bool = true;
/// `int(hidden * mlp_ratio * 2/3)` rounded up to a multiple of 8 = 4096.
/// `weights_in` is `[2 * SWIGLU_HIDDEN, HIDDEN]`.
pub const SWIGLU_HIDDEN: usize = 4096;
pub const QKV_BIAS: bool = true;
pub const LAYERSCALE_INIT: f32 = 1.0;
/// 224-crop token count: CLS + 16×16.
pub const TOKENS: usize = 1 + (PROC_CROP / PATCH) * (PROC_CROP / PATCH);

/// BitImageProcessor ImageNet mean / std from the pinned preprocessor.
pub const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Live `BitImageProcessor` on the paint box (facebook/dinov2-giant @
/// 611a9d42, dumped 2026-08-16): resize shortest=256, center-crop 224.
/// Model `image_size` is 518; Hunyuan still feeds the 224 processor.
pub const PROC_SHORTEST_EDGE: usize = 256;
pub const PROC_CROP: usize = 224;

pub fn tokens_for_spatial(size: usize) -> usize {
    // CLS + patches. size must be divisible by PATCH.
    1 + (size / PATCH) * (size / PATCH)
}

pub fn processor_spatial() -> usize {
    PROC_CROP
}

/// Resize so the shortest edge is `shortest`, then center-crop `crop`x`crop`.
/// Matches BitImageProcessor `do_resize` + `do_center_crop` (PIL `resample=3`
/// / BICUBIC, cubic `a = -0.5`).
pub fn preprocess_rgb8(
    rgb: &[u8],
    width: usize,
    height: usize,
    shortest: usize,
    crop: usize,
) -> Result<Vec<f32>, PbrError> {
    if width == 0 || height == 0 || shortest == 0 || crop == 0 {
        return Err(PbrError::InvalidParams("dino preprocess dim is zero".into()));
    }
    if rgb.len() != width * height * 3 {
        return Err(PbrError::InvalidParams("dino preprocess rgb length".into()));
    }
    let (rw, rh) = if width <= height {
        let rw = shortest;
        let rh = ((height as f32) * (shortest as f32) / (width as f32)).round().max(1.0) as usize;
        (rw, rh)
    } else {
        let rh = shortest;
        let rw = ((width as f32) * (shortest as f32) / (height as f32)).round().max(1.0) as usize;
        (rw, rh)
    };
    let resized = resize_rgb8_bicubic_pil(rgb, width, height, rw, rh)?;
    let x0 = rw.saturating_sub(crop) / 2;
    let y0 = rh.saturating_sub(crop) / 2;
    let cw = crop.min(rw);
    let ch = crop.min(rh);
    let mut planar = vec![0.0f32; 3 * crop * crop];
    let plane = crop * crop;
    for y in 0..ch {
        for x in 0..cw {
            let si = ((y0 + y) * rw + (x0 + x)) * 3;
            let di = y * crop + x;
            for c in 0..3 {
                let v01 = resized[si + c] as f32 / 255.0;
                planar[c * plane + di] = (v01 - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
            }
        }
    }
    Ok(planar)
}

pub fn preprocess_official(rgb: &[u8], width: usize, height: usize) -> Result<Vec<f32>, PbrError> {
    preprocess_rgb8(rgb, width, height, PROC_SHORTEST_EDGE, PROC_CROP)
}

/// Trained position-embedding token count (CLS + 37×37) at `image_size` 518.
/// Box dump 2026-08-16: `embeddings.position_embeddings` is `[1, 1370, 1536]`.
pub fn trained_pos_tokens() -> usize {
    tokens_for_spatial(MODEL_IMAGE_SIZE)
}

fn cubic_kernel_a(x: f32, a: f32) -> f32 {
    let ax = x.abs();
    if ax <= 1.0 {
        (a + 2.0) * ax * ax * ax - (a + 3.0) * ax * ax + 1.0
    } else if ax < 2.0 {
        a * ax * ax * ax - 5.0 * a * ax * ax + 8.0 * a * ax - 4.0 * a
    } else {
        0.0
    }
}

fn cubic_kernel(x: f32) -> f32 {
    // PyTorch `F.interpolate(..., mode="bicubic")` uses a = -0.75.
    cubic_kernel_a(x, -0.75)
}

/// PIL `Image.resize(..., BICUBIC)` (BitImageProcessor resample=3): a = -0.5,
/// separable, and — the part a naive 4-tap port misses — ANTIALIASED for
/// downscaling: the filter support widens by the scale factor (support =
/// 2 * scale taps each side) and the weights renormalize, exactly PIL's
/// `ImagingResampleHorizontal/Vertical` (libImaging/Resample.c). Center
/// convention `(i + 0.5) * scale`, source window `[center - support,
/// center + support)`, edges clamped by window clipping (not by clamping
/// samples).
pub fn resize_rgb8_bicubic_pil(
    rgb: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Result<Vec<u8>, PbrError> {
    if rgb.len() != src_w * src_h * 3 || src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Err(PbrError::InvalidParams("dino bicubic resize size".into()));
    }
    if src_w == dst_w && src_h == dst_h {
        return Ok(rgb.to_vec());
    }
    // Horizontal pass to f32 [src_h][dst_w][3], then vertical to [dst_h][dst_w][3].
    let (xw, xb) = pil_precompute_coeffs(src_w, dst_w);
    let mut tmp = vec![0.0f32; src_h * dst_w * 3];
    for y in 0..src_h {
        for x in 0..dst_w {
            let (x_min, ws) = (xb[x], &xw[x]);
            for c in 0..3 {
                let mut acc = 0.0f32;
                for (k, w) in ws.iter().enumerate() {
                    acc += *w * rgb[(y * src_w + x_min + k) * 3 + c] as f32;
                }
                tmp[(y * dst_w + x) * 3 + c] = acc;
            }
        }
    }
    let (yw, yb) = pil_precompute_coeffs(src_h, dst_h);
    let mut out = vec![0u8; dst_w * dst_h * 3];
    for y in 0..dst_h {
        let (y_min, ws) = (yb[y], &yw[y]);
        for x in 0..dst_w {
            for c in 0..3 {
                let mut acc = 0.0f32;
                for (k, w) in ws.iter().enumerate() {
                    acc += *w * tmp[((y_min + k) * dst_w + x) * 3 + c];
                }
                out[(y * dst_w + x) * 3 + c] = (acc + 0.5).floor().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(out)
}

/// PIL `precompute_coeffs` for BICUBIC (support 2.0, a = -0.5): per output
/// pixel, the source start index and normalized weights.
fn pil_precompute_coeffs(in_size: usize, out_size: usize) -> (Vec<Vec<f32>>, Vec<usize>) {
    const FILTER_SUPPORT: f64 = 2.0;
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = FILTER_SUPPORT * filterscale;
    let mut weights = Vec::with_capacity(out_size);
    let mut bounds = Vec::with_capacity(out_size);
    for xx in 0..out_size {
        let center = (xx as f64 + 0.5) * scale;
        let mut xmin = (center - support + 0.5).floor() as i64;
        if xmin < 0 {
            xmin = 0;
        }
        let mut xmax = (center + support + 0.5).floor() as i64;
        if xmax > in_size as i64 {
            xmax = in_size as i64;
        }
        let ss = 1.0 / filterscale;
        let mut ws = Vec::with_capacity((xmax - xmin) as usize);
        let mut total = 0.0f64;
        for x in xmin..xmax {
            let w = cubic_kernel_a((((x - xmin) as f64 + xmin as f64 - center + 0.5) * ss) as f32, -0.5) as f64;
            ws.push(w);
            total += w;
        }
        if total != 0.0 {
            for w in &mut ws {
                *w /= total;
            }
        }
        weights.push(ws.iter().map(|w| *w as f32).collect());
        bounds.push(xmin as usize);
    }
    (weights, bounds)
}

/// Bicubic, `align_corners=False`, sample at `(i + 0.5) * old/new - 0.5`.
/// `src` is `[src_h * src_w * dim]` row-major, last dim is channels.
pub fn interpolate_grid_bicubic(
    src: &[f32],
    src_h: usize,
    src_w: usize,
    dst_h: usize,
    dst_w: usize,
    dim: usize,
) -> Result<Vec<f32>, PbrError> {
    if src.len() != src_h * src_w * dim || src_h == 0 || src_w == 0 || dst_h == 0 || dst_w == 0 {
        return Err(PbrError::InvalidParams("pos-embed interpolate size".into()));
    }
    if src_h == dst_h && src_w == dst_w {
        return Ok(src.to_vec());
    }
    let mut out = vec![0.0f32; dst_h * dst_w * dim];
    let scale_y = src_h as f32 / dst_h as f32;
    let scale_x = src_w as f32 / dst_w as f32;
    let at = |y: i32, x: i32, c: usize| -> f32 {
        let yy = y.clamp(0, src_h as i32 - 1) as usize;
        let xx = x.clamp(0, src_w as i32 - 1) as usize;
        src[(yy * src_w + xx) * dim + c]
    };
    for y in 0..dst_h {
        let fy = (y as f32 + 0.5) * scale_y - 0.5;
        let y0 = fy.floor() as i32;
        let ty = fy - y0 as f32;
        for x in 0..dst_w {
            let fx = (x as f32 + 0.5) * scale_x - 0.5;
            let x0 = fx.floor() as i32;
            let tx = fx - x0 as f32;
            for c in 0..dim {
                let mut acc = 0.0f32;
                for ky in -1i32..=2 {
                    let wy = cubic_kernel(ty - ky as f32);
                    for kx in -1i32..=2 {
                        acc += wy * cubic_kernel(tx - kx as f32) * at(y0 + ky, x0 + kx, c);
                    }
                }
                out[(y * dst_w + x) * dim + c] = acc;
            }
        }
    }
    Ok(out)
}

/// Transformers `Dinov2Model.interpolate_pos_encoding` for a square crop.
/// `pos` is `[1 + src_grid*src_grid, hidden]` (CLS first).
pub fn interpolate_pos_embed(
    pos: &[f32],
    hidden: usize,
    src_grid: usize,
    dst_grid: usize,
) -> Result<Vec<f32>, PbrError> {
    let src_tokens = 1 + src_grid * src_grid;
    if pos.len() != src_tokens * hidden {
        return Err(PbrError::InvalidParams(format!(
            "pos embed {} != {}x{}",
            pos.len(),
            src_tokens,
            hidden
        )));
    }
    if src_grid == dst_grid {
        return Ok(pos.to_vec());
    }
    let cls = &pos[..hidden];
    let patches = interpolate_grid_bicubic(
        &pos[hidden..],
        src_grid,
        src_grid,
        dst_grid,
        dst_grid,
        hidden,
    )?;
    let mut out = Vec::with_capacity((1 + dst_grid * dst_grid) * hidden);
    out.extend_from_slice(cls);
    out.extend(patches);
    Ok(out)
}

/// Deterministic HWC uint8 ramp shared with `oracle/dino_forward_oracle.py`.
pub fn ramp_rgb8(size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 3];
    let denom = (size.saturating_sub(1)).max(1) as f64;
    for y in 0..size {
        let yn = y as f64 / denom;
        for x in 0..size {
            let xn = x as f64 / denom;
            let i = (y * size + x) * 3;
            out[i] = ((xn + yn) * 0.5 * 255.0).round().clamp(0.0, 255.0) as u8;
            out[i + 1] = (xn * 255.0).round().clamp(0.0, 255.0) as u8;
            out[i + 2] = (yn * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

pub fn default_snapshot_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("MAKEPAD_DINO_PATH") {
        return std::path::PathBuf::from(p);
    }
    std::path::PathBuf::from(
        r"C:\Users\playe\.cache\huggingface\hub\models--facebook--dinov2-giant\snapshots\611a9d42f2335e0f921f1e313ad3c1b7178d206d",
    )
}

#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
pub use exec::DinoVit;

#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
mod exec {
    use super::*;
    use crate::safetensors;
    use makepad_ai_common::backend::cuda::{
        gpu_add, gpu_attention_packed_f32, gpu_concat_rows, gpu_conv2d_planar_strided, gpu_device_available,
        gpu_download, gpu_layer_norm_pytorch, gpu_linear_f32_resident, gpu_silu, gpu_slice_cols,
        gpu_upload, GpuTensor,
    };
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::BufReader;
    use std::path::Path;

    const NS: &str = "paint-dino";
    const ATTN_SCALE: f32 = 1.0 / 8.0; // 1/sqrt(64)

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

    fn bf16_to_f32(bits: u16) -> f32 {
        f32::from_bits((bits as u32) << 16)
    }

    fn tensor_to_f32(
        record: &crate::safetensors::SafeTensorRecord,
        raw: &[u8],
    ) -> Result<Vec<f32>, String> {
        match record.dtype {
            crate::torch_bin::TorchDtype::F32 => {
                if raw.len() != record.numel * 4 {
                    return Err(format!("{} f32 byte length", record.name));
                }
                Ok(raw
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect())
            }
            crate::torch_bin::TorchDtype::F16 => {
                if raw.len() != record.numel * 2 {
                    return Err(format!("{} f16 byte length", record.name));
                }
                Ok(raw
                    .chunks_exact(2)
                    .map(|b| half_to_f32(u16::from_le_bytes([b[0], b[1]])))
                    .collect())
            }
            crate::torch_bin::TorchDtype::BF16 => {
                if raw.len() != record.numel * 2 {
                    return Err(format!("{} bf16 byte length", record.name));
                }
                Ok(raw
                    .chunks_exact(2)
                    .map(|b| bf16_to_f32(u16::from_le_bytes([b[0], b[1]])))
                    .collect())
            }
            other => Err(format!("{} unsupported dtype {other:?}", record.name)),
        }
    }

    struct Linear {
        weight: Vec<f32>,
        bias: Vec<f32>,
        out_f: usize,
        in_f: usize,
    }

    struct DinoBlock {
        n1_w: Vec<f32>,
        n1_b: Vec<f32>,
        q: Linear,
        k: Linear,
        v: Linear,
        proj: Linear,
        ls1: Vec<f32>,
        n2_w: Vec<f32>,
        n2_b: Vec<f32>,
        mlp_in: Linear,
        mlp_out: Linear,
        ls2: Vec<f32>,
    }

    /// facebook/dinov2-giant (`Dinov2Model`) last_hidden_state.
    pub struct DinoVit {
        patch_w: Vec<f32>,
        patch_b: Vec<f32>,
        cls: Vec<f32>,
        pos: Vec<f32>,
        layers: Vec<DinoBlock>,
        ln_w: Vec<f32>,
        ln_b: Vec<f32>,
    }

    impl Linear {
        fn take(w: &mut HashMap<String, Vec<f32>>, prefix: &str) -> Result<Self, String> {
            let weight = w
                .remove(&format!("{prefix}.weight"))
                .ok_or_else(|| format!("missing {prefix}.weight"))?;
            let bias = w
                .remove(&format!("{prefix}.bias"))
                .ok_or_else(|| format!("missing {prefix}.bias"))?;
            let out_f = bias.len();
            if out_f == 0 || weight.len() % out_f != 0 {
                return Err(format!("{prefix} weight/bias rank"));
            }
            let in_f = weight.len() / out_f;
            Ok(Self {
                weight,
                bias,
                out_f,
                in_f,
            })
        }

        fn apply(&self, x: &GpuTensor) -> Result<GpuTensor, String> {
            if x.cols() != self.in_f {
                return Err(format!(
                    "linear in {} vs x.cols {}",
                    self.in_f,
                    x.cols()
                ));
            }
            let w = gpu_upload(&self.weight, self.out_f, self.in_f)?;
            let b = gpu_upload(&self.bias, 1, self.out_f)?;
            gpu_linear_f32_resident(x, &w, Some(&b))
        }
    }

    impl DinoVit {
        pub fn load(path: &Path) -> Result<Self, String> {
            if !gpu_device_available() {
                return Err("CUDA unavailable".into());
            }
            if std::env::var("MAKEPAD_PBR_TAP_PARITY").as_deref() == Ok("1") {
                std::env::set_var("FLUX_VAE_CONV_GEMM", "0");
                std::env::set_var("FLUX_VAE_CONV_IM2COL", "0");
                // Composite attn defaults to f16 GEMM; 40 layers accumulate past 1e-3.
                std::env::set_var("FLUX_ATTN_F16", "0");
            }
            let st = if path.is_dir() {
                path.join("model.safetensors")
            } else {
                path.to_path_buf()
            };
            let mut file = BufReader::new(File::open(&st).map_err(|e| e.to_string())?);
            let index = safetensors::read_index_from(&mut file).map_err(|e| e.to_string())?;
            let mut raw = Vec::new();
            let mut w = HashMap::new();
            for record in &index.tensors {
                index
                    .read_tensor_into(&mut file, record, &mut raw)
                    .map_err(|e| e.to_string())?;
                w.insert(record.name.clone(), tensor_to_f32(record, &raw)?);
            }
            let patch_w = w
                .remove("embeddings.patch_embeddings.projection.weight")
                .ok_or("missing patch weight")?;
            let patch_b = w
                .remove("embeddings.patch_embeddings.projection.bias")
                .ok_or("missing patch bias")?;
            let cls = w.remove("embeddings.cls_token").ok_or("missing cls_token")?;
            let pos_raw = w
                .remove("embeddings.position_embeddings")
                .ok_or("missing position_embeddings")?;
            if pos_raw.len() != trained_pos_tokens() * HIDDEN {
                return Err(format!(
                    "pos embed {} vs {}x{}",
                    pos_raw.len(),
                    trained_pos_tokens(),
                    HIDDEN
                ));
            }
            let src_grid = MODEL_IMAGE_SIZE / PATCH;
            let dst_grid = PROC_CROP / PATCH;
            let pos = interpolate_pos_embed(&pos_raw, HIDDEN, src_grid, dst_grid)
                .map_err(|e| e.to_string())?;
            let mut layers = Vec::with_capacity(DEPTH);
            for i in 0..DEPTH {
                let p = format!("encoder.layer.{i}");
                let take_vec = |w: &mut HashMap<String, Vec<f32>>, name: &str| {
                    w.remove(name)
                        .ok_or_else(|| format!("missing {name}"))
                };
                layers.push(DinoBlock {
                    n1_w: take_vec(&mut w, &format!("{p}.norm1.weight"))?,
                    n1_b: take_vec(&mut w, &format!("{p}.norm1.bias"))?,
                    q: Linear::take(&mut w, &format!("{p}.attention.attention.query"))?,
                    k: Linear::take(&mut w, &format!("{p}.attention.attention.key"))?,
                    v: Linear::take(&mut w, &format!("{p}.attention.attention.value"))?,
                    proj: Linear::take(&mut w, &format!("{p}.attention.output.dense"))?,
                    ls1: take_vec(&mut w, &format!("{p}.layer_scale1.lambda1"))?,
                    n2_w: take_vec(&mut w, &format!("{p}.norm2.weight"))?,
                    n2_b: take_vec(&mut w, &format!("{p}.norm2.bias"))?,
                    mlp_in: Linear::take(&mut w, &format!("{p}.mlp.weights_in"))?,
                    mlp_out: Linear::take(&mut w, &format!("{p}.mlp.weights_out"))?,
                    ls2: take_vec(&mut w, &format!("{p}.layer_scale2.lambda1"))?,
                });
            }
            let ln_w = w.remove("layernorm.weight").ok_or("missing layernorm.weight")?;
            let ln_b = w.remove("layernorm.bias").ok_or("missing layernorm.bias")?;
            if layers[0].mlp_in.out_f != 2 * SWIGLU_HIDDEN || layers[0].mlp_in.in_f != HIDDEN {
                return Err(format!(
                    "swiglu in {}x{}, expected {}x{}",
                    layers[0].mlp_in.out_f,
                    layers[0].mlp_in.in_f,
                    2 * SWIGLU_HIDDEN,
                    HIDDEN
                ));
            }
            Ok(Self {
                patch_w,
                patch_b,
                cls,
                pos,
                layers,
                ln_w,
                ln_b,
            })
        }

        fn layer_norm(x: &GpuTensor, w: &[f32], b: &[f32]) -> Result<GpuTensor, String> {
            gpu_layer_norm_pytorch(x, w, b, LN_EPS)
        }

        fn scale_tokens(x: &GpuTensor, scale: &[f32]) -> Result<GpuTensor, String> {
            if scale.len() != x.cols() {
                return Err(format!("layerscale {} vs cols {}", scale.len(), x.cols()));
            }
            let mut h = gpu_download(x)?;
            let cols = x.cols();
            for row in h.chunks_exact_mut(cols) {
                for (v, s) in row.iter_mut().zip(scale.iter()) {
                    *v *= *s;
                }
            }
            gpu_upload(&h, x.rows(), cols)
        }

        fn swiglu(x: &GpuTensor, block: &DinoBlock) -> Result<GpuTensor, String> {
            let packed = block.mlp_in.apply(x)?;
            if packed.cols() != 2 * SWIGLU_HIDDEN {
                return Err(format!("swiglu packed cols {}", packed.cols()));
            }
            // HF Dinov2SwiGLUFFN: silu(first half) * second half.
            let x1 = gpu_silu(&gpu_slice_cols(&packed, 0, SWIGLU_HIDDEN)?)?;
            let x2 = gpu_slice_cols(&packed, SWIGLU_HIDDEN, SWIGLU_HIDDEN)?;
            let a = gpu_download(&x1)?;
            let b = gpu_download(&x2)?;
            let mixed: Vec<f32> = a.iter().zip(b.iter()).map(|(u, v)| u * v).collect();
            let hidden = gpu_upload(&mixed, packed.rows(), SWIGLU_HIDDEN)?;
            block.mlp_out.apply(&hidden)
        }

        fn block(x: &GpuTensor, block: &DinoBlock) -> Result<GpuTensor, String> {
            let n1 = Self::layer_norm(x, &block.n1_w, &block.n1_b)?;
            let q = block.q.apply(&n1)?;
            let k = block.k.apply(&n1)?;
            let v = block.v.apply(&n1)?;
            // f32 QK/PV/softmax always. DINOv2-giant carries massive-activation
            // outlier tokens (|x| in the hundreds by mid-depth); the default
            // FLUX_ATTN_F16 composite path saturates them and the error grows
            // 0.25 -> 280 max_abs across layers 16..39 (oracle bisect on the
            // elf reference), which starves the paint UNet of reference
            // structure. HF fp16 SDPA accumulates in f32; so must we.
            let attn = gpu_attention_packed_f32(&q, &k, &v, HEADS, ATTN_SCALE)?;
            let attn = block.proj.apply(&attn)?;
            let attn = Self::scale_tokens(&attn, &block.ls1)?;
            let h = gpu_add(x, &attn)?;
            let n2 = Self::layer_norm(&h, &block.n2_w, &block.n2_b)?;
            let mlp = Self::swiglu(&n2, block)?;
            let mlp = Self::scale_tokens(&mlp, &block.ls2)?;
            gpu_add(&h, &mlp)
        }

        pub fn embeddings(&self, pixels: &[f32]) -> Result<Vec<f32>, String> {
            let plane = PROC_CROP * PROC_CROP;
            if pixels.len() != 3 * plane {
                return Err(format!(
                    "pixels {} vs 3x{PROC_CROP}x{PROC_CROP}",
                    pixels.len()
                ));
            }
            let x = gpu_upload(pixels, 3, plane)?;
            let grid = PROC_CROP / PATCH;
            let patches = gpu_conv2d_planar_strided(
                &x,
                PROC_CROP,
                PROC_CROP,
                grid,
                grid,
                NS,
                "patch",
                &self.patch_w,
                &self.patch_b,
                HIDDEN,
                PATCH,
                PATCH,
                0,
                0,
                PATCH,
                PATCH,
            )?;
            let planar = gpu_download(&patches)?;
            let mut tokens = vec![0.0f32; grid * grid * HIDDEN];
            let pplane = grid * grid;
            for c in 0..HIDDEN {
                for i in 0..pplane {
                    tokens[i * HIDDEN + c] = planar[c * pplane + i];
                }
            }
            let patches = gpu_upload(&tokens, pplane, HIDDEN)?;
            let cls = gpu_upload(&self.cls, 1, HIDDEN)?;
            let tokens = gpu_concat_rows(&cls, &patches)?;
            let pos = gpu_upload(&self.pos, TOKENS, HIDDEN)?;
            gpu_download(&gpu_add(&tokens, &pos)?)
        }

        /// Isolated block with sub-op taps: returns (norm1, attn_out_scaled,
        /// h_after_attn, norm2, mlp_out_scaled, out). Debug/bisect only.
        pub fn block_taps_at(
            &self,
            hidden: &[f32],
            layer: usize,
        ) -> Result<[Vec<f32>; 6], String> {
            if layer >= self.layers.len() {
                return Err(format!("layer {layer} >= {}", self.layers.len()));
            }
            if hidden.len() != TOKENS * HIDDEN {
                return Err(format!("hidden {} vs {TOKENS}x{HIDDEN}", hidden.len()));
            }
            let block = &self.layers[layer];
            let x = gpu_upload(hidden, TOKENS, HIDDEN)?;
            let n1 = Self::layer_norm(&x, &block.n1_w, &block.n1_b)?;
            let q = block.q.apply(&n1)?;
            let k = block.k.apply(&n1)?;
            let v = block.v.apply(&n1)?;
            let attn = gpu_attention_packed_f32(&q, &k, &v, HEADS, ATTN_SCALE)?;
            let attn = block.proj.apply(&attn)?;
            let attn = Self::scale_tokens(&attn, &block.ls1)?;
            let h = gpu_add(&x, &attn)?;
            let n2 = Self::layer_norm(&h, &block.n2_w, &block.n2_b)?;
            let mlp = Self::swiglu(&n2, block)?;
            let mlp = Self::scale_tokens(&mlp, &block.ls2)?;
            let out = gpu_add(&h, &mlp)?;
            Ok([
                gpu_download(&n1)?,
                gpu_download(&attn)?,
                gpu_download(&h)?,
                gpu_download(&n2)?,
                gpu_download(&mlp)?,
                gpu_download(&out)?,
            ])
        }

        pub fn block_at(&self, hidden: &[f32], layer: usize) -> Result<Vec<f32>, String> {
            if layer >= self.layers.len() {
                return Err(format!("layer {layer} >= {}", self.layers.len()));
            }
            if hidden.len() != TOKENS * HIDDEN {
                return Err(format!("hidden {} vs {TOKENS}x{HIDDEN}", hidden.len()));
            }
            let x = gpu_upload(hidden, TOKENS, HIDDEN)?;
            gpu_download(&Self::block(&x, &self.layers[layer])?)
        }

        /// Official `AutoModel(...)[0]` / `last_hidden_state`: `[TOKENS, HIDDEN]`.
        pub fn forward(&self, pixels: &[f32]) -> Result<Vec<f32>, String> {
            let emb = self.embeddings(pixels)?;
            let dump_dir = std::env::var("MAKEPAD_PBR_DINO_LAYER_DUMP").ok();
            let dump = |tag: &str, data: &[f32]| {
                if let Some(dir) = &dump_dir {
                    let mut bytes = Vec::with_capacity(data.len() * 4);
                    for v in data {
                        bytes.extend_from_slice(&v.to_le_bytes());
                    }
                    let _ = std::fs::write(format!("{dir}/dino_layer_{tag}.f32"), bytes);
                }
            };
            dump("emb", &emb);
            let mut x = gpu_upload(&emb, TOKENS, HIDDEN)?;
            for (i, layer) in self.layers.iter().enumerate() {
                x = Self::block(&x, layer)?;
                if dump_dir.is_some() {
                    dump(&format!("{i:02}"), &gpu_download(&x)?);
                }
            }
            let x = Self::layer_norm(&x, &self.ln_w, &self.ln_b)?;
            gpu_download(&x)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_matches_pinned_config() {
        assert_eq!(HIDDEN, hunyuan::unet_arch().dino_hidden_dim as usize);
        assert_eq!(HEADS, 24);
        assert_eq!(DEPTH, 40);
        assert_eq!(PATCH, 14);
        assert_eq!(MODEL_IMAGE_SIZE % PATCH, 0);
        assert_eq!(tokens_for_spatial(MODEL_IMAGE_SIZE), 1 + 37 * 37);
        assert_eq!(tokens_for_spatial(PROC_CROP), 1 + 16 * 16);
        assert_eq!(TOKENS, 257);
        assert_eq!(SWIGLU_HIDDEN, 4096);
        assert!(USE_SWIGLU);
        assert!(QKV_BIAS);
    }

    #[test]
    fn preprocess_black_matches_box_oracle() {
        let rgb = vec![0u8; 512 * 512 * 3];
        let out = preprocess_official(&rgb, 512, 512).unwrap();
        let plane = PROC_CROP * PROC_CROP;
        let min = out.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = out.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        // Box dump: pixel_min -2.1179039478302, pixel_max -1.804444432258606
        assert!((min - ((0.0 - IMAGENET_MEAN[0]) / IMAGENET_STD[0])).abs() < 1e-5);
        assert!((max - ((0.0 - IMAGENET_MEAN[2]) / IMAGENET_STD[2])).abs() < 1e-5);
        assert_eq!(out.len(), 3 * plane);
    }

    #[test]
    fn pos_embed_identity_and_grid_shrink() {
        assert_eq!(trained_pos_tokens(), 1 + 37 * 37);
        let hidden = 4;
        let src = 4;
        let mut pos = vec![0.0f32; (1 + src * src) * hidden];
        for i in 0..hidden {
            pos[i] = 7.0 + i as f32;
        }
        for y in 0..src {
            for x in 0..src {
                for c in 0..hidden {
                    pos[(1 + y * src + x) * hidden + c] = (y * 10 + x) as f32 + c as f32 * 0.1;
                }
            }
        }
        let same = interpolate_pos_embed(&pos, hidden, src, src).unwrap();
        assert_eq!(same, pos);
        let dst = interpolate_pos_embed(&pos, hidden, src, 2).unwrap();
        assert_eq!(dst.len(), (1 + 4) * hidden);
        assert_eq!(&dst[..hidden], &pos[..hidden]);
        assert!(dst[hidden..].iter().all(|v| v.is_finite()));
    }

    #[test]
    fn preprocess_constant_gray_is_channel_constant() {
        let rgb = vec![128u8; 64 * 64 * 3];
        let out = preprocess_official(&rgb, 64, 64).unwrap();
        assert_eq!(out.len(), 3 * PROC_CROP * PROC_CROP);
        let plane = PROC_CROP * PROC_CROP;
        for c in 0..3 {
            let v = out[c * plane];
            assert!(out[c * plane..c * plane + plane].iter().all(|x| (*x - v).abs() < 1e-5));
            let expect = (128.0 / 255.0 - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
            assert!((v - expect).abs() < 1e-4, "ch {c} {v} vs {expect}");
        }
    }
}
