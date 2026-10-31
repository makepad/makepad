//! Prepared int8 GEMM, independent of graph executors.
//! Weight quantisation/upload happens once. Recording reuses all scratch;
//! a normal invocation is one column-quantisation dispatch and one GEMM.
use crate::gemm::{matrix_support, register_support};
use crate::{Buffer, BufferKind, Device, Kernel, Library, Result, Stream};
use std::sync::Arc;

// Largest power-of-two span whose worst-case unsigned dot sum fits i32:
// 32768 * 255 * 255 < i32::MAX. Longer matrices use ordered partial GEMMs.
const ACCUMULATION_SPAN: u32 = 32768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Int8GemmMode {
    Dot4 { rows: u32 },
    Cooperative { shape: [u32; 3], subgroup: u32 },
}

pub struct Int8Weights {
    pub codes: Buffer,
    pub metadata: Buffer,
    n: u32,
    k: u32,
    kp: u32,
}

fn padded_k(k: u32) -> Result<u32> {
    k.checked_add(63).map(|v| v / 64 * 64).ok_or_else(|| "padded K exceeds u32 addressing".into())
}

impl Int8Weights {
    /// Symmetric [-127,127], round halfway away from zero, one max-absolute
    /// scale per row. Zero rows and K padding have zero signed codes.
    pub fn upload(device: &Device, n: u32, k: u32, weights: &[f32]) -> Result<Self> {
        let count = n.checked_mul(k).ok_or("weight matrix exceeds u32 addressing")?;
        if weights.len() != count as usize { return Err("weight slice does not match [n,k]".into()); }
        let kp = padded_k(k)?;
        let size = n.checked_mul(kp).ok_or("padded weight matrix exceeds u32 addressing")?;
        let spans = kp.div_ceil(ACCUMULATION_SPAN).max(1);
        let mut codes = vec![0u8; size as usize];
        let mut metadata = vec![0u8; n as usize * spans as usize * 8];
        for r in 0..n as usize {
            let row = &weights[r * k as usize..(r + 1) * k as usize];
            let mut amax = 0.0f32;
            for &v in row {
                if !v.is_finite() { return Err("int8 weights must be finite".into()); }
                amax = amax.max(v.abs());
            }
            let scale = amax / 127.0;
            let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };
            for (d, &v) in row.iter().enumerate() {
                codes[r * kp as usize + d] = (v * inv).round().clamp(-127.0, 127.0) as i8 as u8;
            }
            for span in 0..spans {
                let start = span * ACCUMULATION_SPAN;
                let end = (u64::from(start) + u64::from(ACCUMULATION_SPAN)).min(u64::from(kp)) as u32;
                let sum: i32 = codes[r * kp as usize + start as usize..r * kp as usize + end as usize]
                    .iter().map(|v| *v as i8 as i32).sum();
                let at = (span as usize * n as usize + r) * 8;
                metadata[at..at + 4].copy_from_slice(&scale.to_le_bytes());
                metadata[at + 4..at + 8].copy_from_slice(&(sum as f32).to_le_bytes());
            }
        }
        let code_buffer = device.buffer(codes.len() as u64, BufferKind::Device)?;
        let meta_buffer = device.buffer(metadata.len() as u64, BufferKind::Device)?;
        if !codes.is_empty() { device.upload(&code_buffer, 0, &codes)?; }
        if !metadata.is_empty() { device.upload(&meta_buffer, 0, &metadata)?; }
        Ok(Self { codes: code_buffer, metadata: meta_buffer, n, k, kp })
    }
}

pub struct Int8Gemm {
    pub kernel: Arc<Kernel>,
    pub quantizer: Arc<Kernel>,
    pub shape: Option<crate::CooperativeMatrixShape>,
    /// Column-major signed bytes and (scale, signed sum) metadata.
    pub activation_codes: Buffer,
    pub activation_metadata: Buffer,
    dims: [u32; 3],
    kp: u32,
    groups: [u32; 3],
    transpose_io: bool,
}

impl Int8Gemm {
    pub fn supports(device: &Device, mode: Int8GemmMode) -> Result<()> {
        if device.features.max_workgroup_invocations.min(device.features.max_workgroup_size[0]) < 128 {
            return Err("column quantisation requires 128 invocations".into());
        }
        match mode {
            Int8GemmMode::Dot4 { rows } => {
                if !device.features.dot4_u8_accelerated { return Err("accelerated dot4_u8 unavailable".into()); }
                register_support(device, rows, 8 * 1024)
            }
            Int8GemmMode::Cooperative { shape, subgroup } => {
                if !device.features.shader_int8 { return Err("shaderInt8 unavailable".into()); }
                integer_kernel(shape)?;
                matrix_support(device, shape, subgroup, true, 20 * 1024).map(|_| ())
            }
        }
    }

    pub fn new(device: &Device, library: &mut Library, n: u32, k: u32, m: u32, mode: Int8GemmMode) -> Result<Self> {
        Self::new_layout(device, library, n, k, m, mode, false)
    }

    /// X[m,k] and dst[m,n] are contiguous f32, bias[n] is optional. Weights
    /// keep their per-row scales; activations are quantised per logical column
    /// of X^T directly from the input rows. Always uses dot4 4x4.
    pub fn new_nt(device: &Device, library: &mut Library, n: u32, k: u32, m: u32) -> Result<Self> {
        Self::new_layout(device, library, n, k, m, Int8GemmMode::Dot4 { rows: 4 }, true)
    }

    fn new_layout(device: &Device, library: &mut Library, n: u32, k: u32, m: u32, mode: Int8GemmMode, transpose_io: bool) -> Result<Self> {
        Self::supports(device, mode)?;
        let kp = padded_k(k)?;
        for (a, b) in [(n, kp), (m, kp), (k, m), (n, m)] {
            a.checked_mul(b).ok_or("int8 GEMM exceeds u32 element addressing")?;
        }
        if kp > ACCUMULATION_SPAN {
            eprintln!("int8 GEMM K={k}: {} ordered accumulation spans avoid integer overflow", kp.div_ceil(ACCUMULATION_SPAN));
        }
        let (name, constants, shape, subgroup, groups) = match mode {
            Int8GemmMode::Dot4 { rows } => (
                if transpose_io { "gemm_i8_dot4_nt_4x4" } else if rows == 4 { "gemm_i8_dot4_4x4" } else { "gemm_i8_dot4_8x4" },
                Vec::new(), None, None, [n.div_ceil(64), m.div_ceil(64), 1],
            ),
            Int8GemmMode::Cooperative { shape, subgroup } => {
                let (supported, subgroups) = matrix_support(device, shape, subgroup, true, 20 * 1024)?;
                let (bn, bm) = if shape[0] == 64 { (64, 16 * subgroups) } else { (16 * subgroups, 64) };
                (integer_kernel(shape)?, vec![("workgroup_size", subgroup * subgroups), ("subgroups", subgroups)],
                    Some(supported), Some(subgroup), [n.div_ceil(bn), m.div_ceil(bm), 1])
            }
        };
        let kernel = if let Some(subgroup) = subgroup {
            let compiled = library.compile_constants(name, &constants)?;
            Arc::new(device.kernel(name, &compiled, Some(subgroup))?)
        } else { library.get_constants(device, name, &constants)? };
        let quantizer = library.get_constants(device,
            if transpose_io { "gemm_quantize_columns_nt" } else { "gemm_quantize_columns" },
            &[("vector_x", u32::from(m % 4 == 0))])?;
        let activation_codes = device.buffer(u64::from(m) * u64::from(kp), BufferKind::Device)?;
        let activation_metadata = device.buffer(u64::from(m) * u64::from(kp.div_ceil(ACCUMULATION_SPAN).max(1)) * 8, BufferKind::Device)?;
        Ok(Self { kernel, quantizer, shape, activation_codes, activation_metadata, dims: [n, k, m], kp, groups, transpose_io })
    }

    /// X is finite contiguous f32 [k,m]. Scratch is owned by this prepared GEMM;
    /// callers must order reuse of it, and keep all buffers alive to completion.
    pub fn encode_quantize(&self, stream: &mut Stream, x: &Buffer) -> Result<()> {
        let [_, k, m] = self.dims;
        if x.size < u64::from(k) * u64::from(m) * 4 { return Err("activation buffer is smaller than [k,m]".into()); }
        self.quantize_address(stream, x.address);
        Ok(())
    }

    fn quantize_address(&self, stream: &mut Stream, x: u64) {
        let [_, k, m] = self.dims;
        if m == 0 { return; }
        stream.dispatch(&self.quantizer.args().buffer("x", x).buffer("x4", x)
            .buffer("xq", self.activation_codes.address).buffer("xs", self.activation_metadata.address)
            .u32("k", k).u32("kp", self.kp).u32("m", m), [m.div_ceil(4), 1, 1]);
    }

    /// Uses already-quantised activations; useful for separate kernel timing.
    /// A barrier is required after encode_quantize, supplied by encode below.
    pub fn encode_matmul(&self, stream: &mut Stream, weights: &Int8Weights, dst: &Buffer) -> Result<()> {
        let [n, _, m] = self.dims;
        if dst.size < u64::from(n) * u64::from(m) * 4 { return Err("output buffer is smaller than [n,m]".into()); }
        self.matmul_address(stream, weights, dst.address, None)
    }

    fn matmul_address(&self, stream: &mut Stream, weights: &Int8Weights, dst: u64, bias: Option<u64>) -> Result<()> {
        let [n, k, m] = self.dims;
        if (weights.n, weights.k, weights.kp) != (n, k, self.kp) { return Err("quantised weight dimensions do not match GEMM".into()); }
        if bias.is_some() && !self.transpose_io { return Err("bias requires an NT int8 GEMM".into()); }
        if n == 0 || m == 0 { return Ok(()); }
        for span in 0..self.kp.div_ceil(ACCUMULATION_SPAN).max(1) {
            if span > 0 { stream.barrier(); }
            let start = span * ACCUMULATION_SPAN;
            let end = (u64::from(start) + u64::from(ACCUMULATION_SPAN)).min(u64::from(self.kp)) as u32;
            stream.dispatch(&self.kernel.args().buffer("w4", weights.codes.address).buffer("ws", weights.metadata.address)
                .buffer("x4", self.activation_codes.address).buffer("xs", self.activation_metadata.address).buffer("dst", dst)
                .buffer("bias", bias.unwrap_or(dst)).u32("has_bias", u32::from(bias.is_some()))
                .u32("n", n).u32("kp", self.kp).u32("m", m).u32("start", start).u32("end", end), self.groups);
        }
        Ok(())
    }

    pub fn encode(&self, stream: &mut Stream, weights: &Int8Weights, x: &Buffer, dst: &Buffer) -> Result<()> {
        self.encode_quantize(stream, x)?;
        stream.barrier();
        self.encode_matmul(stream, weights, dst)
    }

    /// Arena recording: the caller validates X, dst and bias sizes/alignment
    /// and keeps this GEMM, its scratch and the addressed buffers alive until
    /// the recording's final replay. Scratch reuse requires a caller barrier.
    pub fn encode_addresses(&self, stream: &mut Stream, weights: &Int8Weights, x: u64, dst: u64, bias: Option<u64>) -> Result<()> {
        self.quantize_address(stream, x);
        stream.barrier();
        self.matmul_address(stream, weights, dst, bias)
    }
}

fn integer_kernel(shape: [u32; 3]) -> Result<&'static str> {
    Ok(match shape {
        [16, 16, 16] => "gemm_i8_coop_16x16x16",
        [64, 16, 16] => "gemm_i8_coop_64x16x16",
        [16, 16, 32] => "gemm_i8_coop_16x16x32",
        _ => return Err(format!("no int8 GEMM kernel for {shape:?}")),
    })
}
