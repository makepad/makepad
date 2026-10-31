//! GPU storage and command recording. Only the transcribing worker calls this.

use crate::whisper::model::{DecoderLayer, EncoderLayer, WhisperModel};
use crate::whisper::quant::*;
use crate::whisper::tensor::{RawTensor, Tensor};
use makepad_ai_vulkan::{ActivationType, BlockQuant, Buffer, BufferKind, Device, Gemm, GemmMode, Int8Gemm, Int8GemmMode, Int8Weights, Kernel, Library, QuantizedMatmul, Result, Stream, KERNEL_SOURCES};
use std::collections::HashMap;
use std::sync::Arc;

pub(super) fn bytes(values: &[f32]) -> &[u8] {
    // The Vulkan targets are little-endian; the slice is borrowed only for
    // the synchronous upload. f32 has no padding or invalid bit patterns.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

pub(super) fn count(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b)
        .filter(|&n| n > 0 && n <= u32::MAX as usize)
        .ok_or_else(|| format!("unsupported Whisper tensor size {a} x {b}"))
}

pub(super) fn upload(device: &Device, values: &[f32]) -> Result<Buffer> {
    let buffer = device.buffer(std::mem::size_of_val(values).max(16) as u64, BufferKind::Device)?;
    if !values.is_empty() {
        device.upload(&buffer, 0, bytes(values))?;
    }
    Ok(buffer)
}

pub(super) fn read(device: &Device, buffer: &Buffer, offset: u64, len: usize) -> Result<Vec<f32>> {
    Ok(device.download(buffer, offset, len * 4)?.chunks_exact(4)
        .map(|v| f32::from_le_bytes(v.try_into().unwrap())).collect())
}

pub(super) struct Matrix {
    pub buffer: Buffer,
    pub int8: Option<Int8Weights>,
    pub rows: usize,
    pub cols: usize,
    pub stride: usize,
    pub half: bool,
    pub quant: Option<BlockQuant>,
}

impl Matrix {
    pub fn raw(device: &Device, data: &[u8], ty: u32, rows: usize, cols: usize) -> Result<Self> {
        let n = count(rows, cols)?;
        let (block, size) = match ty {
            GGML_TYPE_F32 => (1, 4), GGML_TYPE_F16 => (1, 2),
            GGML_TYPE_Q4_0 => (QK, 18), GGML_TYPE_Q4_1 => (QK, 20),
            GGML_TYPE_Q8_0 => (QK, 34),
            GGML_TYPE_Q5_K => (256, 176), GGML_TYPE_Q4_K => (256, 144),
            _ => return Err(format!("unsupported Whisper weight type {ty}")),
        };
        if n % block != 0 || data.len() != n / block * size {
            return Err("Whisper weight storage does not match its shape".into());
        }
        let quant = match ty {
            GGML_TYPE_Q8_0 => Some(BlockQuant::Q8_0),
            GGML_TYPE_Q5_K => Some(BlockQuant::Q5K),
            GGML_TYPE_Q4_K => Some(BlockQuant::Q4K),
            _ => None,
        };
        if let Some(format) = quant {
            if cols % block != 0 {
                return Err("Whisper quantised rows must contain complete file blocks".into());
            }
            let packed = format.pack(data, rows as u32, cols as u32)?;
            let buffer = device.buffer(packed.len() as u64, BufferKind::Device)?;
            device.upload(&buffer, 0, &packed)?;
            return Ok(Self { buffer, int8: None, rows, cols, stride: cols, half: false, quant });
        }
        if ty != GGML_TYPE_F16 && ty != GGML_TYPE_F32 {
            // The legacy Q4 formats retain their existing compatibility path.
            let values = RawTensor { data: data.to_vec().into(), shape: vec![cols, rows], ggml_type: ty }.to_f32();
            return Self::f32(device, &values.data, rows, cols);
        }
        let stride = cols.div_ceil(8) * 8;
        count(rows, stride)?;
        let mut padded = vec![0u8; rows * stride * size];
        for r in 0..rows {
            padded[r * stride * size..r * stride * size + cols * size]
                .copy_from_slice(&data[r * cols * size..(r + 1) * cols * size]);
        }
        let buffer = device.buffer(padded.len() as u64, BufferKind::Device)?;
        device.upload(&buffer, 0, &padded)?;
        Ok(Self { buffer, int8: None, rows, cols, stride, half: ty == GGML_TYPE_F16, quant: None })
    }

    pub fn f32(device: &Device, data: &[f32], rows: usize, cols: usize) -> Result<Self> {
        Self::raw(device, bytes(data), GGML_TYPE_F32, rows, cols)
    }

}

pub(super) struct Linear {
    pub w: Matrix,
    pub b: Option<Buffer>,
}

impl Linear {
    fn has_shape(&self, rows: usize, cols: usize) -> bool {
        self.w.rows == rows && self.w.cols == cols
    }
    pub fn new(device: &Device, w: &RawTensor, b: Option<&Tensor>) -> Result<Self> {
        Self::raw(device, &w.data, w.ggml_type, w.n_rows(), w.n_cols(), b.map(|v| v.data.as_slice()))
    }

    pub(super) fn encoder(device: &Device, w: &RawTensor, b: Option<&Tensor>, int8: bool) -> Result<Self> {
        let mut linear = Self::new(device, w, b)?;
        if int8 && linear.w.quant.is_none() {
            linear.w.int8 = Some(Int8Weights::upload(device, w.n_rows() as u32, w.n_cols() as u32, &w.to_f32().data)?);
        }
        Ok(linear)
    }

    pub fn raw(device: &Device, w: &[u8], ty: u32, rows: usize, cols: usize, b: Option<&[f32]>) -> Result<Self> {
        if b.is_some_and(|b| b.len() != rows) { return Err("Whisper bias shape mismatch".into()); }
        Ok(Self { w: Matrix::raw(device, w, ty, rows, cols)?, b: b.map(|b| upload(device, b)).transpose()? })
    }
}

pub(super) struct Norm {
    pub w: Buffer,
    pub b: Buffer,
    pub cols: usize,
}

impl Norm {
    pub fn new(device: &Device, w: &[f32], b: &[f32]) -> Result<Self> {
        if w.is_empty() || w.len() != b.len() { return Err("Whisper norm shape mismatch".into()); }
        Ok(Self { w: upload(device, w)?, b: upload(device, b)?, cols: w.len() })
    }
}

pub(super) struct Attention {
    pub norm: Norm,
    pub q: Linear,
    pub k: Linear,
    pub v: Linear,
    pub out: Linear,
}

impl Attention {
    fn validate(&self, width: usize) -> Result<()> {
        if self.norm.cols != width || !self.q.has_shape(width, width) || !self.k.has_shape(width, width)
            || !self.v.has_shape(width, width) || !self.out.has_shape(width, width) {
            return Err("Whisper attention matrix shape mismatch".into());
        }
        Ok(())
    }
}

pub(super) struct Ffn {
    pub norm: Norm,
    pub up: Linear,
    pub down: Linear,
}

impl Ffn {
    fn validate(&self, width: usize) -> Result<()> {
        if self.norm.cols != width || !self.up.has_shape(count(width, 4)?, width)
            || !self.down.has_shape(width, count(width, 4)?) {
            return Err("Whisper FFN matrix shape mismatch".into());
        }
        Ok(())
    }
}

pub(super) struct EncoderWeights {
    pub attn: Attention,
    pub ffn: Ffn,
}

impl EncoderWeights {
    pub fn new(device: &Device, l: &EncoderLayer, int8: bool) -> Result<Self> {
        let weights = Self {
            attn: Attention {
                norm: Norm::new(device, &l.attn_ln_0_w.data, &l.attn_ln_0_b.data)?,
                q: Linear::encoder(device, &l.attn_q_w, Some(&l.attn_q_b), int8)?,
                k: Linear::encoder(device, &l.attn_k_w, None, int8)?,
                v: Linear::encoder(device, &l.attn_v_w, Some(&l.attn_v_b), int8)?,
                out: Linear::encoder(device, &l.attn_ln_1_w, Some(&l.attn_ln_1_b), int8)?,
            },
            ffn: Ffn {
                norm: Norm::new(device, &l.mlp_ln_w.data, &l.mlp_ln_b.data)?,
                up: Linear::encoder(device, &l.mlp_0_w, Some(&l.mlp_0_b), int8)?,
                down: Linear::encoder(device, &l.mlp_1_w, Some(&l.mlp_1_b), int8)?,
            },
        };
        let width = weights.attn.norm.cols;
        weights.attn.validate(width)?;
        weights.ffn.validate(width)?;
        Ok(weights)
    }
}

pub(super) struct DecoderWeights {
    pub self_attn: Attention,
    pub cross: Attention,
    pub ffn: Ffn,
}

impl DecoderWeights {
    pub fn new(device: &Device, l: &DecoderLayer) -> Result<Self> {
        let weights = Self {
            self_attn: Attention {
                norm: Norm::new(device, &l.attn_ln_0_w.data, &l.attn_ln_0_b.data)?,
                q: Linear::new(device, &l.attn_q_w, Some(&l.attn_q_b))?,
                k: Linear::new(device, &l.attn_k_w, None)?,
                v: Linear::new(device, &l.attn_v_w, Some(&l.attn_v_b))?,
                out: Linear::new(device, &l.attn_ln_1_w, Some(&l.attn_ln_1_b))?,
            },
            cross: Attention {
                norm: Norm::new(device, &l.cross_attn_ln_0_w.data, &l.cross_attn_ln_0_b.data)?,
                q: Linear::new(device, &l.cross_attn_q_w, Some(&l.cross_attn_q_b))?,
                k: Linear::new(device, &l.cross_attn_k_w, None)?,
                v: Linear::new(device, &l.cross_attn_v_w, Some(&l.cross_attn_v_b))?,
                out: Linear::new(device, &l.cross_attn_ln_1_w, Some(&l.cross_attn_ln_1_b))?,
            },
            ffn: Ffn {
                norm: Norm::new(device, &l.mlp_ln_w.data, &l.mlp_ln_b.data)?,
                up: Linear::new(device, &l.mlp_0_w, Some(&l.mlp_0_b))?,
                down: Linear::new(device, &l.mlp_1_w, Some(&l.mlp_1_b))?,
            },
        };
        let width = weights.self_attn.norm.cols;
        weights.self_attn.validate(width)?;
        weights.cross.validate(width)?;
        weights.ffn.validate(width)?;
        Ok(weights)
    }
}

pub(super) struct ModelWeights {
    pub enc: Vec<EncoderWeights>,
    pub dec: Vec<DecoderWeights>,
    pub conv1: Linear,
    pub conv2: Linear,
    pub pos: Buffer,
    pub enc_norm: Norm,
    pub dec_norm: Norm,
    pub logits: Matrix,
}

impl ModelWeights {
    pub fn resident_bytes(&self) -> std::collections::BTreeMap<&'static str, u64> {
        let mut sizes = std::collections::BTreeMap::new();
        let mut matrix = |w: &Matrix| {
            let ty = match w.quant {
                Some(BlockQuant::Q8_0) => "q8_0",
                Some(BlockQuant::Q5K) => "q5_k",
                Some(BlockQuant::Q4K) => "q4_k",
                None if w.half => "f16",
                None => "f32",
            };
            *sizes.entry(ty).or_default() += w.buffer.size;
            if let Some(extra) = &w.int8 {
                *sizes.entry("int8 auxiliary").or_default() += extra.codes.size + extra.metadata.size;
            }
        };
        let linears: Vec<_> = std::iter::once(&self.conv1).chain(std::iter::once(&self.conv2))
            .chain(self.enc.iter().flat_map(|l| [&l.attn.q, &l.attn.k, &l.attn.v, &l.attn.out, &l.ffn.up, &l.ffn.down]))
            .chain(self.dec.iter().flat_map(|l| [&l.self_attn.q, &l.self_attn.k, &l.self_attn.v, &l.self_attn.out,
                &l.cross.q, &l.cross.k, &l.cross.v, &l.cross.out, &l.ffn.up, &l.ffn.down])).collect();
        for l in &linears { matrix(&l.w); }
        matrix(&self.logits);
        let norms = std::iter::once(&self.enc_norm).chain(std::iter::once(&self.dec_norm))
            .chain(self.enc.iter().flat_map(|l| [&l.attn.norm, &l.ffn.norm]))
            .chain(self.dec.iter().flat_map(|l| [&l.self_attn.norm, &l.cross.norm, &l.ffn.norm]));
        *sizes.entry("f32").or_default() += self.pos.size
            + norms.map(|n| n.w.size + n.b.size).sum::<u64>()
            + linears.iter().filter_map(|l| l.b.as_ref()).map(|b| b.size).sum::<u64>();
        sizes
    }

    pub fn new(device: &Device, model: &WhisperModel, int8: bool) -> Result<Self> {
        let conv = |w: &RawTensor, b: &Tensor| -> Result<Linear> {
            if w.shape.len() != 3 || w.shape[0] != 3 { return Err("unsupported Whisper convolution shape".into()); }
            Linear::raw(device, &w.data, w.ggml_type, w.shape[2], count(w.shape[0], w.shape[1])?, Some(&b.data))
        };
        let weights = Self {
            enc: model.encoder_layers.iter().map(|l| EncoderWeights::new(device, l, int8)).collect::<Result<_>>()?,
            dec: model.decoder_layers.iter().map(|l| DecoderWeights::new(device, l)).collect::<Result<_>>()?,
            conv1: conv(&model.e_conv_1_w, &model.e_conv_1_b)?,
            conv2: conv(&model.e_conv_2_w, &model.e_conv_2_b)?,
            pos: upload(device, &model.e_pe.data)?,
            enc_norm: Norm::new(device, &model.e_ln_w.data, &model.e_ln_b.data)?,
            dec_norm: Norm::new(device, &model.d_ln_w.data, &model.d_ln_b.data)?,
            // This tensor is held as f32 by the model. Retaining those
            // values also preserves logits for models with f32 embeddings.
            logits: Matrix::f32(device, &model.d_te.data, model.hparams.n_vocab as usize, model.hparams.n_text_state as usize)?,
        };
        let width = model.hparams.n_audio_state as usize;
        if width != model.hparams.n_text_state as usize
            || !weights.conv1.has_shape(width, count(model.hparams.n_mels as usize, 3)?)
            || !weights.conv2.has_shape(width, count(width, 3)?)
            || weights.enc_norm.cols != width || weights.dec_norm.cols != width
            || model.e_pe.data.len() != count(model.hparams.n_audio_ctx as usize, width)?
            || model.d_pe.data.len() != count(model.hparams.n_text_ctx as usize, width)? {
            return Err("Whisper model dimensions do not match its weights".into());
        }
        for l in &weights.enc { l.attn.validate(width)?; }
        for l in &weights.dec { l.self_attn.validate(width)?; }
        Ok(weights)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Span {
    pub offset: u64,
    pub len: usize,
}

#[derive(Default)]
pub(super) struct Layout {
    pub bytes: u64,
}

impl Layout {
    pub fn take(&mut self, len: usize) -> Result<Span> {
        count(len, 1)?;
        let span = Span { offset: self.bytes, len };
        self.bytes = self.bytes.checked_add((len as u64 * 4).div_ceil(16) * 16)
            .ok_or("Whisper activation allocation overflow")?;
        Ok(span)
    }
}

pub(super) struct Scratch {
    pub norm: Span,
    pub q: Span,
    pub k: Span,
    pub v: Span,
    pub att: Span,
    pub proj: Span,
    pub ff: Span,
}

impl Scratch {
    pub fn new(plan: &mut Layout, rows: usize, width: usize) -> Result<Self> {
        let n = count(rows, width)?;
        Ok(Self {
            norm: plan.take(n)?, q: plan.take(n)?, k: plan.take(n)?, v: plan.take(n)?,
            att: plan.take(n)?, proj: plan.take(n)?, ff: plan.take(count(n, 4)?)?,
        })
    }
}

// Rust drops fields in declaration order: the stream waits before buffers and
// pipelines are freed; all resources precede the shared device reference.
pub(super) struct Ops {
    pub stream: Stream,
    kernels: HashMap<&'static str, Arc<Kernel>>,
    library: Library,
    int8_gemms: HashMap<[u32; 3], Int8Gemm>,
    quant_gemms: HashMap<([u32; 3], BlockQuant), QuantizedMatmul>,
    quant_scratch: Option<Buffer>,
    retired_quant_scratch: Vec<Buffer>,
    pub int8_encoder: bool,
    pub arena: Buffer,
    readback: Option<Buffer>,
    pub device: Arc<Device>,
}

impl Ops {
    pub fn new() -> Result<Self> {
        let device = Device::shared()?;
        if device.features.max_shared_bytes < 20 * 1024 || device.features.max_workgroup_invocations < 128 {
            return Err("Whisper requires 20 KiB workgroup memory and 128 invocations".into());
        }
        eprintln!("[voice] {}", device.features.description());
        let int8_encoder = match std::env::var("MAKEPAD_WHISPER_GEMM").as_deref() {
            Ok("int8") => true,
            Err(_) | Ok("f16") | Ok("register") => false,
            Ok(value) => return Err(format!("unknown MAKEPAD_WHISPER_GEMM={value}; expected f16 or int8")),
        };
        if int8_encoder {
            Int8Gemm::supports(&device, Int8GemmMode::Dot4 { rows: 4 })?;
            eprintln!("[voice] encoder GEMM: int8 dot4 4x4 (convolutions and decoder stay floating point)");
        }
        let mut library = Library::new(KERNEL_SOURCES)?;
        let mut kernels = HashMap::new();
        for name in ["whisper_gemm_f16", "whisper_gemm_f32", "whisper_gemv_f16", "whisper_gemv_f32",
                     "whisper_elementwise", "whisper_norm", "whisper_im2col", "whisper_attention"] {
            kernels.insert(name, library.get(&device, name)?);
        }
        // The encoder tile needs 32 KiB of workgroup memory; smaller devices
        // keep the existing attention.
        if device.features.max_shared_bytes >= 32 * 1024 {
            let name = "attention_d64";
            match library.get(&device, name) {
                Ok(kernel) => { kernels.insert(name, kernel); }
                Err(error) => eprintln!("[voice] encoder attention tile unavailable: {error}; using existing attention"),
            }
        }
        Ok(Self { kernels, library, int8_gemms: HashMap::new(), quant_gemms: HashMap::new(), int8_encoder,
            quant_scratch: None, retired_quant_scratch: Vec::new(),
            arena: device.buffer(16, BufferKind::Device)?, readback: None, stream: device.stream(0)?, device })
    }

    pub fn reserve(&mut self, plan: &Layout) -> Result<()> {
        if self.arena.size < plan.bytes { self.arena = self.device.buffer(plan.bytes, BufferKind::Device)?; }
        Ok(())
    }

    pub fn addr(&self, span: Span) -> u64 { self.arena.address + span.offset }

    pub fn write(&self, span: Span, data: &[f32]) -> Result<()> {
        if span.len != data.len() { return Err("Whisper activation shape mismatch".into()); }
        self.device.upload(&self.arena, span.offset, bytes(data))
    }

    pub fn read(&mut self, span: Span) -> Result<Vec<f32>> {
        // A discrete device can expose host-visible device memory through a
        // bus aperture. CPU reads there are much slower than a transfer into
        // cached host memory. Phones keep their direct unified-memory copy.
        if self.device.features.discrete {
            let size = span.len as u64 * 4;
            if self.readback.as_ref().is_none_or(|b| b.size < size) {
                self.readback = Some(self.device.buffer(size, BufferKind::Staging)?);
            }
            let dst = self.readback.as_ref().unwrap();
            self.stream.begin()?;
            self.stream.barrier();
            self.stream.copy(&self.arena, span.offset, dst, 0, size);
            self.stream.submit_wait()?;
            read(&self.device, dst, 0, span.len)
        } else {
            read(&self.device, &self.arena, span.offset, span.len)
        }
    }

    pub fn begin(&mut self) -> Result<()> {
        self.stream.begin()?;
        self.stream.barrier();
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        self.stream.submit_wait()?;
        self.retired_quant_scratch.clear();
        Ok(())
    }

    pub fn matmul(&mut self, x: u64, dst: u64, rows: usize, weight: &Matrix, bias: Option<&Buffer>) -> Result<()> {
        if let Some(format) = weight.quant {
            let dims = [weight.rows as u32, weight.cols as u32, rows as u32];
            if let std::collections::hash_map::Entry::Vacant(entry) = self.quant_gemms.entry((dims, format)) {
                entry.insert(QuantizedMatmul::new(&self.device, &mut self.library, dims[0], dims[1], dims[2], format)?);
            }
            let gemm = &self.quant_gemms[&(dims, format)];
            if self.quant_scratch.as_ref().is_none_or(|b| b.size < gemm.scratch_bytes()) {
                let scratch = self.device.buffer(gemm.scratch_bytes(), BufferKind::Device)?;
                if let Some(old) = self.quant_scratch.replace(scratch) {
                    // Earlier dispatches in this recording still reference it.
                    self.retired_quant_scratch.push(old);
                }
            }
            gemm.encode_addresses(&mut self.stream, weight.buffer.address, x, dst, bias.map(|b| b.address),
                self.quant_scratch.as_ref().unwrap().address);
            self.stream.barrier();
            return Ok(());
        }
        if let Some(weights) = weight.int8.as_ref().filter(|_| rows > 4) {
            let dims = [weight.rows as u32, weight.cols as u32, rows as u32];
            if let std::collections::hash_map::Entry::Vacant(entry) = self.int8_gemms.entry(dims) {
                entry.insert(Int8Gemm::new_nt(&self.device, &mut self.library, dims[0], dims[1], dims[2])?);
            }
            self.int8_gemms[&dims].encode_addresses(&mut self.stream, weights, x, dst, bias.map(|b| b.address))?;
            self.stream.barrier();
            return Ok(());
        }
        let weights = if weight.half { ActivationType::F16 } else { ActivationType::F32 };
        if rows > 4 && Gemm::supports(&self.device, ActivationType::F32, GemmMode::Register { rows: 4, weights }).is_ok() {
            let gemm = Gemm::new_nt(&self.device, &mut self.library, weight.rows as u32, weight.cols as u32,
                rows as u32, weights, weight.stride as u32)?;
            gemm.encode_addresses(&mut self.stream, weight.buffer.address, x, dst, bias.map(|b| b.address))?;
            self.stream.barrier();
            return Ok(());
        }
        let name = match (rows <= 4, weight.half) {
            (true, true) => "whisper_gemv_f16", (true, false) => "whisper_gemv_f32",
            (false, true) => "whisper_gemm_f16", (false, false) => "whisper_gemm_f32",
        };
        let k = &self.kernels[name];
        let args = k.args().buffer("x", x).buffer("x4", x)
            .buffer("w4", weight.buffer.address).buffer("wf4", weight.buffer.address)
            .buffer("bias", bias.map_or(weight.buffer.address, |b| b.address)).buffer("dst", dst)
            .u32("m", rows as u32).u32("n", weight.rows as u32).u32("k", weight.cols as u32)
            .u32("ws", weight.stride as u32).u32("has_bias", u32::from(bias.is_some()));
        let groups = if rows <= 4 { [weight.rows.div_ceil(4) as u32, rows as u32, 1] }
            else { [weight.rows.div_ceil(32) as u32, rows.div_ceil(32) as u32, 1] };
        self.stream.dispatch(&args, groups);
        self.stream.barrier();
        Ok(())
    }

    pub fn linear(&mut self, x: Span, dst: Span, rows: usize, l: &Linear) -> Result<()> {
        self.matmul(self.addr(x), self.addr(dst), rows, &l.w, l.b.as_ref())
    }

    pub fn element(&mut self, a: u64, b: u64, dst: u64, n: usize, b_len: usize, op: u32) {
        let k = &self.kernels["whisper_elementwise"];
        self.stream.dispatch(&k.args().buffer("a", a).buffer("b", b).buffer("dst", dst)
            .u32("n", n as u32).u32("b_len", b_len as u32).u32("op", op), [(n as u32).div_ceil(128), 1, 1]);
        self.stream.barrier();
    }

    pub fn add(&mut self, a: Span, b: Span, dst: Span) {
        self.element(self.addr(a), self.addr(b), self.addr(dst), a.len, b.len, 0);
    }

    pub fn gelu(&mut self, x: Span) {
        self.element(self.addr(x), self.addr(x), self.addr(x), x.len, x.len, 2);
    }

    pub fn norm(&mut self, x: Span, dst: Span, cols: usize, norm: Option<&Norm>, eps: f32) {
        let k = &self.kernels["whisper_norm"];
        self.stream.dispatch(&k.args().buffer("x", self.addr(x)).buffer("dst", self.addr(dst))
            .buffer("mul", norm.map_or(self.addr(x), |n| n.w.address))
            .buffer("add", norm.map_or(self.addr(x), |n| n.b.address))
            .u32("cols", cols as u32).u32("affine", u32::from(norm.is_some())).f32("eps", eps),
            [(x.len / cols) as u32, 1, 1]);
        self.stream.barrier();
    }

    pub fn im2col(&mut self, x: Span, dst: Span, ic: usize, iw: usize, kw: usize, stride: usize, pad: usize, time_major: bool) {
        let ow = dst.len / (ic * kw);
        let k = &self.kernels["whisper_im2col"];
        self.stream.dispatch(&k.args().buffer("x", self.addr(x)).buffer("dst", self.addr(dst))
            .u32("ic", ic as u32).u32("iw", iw as u32).u32("ow", ow as u32).u32("kw", kw as u32)
            .u32("stride", stride as u32).u32("pad", pad as u32).u32("time_major", u32::from(time_major)),
            [(dst.len as u32).div_ceil(128), 1, 1]);
        self.stream.barrier();
    }

    pub fn attention(&mut self, q: u64, key: u64, val: u64, dst: u64, nq: usize, nk: usize, heads: usize, past: Option<usize>, scale: f32) {
        let k = &self.kernels["whisper_attention"];
        self.stream.dispatch(&k.args().buffer("q", q).buffer("k", key).buffer("v", val).buffer("dst", dst)
            .u32("nq", nq as u32).u32("nk", nk as u32).u32("heads", heads as u32)
            .u32("past", past.unwrap_or(0) as u32).u32("causal", u32::from(past.is_some())).f32("scale", scale),
            [heads as u32, nq.div_ceil(8) as u32, 1]);
        self.stream.barrier();
    }

    pub fn qkv(&mut self, x: Span, s: &Scratch, rows: usize, a: &Attention) -> Result<()> {
        self.norm(x, s.norm, a.norm.cols, Some(&a.norm), 1e-5);
        self.linear(s.norm, s.q, rows, &a.q)?;
        self.linear(s.norm, s.k, rows, &a.k)?;
        self.linear(s.norm, s.v, rows, &a.v)
    }

    pub fn ffn(&mut self, x: Span, s: &Scratch, rows: usize, f: &Ffn) -> Result<()> {
        self.norm(x, s.norm, f.norm.cols, Some(&f.norm), 1e-5);
        self.linear(s.norm, s.ff, rows, &f.up)?;
        self.gelu(s.ff);
        self.linear(s.ff, s.proj, rows, &f.down)?;
        self.add(s.proj, x, x);
        Ok(())
    }

    pub fn encoder_attention(&mut self, x: Span, s: &Scratch, rows: usize, heads: usize, a: &Attention) -> Result<()> {
        self.qkv(x, s, rows, a)?;
        if let Some(k) = self.kernels.get("attention_d64")
            .filter(|_| rows > 8 && rows <= 1500 && heads.checked_mul(64) == Some(a.norm.cols))
        {
            self.stream.dispatch(&k.args().buffer("q", self.addr(s.q)).buffer("k", self.addr(s.k))
                .buffer("v", self.addr(s.v)).buffer("dst", self.addr(s.att))
                .u32("nq", rows as u32).u32("nk", rows as u32).f32("scale", 0.125)
                .u32("q_row", heads as u32 * 16).u32("q_head", 16).u32("k_row", heads as u32 * 16).u32("k_head", 16)
                .u32("v_row", heads as u32 * 16).u32("v_head", 16).u32("d_row", heads as u32 * 16).u32("d_head", 16),
                [heads as u32, rows.div_ceil(64) as u32, 1]);
            self.stream.barrier();
        } else {
            self.attention(self.addr(s.q), self.addr(s.k), self.addr(s.v), self.addr(s.att), rows, rows, heads, None, 0.125);
        }
        self.linear(s.att, s.proj, rows, &a.out)?;
        self.add(s.proj, x, x);
        Ok(())
    }

    pub fn encoder_layer(&mut self, x: Span, s: &Scratch, rows: usize, heads: usize, l: &EncoderWeights) -> Result<()> {
        self.encoder_attention(x, s, rows, heads, &l.attn)?;
        self.ffn(x, s, rows, &l.ffn)
    }

    pub fn cross_ffn(&mut self, x: Span, s: &Scratch, rows: usize, heads: usize, key: &Buffer, val: &Buffer, nk: usize, l: &DecoderWeights) -> Result<()> {
        self.norm(x, s.norm, l.cross.norm.cols, Some(&l.cross.norm), 1e-5);
        self.linear(s.norm, s.q, rows, &l.cross.q)?;
        self.attention(self.addr(s.q), key.address, val.address, self.addr(s.att), rows, nk, heads, None, 0.125);
        self.linear(s.att, s.proj, rows, &l.cross.out)?;
        self.add(s.proj, x, x);
        self.ffn(x, s, rows, &l.ffn)
    }
}
