//! Small recording graph for Kokoro's immutable weights and transient tensors.
//! Only the synthesis worker enters this module; its stream shares the
//! process-wide device and locks the queue only when submitting.

use super::Context;
use crate::kokoro::ops::Mat;
use crate::kokoro::weights::HostWeight;
use makepad_ai_vulkan::{Buffer, BufferKind, Device, Kernel, Library, Result, Stream};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Weak};

fn bytes(values: &[f32]) -> &[u8] {
    // f32 has no padding; uploads finish before the borrowed slice expires.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

fn count(rows: usize, cols: usize) -> Result<usize> {
    rows.checked_mul(cols).filter(|&n| n > 0 && n <= u32::MAX as usize)
        .ok_or_else(|| format!("Kokoro tensor cannot be indexed with u32: {rows} x {cols}"))
}

pub(super) struct Weight {
    pub buffer: Buffer,
    pub rows: usize,
    pub cols: usize,
    pub stride: usize,
}

type WeightKey = (usize, usize, usize, usize, bool);

pub(super) struct Weights {
    pub owner: Weak<()>,
    values: HashMap<WeightKey, Rc<Weight>>,
}

impl Weights {
    pub fn bytes(&self) -> u64 { self.values.values().map(|w| w.buffer.size).sum() }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Tensor {
    pub id: usize,
    pub rows: usize,
    pub cols: usize,
    pub stride: usize,
}

impl Tensor {
    fn len(self) -> usize { self.rows * self.stride }
    fn bytes(self) -> u64 { self.len() as u64 * 4 }
}

enum Op {
    Input(Vec<f32>),
    Gemm { weight: Rc<Weight>, bias: Option<Rc<Weight>> },
    Linear { weight: Rc<Weight>, bias: Rc<Weight> },
    Lstm { weight: Rc<Weight>, bias: Rc<Weight>, reverse: bool },
    Im2col { kernel: usize, stride: usize, pad: usize, dilation: usize },
    Overlap { bias: Option<Rc<Weight>>, kernel: usize, stride: usize, pad: usize },
    Element { op: u32, scale: f32, alpha: Option<Rc<Weight>> },
    Norm { affine: Option<(Rc<Weight>, Rc<Weight>)>, eps: f32, activation: u32, slope: f32, alpha: Option<Rc<Weight>> },
    Layout { op: u32, factor: usize },
    Depthwise { weight: Rc<Weight>, bias: Rc<Weight>, kernel: usize, stride: usize, pad: usize },
    Concat,
    Attention { heads: usize },
}

struct Node {
    out: Tensor,
    inputs: Vec<Tensor>,
    op: Op,
}

pub(super) struct Graph<'a> {
    ctx: &'a mut Context,
    owner: Option<usize>,
    nodes: Vec<Node>,
}

impl<'a> Graph<'a> {
    pub fn new(ctx: &'a mut Context, owner: Option<&Arc<()>>) -> Self {
        ctx.weights.retain(|_, w| w.owner.strong_count() != 0);
        let owner = owner.map(|id| {
            let key = Arc::as_ptr(id) as usize;
            ctx.weights.entry(key).or_insert_with(|| Weights {
                owner: Arc::downgrade(id), values: HashMap::new(),
            });
            key
        });
        Self { ctx, owner, nodes: Vec::new() }
    }

    pub fn set_owner(&mut self, owner: &Arc<()>) {
        let key = Arc::as_ptr(owner) as usize;
        self.ctx.weights.entry(key).or_insert_with(|| Weights {
            owner: Arc::downgrade(owner), values: HashMap::new(),
        });
        self.owner = Some(key);
    }

    fn node(&mut self, rows: usize, cols: usize, inputs: &[Tensor], op: Op) -> Result<Tensor> {
        let stride = cols.checked_add(3).ok_or("Kokoro row overflow")? / 4 * 4;
        count(rows, stride)?;
        let out = Tensor { id: self.nodes.len(), rows, cols, stride };
        self.nodes.push(Node { out, inputs: inputs.to_vec(), op });
        Ok(out)
    }

    pub fn input(&mut self, x: &Mat) -> Result<Tensor> {
        if x.data.len() != count(x.rows, x.cols)? { return Err("Kokoro input shape mismatch".into()); }
        // Inputs are uploaded before the graph runs. Keeping them in a prefix
        // ensures their allocations cannot alias an earlier computed value.
        if self.nodes.iter().any(|n| !matches!(n.op, Op::Input(_))) {
            return Err("Kokoro graph inputs must precede its operations".into());
        }
        self.node(x.rows, x.cols, &[], Op::Input(x.data.clone()))
    }

    // Transposing [in, out * tap] once at upload supplies all tap products
    // for transposed convolutions. Keys belong to a live immutable identity.
    fn weight<W: HostWeight + ?Sized>(&mut self, data: &W, rows: usize, cols: usize, transposed: bool) -> Result<Rc<Weight>> {
        if data.count() != count(rows, cols)? { return Err("Kokoro weight shape mismatch".into()); }
        let key = (data.key(), data.count(), rows, cols, transposed);
        if let Some(owner) = self.owner {
            if let Some(w) = self.ctx.weights[&owner].values.get(&key) { return Ok(w.clone()); }
        }
        let data = data.host();
        let stride = cols.div_ceil(4) * 4;
        let mut packed = vec![0.0; count(rows, stride)?];
        for r in 0..rows {
            for c in 0..cols {
                let src = if transposed { c * rows + r } else { r * cols + c };
                packed[r * stride + c] = data[src];
            }
        }
        let buffer = self.ctx.ops.device.buffer(std::mem::size_of_val(packed.as_slice()) as u64, BufferKind::Device)?;
        self.ctx.ops.device.upload(&buffer, 0, bytes(&packed))?;
        let w = Rc::new(Weight { buffer, rows, cols, stride });
        if let Some(owner) = self.owner { self.ctx.weights.get_mut(&owner).unwrap().values.insert(key, w.clone()); }
        Ok(w)
    }

    fn vector<W: HostWeight + ?Sized>(&mut self, data: &W) -> Result<Rc<Weight>> { self.weight(data, 1, data.count(), false) }

    fn gemm(&mut self, x: Tensor, weight: Rc<Weight>, bias: Option<Rc<Weight>>) -> Result<Tensor> {
        if x.rows != weight.cols || bias.as_ref().is_some_and(|b| b.cols != weight.rows) {
            return Err("Kokoro GEMM dimensions disagree".into());
        }
        self.node(weight.rows, x.cols, &[x], Op::Gemm { weight, bias })
    }

    pub fn matmul(&mut self, x: Tensor, weight: &[f32], rows: usize, bias: Option<&[f32]>) -> Result<Tensor> {
        let w = self.weight(weight, rows, x.rows, false)?;
        let b = bias.map(|b| self.vector(b)).transpose()?;
        self.gemm(x, w, b)
    }

    pub fn linear(&mut self, x: Tensor, weight: &[f32], bias: &[f32]) -> Result<Tensor> {
        // Model projection widths are aligned. Reuse the speech runtime's
        // tiled row-major GEMM/GEMV to avoid two layout dispatches per linear.
        // Ragged public-op shapes use the general padded representation.
        if x.cols % 8 == 0 && bias.len() % 4 == 0 {
            let weight = self.weight(weight, bias.len(), x.cols, false)?;
            let bias = self.vector(bias)?;
            return self.node(x.rows, weight.rows, &[x], Op::Linear { weight, bias });
        }
        let transposed = self.transpose(x)?;
        let y = self.matmul(transposed, weight, bias.len(), Some(bias))?;
        self.transpose(y)
    }

    pub fn conv<W: HostWeight + ?Sized>(&mut self, x: Tensor, weight: &W, bias: Option<&[f32]>, channels: usize,
        pad: usize, stride: usize, dilation: usize, transposed: bool) -> Result<Tensor> {
        let width = count(channels, x.rows)?;
        if weight.count() % width != 0 || stride == 0 || dilation == 0 { return Err("Kokoro convolution shape mismatch".into()); }
        let kernel = weight.count() / width;
        if kernel == 0 { return Err("empty Kokoro convolution".into()); }
        let time = if transposed {
            (x.cols - 1).checked_mul(stride).and_then(|t| t.checked_add(kernel)).and_then(|t| t.checked_sub(2 * pad))
        } else {
            x.cols.checked_add(2 * pad).and_then(|t| t.checked_sub(dilation * (kernel - 1) + 1)).map(|t| t / stride + 1)
        }.filter(|&t| t > 0).ok_or("Kokoro convolution output overflow")?;
        let b = bias.map(|b| self.vector(b)).transpose()?;
        if b.as_ref().is_some_and(|b| b.cols != channels) { return Err("Kokoro convolution bias mismatch".into()); }
        if transposed {
            // GEMM at the input rate followed by overlap-add, the dual of
            // im2col. Expanding zeros before GEMM would multiply its work and
            // scratch by the upsampling stride (six or ten in this model).
            let w = self.weight(weight, channels * kernel, x.rows, true)?;
            let taps = self.gemm(x, w, None)?;
            return self.node(channels, time, &[taps], Op::Overlap { bias: b, kernel, stride, pad });
        }
        let w = self.weight(weight, channels, x.rows * kernel, false)?;
        let patches = if kernel == 1 && stride == 1 && pad == 0 { x } else {
            self.node(x.rows * kernel, time, &[x], Op::Im2col { kernel, stride, pad, dilation })?
        };
        self.gemm(patches, w, b)
    }

    pub fn depthwise<W: HostWeight + ?Sized>(&mut self, x: Tensor, weight: &W, bias: &[f32], stride: usize, pad: usize, output_pad: usize) -> Result<Tensor> {
        if weight.count() % x.rows != 0 || bias.len() != x.rows || stride == 0 { return Err("Kokoro depthwise shape mismatch".into()); }
        let kernel = weight.count() / x.rows;
        let time = (x.cols - 1) * stride + kernel + output_pad - 2 * pad;
        let weight = self.vector(weight)?;
        let bias = self.vector(bias)?;
        self.node(x.rows, time, &[x], Op::Depthwise { weight, bias, kernel, stride, pad })
    }

    pub fn element(&mut self, x: Tensor, other: Option<Tensor>, op: u32, scale: f32, alpha: Option<&[f32]>) -> Result<Tensor> {
        let mut inputs = vec![x];
        if let Some(b) = other {
            if (x.rows, x.cols) != (b.rows, b.cols) { return Err("Kokoro elementwise shape mismatch".into()); }
            inputs.push(b);
        }
        if alpha.is_some_and(|a| a.len() != x.rows) { return Err("Kokoro activation shape mismatch".into()); }
        let alpha = alpha.map(|a| self.vector(a)).transpose()?;
        self.node(x.rows, x.cols, &inputs, Op::Element { op, scale, alpha })
    }

    pub fn add(&mut self, x: Tensor, y: Tensor, scale: f32) -> Result<Tensor> { self.element(x, Some(y), 1, scale, None) }
    pub fn leaky(&mut self, x: Tensor, slope: f32) -> Result<Tensor> { self.element(x, None, 2, slope, None) }

    pub fn norm(&mut self, x: Tensor, affine: Option<(&[f32], &[f32])>, adain: Option<Tensor>, eps: f32,
        activation: u32, slope: f32, alpha: Option<&[f32]>) -> Result<Tensor> {
        let affine = affine.map(|(w, b)| -> Result<_> {
            if w.len() != x.cols || b.len() != x.cols { return Err("Kokoro norm affine mismatch".into()); }
            Ok((self.vector(w)?, self.vector(b)?))
        }).transpose()?;
        let alpha = alpha.map(|a| self.vector(a)).transpose()?;
        let mut inputs = vec![x];
        if let Some(a) = adain {
            if a.rows != 1 || a.cols != 2 * x.rows { return Err("Kokoro AdaIN affine mismatch".into()); }
            inputs.push(a);
        }
        self.node(x.rows, x.cols, &inputs, Op::Norm { affine, eps, activation, slope, alpha })
    }

    pub fn transpose(&mut self, x: Tensor) -> Result<Tensor> { self.node(x.cols, x.rows, &[x], Op::Layout { op: 0, factor: 1 }) }
    pub fn repeat(&mut self, x: Tensor, factor: usize) -> Result<Tensor> { self.node(x.rows, count(x.cols, factor)?, &[x], Op::Layout { op: 1, factor }) }
    pub fn reflect(&mut self, x: Tensor, left: usize) -> Result<Tensor> {
        if left >= x.cols { return Err("Kokoro reflection exceeds its input".into()); }
        self.node(x.rows, x.cols + left, &[x], Op::Layout { op: 2, factor: left })
    }
    pub fn concat(&mut self, parts: &[Tensor]) -> Result<Tensor> {
        let cols = parts.first().ok_or("empty Kokoro concatenation")?.cols;
        if parts.iter().any(|p| p.cols != cols) { return Err("Kokoro concatenation shape mismatch".into()); }
        self.node(parts.iter().map(|p| p.rows).sum(), cols, parts, Op::Concat)
    }
    pub fn attention(&mut self, q: Tensor, k: Tensor, v: Tensor, heads: usize) -> Result<Tensor> {
        if q.cols != heads * 64 || k.cols != q.cols || v.cols != q.cols || k.rows != v.rows {
            return Err("Kokoro attention dimensions disagree".into());
        }
        self.node(q.rows, q.cols, &[q, k, v], Op::Attention { heads })
    }

    pub fn lstm(&mut self, x: Tensor, layer: &crate::kokoro::ops::LstmLayer, reverse: bool) -> Result<Tensor> {
        if layer.bias_hh.len() != 4 * layer.hidden { return Err("Kokoro recurrent bias mismatch".into()); }
        let gates = self.matmul(x, &layer.weight_ih, 4 * layer.hidden, Some(&layer.bias_ih))?;
        let weight = self.weight(&layer.weight_hh, 4 * layer.hidden, layer.hidden, false)?;
        let bias = self.vector(&layer.bias_hh)?;
        self.node(layer.hidden, x.cols, &[gates], Op::Lstm { weight, bias, reverse })
    }

    pub fn finish(self, outputs: &[Tensor]) -> Result<Vec<Mat>> {
        let mut last: Vec<_> = (0..self.nodes.len()).collect();
        for (i, n) in self.nodes.iter().enumerate() {
            for input in &n.inputs { last[input.id] = i; }
        }
        for output in outputs { last[output.id] = self.nodes.len(); }
        let mut free = Vec::<(u64, u64)>::new();
        let mut size = 0u64;
        let mut offsets = vec![0; self.nodes.len()];
        let mut retire = vec![Vec::new(); self.nodes.len()];
        for (id, &end) in last.iter().enumerate() {
            if end < retire.len() { retire[end].push(id); }
        }
        for (i, n) in self.nodes.iter().enumerate() {
            let need = n.out.bytes();
            offsets[i] = if let Some(j) = free.iter().position(|&(_, len)| len >= need) {
                let offset = free[j].0;
                free[j].0 += need;
                free[j].1 -= need;
                if free[j].1 == 0 { free.remove(j); }
                offset
            } else {
                let offset = size;
                size = size.checked_add(need).ok_or("Kokoro activation arena overflow")?;
                offset
            };
            for &id in &retire[i] { free.push((offsets[id], self.nodes[id].out.bytes())); }
            free.sort_unstable_by_key(|&(off, _)| off);
            let mut j = 1;
            while j < free.len() {
                if free[j - 1].0 + free[j - 1].1 == free[j].0 {
                    free[j - 1].1 += free.remove(j).1;
                } else { j += 1; }
            }
        }
        let op = &mut self.ctx.ops;
        if op.arena.size < size { op.arena = op.device.buffer(size, BufferKind::Device)?; }
        // A single staging allocation packs all inputs, then all requested
        // outputs. Discrete devices never read through a device-memory aperture.
        let input_bytes: u64 = self.nodes.iter().filter(|n| matches!(n.op, Op::Input(_))).map(|n| n.out.bytes()).sum();
        let output_bytes: u64 = outputs.iter().map(|t| t.bytes()).sum();
        if op.staging.size < input_bytes.max(output_bytes) { op.staging = op.device.buffer(input_bytes.max(output_bytes), BufferKind::Staging)?; }
        let mut packed_offset = 0;
        for n in &self.nodes {
            if let Op::Input(data) = &n.op {
                let mut packed = vec![0.0; n.out.len()];
                for (dst, src) in packed.chunks_mut(n.out.stride).zip(data.chunks(n.out.cols)) { dst[..src.len()].copy_from_slice(src); }
                op.device.upload(&op.staging, packed_offset, bytes(&packed))?;
                packed_offset += n.out.bytes();
            }
        }
        op.stream.begin()?;
        packed_offset = 0;
        for n in &self.nodes {
            if matches!(n.op, Op::Input(_)) {
                op.stream.copy(&op.staging, packed_offset, &op.arena, offsets[n.out.id], n.out.bytes());
                packed_offset += n.out.bytes();
            }
        }
        op.stream.barrier();
        for n in &self.nodes {
            if !matches!(n.op, Op::Input(_)) {
                op.record(n, &offsets);
                op.stream.barrier();
            }
        }
        packed_offset = 0;
        for output in outputs {
            op.stream.copy(&op.arena, offsets[output.id], &op.staging, packed_offset, output.bytes());
            packed_offset += output.bytes();
        }
        op.stream.submit_wait()?;
        self.ctx.submissions += 1;
        self.ctx.dispatches += self.nodes.iter().filter(|n| !matches!(n.op, Op::Input(_) | Op::Concat)).count();
        self.ctx.peak_arena = self.ctx.peak_arena.max(size);
        let data = op.device.download(&op.staging, 0, output_bytes as usize)?;
        let mut at = 0;
        let mut result = Vec::with_capacity(outputs.len());
        for output in outputs {
            let mut values = Vec::with_capacity(output.rows * output.cols);
            for r in 0..output.rows {
                for c in 0..output.cols {
                    let off = at + (r * output.stride + c) * 4;
                    values.push(f32::from_le_bytes(data[off..off + 4].try_into().unwrap()));
                }
            }
            at += output.bytes() as usize;
            result.push(Mat::from_vec(output.rows, output.cols, values));
        }
        Ok(result)
    }
}

// The stream waits before buffers/pipelines are freed; all resources precede
// the shared device reference.
pub(super) struct Ops {
    stream: Stream,
    kernels: HashMap<&'static str, Arc<Kernel>>,
    arena: Buffer,
    staging: Buffer,
    device: Arc<Device>,
}

impl Ops {
    pub fn new() -> Result<Self> {
        let device = Device::shared()?;
        if device.features.max_shared_bytes < 20 * 1024 || device.features.max_workgroup_invocations < 256 {
            return Err("Kokoro requires 20 KiB shared memory and 256 workgroup invocations".into());
        }
        eprintln!("[kokoro] {}", device.features.description());
        let mut library = Library::new(&[
            ("kernels/kokoro.splash", include_str!("../../../../../vulkan/kernels/kokoro.splash")),
            ("kernels/whisper.splash", include_str!("../../../../../vulkan/kernels/whisper.splash")),
        ])?;
        let mut kernels = HashMap::new();
        for name in ["kokoro_gemm", "kokoro_im2col", "kokoro_overlap", "kokoro_element", "kokoro_norm", "kokoro_layout", "kokoro_depthwise", "kokoro_lstm", "whisper_attention", "whisper_gemm_f32", "whisper_gemv_f32"] {
            kernels.insert(name, library.get(&device, name)?);
        }
        Ok(Self { kernels, arena: device.buffer(16, BufferKind::Device)?, staging: device.buffer(16, BufferKind::Staging)?, stream: device.stream(0)?, device })
    }

    fn record(&mut self, node: &Node, offsets: &[u64]) {
        let out = node.out;
        let addr = |t: Tensor| self.arena.address + offsets[t.id];
        let x = node.inputs[0];
        let dst = addr(out);
        let flat_groups = (out.len() / 4).div_ceil(128) as u32;
        let groups = [flat_groups.min(65535), flat_groups.div_ceil(65535), 1];
        match &node.op {
            Op::Input(_) => unreachable!(),
            Op::Gemm { weight, bias } => {
                let k = &self.kernels["kokoro_gemm"];
                let args = k.args().buffer("a", weight.buffer.address).buffer("b", addr(x)).buffer("dst", dst)
                    .buffer("bias", bias.as_ref().map_or(dst, |b| b.buffer.address))
                    .u32("m", out.rows as u32).u32("n", out.cols as u32).u32("k", x.rows as u32)
                    .u32("astride", weight.stride as u32).u32("bstride", x.stride as u32).u32("has_bias", u32::from(bias.is_some()));
                self.stream.dispatch(&args, [out.cols.div_ceil(32) as u32, out.rows.div_ceil(32) as u32, 1]);
            }
            Op::Linear { weight, bias } => {
                let small = x.rows <= 4;
                let k = &self.kernels[if small { "whisper_gemv_f32" } else { "whisper_gemm_f32" }];
                let args = k.args().buffer("x", addr(x)).buffer("x4", addr(x))
                    .buffer("w4", weight.buffer.address).buffer("wf4", weight.buffer.address)
                    .buffer("bias", bias.buffer.address).buffer("dst", dst)
                    .u32("m", x.rows as u32).u32("n", out.cols as u32).u32("k", x.cols as u32)
                    .u32("ws", weight.stride as u32).u32("has_bias", 1);
                self.stream.dispatch(&args, if small { [out.cols.div_ceil(4) as u32, x.rows as u32, 1] }
                    else { [out.cols.div_ceil(32) as u32, x.rows.div_ceil(32) as u32, 1] });
            }
            Op::Lstm { weight, bias, reverse } => {
                let k = &self.kernels["kokoro_lstm"];
                self.stream.dispatch(&k.args().buffer("projected", addr(x)).buffer("w", weight.buffer.address)
                    .buffer("bias", bias.buffer.address).buffer("dst", dst)
                    .u32("hidden", out.rows as u32).u32("time", out.cols as u32).u32("stride", out.stride as u32)
                    .u32("ws", weight.stride as u32).u32("reverse", u32::from(*reverse)), [1, 1, 1]);
            }
            Op::Im2col { kernel, stride, pad, dilation } => {
                let k = &self.kernels["kokoro_im2col"];
                self.stream.dispatch(&k.args().buffer("x", addr(x)).buffer("x4", addr(x)).buffer("dst", dst)
                    .u32("ic", x.rows as u32).u32("iw", x.cols as u32).u32("xs", x.stride as u32)
                    .u32("ow", out.cols as u32).u32("os", out.stride as u32).u32("kw", *kernel as u32)
                    .u32("stride", *stride as u32).u32("pad", *pad as u32).u32("dilation", *dilation as u32), groups);
            }
            Op::Overlap { bias, kernel, stride, pad } => {
                let k = &self.kernels["kokoro_overlap"];
                self.stream.dispatch(&k.args().buffer("x", addr(x)).buffer("dst", dst)
                    .buffer("bias", bias.as_ref().map_or(dst, |b| b.buffer.address))
                    .u32("has_bias", u32::from(bias.is_some())).u32("channels", out.rows as u32)
                    .u32("iw", x.cols as u32).u32("xs", x.stride as u32).u32("ow", out.cols as u32).u32("os", out.stride as u32)
                    .u32("kw", *kernel as u32).u32("stride", *stride as u32).u32("pad", *pad as u32), groups);
            }
            Op::Element { op, scale, alpha } => {
                let k = &self.kernels["kokoro_element"];
                self.stream.dispatch(&k.args().buffer("x", addr(x)).buffer("b", addr(*node.inputs.get(1).unwrap_or(&x)))
                    .buffer("alpha", alpha.as_ref().map_or(dst, |w| w.buffer.address)).buffer("dst", dst)
                    .u32("count", out.len() as u32).u32("stride", out.stride as u32).u32("op", *op).f32("scale", *scale), groups);
            }
            Op::Norm { affine, eps, activation, slope, alpha } => {
                let k = &self.kernels["kokoro_norm"];
                let mode = if node.inputs.len() == 2 { 2 } else { u32::from(affine.is_some()) };
                let a = if mode == 2 { addr(node.inputs[1]) } else { affine.as_ref().map_or(dst, |a| a.0.buffer.address) };
                self.stream.dispatch(&k.args().buffer("x", addr(x)).buffer("dst", dst).buffer("affine", a)
                    .buffer("beta", affine.as_ref().map_or(dst, |a| a.1.buffer.address))
                    .buffer("alpha", alpha.as_ref().map_or(dst, |w| w.buffer.address))
                    .u32("cols", x.cols as u32).u32("stride", x.stride as u32).u32("rows", x.rows as u32)
                    .u32("mode", mode).u32("activation", *activation).f32("eps", *eps).f32("slope", *slope), [x.rows as u32, 1, 1]);
            }
            Op::Layout { op, factor } => {
                let k = &self.kernels["kokoro_layout"];
                self.stream.dispatch(&k.args().buffer("x", addr(x)).buffer("dst", dst)
                    .u32("rows", out.rows as u32).u32("cols", x.cols as u32).u32("xs", x.stride as u32)
                    .u32("os", out.stride as u32).u32("ow", out.cols as u32).u32("op", *op).u32("factor", *factor as u32), groups);
            }
            Op::Depthwise { weight, bias, kernel, stride, pad } => {
                let k = &self.kernels["kokoro_depthwise"];
                self.stream.dispatch(&k.args().buffer("x", addr(x)).buffer("dst", dst)
                    .buffer("w", weight.buffer.address).buffer("bias", bias.buffer.address)
                    .u32("channels", x.rows as u32).u32("iw", x.cols as u32).u32("xs", x.stride as u32)
                    .u32("ow", out.cols as u32).u32("os", out.stride as u32).u32("kw", *kernel as u32)
                    .u32("stride", *stride as u32).u32("pad", *pad as u32), groups);
            }
            Op::Concat => {
                let mut offset = offsets[out.id];
                for input in &node.inputs {
                    self.stream.copy(&self.arena, offsets[input.id], &self.arena, offset, input.bytes());
                    offset += input.bytes();
                }
            }
            Op::Attention { heads } => {
                let k = &self.kernels["whisper_attention"];
                self.stream.dispatch(&k.args().buffer("q", addr(x)).buffer("k", addr(node.inputs[1])).buffer("v", addr(node.inputs[2])).buffer("dst", dst)
                    .u32("nq", x.rows as u32).u32("nk", node.inputs[1].rows as u32).u32("heads", *heads as u32)
                    .u32("past", 0).u32("causal", 0).f32("scale", 0.125), [*heads as u32, x.rows.div_ceil(8) as u32, 1]);
            }
        }
    }
}
