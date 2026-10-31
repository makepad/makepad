//! NT matrix multiplication through the shared integer-dot kernels.
use crate::{Buffer, Device, Kernel, Library, Result, Stream};
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockQuant {
    Q8_0,
    Q5K,
    Q4K,
}

impl BlockQuant {
    pub fn block_elements(self) -> u32 {
        match self { Self::Q8_0 => 32, Self::Q5K | Self::Q4K => 256 }
    }

    pub fn block_bytes(self) -> u32 {
        match self { Self::Q8_0 => 34, Self::Q5K => 176, Self::Q4K => 144 }
    }

    /// The kernels consume whole 256-value units, aligned to 16 bytes.
    pub fn row_bytes(self, k: u32) -> u64 {
        u64::from(k.div_ceil(256)) * match self {
            Self::Q8_0 => 272, Self::Q5K => 176, Self::Q4K => 144,
        }
    }

    /// Pack file rows for the shared kernels. Q8_0 puts eight half scales
    /// before their 256 codes, exactly as in the LLM upload. Missing blocks
    /// in the final unit are zero. K formats already have the right layout.
    pub fn pack(self, data: &[u8], n: u32, k: u32) -> Result<Cow<'_, [u8]>> {
        let row = u64::from(k.div_ceil(self.block_elements())) * u64::from(self.block_bytes());
        if n == 0 || k == 0 || u64::from(n) * row != data.len() as u64 {
            return Err("quantised weight storage does not match its shape".into());
        }
        if self != Self::Q8_0 { return Ok(Cow::Borrowed(data)); }
        let stride = usize::try_from(self.row_bytes(k)).map_err(|_| "quantised row size overflow")?;
        let mut out = vec![0; stride.checked_mul(n as usize).ok_or("quantised weight size overflow")?];
        for (src, dst) in data.chunks_exact(row as usize).zip(out.chunks_exact_mut(stride)) {
            for (b, block) in src.chunks_exact(34).enumerate() {
                let unit = b / 8 * 272;
                let part = b % 8;
                dst[unit + part * 2..unit + part * 2 + 2].copy_from_slice(&block[..2]);
                dst[unit + 16 + part * 32..unit + 16 + (part + 1) * 32].copy_from_slice(&block[2..]);
            }
        }
        Ok(Cow::Owned(out))
    }

    fn kernel(self, m: u32) -> &'static str {
        match (self, m) {
            (Self::Q8_0, 1..=4) => "gemv_q80",
            (Self::Q5K, 1..=4) => "gemv_q5k",
            (Self::Q4K, 1..=4) => "gemv_q4k",
            (Self::Q8_0, 5..=31) => "gemm_q80",
            (Self::Q5K, 5..=31) => "gemm_q5k",
            (Self::Q4K, 5..=31) => "gemm_q4k",
            (Self::Q8_0, _) => "gemm_q80_r8k16",
            (Self::Q5K, _) => "gemm_q5k_r8k16",
            (Self::Q4K, _) => "gemm_q4k_r8k16",
        }
    }
}

pub struct QuantizedMatmul {
    pub kernel: Arc<Kernel>,
    quantize: Arc<Kernel>,
    pad: Option<Arc<Kernel>>,
    bias: Arc<Kernel>,
    scratch_bytes: u64,
    dims: [u32; 3],
    padded_k: u32,
    stride: u32,
    weight_bytes: u64,
    groups: [u32; 3],
}

impl QuantizedMatmul {
    /// dst[m,n] = X[m,k] * W[n,k]^T + bias[n]. W must be packed with
    /// BlockQuant::pack. X is quantised per 32 values into biased int8
    /// codes and f32 (scale, sum) pairs by the LLM's quantize_q8 kernel.
    /// m <= 4 uses GEMV; 5..31 the 32x32 tile; >=32 the 64x64 r8k16 tile.
    /// Caller-owned scratch and pipelines must outlive every replay.
    pub fn new(device: &Device, library: &mut Library, n: u32, k: u32, m: u32, format: BlockQuant) -> Result<Self> {
        if n == 0 || k == 0 || m == 0 {
            return Err("quantised matmul dimensions must be nonzero".into());
        }
        let padded_k = k.div_ceil(256).checked_mul(256).ok_or("quantised row size overflow")?;
        let stride = u32::try_from(format.row_bytes(k)).map_err(|_| "quantised row size overflow")?;
        for (a, b) in [(n, stride), (m, padded_k), (m, n)] {
            a.checked_mul(b).ok_or("quantised matmul exceeds u32 addressing")?;
        }
        let kernel = library.get(device, format.kernel(m))?;
        let quantize = library.get(device, "quantize_q8")?;
        let pad = if k != padded_k { Some(library.get(device, "pad_rows")?) } else { None };
        let bias = library.get(device, "add_row_bias")?;
        let values = u64::from(m) * u64::from(padded_k);
        // Codes, scale/sum pairs, and (only for a short row) padded f32 X.
        // All boundaries are multiples of 16 because padded_k is 256-aligned.
        let scratch_bytes = values + values / 32 * 8 + if pad.is_some() { values * 4 } else { 0 };
        let groups = if m <= 4 { [n.div_ceil(8), 1, 1] }
            else if m < 32 { [n.div_ceil(32), m.div_ceil(32), 1] }
            else { [n.div_ceil(64), m.div_ceil(64), 1] };
        Ok(Self { kernel, quantize, pad, bias, scratch_bytes, dims: [n, k, m], padded_k, stride,
            weight_bytes: u64::from(n) * u64::from(stride), groups })
    }

    /// One workspace can serve all serial matmuls, irrespective of format or
    /// shape. Retain older allocations until submission completes if it grows
    /// while recording. Prepared operations contain no per-shape GPU storage.
    pub fn scratch_bytes(&self) -> u64 { self.scratch_bytes }

    pub fn encode(&self, stream: &mut Stream, w: &Buffer, x: &Buffer, dst: &Buffer, bias: Option<&Buffer>, scratch: &Buffer) -> Result<()> {
        let [n, k, m] = self.dims;
        if w.size < self.weight_bytes || x.size < u64::from(m) * u64::from(k) * 4
            || dst.size < u64::from(m) * u64::from(n) * 4
            || bias.is_some_and(|b| b.size < u64::from(n) * 4) || scratch.size < self.scratch_bytes
        {
            return Err("quantised matmul buffer is smaller than its matrix".into());
        }
        self.encode_addresses(stream, w.address, x.address, dst.address, bias.map(|b| b.address), scratch.address);
        Ok(())
    }

    /// The caller validates sizes/alignment and retains all resources until
    /// the recording's final replay. Calls sharing this scratch are serial;
    /// the caller places a barrier after the result's final consumer.
    pub fn encode_addresses(&self, stream: &mut Stream, w: u64, mut x: u64, dst: u64, bias: Option<u64>, scratch: u64) {
        let [n, k, m] = self.dims;
        let values = m * self.padded_k;
        let xq = scratch;
        let xs = xq + u64::from(values);
        if let Some(pad) = &self.pad {
            let padded = xs + u64::from(values / 32) * 8;
            stream.dispatch(&pad.args().buffer("src", x).buffer("dst", padded)
                .u32("cols", k).u32("stride", self.padded_k).u32("len", values),
                [values.div_ceil(256), 1, 1]);
            stream.barrier();
            x = padded;
        }
        stream.dispatch(&self.quantize.args().buffer("x", x).buffer("xq", xq).buffer("xs", xs)
            .u32("blocks", values / 32).u32("k", self.padded_k).u32("x_col", self.padded_k),
            [(values / 32).div_ceil(64), 1, 1]);
        stream.barrier();
        stream.dispatch(&self.kernel.args().buffer("w", w).buffer("w4", w)
            .buffer("xq", xq).buffer("xq4", xq).buffer("xs", xs).buffer("xs4", xs).buffer("dst", dst)
            .u32("n", n).u32("k", self.padded_k).u32("m", m).u32("w_row", self.stride).u32("d_col", n), self.groups);
        if let Some(bias) = bias {
            stream.barrier();
            stream.dispatch(&self.bias.args().buffer("dst", dst).buffer("bias", bias)
                .u32("cols", n).u32("len", m * n), [(m * n).div_ceil(256), 1, 1]);
        }
    }
}
