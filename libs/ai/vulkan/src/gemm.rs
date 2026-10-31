//! Prepared float GEMM for contiguous row-major matrices. Construction compiles
//! once; recording and replay allocate no tiles or intermediate device buffers.
//! Kept independent of graph executors so callers can validate it separately.
use crate::{Buffer, Device, Kernel, Library, Result, Stream};
use makepad_vulkan::vk;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationType { F32, F16 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GemmMode {
    Auto,
    /// Also useful for comparing devices without cooperative matrices.
    Shared,
    /// Explicit shape and subgroup; unsupported requests never fall back.
    Cooperative { shape: [u32; 3], subgroup: u32 },
    /// 64x64 shared tile, 4x4 or 8x4 registers, full f32 accumulation.
    Register { rows: u32, weights: ActivationType },
    /// A phone experiment from GEMM_CANDIDATES. Explicit requests fail if
    /// unsupported; only Auto's environment override falls back.
    Tuned { name: &'static str, weights: ActivationType },
}

/// Names are identical to the Splash prototypes and benchmark labels. An NN
/// or NT name selects the same tile in either layout. hs rounds shared tiles
/// to f16 but accumulates in f32; h8/h16 also use short f16 arithmetic chains.
#[derive(Clone, Copy, Debug)]
pub struct GemmCandidate {
    pub name: &'static str,
    pub nt_name: &'static str,
    pub tile: [u32; 2],
    pub workgroup: u32,
    pub shared_bytes: u32,
    pub half_shared: bool,
    pub half_chain: u32,
    pub subgroup: Option<u32>,
}

macro_rules! candidates {
    ($(($name:literal, $tile:expr, $wg:expr, $shared:expr, $half:expr, $chain:expr, $sg:expr)),* $(,)?) => {
        &[$(GemmCandidate { name: concat!("gemm_tune_", $name), nt_name: concat!("gemm_tune_nt_", $name),
            tile: $tile, workgroup: $wg, shared_bytes: $shared, half_shared: $half, half_chain: $chain, subgroup: $sg }),*]
    };
}

pub const GEMM_CANDIDATES: &[GemmCandidate] = candidates![
    ("4x8_k8_w128", [64, 64], 128, 4096, false, 0, None),
    ("4x8_k16_w128", [64, 64], 128, 8192, false, 0, None),
    ("8x8_k8_w64", [64, 64], 64, 4096, false, 0, None),
    ("8x8_k16_w64", [64, 64], 64, 8192, false, 0, None),
    ("hs_8x8_k16_w64", [64, 64], 64, 4096, true, 0, None),
    ("h8_4x8_k8_w128", [64, 64], 128, 2048, true, 8, None),
    ("h16_4x8_k16_w128", [64, 64], 128, 4096, true, 16, None),
    ("h8_8x8_k8_w64", [64, 64], 64, 2048, true, 8, None),
    ("h16_8x8_k16_w64", [64, 64], 64, 4096, true, 16, None),
];

impl GemmMode {
    pub fn from_name(name: &str, weights: ActivationType) -> Result<Self> {
        match name.trim() {
            "register_4x4" | "gemm_register_4x4" | "gemm_register_nt_4x4" => Ok(Self::Register { rows: 4, weights }),
            "register_8x4" | "gemm_register_8x4" | "gemm_register_nt_8x4" => Ok(Self::Register { rows: 8, weights }),
            name => GEMM_CANDIDATES.iter().find(|c| c.name == name || c.nt_name == name)
                .map(|c| Self::Tuned { name: c.name, weights })
                .ok_or_else(|| format!("unknown float GEMM kernel {name:?}")),
        }
    }

    pub fn candidate(self) -> Option<&'static GemmCandidate> {
        if let Self::Tuned { name, .. } = self {
            GEMM_CANDIDATES.iter().find(|c| c.name == name || c.nt_name == name)
        } else { None }
    }

    pub fn weights(self) -> ActivationType {
        match self {
            Self::Register { weights, .. } | Self::Tuned { weights, .. } => weights,
            _ => ActivationType::F16,
        }
    }

    /// The default tile: 4x8 registers per invocation, k steps of 8, 128
    /// invocations. Measured on the phone (PowerVR): 440-550 GFLOPS NN/NT
    /// vs 182-190 for register 4x4, same f32 accuracy.
    fn default_tuned(device: &Device, activations: ActivationType, weights: ActivationType) -> Option<Self> {
        let mode = Self::Tuned { name: "gemm_tune_4x8_k8_w128", weights };
        Gemm::supports(device, activations, mode).ok().map(|_| mode)
    }

    // Read only during preparation. Shapes already recorded in a stream do
    // not read the environment, allocate, or change their pipeline on replay.
    fn environment(device: &Device, activations: ActivationType, weights: ActivationType) -> Option<Self> {
        let name = std::env::var("MAKEPAD_GEMM_KERNEL").ok()?;
        if name.trim().is_empty() { return None; }
        let selected = Self::from_name(&name, weights).and_then(|mode| {
            Gemm::supports(device, activations, mode)?;
            Ok(mode)
        });
        match selected {
            Ok(mode) => Some(mode),
            Err(reason) => {
                eprintln!("MAKEPAD_GEMM_KERNEL={name:?}: {reason}; using register 4x4 fallback");
                None
            }
        }
    }
}

pub struct Gemm {
    pub kernel: Arc<Kernel>,
    pub shape: Option<crate::CooperativeMatrixShape>,
    /// The resolved mode, including an Auto environment selection.
    pub mode: GemmMode,
    dims: [u32; 3],
    groups: [u32; 3],
    activations: ActivationType,
    weights: ActivationType,
    weight_stride: u32,
    fused_bias: bool,
}

impl Gemm {
    /// Prepares dst[n,m] = W[n,k] * X[k,m], accumulating in f32. Auto uses
    /// f16 weights and the 4x8 tuned tile, else register 4x4, preserving X's
    /// precision; devices below that capacity use Shared, which rounds X to
    /// f16. Cooperative is explicit only; feature support alone does not
    /// establish speed. MAKEPAD_GEMM_KERNEL can select a named register/tuned
    /// kernel in Auto.
    pub fn new(device: &Device, library: &mut Library, n: u32, k: u32, m: u32, activations: ActivationType, mode: GemmMode) -> Result<Self> {
        let mode = if mode == GemmMode::Auto {
            if let Some(selected) = GemmMode::environment(device, activations, ActivationType::F16) { selected }
            else if let Some(tuned) = GemmMode::default_tuned(device, activations, ActivationType::F16) { tuned }
            else if register_support(device, 4, 16 * 1024).is_ok() {
                GemmMode::Register { rows: 4, weights: ActivationType::F16 }
            } else { GemmMode::Shared }
        } else { mode };
        Self::supports(device, activations, mode)?;
        for (a, b) in [(n, k), (k, m), (n, m)] {
            a.checked_mul(b).ok_or("GEMM matrix exceeds u32 element addressing")?;
        }
        if matches!(mode, GemmMode::Tuned { .. }) {
            return Self::tuned(device, library, [n, k, m], activations, mode, k, false);
        }
        if matches!(mode, GemmMode::Register { .. } | GemmMode::Cooperative { .. }) {
            return Self::explicit(device, library, [n, k, m], activations, mode);
        }
        let f = &device.features;
        let invocations = f.max_workgroup_invocations.min(f.max_workgroup_size[0]);
        let half_x = u32::from(activations == ActivationType::F16);
        let workgroup = invocations.min(256) / 16 * 16;
        if f.max_shared_bytes < 2 * 1056 * 4 || workgroup == 0 {
            return Err("device cannot run the shared GEMM tile".into());
        }
        let kernel = library.get_constants(device, "gemm_shared", &[("half_x", half_x), ("workgroup_size", workgroup)])?;
        Ok(Self { kernel, shape: None, mode, dims: [n, k, m], groups: [n.div_ceil(32), m.div_ceil(workgroup / 8), 1],
            activations, weights: ActivationType::F16, weight_stride: k, fused_bias: false })
    }

    /// Prepares dst[m,n] = X[m,k] * W[n,k]^T with optional bias[n]. X and
    /// dst are contiguous f32; W may have padded rows. Auto honours
    /// MAKEPAD_GEMM_KERNEL, then the 4x8 tuned tile, then register 4x4. Tile transposition
    /// uses shared memory, with no intermediate global buffer.
    pub fn new_nt(device: &Device, library: &mut Library, n: u32, k: u32, m: u32, weights: ActivationType, weight_stride: u32) -> Result<Self> {
        Self::new_nt_with_mode(device, library, n, k, m, weights, weight_stride, GemmMode::Auto)
    }

    pub fn new_nt_with_mode(device: &Device, library: &mut Library, n: u32, k: u32, m: u32, weights: ActivationType, weight_stride: u32, mode: GemmMode) -> Result<Self> {
        let mode = if mode == GemmMode::Auto {
            GemmMode::environment(device, ActivationType::F32, weights)
                .or_else(|| GemmMode::default_tuned(device, ActivationType::F32, weights))
                .unwrap_or(GemmMode::Register { rows: 4, weights })
        } else { mode };
        if !matches!(mode, GemmMode::Register { .. } | GemmMode::Tuned { .. }) || mode.weights() != weights {
            return Err("NT GEMM needs a register/tuned mode with matching weight storage".into());
        }
        Self::supports(device, ActivationType::F32, mode)?;
        if weight_stride < k { return Err("GEMM weight stride is smaller than k".into()); }
        for (a, b) in [(n, weight_stride), (k, m), (n, m)] {
            a.checked_mul(b).ok_or("GEMM matrix exceeds u32 element addressing")?;
        }
        if matches!(mode, GemmMode::Tuned { .. }) {
            return Self::tuned(device, library, [n, k, m], ActivationType::F32, mode, weight_stride, true);
        }
        let name = if matches!(mode, GemmMode::Register { rows: 8, .. }) { "gemm_register_nt_8x4" } else { "gemm_register_nt_4x4" };
        let kernel = library.get_constants(device, name, &[
            ("half_w", u32::from(weights == ActivationType::F16)),
            ("vector_w", u32::from(k % 8 == 0 && weight_stride % 8 == 0)),
            ("vector_x", u32::from(k % 4 == 0)),
        ])?;
        Ok(Self { kernel, shape: None, mode, dims: [n, k, m], groups: [n.div_ceil(64), m.div_ceil(64), 1],
            activations: ActivationType::F32, weights, weight_stride, fused_bias: true })
    }

    /// Capability-only check, so a benchmark can skip unsupported variants
    /// without hiding a shader compilation, pipeline or correctness failure.
    pub fn supports(device: &Device, activations: ActivationType, mode: GemmMode) -> Result<()> {
        let weights = mode.weights();
        if (weights == ActivationType::F16 || activations == ActivationType::F16 || mode.candidate().is_some_and(|c| c.half_shared))
            && (!device.features.shader_f16 || !device.features.storage_16bit) {
            return Err("f16 GEMM requires shaderFloat16 and storageBuffer16BitAccess".into());
        }
        match mode {
            GemmMode::Register { rows, .. } => register_support(device, rows, 16 * 1024),
            GemmMode::Tuned { name, .. } => {
                let c = mode.candidate().ok_or_else(|| format!("unknown float GEMM kernel {name:?}"))?;
                let f = &device.features;
                if f.max_workgroup_invocations.min(f.max_workgroup_size[0]) < c.workgroup || f.max_shared_bytes < c.shared_bytes {
                    return Err(format!("{name} requires {} invocations and {} shared bytes", c.workgroup, c.shared_bytes));
                }
                if let Some(subgroup) = c.subgroup {
                    let available = if f.subgroup_size_control { (f.min_subgroup_size..=f.max_subgroup_size).contains(&subgroup) }
                        else { subgroup == f.subgroup_size };
                    if !available || c.workgroup % subgroup != 0 {
                        return Err(format!("{name} requires full subgroups of {subgroup} (default {}, range {}..{})", f.subgroup_size, f.min_subgroup_size, f.max_subgroup_size));
                    }
                }
                Ok(())
            }
            GemmMode::Cooperative { shape, subgroup } => {
                float_kernel(shape)?;
                matrix_support(device, shape, subgroup, false, 24 * 1024).map(|_| ())
            }
            _ => Ok(()),
        }
    }

    fn explicit(device: &Device, library: &mut Library, dims: [u32; 3], activations: ActivationType, mode: GemmMode) -> Result<Self> {
        let [n, k, m] = dims;
        let half_x = u32::from(activations == ActivationType::F16);
        let (name, constants, shape, subgroup, groups, weights) = match mode {
            GemmMode::Register { rows, weights } => (
                if rows == 4 { "gemm_register_4x4" } else { "gemm_register_8x4" },
                vec![("half_w", u32::from(weights == ActivationType::F16)), ("half_x", half_x),
                    ("vector_w", u32::from(k % 8 == 0)), ("vector_x", u32::from(m % 4 == 0)),
                    ("half_vector8", u32::from(m % 8 == 0))],
                None, None, [n.div_ceil(64), m.div_ceil(64), 1], weights,
            ),
            GemmMode::Cooperative { shape, subgroup } => {
                let (supported, subgroups) = matrix_support(device, shape, subgroup, false, 24 * 1024)?;
                let wide = shape[0] == 64;
                let rows = if wide { 1 } else if subgroups >= 2 { 4 } else { 2 };
                let cols = if wide { subgroups } else if subgroups == 4 { 4 } else { 2 };
                (float_kernel(shape)?, vec![("workgroup_size", subgroup * subgroups),
                    ("subgroups", subgroups), ("row_fragments", rows), ("col_fragments", cols),
                    ("half_x", half_x), ("vector_io", u32::from(k % 8 == 0 && m % 4 == 0)),
                    ("half_vector8", u32::from(m % 8 == 0)), ("vector_w", u32::from(k % 8 == 0)),
                    ("vector_x", u32::from(m % 4 == 0))],
                    Some(supported), Some(subgroup), [n.div_ceil(shape[0] * rows), m.div_ceil(shape[1] * cols), 1], ActivationType::F16)
            }
            _ => unreachable!(),
        };
        let kernel = if let Some(subgroup) = subgroup {
            let compiled = library.compile_constants(name, &constants)?;
            Arc::new(device.kernel(name, &compiled, Some(subgroup))?)
        } else { library.get_constants(device, name, &constants)? };
        Ok(Self { kernel, shape, mode, dims, groups, activations, weights, weight_stride: k, fused_bias: false })
    }

    fn tuned(device: &Device, library: &mut Library, dims: [u32; 3], activations: ActivationType, mode: GemmMode, weight_stride: u32, nt: bool) -> Result<Self> {
        let [n, k, m] = dims;
        let c = mode.candidate().ok_or("unknown tuned GEMM")?;
        let name = if nt { c.nt_name } else { c.name };
        let weights = mode.weights();
        let kernel = library.get_constants_subgroup(device, name, &[
            ("half_w", u32::from(weights == ActivationType::F16)),
            ("half_x", u32::from(activations == ActivationType::F16)),
            ("vector_w", u32::from(k % 8 == 0 && weight_stride % 8 == 0)),
            ("vector_x", u32::from(if nt { k % 4 == 0 } else { m % 4 == 0 })),
            ("half_vector8", u32::from(m % 8 == 0)),
        ], c.subgroup)?;
        if kernel.layout.shared_bytes != c.shared_bytes || kernel.workgroup() != [c.workgroup, 1, 1] {
            return Err(format!("{name}: Splash layout differs from GEMM catalogue"));
        }
        Ok(Self { kernel, shape: None, mode, dims, groups: [n.div_ceil(c.tile[0]), m.div_ceil(c.tile[1]), 1],
            activations, weights, weight_stride, fused_bias: nt })
    }

    /// Appends one dispatch. Buffers must remain alive until submission ends;
    /// the caller inserts inter-dispatch barriers through `Stream::barrier`.
    pub fn encode(&self, stream: &mut Stream, w: &Buffer, x: &Buffer, dst: &Buffer) -> Result<()> {
        self.encode_bias(stream, w, x, dst, None)
    }

    pub fn encode_bias(&self, stream: &mut Stream, w: &Buffer, x: &Buffer, dst: &Buffer, bias: Option<&Buffer>) -> Result<()> {
        let [n, k, m] = self.dims;
        let x_width = if self.activations == ActivationType::F16 { 2 } else { 4 };
        let w_width = if self.weights == ActivationType::F16 { 2 } else { 4 };
        if w.size < u64::from(n) * u64::from(self.weight_stride) * w_width || x.size < u64::from(k) * u64::from(m) * x_width
            || dst.size < u64::from(n) * u64::from(m) * 4 {
            return Err("GEMM buffer is smaller than its matrix".into());
        }
        if bias.is_some_and(|b| b.size < u64::from(n) * 4) { return Err("GEMM bias is smaller than n".into()); }
        self.encode_addresses(stream, w.address, x.address, dst.address, bias.map(|b| b.address))
    }

    /// Records addresses within caller-owned arenas. The caller validates
    /// sizes and 16-byte base alignment and keeps buffers and this pipeline
    /// (or its library) alive through the recording's final replay.
    pub fn encode_addresses(&self, stream: &mut Stream, w: u64, x: u64, dst: u64, bias: Option<u64>) -> Result<()> {
        if bias.is_some() && !self.fused_bias { return Err("bias requires an NT GEMM".into()); }
        let [n, k, m] = self.dims;
        if n == 0 || m == 0 { return Ok(()); }
        let args = self.kernel.args()
            .buffer("w", w).buffer("w4", w).buffer("wf", w).buffer("wf4", w)
            .buffer("x", x).buffer("x4", x).buffer("xh", x).buffer("xh4", x)
            .buffer("xh2", x).buffer("dst", dst).buffer("bias", bias.unwrap_or(dst))
            .u32("has_bias", u32::from(bias.is_some())).u32("ws", self.weight_stride)
            .u32("n", n).u32("k", k).u32("m", m);
        stream.dispatch(&args, self.groups);
        Ok(())
    }
}

fn float_kernel(shape: [u32; 3]) -> Result<&'static str> {
    Ok(match shape {
        [16, 16, 16] => "gemm_coop_16x16x16",
        [64, 16, 16] => "gemm_coop_64x16x16",
        [16, 16, 32] => "gemm_coop_16x16x32",
        [16, 8, 16] => "gemm_coop_16x8x16",
        [16, 8, 8] => "gemm_coop_16x8x8",
        [8, 8, 16] => "gemm_coop_8x8x16",
        _ => return Err(format!("no float GEMM kernel for {shape:?}")),
    })
}

pub(crate) fn register_support(device: &Device, rows: u32, shared: u32) -> Result<()> {
    if rows != 4 && rows != 8 { return Err("register rows must be 4 or 8".into()); }
    let f = &device.features;
    if f.max_workgroup_invocations.min(f.max_workgroup_size[0]) < 1024 / rows || f.max_shared_bytes < shared {
        return Err(format!("{rows}x4 register tile requires {} invocations and {shared} shared bytes", 1024 / rows));
    }
    Ok(())
}

pub(crate) fn matrix_support(device: &Device, shape: [u32; 3], subgroup: u32, integer: bool, shared: u32) -> Result<(crate::CooperativeMatrixShape, u32)> {
    let f = &device.features;
    if !f.cooperative_matrix || f.max_shared_bytes < shared { return Err("cooperative matrix or shared-memory capacity unavailable".into()); }
    if subgroup == 0 || !subgroup.is_power_of_two()
        || if f.subgroup_size_control { subgroup < f.min_subgroup_size || subgroup > f.max_subgroup_size }
            else { subgroup != f.subgroup_size } {
        return Err(format!("required subgroup {subgroup} unavailable (default {}, range {}..{})", f.subgroup_size, f.min_subgroup_size, f.max_subgroup_size));
    }
    let count = f.max_workgroup_invocations.min(f.max_workgroup_size[0]) / subgroup;
    let subgroups = if count >= 4 { 4 } else if count >= 2 { 2 } else if count >= 1 { 1 } else { return Err("subgroup exceeds workgroup limit".into()); };
    let component = if integer { vk::ComponentTypeKHR::SINT8 } else { vk::ComponentTypeKHR::FLOAT16 };
    let accumulator = if integer { vk::ComponentTypeKHR::SINT32 } else { vk::ComponentTypeKHR::FLOAT32 };
    let found = f.cooperative_matrix_shapes.iter().find(|s| {
        [s.m, s.n, s.k] == shape && s.scope == vk::ScopeKHR::SUBGROUP && !s.saturating
            && s.a == component && s.b == component && s.c == accumulator && s.result == accumulator
    }).copied().ok_or_else(|| format!("cooperative {component:?} -> {accumulator:?} {shape:?} unavailable"))?;
    Ok((found, subgroups))
}
