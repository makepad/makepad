//! Native single-image DA3METRIC-LARGE inference.
//!
//! This is the bounded production contract used by AI Content's metric-depth
//! domain: DINOv2 ViT-L/14 (blocks 4/11/17/23) followed by the 256-channel
//! DPT depth/sky head.  Multi-view attention, pose estimation, nested DA3 and
//! Gaussian-splat branches are deliberately outside this model ID.
//!
//! The checkpoint is f32.  This implementation keeps weights and activations
//! f32 on CUDA, reusing the resident transformer and planar-image primitives
//! already qualified by TRELLIS and BiRefNet.  There is no subprocess or
//! alternate model fallback.

use crate::backend::{
    gpu_add, gpu_attention_packed, gpu_attention_packed_flash_bf16,
    gpu_birefnet_image_to_patches, gpu_birefnet_relu,
    gpu_birefnet_resize_bilinear, gpu_birefnet_tokens_to_planar, gpu_concat_cols,
    gpu_concat_rows, gpu_conv2d_planar_cached, gpu_download, gpu_gelu_erf, gpu_graph_capture,
    gpu_graph_launch, gpu_layer_norm_mod, gpu_linear_f32_resident,
    gpu_linear_nt_cached_bf16_f32acc, gpu_pixel_shuffle_planar_cached, gpu_slice_cols,
    gpu_slice_rows, gpu_upload, gpu_upload_into, GpuLinearPart, GpuStepGraph, GpuTensor,
};
use makepad_ai_common::quant::GGML_TYPE_BF16;
use crate::{emit_progress, DiffusionError, ProgressHook, Result};
use std::io::{Read, Seek, SeekFrom};
use std::cell::RefCell;
use std::path::Path;
use std::sync::Mutex;

pub const DA3_PROCESS_RESOLUTION: usize = 504;
pub const DA3_PATCH: usize = 14;
pub const DA3_HIDDEN: usize = 1024;
pub const DA3_DEPTH: usize = 24;
pub const DA3_HEADS: usize = 16;
pub const DA3_HEAD_DIM: usize = 64;
pub const DA3_MLP: usize = 4096;
pub const DA3_DPT_FEATURES: usize = 256;
pub const DA3_NORM_EPS: f32 = 1e-6;
pub const DA3_BASE_PATCH_SIDE: usize = 37; // 518 / 14
pub const DA3_TAP_LAYERS: [usize; 4] = [4, 11, 17, 23];

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];
const PREFIX: &str = "model.";
const CACHE_NAMESPACE: &str = "da3-metric-large";

/// Exact runtime identity surfaced by the native service and validator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Da3ExecutionInfo {
    pub compute_backend: &'static str,
    pub weight_precision: &'static str,
    pub activation_precision: &'static str,
    pub accumulation_precision: &'static str,
}

/// Explicit DA3 compute mode.  Selection is structural (it decides which
/// weight copies exist on device), never an automatic fallback: a model
/// loaded in one mode can only ever run in that mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Da3Precision {
    /// Production default: bf16 tensor-core transformer linears and bf16
    /// flash attention with f32 accumulation, conv GEMM operands f16.
    /// Matches the numerical class of the bf16-autocast torch reference
    /// while beating its warm latency.
    FullBf16,
    /// Explicit validation / high-accuracy mode: true f32 linears and f32
    /// attention (conv GEMM operands remain f16).  Requires
    /// `FLUX_ATTN_F16=0` in the environment and refuses to load otherwise;
    /// never selected automatically.
    StrictF32,
}

impl Da3Precision {
    pub fn execution_info(self) -> Da3ExecutionInfo {
        match self {
            Da3Precision::FullBf16 => Da3ExecutionInfo {
                compute_backend: "makepad-ggml-cuda",
                weight_precision: "linears bf16, convs f16, rest f32",
                activation_precision: "gemm operands bf16/f16, rest f32",
                accumulation_precision: "f32",
            },
            Da3Precision::StrictF32 => Da3ExecutionInfo {
                compute_backend: "makepad-ggml-cuda",
                weight_precision: "f32 (conv gemm operands f16)",
                activation_precision: "f32 (conv gemm operands f16)",
                accumulation_precision: "f32",
            },
        }
    }
}

/// Environment preconditions for a mode, checked fail-closed at load time.
/// The attention composite and the conv im2col path read these process-wide
/// switches inside the shared CUDA backend, so a misconfigured environment
/// must refuse to load rather than silently change compute.
fn precision_env_error(precision: Da3Precision) -> Option<&'static str> {
    if std::env::var("FLUX_VAE_CONV_GEMM").as_deref() == Ok("0") {
        return Some(
            "DA3 requires the conv GEMM path; unset FLUX_VAE_CONV_GEMM or set it to 1",
        );
    }
    match precision {
        Da3Precision::StrictF32 => {
            if std::env::var("FLUX_ATTN_F16").as_deref() != Ok("0") {
                return Some(
                    "DA3 strict-f32 requires FLUX_ATTN_F16=0 in the service environment; \
                     refusing to run with f16 attention operands",
                );
            }
            None
        }
        Da3Precision::FullBf16 => None,
    }
}

#[derive(Clone, Debug)]
pub struct Da3Preprocessed {
    /// ImageNet-normalized planar RGB.
    pub pixels: Vec<f32>,
    pub width: usize,
    pub height: usize,
    /// Scale from original pixel coordinates to processed pixel coordinates.
    pub scale_x: f32,
    pub scale_y: f32,
}

#[derive(Clone, Debug)]
pub struct Da3Prediction {
    /// Canonical unscaled DA3 metric-head values (the exponential head output).
    pub depth: Vec<f32>,
    /// ReLU sky-head score, before boolean thresholding.
    pub sky: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

impl Da3Prediction {
    /// Apply DA3's deterministic sky-depth policy.  The reference randomly
    /// subsamples when there are over 100k non-sky pixels; using the exact
    /// full p99 removes output nondeterminism without changing learned math.
    pub fn apply_sky_depth_policy(&mut self) {
        let mut non_sky: Vec<f32> = self
            .depth
            .iter()
            .zip(&self.sky)
            .filter_map(|(&depth, &sky)| (sky < 0.3 && depth.is_finite()).then_some(depth))
            .collect();
        let sky_count = self.sky.iter().filter(|&&value| value >= 0.3).count();
        if non_sky.len() <= 10 || sky_count <= 10 {
            return;
        }
        // torch.quantile(..., 0.99), linear interpolation.  Selection instead
        // of a full sort; identical order statistics.
        let position = 0.99f64 * (non_sky.len() - 1) as f64;
        let lower = position.floor() as usize;
        let upper = position.ceil() as usize;
        let fraction = (position - lower as f64) as f32;
        let (_, lower_value, above) = non_sky.select_nth_unstable_by(lower, f32::total_cmp);
        let lower_value = *lower_value;
        let upper_value = if upper > lower {
            above
                .iter()
                .copied()
                .min_by(f32::total_cmp)
                .unwrap_or(lower_value)
        } else {
            lower_value
        };
        let far = lower_value * (1.0 - fraction) + upper_value * fraction;
        for (depth, &sky) in self.depth.iter_mut().zip(&self.sky) {
            if sky >= 0.3 {
                *depth = far;
            }
        }
    }
}

struct DinoLayer {
    norm1: GpuTensor,
    norm2: GpuTensor,
    linears: DinoLinears,
}

/// Per-mode transformer linear storage.  Exactly one representation exists
/// per loaded model, so the compute mode cannot drift at run time.
enum DinoLinears {
    /// Strict-f32: resident f32 weights, plain `cublas_sgemm`.
    F32 {
        qkv_w: GpuTensor,
        qkv_b: GpuTensor,
        proj_w: GpuTensor,
        proj_b: GpuTensor,
        fc1_w: GpuTensor,
        fc1_b: GpuTensor,
        fc2_w: GpuTensor,
        fc2_b: GpuTensor,
    },
    /// Full-bf16: host BF16 copies driving the tensor-core cached-linear
    /// path (f32 accumulation), mirroring the production torch reference's
    /// bf16 autocast GEMM semantics.
    Bf16(DinoLayerBf16),
}

struct DinoLayerBf16 {
    qkv: Vec<u8>,
    qkv_key: String,
    qkv_bias: Vec<f32>,
    proj: Vec<u8>,
    proj_key: String,
    proj_bias: Vec<f32>,
    fc1: Vec<u8>,
    fc1_key: String,
    fc1_bias: Vec<f32>,
    fc2: Vec<u8>,
    fc2_key: String,
    fc2_bias: Vec<f32>,
}

impl DinoLayer {
    fn bf16_linear(
        x: &GpuTensor,
        bytes: &[u8],
        key: &str,
        n: usize,
        bias: &[f32],
    ) -> Result<GpuTensor> {
        gpu_linear_nt_cached_bf16_f32acc(
            x,
            CACHE_NAMESPACE,
            &[GpuLinearPart {
                bt_ggml_type: GGML_TYPE_BF16,
                n,
                cache_key: key,
                bytes,
            }],
            bias,
        )
        .map_err(DiffusionError::model)
    }

    fn qkv(&self, x: &GpuTensor) -> Result<GpuTensor> {
        match &self.linears {
            DinoLinears::F32 { qkv_w, qkv_b, .. } => {
                gpu_linear_f32_resident(x, qkv_w, Some(qkv_b)).map_err(DiffusionError::model)
            }
            DinoLinears::Bf16(bf16) => {
                Self::bf16_linear(x, &bf16.qkv, &bf16.qkv_key, 3 * DA3_HIDDEN, &bf16.qkv_bias)
            }
        }
    }

    fn proj(&self, x: &GpuTensor) -> Result<GpuTensor> {
        match &self.linears {
            DinoLinears::F32 { proj_w, proj_b, .. } => {
                gpu_linear_f32_resident(x, proj_w, Some(proj_b)).map_err(DiffusionError::model)
            }
            DinoLinears::Bf16(bf16) => {
                Self::bf16_linear(x, &bf16.proj, &bf16.proj_key, DA3_HIDDEN, &bf16.proj_bias)
            }
        }
    }

    fn fc1(&self, x: &GpuTensor) -> Result<GpuTensor> {
        match &self.linears {
            DinoLinears::F32 { fc1_w, fc1_b, .. } => {
                gpu_linear_f32_resident(x, fc1_w, Some(fc1_b)).map_err(DiffusionError::model)
            }
            DinoLinears::Bf16(bf16) => {
                Self::bf16_linear(x, &bf16.fc1, &bf16.fc1_key, DA3_MLP, &bf16.fc1_bias)
            }
        }
    }

    fn fc2(&self, x: &GpuTensor) -> Result<GpuTensor> {
        match &self.linears {
            DinoLinears::F32 { fc2_w, fc2_b, .. } => {
                gpu_linear_f32_resident(x, fc2_w, Some(fc2_b)).map_err(DiffusionError::model)
            }
            DinoLinears::Bf16(bf16) => {
                Self::bf16_linear(x, &bf16.fc2, &bf16.fc2_key, DA3_HIDDEN, &bf16.fc2_bias)
            }
        }
    }
}

/// Round-to-nearest-even f32 -> bf16 bytes, matching torch's conversion.
fn f32_to_bf16_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 2);
    for &value in values {
        let bits = value.to_bits();
        let rounded = ((bits.wrapping_add(0x7fff + ((bits >> 16) & 1))) >> 16) as u16;
        out.extend_from_slice(&rounded.to_le_bytes());
    }
    out
}

/// Pack LayerNorm weight/bias into one resident row for `gpu_layer_norm_mod`,
/// which applies `(1 + scale)`: storing `weight - 1` keeps the resident path
/// bit-exact for weights in [0.5, 2] (Sterbenz) and within 1 ulp outside.
/// Resident norms keep the whole forward free of per-call host uploads, which
/// CUDA graph capture requires.
fn norm_mods(weights: &Da3WeightFile, prefix: &str, dim: usize) -> Result<GpuTensor> {
    let weight = tensor(weights, &format!("{prefix}.weight"), dim)?;
    let bias = tensor(weights, &format!("{prefix}.bias"), dim)?;
    let mut packed = Vec::with_capacity(2 * dim);
    packed.extend(weight.iter().map(|value| value - 1.0));
    packed.extend_from_slice(&bias);
    upload(&packed, 1, 2 * dim)
}

struct Conv2d {
    key: String,
    weights: Vec<f32>,
    bias: Vec<f32>,
    out_channels: usize,
    kw: usize,
    kh: usize,
    pad_x: usize,
    pad_y: usize,
}

impl Conv2d {
    fn load(
        weights: &Da3WeightFile,
        name: &str,
        in_channels: usize,
        out_channels: usize,
        kw: usize,
        kh: usize,
        bias: bool,
    ) -> Result<Self> {
        let values = tensor(weights, &format!("{name}.weight"), out_channels * in_channels * kw * kh)?;
        let bias_values = if bias {
            tensor(weights, &format!("{name}.bias"), out_channels)?
        } else {
            vec![0.0; out_channels]
        };
        Ok(Self {
            key: name.to_string(),
            weights: values,
            bias: bias_values,
            out_channels,
            kw,
            kh,
            pad_x: kw / 2,
            pad_y: kh / 2,
        })
    }

    fn forward(&self, input: &Planar) -> Result<Planar> {
        let tensor = gpu_conv2d_planar_cached(
            &input.tensor,
            input.width,
            input.height,
            CACHE_NAMESPACE,
            &self.key,
            &self.weights,
            &self.bias,
            self.out_channels,
            self.kw,
            self.kh,
            self.pad_x,
            self.pad_y,
        )
        .map_err(DiffusionError::model)?;
        Ok(Planar {
            tensor,
            width: input.width,
            height: input.height,
        })
    }
}

struct ConvTransposeNoOverlap {
    weight: GpuTensor, // [out_channels * scale^2, in_channels]
    bias: Vec<f32>,
    bias_key: String,
    out_channels: usize,
    scale: usize,
}

impl ConvTransposeNoOverlap {
    fn load(
        weights: &Da3WeightFile,
        name: &str,
        channels: usize,
        scale: usize,
    ) -> Result<Self> {
        // PyTorch ConvTranspose2d stores [in, out, kh, kw].  The resident
        // linear consumes [out*kh*kw, in], ordered [out, ky, kx].
        let source = tensor(
            weights,
            &format!("{name}.weight"),
            channels * channels * scale * scale,
        )?;
        let mut reordered = vec![0.0f32; source.len()];
        for output in 0..channels {
            for ky in 0..scale {
                for kx in 0..scale {
                    let feature = (output * scale + ky) * scale + kx;
                    for input in 0..channels {
                        let src = ((input * channels + output) * scale + ky) * scale + kx;
                        reordered[feature * channels + input] = source[src];
                    }
                }
            }
        }
        Ok(Self {
            weight: upload(&reordered, channels * scale * scale, channels)?,
            bias: tensor(weights, &format!("{name}.bias"), channels)?,
            bias_key: name.to_string(),
            out_channels: channels,
            scale,
        })
    }

    fn forward(&self, input: Planar) -> Result<Planar> {
        let tokens = gpu_birefnet_tokens_to_planar(&input.tensor)
            .map_err(DiffusionError::model)?;
        let expanded = gpu_linear_f32_resident(&tokens, &self.weight, None)
            .map_err(DiffusionError::model)?;
        let expanded = gpu_birefnet_tokens_to_planar(&expanded)
            .map_err(DiffusionError::model)?;
        let tensor = gpu_pixel_shuffle_planar_cached(
            &expanded,
            input.width,
            input.height,
            self.out_channels,
            self.scale,
            CACHE_NAMESPACE,
            &self.bias_key,
            &self.bias,
        )
        .map_err(DiffusionError::model)?;
        Ok(Planar {
            tensor,
            width: input.width * self.scale,
            height: input.height * self.scale,
        })
    }
}

struct ResidualConvUnit {
    conv1: Conv2d,
    conv2: Conv2d,
}

impl ResidualConvUnit {
    fn load(weights: &Da3WeightFile, prefix: &str) -> Result<Self> {
        Ok(Self {
            conv1: Conv2d::load(weights, &format!("{prefix}.conv1"), 256, 256, 3, 3, true)?,
            conv2: Conv2d::load(weights, &format!("{prefix}.conv2"), 256, 256, 3, 3, true)?,
        })
    }

    fn forward(&self, input: &Planar) -> Result<Planar> {
        let activated = relu(input)?;
        let hidden = self.conv1.forward(&activated)?;
        let activated = relu(&hidden)?;
        let hidden = self.conv2.forward(&activated)?;
        add_planar(input, &hidden)
    }
}

struct FusionBlock {
    residual: Option<ResidualConvUnit>,
    main: ResidualConvUnit,
    out: Conv2d,
}

impl FusionBlock {
    fn load(weights: &Da3WeightFile, index: usize, has_residual: bool) -> Result<Self> {
        let prefix = format!("{PREFIX}head.scratch.refinenet{index}");
        Ok(Self {
            residual: if has_residual {
                Some(ResidualConvUnit::load(weights, &format!("{prefix}.resConfUnit1"))?)
            } else {
                None
            },
            main: ResidualConvUnit::load(weights, &format!("{prefix}.resConfUnit2"))?,
            out: Conv2d::load(weights, &format!("{prefix}.out_conv"), 256, 256, 1, 1, true)?,
        })
    }

    fn forward(&self, top: Planar, lateral: Option<&Planar>, out_size: (usize, usize)) -> Result<Planar> {
        let merged = match (&self.residual, lateral) {
            (Some(unit), Some(lateral)) => {
                let residual = unit.forward(lateral)?;
                add_planar(&top, &residual)?
            }
            (None, None) => top,
            _ => return Err(DiffusionError::workflow("DA3 fusion residual contract mismatch")),
        };
        let hidden = self.main.forward(&merged)?;
        let hidden = resize(&hidden, out_size.0, out_size.1, true)?;
        self.out.forward(&hidden)
    }
}

struct Planar {
    tensor: GpuTensor,
    width: usize,
    height: usize,
}

pub struct Da3MetricLarge {
    patch_w: GpuTensor,
    patch_b: GpuTensor,
    cls: GpuTensor,
    base_pos: Vec<f32>,
    layers: Vec<DinoLayer>,
    final_norm: GpuTensor,
    projects: Vec<Conv2d>,
    resize0: ConvTransposeNoOverlap,
    resize1: ConvTransposeNoOverlap,
    resize3: Conv2d,
    scratch_layers: Vec<Conv2d>,
    fusion: Vec<FusionBlock>, // index 0 == refinenet1
    output_conv1: Conv2d,
    depth_conv: Conv2d,
    depth_out: Conv2d,
    sky_conv: Conv2d,
    sky_out: Conv2d,
    /// Device-resident interpolated position embedding for the last seen
    /// patch grid; recomputing it on the host cost ~50ms per request.
    pos_cache: Mutex<Option<((usize, usize), GpuTensor)>>,
    /// Resident one-hot selection matrix for the stride-two downsample of
    /// `resize_layers.3`.  A GEMM with one-hot rows is exact for finite
    /// values and, unlike an index gather, uploads nothing per call.
    stride_cache: Mutex<Option<((usize, usize), GpuTensor)>>,
    precision: Da3Precision,
    /// Process-unique instance id keying the captured-graph cache. An
    /// address key replayed a stale graph (freed device buffers) when a
    /// model was dropped and a reload landed at the same address.
    generation: u64,
}

struct Da3WeightFile {
    path: std::path::PathBuf,
    data_offset: u64,
    tensors: std::collections::HashMap<String, (u64, u64)>,
}

impl Da3WeightFile {
    fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = std::fs::File::open(&path)
            .map_err(|err| DiffusionError::model(format!("{}: {err}", path.display())))?;
        let mut len_bytes = [0u8; 8];
        file.read_exact(&mut len_bytes)
            .map_err(|err| DiffusionError::model(format!("{} header len: {err}", path.display())))?;
        let header_len = u64::from_le_bytes(len_bytes);
        let mut header = vec![0u8; header_len as usize];
        file.read_exact(&mut header)
            .map_err(|err| DiffusionError::model(format!("{} header: {err}", path.display())))?;
        let header_text = std::str::from_utf8(&header).map_err(|err| {
            DiffusionError::model(format!("{} header utf8: {err}", path.display()))
        })?;
        let mut tensors = std::collections::HashMap::new();
        let mut remaining = header_text;
        while let Some(name_start) = remaining.find('"') {
            remaining = &remaining[name_start + 1..];
            let Some(name_end) = remaining.find('"') else { break };
            let name = remaining[..name_end].to_string();
            remaining = &remaining[name_end + 1..];
            if name == "__metadata__" {
                continue;
            }
            let Some(dtype_at) = remaining.find("\"dtype\"") else { continue };
            let after_dtype = &remaining[dtype_at + 7..];
            if !after_dtype.contains("F32") && !after_dtype.contains("f32") {
                continue;
            }
            let Some(data_off) = remaining.find("\"data_offsets\"") else { continue };
            let after = &remaining[data_off..];
            let Some(bracket) = after.find('[') else { continue };
            let nums = &after[bracket + 1..];
            let mut parts = nums.split([',', ']']);
            let Some(start) = parts.next().and_then(|s| s.trim().parse::<u64>().ok()) else {
                continue;
            };
            let Some(end) = parts.next().and_then(|s| s.trim().parse::<u64>().ok()) else {
                continue;
            };
            tensors.insert(name, (start, end));
        }
        Ok(Self {
            path,
            data_offset: 8 + header_len,
            tensors,
        })
    }

    fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        let (start, end) = self.tensors.get(name).copied().ok_or_else(|| {
            DiffusionError::model(format!(
                "DA3 tensor {name:?} not found in {}",
                self.path.display()
            ))
        })?;
        if end < start || (end - start) % 4 != 0 {
            return Err(DiffusionError::model(format!(
                "DA3 tensor {name:?}: invalid f32 range {start}..{end}"
            )));
        }
        let mut file = std::fs::File::open(&self.path)
            .map_err(|err| DiffusionError::model(format!("{}: {err}", self.path.display())))?;
        file.seek(SeekFrom::Start(self.data_offset + start))
            .map_err(|err| DiffusionError::model(format!("{} seek: {err}", self.path.display())))?;
        let mut bytes = vec![0u8; (end - start) as usize];
        file.read_exact(&mut bytes)
            .map_err(|err| DiffusionError::model(format!("DA3 tensor {name:?}: {err}")))?;
        Ok(bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }
}

fn tensor(weights: &Da3WeightFile, name: &str, len: usize) -> Result<Vec<f32>> {
    let values = weights.tensor_f32(name)?;
    if values.len() != len {
        return Err(DiffusionError::model(format!(
            "DA3 tensor {name:?}: expected {len} values, got {}",
            values.len()
        )));
    }
    Ok(values)
}

fn upload(values: &[f32], rows: usize, cols: usize) -> Result<GpuTensor> {
    gpu_upload(values, rows, cols).map_err(DiffusionError::model)
}

impl Da3MetricLarge {
    /// Load with the production default mode ([`Da3Precision::FullBf16`]).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_precision(path, Da3Precision::FullBf16)
    }

    pub fn load_with_precision(path: impl AsRef<Path>, precision: Da3Precision) -> Result<Self> {
        let weights = Da3WeightFile::load(path)?;
        Self::prepare_with_precision(&weights, None, precision)
    }

    pub fn prepare(weights: &Da3WeightFile, progress: Option<ProgressHook>) -> Result<Self> {
        Self::prepare_with_precision(weights, progress, Da3Precision::FullBf16)
    }

    pub fn prepare_with_precision(
        weights: &Da3WeightFile,
        mut progress: Option<ProgressHook>,
        precision: Da3Precision,
    ) -> Result<Self> {
        if let Some(error) = precision_env_error(precision) {
            return Err(DiffusionError::workflow(error));
        }
        let patch_dim = 3 * DA3_PATCH * DA3_PATCH;
        let patch_w = tensor(
            weights,
            &format!("{PREFIX}backbone.pretrained.patch_embed.proj.weight"),
            DA3_HIDDEN * patch_dim,
        )?;
        let patch_b = tensor(
            weights,
            &format!("{PREFIX}backbone.pretrained.patch_embed.proj.bias"),
            DA3_HIDDEN,
        )?;
        let cls = tensor(
            weights,
            &format!("{PREFIX}backbone.pretrained.cls_token"),
            DA3_HIDDEN,
        )?;
        let base_pos = tensor(
            weights,
            &format!("{PREFIX}backbone.pretrained.pos_embed"),
            (1 + DA3_BASE_PATCH_SIDE * DA3_BASE_PATCH_SIDE) * DA3_HIDDEN,
        )?;

        let mut layers = Vec::with_capacity(DA3_DEPTH);
        for index in 0..DA3_DEPTH {
            emit_progress(
                &mut progress,
                &format!("load DA3 DINO block {}/{DA3_DEPTH}", index + 1),
                index as f64 / DA3_DEPTH as f64,
            )?;
            let prefix = format!("{PREFIX}backbone.pretrained.blocks.{index}");
            let scale1 = tensor(weights, &format!("{prefix}.ls1.gamma"), DA3_HIDDEN)?;
            let scale2 = tensor(weights, &format!("{prefix}.ls2.gamma"), DA3_HIDDEN)?;
            let mut proj_w = tensor(
                weights,
                &format!("{prefix}.attn.proj.weight"),
                DA3_HIDDEN * DA3_HIDDEN,
            )?;
            let mut proj_b = tensor(weights, &format!("{prefix}.attn.proj.bias"), DA3_HIDDEN)?;
            let mut fc2_w = tensor(
                weights,
                &format!("{prefix}.mlp.fc2.weight"),
                DA3_HIDDEN * DA3_MLP,
            )?;
            let mut fc2_b = tensor(weights, &format!("{prefix}.mlp.fc2.bias"), DA3_HIDDEN)?;
            for row in 0..DA3_HIDDEN {
                for value in &mut proj_w[row * DA3_HIDDEN..(row + 1) * DA3_HIDDEN] {
                    *value *= scale1[row];
                }
                proj_b[row] *= scale1[row];
                for value in &mut fc2_w[row * DA3_MLP..(row + 1) * DA3_MLP] {
                    *value *= scale2[row];
                }
                fc2_b[row] *= scale2[row];
            }
            let qkv_w = tensor(
                weights,
                &format!("{prefix}.attn.qkv.weight"),
                3 * DA3_HIDDEN * DA3_HIDDEN,
            )?;
            let qkv_b = tensor(weights, &format!("{prefix}.attn.qkv.bias"), 3 * DA3_HIDDEN)?;
            let fc1_w = tensor(
                weights,
                &format!("{prefix}.mlp.fc1.weight"),
                DA3_MLP * DA3_HIDDEN,
            )?;
            let fc1_b = tensor(weights, &format!("{prefix}.mlp.fc1.bias"), DA3_MLP)?;
            let linears = match precision {
                Da3Precision::StrictF32 => DinoLinears::F32 {
                    qkv_w: upload(&qkv_w, 3 * DA3_HIDDEN, DA3_HIDDEN)?,
                    qkv_b: upload(&qkv_b, 1, 3 * DA3_HIDDEN)?,
                    proj_w: upload(&proj_w, DA3_HIDDEN, DA3_HIDDEN)?,
                    proj_b: upload(&proj_b, 1, DA3_HIDDEN)?,
                    fc1_w: upload(&fc1_w, DA3_MLP, DA3_HIDDEN)?,
                    fc1_b: upload(&fc1_b, 1, DA3_MLP)?,
                    fc2_w: upload(&fc2_w, DA3_HIDDEN, DA3_MLP)?,
                    fc2_b: upload(&fc2_b, 1, DA3_HIDDEN)?,
                },
                Da3Precision::FullBf16 => DinoLinears::Bf16(DinoLayerBf16 {
                    qkv: f32_to_bf16_bytes(&qkv_w),
                    qkv_key: format!("{prefix}.attn.qkv:bf16"),
                    qkv_bias: qkv_b,
                    proj: f32_to_bf16_bytes(&proj_w),
                    proj_key: format!("{prefix}.attn.proj:bf16"),
                    proj_bias: proj_b,
                    fc1: f32_to_bf16_bytes(&fc1_w),
                    fc1_key: format!("{prefix}.mlp.fc1:bf16"),
                    fc1_bias: fc1_b,
                    fc2: f32_to_bf16_bytes(&fc2_w),
                    fc2_key: format!("{prefix}.mlp.fc2:bf16"),
                    fc2_bias: fc2_b,
                }),
            };
            layers.push(DinoLayer {
                norm1: norm_mods(weights, &format!("{prefix}.norm1"), DA3_HIDDEN)?,
                norm2: norm_mods(weights, &format!("{prefix}.norm2"), DA3_HIDDEN)?,
                linears,
            });
        }

        emit_progress(&mut progress, "load DA3 DPT head", 0.96)?;
        let project_channels = [256usize, 512, 1024, 1024];
        let mut projects = Vec::with_capacity(4);
        for (index, &channels) in project_channels.iter().enumerate() {
            projects.push(Conv2d::load(
                weights,
                &format!("{PREFIX}head.projects.{index}"),
                DA3_HIDDEN,
                channels,
                1,
                1,
                true,
            )?);
        }
        let scratch_input = [256usize, 512, 1024, 1024];
        let mut scratch_layers = Vec::with_capacity(4);
        for (index, &channels) in scratch_input.iter().enumerate() {
            scratch_layers.push(Conv2d::load(
                weights,
                &format!("{PREFIX}head.scratch.layer{}_rn", index + 1),
                channels,
                256,
                3,
                3,
                false,
            )?);
        }
        let mut fusion = Vec::with_capacity(4);
        for index in 1..=4 {
            fusion.push(FusionBlock::load(weights, index, index != 4)?);
        }
        let result = Self {
            patch_w: upload(&patch_w, DA3_HIDDEN, patch_dim)?,
            patch_b: upload(&patch_b, 1, DA3_HIDDEN)?,
            cls: upload(&cls, 1, DA3_HIDDEN)?,
            base_pos,
            layers,
            final_norm: norm_mods(
                weights,
                &format!("{PREFIX}backbone.pretrained.norm"),
                DA3_HIDDEN,
            )?,
            projects,
            resize0: ConvTransposeNoOverlap::load(
                weights,
                &format!("{PREFIX}head.resize_layers.0"),
                256,
                4,
            )?,
            resize1: ConvTransposeNoOverlap::load(
                weights,
                &format!("{PREFIX}head.resize_layers.1"),
                512,
                2,
            )?,
            resize3: Conv2d::load(
                weights,
                &format!("{PREFIX}head.resize_layers.3"),
                1024,
                1024,
                3,
                3,
                true,
            )?,
            scratch_layers,
            fusion,
            output_conv1: Conv2d::load(
                weights,
                &format!("{PREFIX}head.scratch.output_conv1"),
                256,
                128,
                3,
                3,
                true,
            )?,
            depth_conv: Conv2d::load(
                weights,
                &format!("{PREFIX}head.scratch.output_conv2.0"),
                128,
                32,
                3,
                3,
                true,
            )?,
            depth_out: Conv2d::load(
                weights,
                &format!("{PREFIX}head.scratch.output_conv2.2"),
                32,
                1,
                1,
                1,
                true,
            )?,
            sky_conv: Conv2d::load(
                weights,
                &format!("{PREFIX}head.scratch.sky_output_conv2.0"),
                128,
                32,
                3,
                3,
                true,
            )?,
            sky_out: Conv2d::load(
                weights,
                &format!("{PREFIX}head.scratch.sky_output_conv2.2"),
                32,
                1,
                1,
                1,
                true,
            )?,
            pos_cache: Mutex::new(None),
            stride_cache: Mutex::new(None),
            precision,
            generation: {
                static NEXT_GENERATION: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
        };
        emit_progress(&mut progress, "DA3 resident", 1.0)?;
        Ok(result)
    }

    pub fn execution_info(&self) -> Da3ExecutionInfo {
        self.precision.execution_info()
    }

    pub fn precision(&self) -> Da3Precision {
        self.precision
    }

    /// Run an already-normalized, patch-divisible planar RGB input.
    pub fn forward_normalized(
        &self,
        pixels: &[f32],
        width: usize,
        height: usize,
        progress: Option<ProgressHook>,
    ) -> Result<Da3Prediction> {
        self.forward_normalized_tapped(pixels, width, height, progress, None)
    }

    /// `forward_normalized` with an optional validation tap sink receiving
    /// activations at the exact oracle hook boundaries (block/feature taps as
    /// token rows, DPT taps as planar `[channel][y][x]` data).  Tap downloads
    /// serialize the GPU, so timed runs must pass `None`.
    pub fn forward_normalized_tapped(
        &self,
        pixels: &[f32],
        width: usize,
        height: usize,
        mut progress: Option<ProgressHook>,
        mut tap: Option<&mut dyn FnMut(&'static str, Vec<f32>)>,
    ) -> Result<Da3Prediction> {
        if width == 0
            || height == 0
            || width % DA3_PATCH != 0
            || height % DA3_PATCH != 0
            || pixels.len() != 3 * width * height
        {
            return Err(DiffusionError::workflow("DA3 normalized input shape mismatch"));
        }
        let stage_timing = std::env::var("DA3_STAGE_TIMING").as_deref() == Ok("1");
        // Warm production requests replay a captured CUDA graph; taps and
        // stage timing need the eager op-by-op path. Progress does NOT force
        // eager: the replay is a single ~16ms launch, so the service's
        // cancellation hook gets one check before and one done after —
        // gating on progress.is_none() kept the shipped service (which
        // always passes Some) permanently on the eager path.
        if tap.is_none() && !stage_timing {
            emit_progress(&mut progress, "DA3 graph", 0.0)?;
            if let Some(prediction) = self.forward_graph(pixels, width, height)? {
                emit_progress(&mut progress, "DA3 done", 1.0)?;
                return Ok(prediction);
            }
        }
        let mut timer = StageTimer {
            sync: stage_timing.then_some(&self.cls),
            last: std::time::Instant::now(),
            totals: Vec::new(),
        };
        let image = upload(pixels, 3, width * height)?;
        let (depth, sky) = self.forward_device(&image, width, height, &mut progress, &mut tap, &mut timer)?;
        let depth_logits = gpu_download(&depth.tensor).map_err(DiffusionError::model)?;
        let sky_logits = gpu_download(&sky.tensor).map_err(DiffusionError::model)?;
        timer.mark("download");
        if let Some(sink) = tap.as_mut() {
            sink("depth_logits", depth_logits.clone());
            sink("sky_logits", sky_logits.clone());
        }
        let prediction = Self::postprocess(depth_logits, sky_logits, width, height);
        timer.mark("host_postprocess");
        timer.report();
        emit_progress(&mut progress, "DA3 done", 1.0)?;
        Ok(prediction)
    }

    fn postprocess(
        depth_logits: Vec<f32>,
        sky_logits: Vec<f32>,
        width: usize,
        height: usize,
    ) -> Da3Prediction {
        let mut prediction = Da3Prediction {
            depth: depth_logits.into_iter().map(f32::exp).collect(),
            sky: sky_logits.into_iter().map(|value| value.max(0.0)).collect(),
            width,
            height,
        };
        prediction.apply_sky_depth_policy();
        prediction
    }

    /// Warm-path CUDA graph execution (flux pattern): persistent input tensor,
    /// two eager warm runs to settle weight caches and the activation pool,
    /// then a captured graph replayed for every following same-shape request.
    /// Returns Ok(None) when the eager path must run instead.
    fn forward_graph(
        &self,
        pixels: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Option<Da3Prediction>> {
        struct Da3GraphState {
            model: usize,
            shape: (usize, usize),
            image: GpuTensor,
            warm_runs: u32,
            capture_failed: bool,
            graph: Option<(GpuStepGraph, GpuTensor, GpuTensor)>,
        }
        thread_local! {
            static DA3_DEVICE_GRAPH: RefCell<Option<Da3GraphState>> = const { RefCell::new(None) };
        }
        DA3_DEVICE_GRAPH.with(|cell| {
            let mut slot = cell.borrow_mut();
            // Keyed by the process-unique instance generation, NOT the
            // address: a drop + reload at the same address must not replay
            // a graph whose nodes reference the dropped model's buffers.
            let model = self.generation as usize;
            let matches = slot
                .as_ref()
                .is_some_and(|state| state.model == model && state.shape == (width, height));
            if matches {
                let state = slot.as_ref().expect("matching DA3 graph state");
                gpu_upload_into(&state.image, pixels).map_err(DiffusionError::model)?;
            } else {
                // Drop the old graph first: its Drop unpins the pool buffers.
                *slot = None;
                *slot = Some(Da3GraphState {
                    model,
                    shape: (width, height),
                    image: upload(pixels, 3, width * height)?,
                    warm_runs: 0,
                    capture_failed: false,
                    graph: None,
                });
            }
            let state = slot.as_mut().expect("DA3 graph state");
            if let Some((graph, depth, sky)) = &state.graph {
                gpu_graph_launch(graph).map_err(DiffusionError::model)?;
                let depth_logits = gpu_download(depth).map_err(DiffusionError::model)?;
                let sky_logits = gpu_download(sky).map_err(DiffusionError::model)?;
                return Ok(Some(Self::postprocess(depth_logits, sky_logits, width, height)));
            }
            if state.warm_runs < 2 || state.capture_failed {
                state.warm_runs = state.warm_runs.saturating_add(1);
                let (depth, sky) = self.forward_device(
                    &state.image,
                    width,
                    height,
                    &mut None,
                    &mut None,
                    &mut StageTimer::disabled(),
                )?;
                let depth_logits = gpu_download(&depth.tensor).map_err(DiffusionError::model)?;
                let sky_logits = gpu_download(&sky.tensor).map_err(DiffusionError::model)?;
                return Ok(Some(Self::postprocess(depth_logits, sky_logits, width, height)));
            }
            let captured = gpu_graph_capture(|| {
                // Capture records without executing.
                self.forward_device(
                    &state.image,
                    width,
                    height,
                    &mut None,
                    &mut None,
                    &mut StageTimer::disabled(),
                )
                .map_err(|err| err.to_string())
            });
            match captured {
                Ok((graph, (depth, sky))) => {
                    gpu_graph_launch(&graph).map_err(DiffusionError::model)?;
                    let depth_logits =
                        gpu_download(&depth.tensor).map_err(DiffusionError::model)?;
                    let sky_logits = gpu_download(&sky.tensor).map_err(DiffusionError::model)?;
                    state.graph = Some((graph, depth.tensor, sky.tensor));
                    Ok(Some(Self::postprocess(depth_logits, sky_logits, width, height)))
                }
                Err(err) => {
                    eprintln!("DA3 device graph capture failed ({err}); running eager");
                    state.capture_failed = true;
                    Ok(None)
                }
            }
        })
    }

    /// Device-resident forward from normalized planar image tensor to
    /// depth/sky logit tensors.  With `tap: None` and a disabled timer this
    /// issues no host transfers, which CUDA graph capture requires.
    fn forward_device(
        &self,
        image: &GpuTensor,
        width: usize,
        height: usize,
        progress: &mut Option<ProgressHook>,
        tap: &mut Option<&mut dyn FnMut(&'static str, Vec<f32>)>,
        timer: &mut StageTimer,
    ) -> Result<(Planar, Planar)> {
        fn emit(
            sink: &mut Option<&mut dyn FnMut(&'static str, Vec<f32>)>,
            name: &'static str,
            tensor: &GpuTensor,
        ) -> Result<()> {
            if let Some(sink) = sink {
                sink(name, gpu_download(tensor).map_err(DiffusionError::model)?);
            }
            Ok(())
        }
        let patch_w = width / DA3_PATCH;
        let patch_h = height / DA3_PATCH;
        let patch_count = patch_w * patch_h;
        // Patch-row assembly runs on the GPU: tile-to-channel gather, then
        // reorder the three per-channel row blocks into `[patch, c*14*14]`
        // rows.  Element order matches the host loop exactly, so the patch
        // GEMM sees bit-identical input.
        let tiles = gpu_birefnet_image_to_patches(image, width, height, DA3_PATCH, DA3_PATCH)
            .map_err(DiffusionError::model)?;
        let red = gpu_slice_rows(&tiles, 0, patch_count).map_err(DiffusionError::model)?;
        let green =
            gpu_slice_rows(&tiles, patch_count, patch_count).map_err(DiffusionError::model)?;
        let blue =
            gpu_slice_rows(&tiles, 2 * patch_count, patch_count).map_err(DiffusionError::model)?;
        let patch_input =
            gpu_concat_cols(&[&red, &green, &blue]).map_err(DiffusionError::model)?;
        let patches = gpu_linear_f32_resident(&patch_input, &self.patch_w, Some(&self.patch_b))
            .map_err(DiffusionError::model)?;
        let hidden = gpu_concat_rows(&self.cls, &patches).map_err(DiffusionError::model)?;
        let mut pos_cache = self
            .pos_cache
            .lock()
            .map_err(|_| DiffusionError::workflow("DA3 position cache poisoned"))?;
        if pos_cache.as_ref().map(|(shape, _)| *shape) != Some((patch_w, patch_h)) {
            let pos = interpolate_position_embedding(&self.base_pos, patch_w, patch_h)?;
            *pos_cache = Some(((patch_w, patch_h), upload(&pos, 1 + patch_count, DA3_HIDDEN)?));
        }
        let pos = &pos_cache.as_ref().expect("DA3 position cache filled").1;
        let mut hidden = gpu_add(&hidden, pos).map_err(DiffusionError::model)?;
        drop(pos_cache);
        timer.mark("patch_embed_pos");

        let mut features = Vec::with_capacity(4);
        for (index, layer) in self.layers.iter().enumerate() {
            emit_progress(
                progress,
                &format!("DA3 DINO block {}/{DA3_DEPTH}", index + 1),
                index as f64 / (DA3_DEPTH + 6) as f64,
            )?;
            let normed = gpu_layer_norm_mod(&hidden, &layer.norm1, 0, DA3_HIDDEN, DA3_NORM_EPS)
                .map_err(DiffusionError::model)?;
            timer.mark("block_norm1");
            let qkv = layer.qkv(&normed)?;
            timer.mark("block_qkv");
            let q = gpu_slice_cols(&qkv, 0, DA3_HIDDEN).map_err(DiffusionError::model)?;
            let k = gpu_slice_cols(&qkv, DA3_HIDDEN, DA3_HIDDEN)
                .map_err(DiffusionError::model)?;
            let v = gpu_slice_cols(&qkv, 2 * DA3_HIDDEN, DA3_HIDDEN)
                .map_err(DiffusionError::model)?;
            timer.mark("block_qkv_slice");
            let scale = 1.0 / (DA3_HEAD_DIM as f32).sqrt();
            let attention = match self.precision {
                // Head-dim-64 flash kernel: bf16 operands, f32 accumulation.
                Da3Precision::FullBf16 => {
                    gpu_attention_packed_flash_bf16(&q, &k, &v, DA3_HEADS, scale)
                }
                // Composite path; the load-time guard pinned FLUX_ATTN_F16=0,
                // so operands stay f32.
                Da3Precision::StrictF32 => gpu_attention_packed(&q, &k, &v, DA3_HEADS, scale),
            }
            .map_err(DiffusionError::model)?;
            timer.mark("block_attention");
            let update = layer.proj(&attention)?;
            timer.mark("block_proj");
            hidden = gpu_add(&hidden, &update).map_err(DiffusionError::model)?;
            timer.mark("block_residual");

            let normed = gpu_layer_norm_mod(&hidden, &layer.norm2, 0, DA3_HIDDEN, DA3_NORM_EPS)
                .map_err(DiffusionError::model)?;
            timer.mark("block_norm2");
            let ff = layer.fc1(&normed)?;
            timer.mark("block_fc1");
            let ff = gpu_gelu_erf(&ff).map_err(DiffusionError::model)?;
            timer.mark("block_gelu");
            let ff = layer.fc2(&ff)?;
            timer.mark("block_fc2");
            hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;
            timer.mark("block_residual");

            if DA3_TAP_LAYERS.contains(&index) {
                let (block_tap, feature_tap) = match index {
                    4 => ("block_4", "feature_4"),
                    11 => ("block_11", "feature_11"),
                    17 => ("block_17", "feature_17"),
                    _ => ("block_23", "feature_23"),
                };
                emit(tap,block_tap, &hidden)?;
                let normalized =
                    gpu_layer_norm_mod(&hidden, &self.final_norm, 0, DA3_HIDDEN, DA3_NORM_EPS)
                        .map_err(DiffusionError::model)?;
                // Drop the class token; DPT receives patch rows only.
                let feature = gpu_slice_rows(&normalized, 1, patch_count)
                    .map_err(DiffusionError::model)?;
                emit(tap,feature_tap, &feature)?;
                features.push(feature);
                timer.mark("feature_norm_slice");
            }
        }
        if features.len() != 4 {
            return Err(DiffusionError::workflow("DA3 missing DINO feature taps"));
        }

        emit_progress(progress, "DA3 DPT projections", 0.82)?;
        let mut resized = Vec::with_capacity(4);
        for (index, feature) in features.into_iter().enumerate() {
            let planar = Planar {
                tensor: gpu_birefnet_tokens_to_planar(&feature)
                    .map_err(DiffusionError::model)?,
                width: patch_w,
                height: patch_h,
            };
            const PROJECT_TAPS: [&str; 4] = ["project_0", "project_1", "project_2", "project_3"];
            const RESIZE_TAPS: [&str; 4] = ["resize_0", "resize_1", "resize_2", "resize_3"];
            let projected = self.projects[index].forward(&planar)?;
            emit(tap,PROJECT_TAPS[index], &projected.tensor)?;
            timer.mark("dpt_project");
            let projected = match index {
                0 => self.resize0.forward(projected)?,
                1 => self.resize1.forward(projected)?,
                2 => projected,
                3 => self.stride_two(self.resize3.forward(&projected)?)?,
                _ => unreachable!(),
            };
            emit(tap,RESIZE_TAPS[index], &projected.tensor)?;
            timer.mark("dpt_resize");
            resized.push(projected);
        }

        emit_progress(progress, "DA3 DPT fusion", 0.88)?;
        let mut lateral = Vec::with_capacity(4);
        for (conv, input) in self.scratch_layers.iter().zip(&resized) {
            lateral.push(conv.forward(input)?);
        }
        timer.mark("dpt_scratch");
        let mut fused = self.fusion[3].forward(
            lateral.remove(3),
            None,
            (lateral[2].width, lateral[2].height),
        )?;
        emit(tap,"refinenet_4", &fused.tensor)?;
        timer.mark("dpt_refinenet4");
        fused = self.fusion[2].forward(
            fused,
            Some(&lateral[2]),
            (lateral[1].width, lateral[1].height),
        )?;
        emit(tap,"refinenet_3", &fused.tensor)?;
        timer.mark("dpt_refinenet3");
        fused = self.fusion[1].forward(
            fused,
            Some(&lateral[1]),
            (lateral[0].width, lateral[0].height),
        )?;
        emit(tap,"refinenet_2", &fused.tensor)?;
        timer.mark("dpt_refinenet2");
        fused = self.fusion[0].forward(
            fused,
            Some(&lateral[0]),
            (lateral[0].width * 2, lateral[0].height * 2),
        )?;
        emit(tap,"refinenet_1", &fused.tensor)?;
        timer.mark("dpt_refinenet1");

        emit_progress(progress, "DA3 depth and sky heads", 0.94)?;
        let neck = self.output_conv1.forward(&fused)?;
        emit(tap,"output_conv1_pre_resize", &neck.tensor)?;
        timer.mark("head_output_conv1");
        let neck = resize(&neck, width, height, true)?;
        timer.mark("head_resize");
        let depth = self.depth_conv.forward(&neck)?;
        let depth = relu(&depth)?;
        let depth = self.depth_out.forward(&depth)?;
        timer.mark("head_depth");
        let sky = self.sky_conv.forward(&neck)?;
        let sky = relu(&sky)?;
        let sky = self.sky_out.forward(&sky)?;
        timer.mark("head_sky");
        Ok((depth, sky))
    }
}

/// DA3_STAGE_TIMING=1: per-op-class GPU time totals.  Each mark synchronizes
/// the stream by downloading the tiny resident cls row, so totals are honest
/// at ~15us sync cost per boundary.
struct StageTimer<'a> {
    sync: Option<&'a GpuTensor>,
    last: std::time::Instant,
    totals: Vec<(&'static str, f64)>,
}

impl StageTimer<'_> {
    fn disabled() -> Self {
        StageTimer {
            sync: None,
            last: std::time::Instant::now(),
            totals: Vec::new(),
        }
    }

    fn mark(&mut self, name: &'static str) {
        let Some(sync) = self.sync else { return };
        let _ = gpu_download(sync);
        let now = std::time::Instant::now();
        let seconds = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        match self.totals.iter_mut().find(|(slot, _)| *slot == name) {
            Some((_, total)) => *total += seconds,
            None => self.totals.push((name, seconds)),
        }
    }

    fn report(&self) {
        if self.sync.is_none() {
            return;
        }
        let total: f64 = self.totals.iter().map(|(_, seconds)| seconds).sum();
        let mut totals = self.totals.clone();
        totals.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (name, seconds) in &totals {
            println!(
                "STAGE {name} ms={:.3} share={:.1}%",
                seconds * 1000.0,
                100.0 * seconds / total
            );
        }
        println!("STAGE_TOTAL ms={:.3}", total * 1000.0);
    }
}

fn relu(input: &Planar) -> Result<Planar> {
    Ok(Planar {
        tensor: gpu_birefnet_relu(&input.tensor).map_err(DiffusionError::model)?,
        width: input.width,
        height: input.height,
    })
}

fn add_planar(left: &Planar, right: &Planar) -> Result<Planar> {
    if left.width != right.width || left.height != right.height {
        return Err(DiffusionError::workflow("DA3 planar add shape mismatch"));
    }
    Ok(Planar {
        tensor: gpu_add(&left.tensor, &right.tensor).map_err(DiffusionError::model)?,
        width: left.width,
        height: left.height,
    })
}

fn resize(input: &Planar, width: usize, height: usize, align_corners: bool) -> Result<Planar> {
    Ok(Planar {
        tensor: gpu_birefnet_resize_bilinear(
            &input.tensor,
            input.width,
            input.height,
            width,
            height,
            align_corners,
        )
        .map_err(DiffusionError::model)?,
        width,
        height,
    })
}

impl Da3MetricLarge {
    fn stride_two(&self, input: Planar) -> Result<Planar> {
        let width = input.width.div_ceil(2);
        let height = input.height.div_ceil(2);
        let mut cache = self
            .stride_cache
            .lock()
            .map_err(|_| DiffusionError::workflow("DA3 stride cache poisoned"))?;
        if cache.as_ref().map(|(shape, _)| *shape) != Some((input.width, input.height)) {
            let in_count = input.width * input.height;
            let out_count = width * height;
            let mut select = vec![0.0f32; out_count * in_count];
            for y in 0..height {
                for x in 0..width {
                    let source = (y * 2) * input.width + x * 2;
                    select[(y * width + x) * in_count + source] = 1.0;
                }
            }
            *cache = Some((
                (input.width, input.height),
                upload(&select, out_count, in_count)?,
            ));
        }
        let select = &cache.as_ref().expect("DA3 stride cache filled").1;
        Ok(Planar {
            tensor: gpu_linear_f32_resident(&input.tensor, select, None)
                .map_err(DiffusionError::model)?,
            width,
            height,
        })
    }
}

/// PyTorch interpolate(..., mode="bicubic", align_corners=false) coefficient.
fn cubic_weight(x: f64) -> f64 {
    let x = x.abs();
    let a = -0.75;
    if x <= 1.0 {
        (a + 2.0) * x * x * x - (a + 3.0) * x * x + 1.0
    } else if x < 2.0 {
        a * x * x * x - 5.0 * a * x * x + 8.0 * a * x - 4.0 * a
    } else {
        0.0
    }
}

fn interpolate_position_embedding(base: &[f32], width: usize, height: usize) -> Result<Vec<f32>> {
    let expected = (1 + DA3_BASE_PATCH_SIDE * DA3_BASE_PATCH_SIDE) * DA3_HIDDEN;
    if base.len() != expected || width == 0 || height == 0 {
        return Err(DiffusionError::workflow("DA3 position embedding shape mismatch"));
    }
    // DINOv2 bypasses interpolation for its square 518px training grid.
    if width == DA3_BASE_PATCH_SIDE && height == DA3_BASE_PATCH_SIDE {
        return Ok(base.to_vec());
    }
    let mut output = vec![0.0f32; (1 + width * height) * DA3_HIDDEN];
    output[..DA3_HIDDEN].copy_from_slice(&base[..DA3_HIDDEN]);
    let source = &base[DA3_HIDDEN..];
    // Preserve DINOv2's historical 0.1 interpolation offset. PyTorch receives
    // this as an explicit scale_factor with recompute_scale_factor=false, so
    // the sampling scale is (target + 0.1) / 37 rather than target / 37.
    let scale_x = (width as f64 + 0.1) / DA3_BASE_PATCH_SIDE as f64;
    let scale_y = (height as f64 + 0.1) / DA3_BASE_PATCH_SIDE as f64;
    for oy in 0..height {
        let fy = (oy as f64 + 0.5) / scale_y - 0.5;
        let iy = fy.floor() as isize;
        for ox in 0..width {
            let fx = (ox as f64 + 0.5) / scale_x - 0.5;
            let ix = fx.floor() as isize;
            let dst = (1 + oy * width + ox) * DA3_HIDDEN;
            for channel in 0..DA3_HIDDEN {
                let mut value = 0.0f64;
                for ky in -1..=2 {
                    let sy = (iy + ky).clamp(0, DA3_BASE_PATCH_SIDE as isize - 1) as usize;
                    let wy = cubic_weight(fy - (iy + ky) as f64);
                    for kx in -1..=2 {
                        let sx = (ix + kx).clamp(0, DA3_BASE_PATCH_SIDE as isize - 1) as usize;
                        let wx = cubic_weight(fx - (ix + kx) as f64);
                        value += source[(sy * DA3_BASE_PATCH_SIDE + sx) * DA3_HIDDEN + channel]
                            as f64
                            * wy
                            * wx;
                    }
                }
                output[dst + channel] = value as f32;
            }
        }
    }
    Ok(output)
}

fn nearest_multiple(value: usize, multiple: usize) -> usize {
    let down = value / multiple * multiple;
    let up = down + multiple;
    if up - value <= value - down { up } else { down }.max(multiple)
}

pub fn processed_dimensions(width: usize, height: usize) -> Result<(usize, usize)> {
    if width == 0 || height == 0 {
        return Err(DiffusionError::workflow("DA3 empty image"));
    }
    let scale = DA3_PROCESS_RESOLUTION as f64 / width.max(height) as f64;
    let first_width = ((width as f64 * scale).round_ties_even() as usize).max(1);
    let first_height = ((height as f64 * scale).round_ties_even() as usize).max(1);
    Ok((
        nearest_multiple(first_width, DA3_PATCH),
        nearest_multiple(first_height, DA3_PATCH),
    ))
}

/// DA3's default `upper_bound_resize` preprocessing.  The reference resizes
/// with `cv2.resize` on u8 (INTER_AREA when no axis grows, INTER_CUBIC
/// otherwise); the replicas below are bit-exact against OpenCV's scalar
/// reference arithmetic, and the image stays RGB u8 between the two resize
/// passes before ImageNet normalization, exactly like the reference.
pub fn preprocess_rgb8(rgb: &[u8], width: usize, height: usize) -> Result<Da3Preprocessed> {
    if rgb.len() != width.saturating_mul(height).saturating_mul(3) {
        return Err(DiffusionError::workflow("DA3 RGB input shape mismatch"));
    }
    let scale = DA3_PROCESS_RESOLUTION as f64 / width.max(height) as f64;
    let first_width = ((width as f64 * scale).round_ties_even() as usize).max(1);
    let first_height = ((height as f64 * scale).round_ties_even() as usize).max(1);
    let first = resize_rgb8(rgb, width, height, first_width, first_height)?;
    let out_width = nearest_multiple(first_width, DA3_PATCH);
    let out_height = nearest_multiple(first_height, DA3_PATCH);
    let resized = if out_width == first_width && out_height == first_height {
        first
    } else {
        resize_rgb8(&first, first_width, first_height, out_width, out_height)?
    };
    let plane = out_width * out_height;
    let mut pixels = vec![0.0f32; 3 * plane];
    for y in 0..out_height {
        for x in 0..out_width {
            let source = (y * out_width + x) * 3;
            let pixel = y * out_width + x;
            for channel in 0..3 {
                let value = resized[source + channel] as f32 / 255.0;
                pixels[channel * plane + pixel] =
                    (value - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
            }
        }
    }
    Ok(Da3Preprocessed {
        pixels,
        width: out_width,
        height: out_height,
        scale_x: out_width as f32 / width as f32,
        scale_y: out_height as f32 / height as f32,
    })
}

fn resize_rgb8(
    input: &[u8],
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
) -> Result<Vec<u8>> {
    if input.len() != in_width * in_height * 3 || out_width == 0 || out_height == 0 {
        return Err(DiffusionError::workflow("DA3 resize shape mismatch"));
    }
    if in_width == out_width && in_height == out_height {
        return Ok(input.to_vec());
    }
    if out_width <= in_width && out_height <= in_height {
        resize_area_rgb8(input, in_width, in_height, out_width, out_height)
    } else {
        resize_cubic_rgb8(input, in_width, in_height, out_width, out_height)
    }
}

/// One tap of OpenCV's fractional `INTER_AREA` weight table
/// (`computeResizeAreaTab` in imgproc's resize.cpp).
struct AreaTap {
    dst: usize,
    src: usize,
    alpha: f32,
}

fn area_tab(ssize: usize, dsize: usize, scale: f64) -> Vec<AreaTap> {
    let mut tab = Vec::with_capacity(dsize * 3);
    for dx in 0..dsize {
        let fsx1 = dx as f64 * scale;
        let fsx2 = fsx1 + scale;
        let cell = scale.min(ssize as f64 - fsx1);
        let sx2 = (fsx2.floor() as isize).min(ssize as isize - 1);
        let sx1 = (fsx1.ceil() as isize).min(sx2);
        if sx1 as f64 - fsx1 > 1e-3 {
            tab.push(AreaTap {
                dst: dx,
                src: (sx1 - 1) as usize,
                alpha: ((sx1 as f64 - fsx1) / cell) as f32,
            });
        }
        for sx in sx1..sx2 {
            tab.push(AreaTap {
                dst: dx,
                src: sx as usize,
                alpha: (1.0 / cell) as f32,
            });
        }
        if fsx2 - sx2 as f64 > 1e-3 {
            tab.push(AreaTap {
                dst: dx,
                src: sx2 as usize,
                alpha: ((fsx2 - sx2 as f64).min(1.0).min(cell) / cell) as f32,
            });
        }
    }
    tab
}

/// OpenCV's `resizeAreaFast_` for exact integer scale factors, including the
/// `(sum + 2) >> 2` special case for 2x2 and count-normalized partial border
/// windows.
fn resize_area_fast_rgb8(
    input: &[u8],
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
    scale_x: usize,
    scale_y: usize,
) -> Vec<u8> {
    let scale = 1.0f32 / (scale_x * scale_y) as f32;
    let full_width = in_width / scale_x;
    let mut output = vec![0u8; out_width * out_height * 3];
    for dy in 0..out_height {
        let out_row = &mut output[dy * out_width * 3..][..out_width * 3];
        let sy0 = dy * scale_y;
        if sy0 >= in_height {
            continue; // saturates to zero like the reference
        }
        let full_rows = sy0 + scale_y <= in_height;
        let full = if full_rows { full_width.min(out_width) } else { 0 };
        if scale_x == 2 && scale_y == 2 && full > 0 {
            let row0 = &input[sy0 * in_width * 3..][..in_width * 3];
            let row1 = &input[(sy0 + 1) * in_width * 3..][..in_width * 3];
            for dx in 0..full {
                for channel in 0..3 {
                    let index = dx * 6 + channel;
                    let sum = row0[index] as u32
                        + row0[index + 3] as u32
                        + row1[index] as u32
                        + row1[index + 3] as u32;
                    out_row[dx * 3 + channel] = ((sum + 2) >> 2) as u8;
                }
            }
        } else {
            for dx in 0..full {
                for channel in 0..3 {
                    let mut sum = 0u32;
                    for sy in 0..scale_y {
                        let row = &input[(sy0 + sy) * in_width * 3..][..in_width * 3];
                        for sx in 0..scale_x {
                            sum += row[(dx * scale_x + sx) * 3 + channel] as u32;
                        }
                    }
                    out_row[dx * 3 + channel] = (sum as f32 * scale)
                        .round_ties_even()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        }
        for dx in full..out_width {
            let sx0 = dx * scale_x;
            if sx0 >= in_width {
                continue; // stays zero
            }
            let x1 = (sx0 + scale_x).min(in_width);
            let y1 = (sy0 + scale_y).min(in_height);
            let count = ((y1 - sy0) * (x1 - sx0)) as f32;
            for channel in 0..3 {
                let mut sum = 0u32;
                for sy in sy0..y1 {
                    for sx in sx0..x1 {
                        sum += input[(sy * in_width + sx) * 3 + channel] as u32;
                    }
                }
                out_row[dx * 3 + channel] = (sum as f32 / count)
                    .round_ties_even()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    output
}

fn resize_area_rgb8(
    input: &[u8],
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
) -> Result<Vec<u8>> {
    // OpenCV derives the forward scale from the inverse scale, so replicate
    // that exact double-rounding chain before anything else.
    let scale_x = 1.0 / (out_width as f64 / in_width as f64);
    let scale_y = 1.0 / (out_height as f64 / in_height as f64);
    let int_x = scale_x.round_ties_even();
    let int_y = scale_y.round_ties_even();
    if (scale_x - int_x).abs() < f64::EPSILON && (scale_y - int_y).abs() < f64::EPSILON {
        return Ok(resize_area_fast_rgb8(
            input, in_width, in_height, out_width, out_height,
            int_x as usize, int_y as usize,
        ));
    }
    // Fractional path: f32 weights and f32 mul-then-add accumulation exactly
    // as ResizeArea_Invoker<uchar, float> computes them (no FMA contraction),
    // finished with cvRound's round-half-to-even.
    let xtab = area_tab(in_width, out_width, scale_x);
    let ytab = area_tab(in_height, out_height, scale_y);
    let row_len = out_width * 3;
    let mut output = vec![0u8; row_len * out_height];
    let mut rows: Vec<(usize, Vec<f32>)> = Vec::new();
    let mut acc = vec![0.0f32; row_len];
    let mut ytap = 0;
    for dy in 0..out_height {
        acc.fill(0.0);
        while ytap < ytab.len() && ytab[ytap].dst == dy {
            let sy = ytab[ytap].src;
            let beta = ytab[ytap].alpha;
            ytap += 1;
            if !rows.iter().any(|(index, _)| *index == sy) {
                if rows.len() >= 4 {
                    rows.remove(0);
                }
                let src_row = &input[sy * in_width * 3..][..in_width * 3];
                let mut buf = vec![0.0f32; row_len];
                for tap in &xtab {
                    let src = tap.src * 3;
                    let dst = tap.dst * 3;
                    for channel in 0..3 {
                        buf[dst + channel] += src_row[src + channel] as f32 * tap.alpha;
                    }
                }
                rows.push((sy, buf));
            }
            let row = &rows.iter().find(|(index, _)| *index == sy).unwrap().1;
            for (value, part) in acc.iter_mut().zip(row.iter()) {
                *value += beta * part;
            }
        }
        for (out, value) in output[dy * row_len..][..row_len].iter_mut().zip(&acc) {
            *out = value.round_ties_even().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(output)
}

/// OpenCV `interpolateCubic` (A = -0.75) in f32, preserving operation order.
fn cubic_coeffs_f32(x: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let c0 = ((A * (x + 1.0) - 5.0 * A) * (x + 1.0) + 8.0 * A) * (x + 1.0) - 4.0 * A;
    let c1 = ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let inv = 1.0 - x;
    let c2 = ((A + 2.0) * inv - (A + 3.0)) * inv * inv + 1.0;
    [c0, c1, c2, 1.0 - c0 - c1 - c2]
}

/// Fixed-point cubic coefficients at OpenCV's 2^11 scale
/// (`saturate_cast<short>(coeff * INTER_RESIZE_COEF_SCALE)`).
fn cubic_fixed_coeffs(x: f32) -> [i32; 4] {
    let coeffs = cubic_coeffs_f32(x);
    let mut fixed = [0i32; 4];
    for (out, coeff) in fixed.iter_mut().zip(coeffs) {
        *out = (coeff * 2048.0).round_ties_even().clamp(-32768.0, 32767.0) as i32;
    }
    fixed
}

/// OpenCV `INTER_CUBIC` for u8, scalar fixed-point reference path
/// (HResizeCubic<uchar, int, short> + VResizeCubic with
/// FixedPtCast<int, uchar, 22>).  OpenCV's SIMD bulk lanes evaluate the
/// vertical pass in f32 instead, which can differ by one u8 level at rare
/// positions; this uniform scalar path is the deterministic contract here.
fn resize_cubic_rgb8(
    input: &[u8],
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
) -> Result<Vec<u8>> {
    let scale_x = 1.0 / (out_width as f64 / in_width as f64);
    let scale_y = 1.0 / (out_height as f64 / in_height as f64);
    let mut x_src = Vec::with_capacity(out_width);
    let mut x_coeffs = Vec::with_capacity(out_width);
    for dx in 0..out_width {
        let position = ((dx as f64 + 0.5) * scale_x - 0.5) as f32;
        let base = position.floor();
        x_src.push(base as isize);
        x_coeffs.push(cubic_fixed_coeffs(position - base));
    }
    let mut y_src = Vec::with_capacity(out_height);
    let mut y_coeffs = Vec::with_capacity(out_height);
    for dy in 0..out_height {
        let position = ((dy as f64 + 0.5) * scale_y - 0.5) as f32;
        let base = position.floor();
        y_src.push(base as isize);
        y_coeffs.push(cubic_fixed_coeffs(position - base));
    }
    let row_len = out_width * 3;
    let hrow = |sy: usize| -> Vec<i32> {
        let src_row = &input[sy * in_width * 3..][..in_width * 3];
        let mut buf = vec![0i32; row_len];
        for dx in 0..out_width {
            let base = x_src[dx];
            for (tap, coeff) in x_coeffs[dx].iter().enumerate() {
                let sx = (base - 1 + tap as isize).clamp(0, in_width as isize - 1) as usize;
                for channel in 0..3 {
                    buf[dx * 3 + channel] += src_row[sx * 3 + channel] as i32 * coeff;
                }
            }
        }
        buf
    };
    let mut rows: Vec<(usize, Vec<i32>)> = Vec::new();
    let mut output = vec![0u8; row_len * out_height];
    for dy in 0..out_height {
        let base = y_src[dy];
        let betas = &y_coeffs[dy];
        let mut sources = [0usize; 4];
        for (slot, source) in sources.iter_mut().enumerate() {
            *source = (base - 1 + slot as isize).clamp(0, in_height as isize - 1) as usize;
        }
        for &sy in &sources {
            if !rows.iter().any(|(index, _)| *index == sy) {
                // Cache depth 8 >= 2 * 4 taps, so the current row's sources
                // are never evicted mid-row.
                if rows.len() >= 8 {
                    rows.remove(0);
                }
                rows.push((sy, hrow(sy)));
            }
        }
        let fetch = |sy: usize| &rows.iter().find(|(index, _)| *index == sy).unwrap().1;
        let (r0, r1, r2, r3) = (
            fetch(sources[0]),
            fetch(sources[1]),
            fetch(sources[2]),
            fetch(sources[3]),
        );
        let out_row = &mut output[dy * row_len..][..row_len];
        for x in 0..row_len {
            let value = r0[x] as i64 * betas[0] as i64
                + r1[x] as i64 * betas[1] as i64
                + r2[x] as i64 * betas[2] as i64
                + r3[x] as i64 * betas[3] as i64;
            out_row[x] = ((value + (1 << 21)) >> 22).clamp(0, 255) as u8;
        }
    }
    Ok(output)
}

/// Processed-space focal length for a horizontal field of view.  DA3's
/// metric scaling must use the image coordinates seen by the network, not
/// the original pre-resize width (the old Python service got this wrong).
pub fn focal_px_for_processed_width(width: usize, horizontal_fov_degrees: f64) -> Result<f64> {
    if width == 0 || !(0.0..180.0).contains(&horizontal_fov_degrees) {
        return Err(DiffusionError::workflow("invalid DA3 focal/FOV"));
    }
    let radians = horizontal_fov_degrees.to_radians();
    Ok(0.5 * width as f64 / (0.5 * radians).tan())
}

/// Default 60° horizontal FOV used when EXIF / DEPTH_FOCAL_PX is absent.
pub const DA3_DEFAULT_HFOV_DEG: f64 = 60.0;
/// FAQ metric recipe: `depth_m = focal_px * net / 300`.
pub const DA3_METRIC_FOCAL_SCALE: f64 = 300.0;
const DA3_FAR_MM: f64 = 65_535.0;

/// Product-contract millimeter map plus sidecar facts.
#[derive(Clone, Debug)]
pub struct Da3MetricMap {
    pub mm: Vec<u16>,
    pub width: usize,
    pub height: usize,
    /// Pre-clip non-sky range in millimeters.
    pub min_mm: f64,
    pub max_mm: f64,
    pub focal_px: f64,
}

/// Scale the network's unscaled exponential depth to millimeters at
/// `out_width x out_height` (usually the original image size).
///
/// Focal is computed from the **processed** width the network saw.  Sky
/// pixels (ReLU score >= 0.3) encode as the 65.535 m far clip and are
/// excluded from min/max, matching the box product script's numeric
/// threshold (not the exported boolean's 0.5 cut).
pub fn encode_metric_mm(
    prediction: &Da3Prediction,
    out_width: usize,
    out_height: usize,
    horizontal_fov_degrees: f64,
) -> Result<Da3MetricMap> {
    if prediction.width == 0 || prediction.height == 0 || out_width == 0 || out_height == 0 {
        return Err(DiffusionError::workflow("DA3 metric encode empty shape"));
    }
    if prediction.depth.len() != prediction.width * prediction.height
        || prediction.sky.len() != prediction.depth.len()
    {
        return Err(DiffusionError::workflow("DA3 metric encode map length mismatch"));
    }
    let focal_px = focal_px_for_processed_width(prediction.width, horizontal_fov_degrees)?;
    let scale = focal_px / DA3_METRIC_FOCAL_SCALE;
    let mut depth_m: Vec<f64> = prediction
        .depth
        .iter()
        .map(|&value| f64::from(value) * scale)
        .collect();
    let mut sky = prediction.sky.clone();
    if out_width != prediction.width || out_height != prediction.height {
        depth_m = resize_bilinear_f64(&depth_m, prediction.width, prediction.height, out_width, out_height);
        sky = resize_bilinear_f32(&sky, prediction.width, prediction.height, out_width, out_height);
    }
    let mut min_mm = f64::INFINITY;
    let mut max_mm = f64::NEG_INFINITY;
    let mut mm = vec![0u16; out_width * out_height];
    for index in 0..mm.len() {
        let sky_hit = sky[index] >= 0.3;
        let metres = depth_m[index];
        if sky_hit {
            mm[index] = 65_535;
            continue;
        }
        if !metres.is_finite() || metres <= 0.0 {
            continue;
        }
        let millimetres = metres * 1000.0;
        if millimetres < min_mm {
            min_mm = millimetres;
        }
        if millimetres > max_mm {
            max_mm = millimetres;
        }
        mm[index] = millimetres.clamp(1.0, DA3_FAR_MM) as u16;
    }
    if !min_mm.is_finite() {
        min_mm = 0.0;
        max_mm = 0.0;
    }
    Ok(Da3MetricMap {
        mm,
        width: out_width,
        height: out_height,
        min_mm,
        max_mm,
        focal_px,
    })
}

fn resize_bilinear_f32(
    input: &[f32],
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
) -> Vec<f32> {
    let mut output = vec![0.0f32; out_width * out_height];
    sample_bilinear(in_width, in_height, out_width, out_height, |dst, i00, i10, i01, i11, tx, ty| {
        let top = input[i00] as f64 * (1.0 - tx) + input[i10] as f64 * tx;
        let bottom = input[i01] as f64 * (1.0 - tx) + input[i11] as f64 * tx;
        output[dst] = (top * (1.0 - ty) + bottom * ty) as f32;
    });
    output
}

fn resize_bilinear_f64(
    input: &[f64],
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
) -> Vec<f64> {
    let mut output = vec![0.0f64; out_width * out_height];
    sample_bilinear(in_width, in_height, out_width, out_height, |dst, i00, i10, i01, i11, tx, ty| {
        let top = input[i00] * (1.0 - tx) + input[i10] * tx;
        let bottom = input[i01] * (1.0 - tx) + input[i11] * tx;
        output[dst] = top * (1.0 - ty) + bottom * ty;
    });
    output
}

fn sample_bilinear(
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
    mut write: impl FnMut(usize, usize, usize, usize, usize, f64, f64),
) {
    if in_width == 0 || in_height == 0 {
        return;
    }
    let scale_x = in_width as f64 / out_width as f64;
    let scale_y = in_height as f64 / out_height as f64;
    for oy in 0..out_height {
        // Clamp the SOURCE coordinate (not just the index) to zero, like
        // torch area_pixel_compute_source_index / cv2 border replication.
        // A negative half-pixel coord with only the index clamped left the
        // fraction negative and EXTRAPOLATED (1.5*row0 - 0.5*row1) along
        // the top/left borders of every upscale.
        let fy = ((oy as f64 + 0.5) * scale_y - 0.5).max(0.0);
        let y0 = fy.floor().clamp(0.0, (in_height - 1) as f64);
        let y1 = (y0 + 1.0).min((in_height - 1) as f64);
        let ty = fy - y0;
        for ox in 0..out_width {
            let fx = ((ox as f64 + 0.5) * scale_x - 0.5).max(0.0);
            let x0 = fx.floor().clamp(0.0, (in_width - 1) as f64);
            let x1 = (x0 + 1.0).min((in_width - 1) as f64);
            let tx = fx - x0;
            write(
                oy * out_width + ox,
                y0 as usize * in_width + x0 as usize,
                y0 as usize * in_width + x1 as usize,
                y1 as usize * in_width + x0 as usize,
                y1 as usize * in_width + x1 as usize,
                tx,
                ty,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upscale_borders_replicate_edges_instead_of_extrapolating() {
        // 2x2 -> 4x4 upscale. With only the index clamped, the corner source
        // coord -0.25 kept a negative fraction and the top-left output went
        // NEGATIVE (-0.5) on this non-negative input. torch/cv2 replicate
        // the edge: corners equal input corners, everything stays in range.
        let input = vec![0.0f64, 1.0, 2.0, 3.0];
        let out = resize_bilinear_f64(&input, 2, 2, 4, 4);
        assert_eq!(out.len(), 16);
        assert!((out[0] - 0.0).abs() < 1e-12, "top-left corner: {}", out[0]);
        assert!((out[3] - 1.0).abs() < 1e-12, "top-right corner: {}", out[3]);
        assert!((out[12] - 2.0).abs() < 1e-12, "bottom-left corner: {}", out[12]);
        assert!((out[15] - 3.0).abs() < 1e-12, "bottom-right corner: {}", out[15]);
        for (i, v) in out.iter().enumerate() {
            assert!(
                (0.0..=3.0).contains(v),
                "bilinear result out of input range at {i}: {v}"
            );
        }
    }

    #[test]
    fn processed_shape_matches_da3_default() {
        assert_eq!(processed_dimensions(512, 512).unwrap(), (504, 504));
        assert_eq!(processed_dimensions(1920, 1080).unwrap(), (504, 280));
        assert_eq!(processed_dimensions(1080, 1920).unwrap(), (280, 504));
        assert_eq!(processed_dimensions(1000, 333).unwrap(), (504, 168));
    }

    #[test]
    fn processed_focal_is_resolution_invariant() {
        let f512 = focal_px_for_processed_width(504, 60.0).unwrap();
        let f1024 = focal_px_for_processed_width(504, 60.0).unwrap();
        assert_eq!(f512, f1024);
        assert!((f512 - 436.476_803_5).abs() < 1e-6);
    }

    /// Deterministic RGB pattern shared with the Python OpenCV replicas in
    /// local/agent_state/da3-native-files/ that generated the FNV vectors.
    fn resize_test_pattern(width: usize, height: usize) -> Vec<u8> {
        let mut image = vec![0u8; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                for c in 0..3 {
                    let value = (x as u32)
                        .wrapping_mul(31)
                        .wrapping_add((y as u32).wrapping_mul(17))
                        .wrapping_add((c as u32).wrapping_mul(97))
                        ^ ((x as u32).wrapping_mul(y as u32).wrapping_add(13) >> 3);
                    image[(y * width + x) * 3 + c] = (value & 0xff) as u8;
                }
            }
        }
        image
    }

    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for &byte in bytes {
            hash = (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3);
        }
        hash
    }

    /// Vectors generated by cv_area_replica.py / cv_cubic_replica.py, which
    /// are validated against cv2 5.0 (INTER_AREA bit-exact including the
    /// integer-scale fast path; INTER_CUBIC = scalar fixed-point path).
    #[test]
    fn resize_matches_opencv_reference_vectors() {
        let cases: &[(&str, usize, usize, usize, usize, bool, u64)] = &[
            ("area_frac_97x61_to_63x41", 97, 61, 63, 41, false, 0x5877c077a98324f2),
            ("area_frac_160x120_to_126x95", 160, 120, 126, 95, false, 0x8723ef54d936a638),
            ("area_ident_x_126x95_to_126x47", 126, 95, 126, 47, false, 0xd6bce8872f6f792c),
            ("area_fast2_96x60_to_48x30", 96, 60, 48, 30, false, 0x9b44ded2fad2f729),
            ("area_fast3_96x60_to_32x20", 96, 60, 32, 20, false, 0x54cf2080e8aa02b1),
            ("cubic_61x47_to_126x95", 61, 47, 126, 95, true, 0x55d133d0c51eb9c1),
            ("cubic_border_5x4_to_2x11", 5, 4, 2, 11, true, 0x6a0c2fbef49d9bbc),
        ];
        for &(name, sw, sh, dw, dh, cubic, expected) in cases {
            let src = resize_test_pattern(sw, sh);
            let out = if cubic {
                resize_cubic_rgb8(&src, sw, sh, dw, dh).unwrap()
            } else {
                resize_area_rgb8(&src, sw, sh, dw, dh).unwrap()
            };
            assert_eq!(fnv1a64(&out), expected, "resize vector {name}");
        }
    }

    /// Full preprocessing chain (cubic upscale, divisibility resize, ImageNet
    /// normalization) against the same Python reference, f32 bit-exact.
    #[test]
    fn preprocess_matches_reference_vector() {
        let src = resize_test_pattern(130, 97);
        let image = preprocess_rgb8(&src, 130, 97).unwrap();
        assert_eq!((image.width, image.height), (504, 378));
        let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
        for value in &image.pixels {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(fnv1a64(&bytes), 0xf58f2fc240184d88);
    }

    #[test]
    fn preprocessing_is_planar_normalized_and_divisible() {
        let mut rgb = vec![0u8; 32 * 16 * 3];
        for (index, value) in rgb.iter_mut().enumerate() {
            *value = (index % 251) as u8;
        }
        let image = preprocess_rgb8(&rgb, 32, 16).unwrap();
        assert_eq!((image.width, image.height), (504, 252));
        assert_eq!(image.pixels.len(), 3 * 504 * 252);
        assert!(image.pixels.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn bf16_conversion_rounds_to_nearest_even() {
        let bytes = f32_to_bf16_bytes(&[
            1.0,
            f32::from_bits(0x3f80_8000), // exact tie above 1.0 -> stays even 0x3f80
            f32::from_bits(0x3f81_8000), // exact tie above odd -> rounds up 0x3f82
            -2.5,
        ]);
        let half = |index: usize| u16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]]);
        assert_eq!(half(0), 0x3f80);
        assert_eq!(half(1), 0x3f80);
        assert_eq!(half(2), 0x3f82);
        assert_eq!(half(3), 0xc020);
    }

    #[test]
    fn precision_modes_report_identity_and_fail_closed() {
        let bf16 = Da3Precision::FullBf16.execution_info();
        assert!(bf16.weight_precision.contains("bf16"));
        assert_eq!(bf16.accumulation_precision, "f32");
        let f32_info = Da3Precision::StrictF32.execution_info();
        assert!(f32_info.weight_precision.starts_with("f32"));
        // Fail-closed: strict-f32 refuses to load unless the process
        // environment pins f32 attention operands.
        std::env::remove_var("FLUX_ATTN_F16");
        assert!(precision_env_error(Da3Precision::StrictF32).is_some());
        assert!(precision_env_error(Da3Precision::FullBf16).is_none());
        std::env::set_var("FLUX_ATTN_F16", "0");
        assert!(precision_env_error(Da3Precision::StrictF32).is_none());
        std::env::remove_var("FLUX_ATTN_F16");
    }

    #[test]
    fn sky_policy_is_deterministic() {
        let mut prediction = Da3Prediction {
            depth: (1..=100).map(|value| value as f32).collect(),
            sky: (0..100)
                .map(|index| if index >= 80 { 1.0 } else { 0.0 })
                .collect(),
            width: 10,
            height: 10,
        };
        prediction.apply_sky_depth_policy();
        let far = prediction.depth[80];
        assert!(prediction.depth[80..].iter().all(|value| *value == far));
        assert!((far - 79.21).abs() < 1e-3);
    }

    #[test]
    fn metric_encode_uses_processed_focal_and_far_clips_sky() {
        let prediction = Da3Prediction {
            depth: vec![1.0; 4],
            sky: vec![0.0, 0.0, 1.0, 0.0],
            width: 2,
            height: 2,
        };
        let map = encode_metric_mm(&prediction, 2, 2, 60.0).unwrap();
        let expected_focal = focal_px_for_processed_width(2, 60.0).unwrap();
        assert!((map.focal_px - expected_focal).abs() < 1e-9);
        assert_eq!(map.mm[2], 65_535);
        let metres = expected_focal / DA3_METRIC_FOCAL_SCALE;
        let millimetres = metres * 1000.0;
        assert_eq!(map.mm[0], millimetres.clamp(1.0, 65_535.0) as u16);
        assert!((map.min_mm - millimetres).abs() < 1e-6);
        assert!((map.max_mm - millimetres).abs() < 1e-6);
    }
}
