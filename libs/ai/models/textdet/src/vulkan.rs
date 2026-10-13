//! Resident detector weights and one command stream per inference. Spatial
//! rows are padded to four floats so the GEMM can use aligned vector loads,
//! including the 1x1 maps in squeeze-excitation blocks.

use crate::cpu::Map;
use crate::ir::{Act, Graph, Op};
use makepad_ai_vulkan::{Args, Buffer, BufferKind, Device, Kernel, Library, Result, Stream};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

// Im2col is process memory on unified-memory phones; reuse a bounded tile.
const SCRATCH_BUDGET: u64 = 16 * 1024 * 1024;

fn bytes(values: &[f32]) -> &[u8] {
    // f32 has no padding, and upload copies before this borrow expires.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

fn product(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b).filter(|&n| n > 0 && n <= u32::MAX as usize)
        .ok_or_else(|| format!("textdet: shape cannot be indexed with u32: {a} x {b}"))
}

fn aligned(n: usize) -> Result<usize> {
    n.checked_add(3).map(|n| n / 4 * 4).ok_or_else(|| "textdet: row size overflow".into())
}

fn window(size: usize, kernel: usize, stride: usize, lo: usize, hi: usize) -> Result<usize> {
    let padded = size.checked_add(lo).and_then(|n| n.checked_add(hi))
        .filter(|&n| n <= u32::MAX as usize).ok_or("textdet: padded size overflow")?;
    if kernel == 0 || stride == 0 || padded < kernel {
        return Err(format!("textdet: window {kernel}, stride {stride} does not fit {size} + {lo} + {hi}"));
    }
    Ok((padded - kernel) / stride + 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shape {
    c: usize,
    h: usize,
    w: usize,
    stride: usize,
}

impl Shape {
    fn new(c: usize, h: usize, w: usize) -> Result<Self> {
        let stride = aligned(product(h, w)?)?;
        product(c, stride)?;
        Ok(Self { c, h, w, stride })
    }

    fn plane(self) -> usize { self.h * self.w }
    fn len(self) -> usize { self.c * self.stride }
    fn bytes(self) -> u64 { self.len() as u64 * 4 }
    fn groups(self) -> [u32; 3] { groups((self.len() / 4).div_ceil(128)) }
}

fn groups(n: usize) -> [u32; 3] {
    // Vulkan guarantees 65535 groups per dimension, even on phone drivers.
    [n.min(65535) as u32, n.div_ceil(65535) as u32, 1]
}

#[derive(Clone, Copy)]
struct Value {
    shape: Shape,
    offset: u64,
}

#[derive(Clone, Copy)]
struct Weight {
    offset: u64,
    rows: usize,
    cols: usize,
    stride: usize,
}

#[derive(Clone, Copy)]
struct Parameters {
    weight: Weight,
    bias: Option<u64>,
}

struct Plan {
    values: Vec<Value>,
    scratch: Vec<Option<Value>>,
    bytes: u64,
}

#[derive(Default)]
struct Offsets {
    end: u64,
    free: Vec<(u64, u64)>,
}

impl Offsets {
    fn alloc(&mut self, bytes: u64) -> Result<u64> {
        if let Some(i) = self.free.iter().position(|&(_, len)| len >= bytes) {
            let offset = self.free[i].0;
            self.free[i].0 += bytes;
            self.free[i].1 -= bytes;
            if self.free[i].1 == 0 { self.free.remove(i); }
            Ok(offset)
        } else {
            let offset = self.end;
            self.end = self.end.checked_add(bytes).ok_or("textdet: arena size overflow")?;
            Ok(offset)
        }
    }

    fn release(&mut self, value: Value) {
        self.free.push((value.offset, value.shape.bytes()));
        self.free.sort_unstable_by_key(|&(offset, _)| offset);
        let mut i = 1;
        while i < self.free.len() {
            if self.free[i - 1].0 + self.free[i - 1].1 == self.free[i].0 {
                let (_, bytes) = self.free.remove(i);
                self.free[i - 1].1 += bytes;
            } else {
                i += 1;
            }
        }
    }
}

fn with_act(args: Args<'_>, act: Act) -> Args<'_> {
    let (code, a, b) = act.code();
    args.u32("act", code).f32("act_a", a).f32("act_b", b)
}

fn maps(args: Args<'_>, x: Shape, out: Shape) -> Args<'_> {
    args.u32("ic", x.c as u32).u32("ih", x.h as u32).u32("iw", x.w as u32)
        .u32("xs", x.stride as u32).u32("channels", out.c as u32)
        .u32("oh", out.h as u32).u32("ow", out.w as u32).u32("os", out.stride as u32)
}

fn convolution(args: Args<'_>, kernel: (usize, usize), stride: (usize, usize), pad: [usize; 4]) -> Args<'_> {
    args.u32("kh", kernel.0 as u32).u32("kw", kernel.1 as u32)
        .u32("sh", stride.0 as u32).u32("sw", stride.1 as u32)
        .u32("pt", pad[0] as u32).u32("pl", pad[1] as u32)
}

const KERNELS: [&str; 9] = ["textdet_gemm", "textdet_im2col2d", "textdet_depthwise", "textdet_shuffle", "textdet_transpose",
    "textdet_element", "textdet_global_avg", "textdet_pool", "textdet_nearest"];

/// Cache compiled kernels per thread, but borrow the process-wide device:
/// camera/detector worker teardown must never destroy a Vulkan device.
struct Shared {
    kernels: HashMap<&'static str, Arc<Kernel>>,
    device: Arc<Device>,
}

thread_local! {
    static SHARED: RefCell<Option<Rc<Shared>>> = const { RefCell::new(None) };
}

fn shared() -> Result<Rc<Shared>> {
    SHARED.with(|cell| {
        if let Some(s) = cell.borrow().as_ref() {
            return Ok(s.clone());
        }
        let device = Device::shared()?;
        let mut library = Library::new(&[("kernels/textdet.splash", include_str!("../../../vulkan/kernels/textdet.splash"))])?;
        let mut kernels = HashMap::new();
        for name in KERNELS {
            kernels.insert(name, library.get(&device, name)?);
        }
        let s = Rc::new(Shared { kernels, device });
        *cell.borrow_mut() = Some(s.clone());
        Ok(s)
    })
}

/// Blocking executor; like the CPU reference, call it on the detector worker
/// (it stays on the thread that made it).
pub struct Detector {
    // The stream owns its command pool and waits before buffers are freed.
    // Resources precede the kernels and shared device reference in ctx.
    stream: Stream,
    weights: Buffer,
    arena: Buffer,
    staging: Buffer,
    ops: Vec<Op>,
    parameters: Vec<Option<Parameters>>,
    scratch_budget: u64,
    ctx: Rc<Shared>,
}

impl Detector {
    pub fn new(graph: &Graph) -> Result<Detector> {
        graph.validate()?;
        for (i, t) in graph.weights.iter().enumerate() {
            let len = t.shape.iter().try_fold(1, |n, &d| product(n, d))?;
            if len != t.data.len() { return Err(format!("textdet: weight {i} data does not match its shape")); }
        }
        let mut packed: Vec<f32> = Vec::new();
        let mut resident = HashMap::new();
        let mut pack = |id: usize, rows: usize, cols: usize, transpose: bool| -> Result<Weight> {
            let key = (id, rows, cols, transpose);
            if let Some(&weight) = resident.get(&key) { return Ok(weight); }
            let source = &graph.weights[id].data;
            if product(rows, cols)? != source.len() { return Err(format!("textdet: weight {id} size mismatch")); }
            let stride = aligned(cols)?;
            let len = product(rows, stride)?;
            let start = packed.len();
            packed.resize(start.checked_add(len).ok_or("textdet: weights size overflow")?, 0.0);
            for r in 0..rows {
                for c in 0..cols {
                    packed[start + r * stride + c] = source[if transpose { c * rows + r } else { r * cols + c }];
                }
            }
            let weight = Weight { offset: start as u64 * 4, rows, cols, stride };
            resident.insert(key, weight);
            Ok(weight)
        };
        let mut parameters = Vec::with_capacity(graph.ops.len());
        for op in &graph.ops {
            let params = match op {
                Op::Conv { weight, bias, out_channels, kernel, stride, pad, .. } => {
                    Self::check_window(*kernel, *stride, *pad)?;
                    let cols = product(graph.weights[*weight].shape[1], product(kernel.0, kernel.1)?)?;
                    Some(Parameters {
                        weight: pack(*weight, *out_channels, cols, false)?,
                        bias: bias.map(|id| pack(id, 1, *out_channels, false).map(|w| w.offset)).transpose()?,
                    })
                }
                Op::ConvTranspose { weight, bias, out_channels, kernel, stride, .. } => {
                    Self::check_window(*kernel, *stride, [0; 4])?;
                    // Both GEMM and the general gather use [out * taps, in].
                    // This transpose happens once, never on the inference path.
                    let rows = product(*out_channels, product(kernel.0, kernel.1)?)?;
                    Some(Parameters {
                        weight: pack(*weight, rows, graph.weights[*weight].shape[0], true)?,
                        bias: bias.map(|id| pack(id, 1, *out_channels, false).map(|w| w.offset)).transpose()?,
                    })
                }
                Op::Pool { kernel, stride, pad, .. } => {
                    Self::check_window((*kernel, *kernel), (*stride, *stride), [*pad; 4])?;
                    None
                }
                _ => None,
            };
            parameters.push(params);
        }

        let ctx = shared()?;
        let device = &ctx.device;
        let weights = device.buffer(packed.len() as u64 * 4, BufferKind::Device)?;
        if !packed.is_empty() { device.upload(&weights, 0, bytes(&packed))?; }
        Ok(Self {
            stream: device.stream(0)?, weights,
            arena: device.buffer(16, BufferKind::Device)?,
            staging: device.buffer(16, BufferKind::Staging)?,
            ops: graph.ops.clone(), parameters, scratch_budget: SCRATCH_BUDGET, ctx: ctx.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_scratch_budget(graph: &Graph, bytes: u64) -> Result<Detector> {
        let mut detector = Self::new(graph)?;
        detector.scratch_budget = bytes;
        Ok(detector)
    }

    /// Bytes retained by the weights, arena and staging buffers, including
    /// capacity kept for reuse after a larger input.
    pub fn gpu_bytes(&self) -> u64 {
        self.weights.size + self.arena.size + self.staging.size
    }

    fn check_window(kernel: (usize, usize), stride: (usize, usize), pad: [usize; 4]) -> Result<()> {
        if [kernel.0, kernel.1, stride.0, stride.1].iter().any(|&v| v == 0 || v > u32::MAX as usize)
            || pad.iter().any(|&v| v > u32::MAX as usize) {
            return Err("textdet: kernel, stride or padding is outside the shader's u32 range".into());
        }
        Ok(())
    }

    fn plan(&self, input: &Map) -> Result<Plan> {
        let first = Shape::new(input.c, input.h, input.w)?;
        if input.data.len() != first.c * first.plane() { return Err("textdet: input data does not match its shape".into()); }
        let mut shapes = vec![first];
        let mut scratch_shapes = Vec::with_capacity(self.ops.len());
        for (i, op) in self.ops.iter().enumerate() {
            let mut scratch = None;
            let out = match op {
                Op::Conv { input, out_channels, kernel, stride, pad, groups, .. } => {
                    let x = shapes[*input];
                    if x.c % groups != 0 || out_channels % groups != 0 {
                        return Err(format!("textdet op {i}: convolution groups do not divide channels"));
                    }
                    if *groups != 1 && *groups != x.c {
                        return Err(format!("textdet op {i}: only dense or depthwise convolution is supported"));
                    }
                    let p = self.parameters[i].unwrap();
                    if p.weight.cols != product(x.c / groups, product(kernel.0, kernel.1)?)? {
                        return Err(format!("textdet op {i}: convolution input channels disagree with weights"));
                    }
                    let out = Shape::new(*out_channels, window(x.h, kernel.0, stride.0, pad[0], pad[2])?,
                        window(x.w, kernel.1, stride.1, pad[1], pad[3])?)?;
                    if *groups == 1 && (*kernel != (1, 1) || *stride != (1, 1) || *pad != [0; 4]) {
                        // Each K row needs whole vec4s, so round the budget
                        // down before reserving scratch or choosing offsets.
                        let columns = (self.scratch_budget / (p.weight.cols as u64 * 16) * 4)
                            .min(out.stride as u64) as usize;
                        if columns == 0 {
                            return Err(format!("textdet op {i}: scratch budget cannot fit four im2col columns"));
                        }
                        scratch = Some(Shape::new(p.weight.cols, 1, columns)?);
                    }
                    out
                }
                Op::ConvTranspose { input, out_channels, kernel, stride, .. } => {
                    let x = shapes[*input];
                    let p = self.parameters[i].unwrap();
                    if x.c != p.weight.cols { return Err(format!("textdet op {i}: transposed convolution input channels disagree with weights")); }
                    let extent = |size: usize, stride: usize, kernel: usize| {
                        (size - 1).checked_mul(stride).and_then(|n| n.checked_add(kernel)).ok_or("textdet: transposed size overflow")
                    };
                    let out = Shape::new(*out_channels, extent(x.h, stride.0, kernel.0)?, extent(x.w, stride.1, kernel.1)?)?;
                    if kernel == stride { scratch = Some(Shape::new(p.weight.rows, x.h, x.w)?); }
                    out
                }
                Op::Add { a, b, .. } => {
                    if shapes[*a] != shapes[*b] { return Err(format!("textdet op {i}: add shapes disagree")); }
                    shapes[*a]
                }
                Op::Mul { x, s } => {
                    let (x, s) = (shapes[*x], shapes[*s]);
                    if x != s && (x.c != s.c || s.h != 1 || s.w != 1) { return Err(format!("textdet op {i}: mul shapes disagree")); }
                    x
                }
                Op::GlobalAvgPool { input } => Shape::new(shapes[*input].c, 1, 1)?,
                Op::Pool { input, kernel, stride, pad, .. } => {
                    let x = shapes[*input];
                    Shape::new(x.c, window(x.h, *kernel, *stride, *pad, *pad)?, window(x.w, *kernel, *stride, *pad, *pad)?)?
                }
                Op::UpsampleLike { input, like } => {
                    let (x, like) = (shapes[*input], shapes[*like]);
                    // Nearest-neighbour indexing multiplies these dimensions
                    // in the shader before dividing by the output extent.
                    product(x.h, like.h)?;
                    product(x.w, like.w)?;
                    Shape::new(x.c, like.h, like.w)?
                }
                Op::Upsample { input, scale } => {
                    let x = shapes[*input];
                    let out = Shape::new(x.c, product(x.h, *scale)?, product(x.w, *scale)?)?;
                    product(x.h, out.h)?;
                    product(x.w, out.w)?;
                    out
                }
                Op::Concat { inputs } => {
                    let first = shapes[*inputs.first().ok_or("textdet: empty concatenation")?];
                    let mut channels = 0usize;
                    for &v in inputs {
                        let x = shapes[v];
                        if (x.h, x.w) != (first.h, first.w) { return Err(format!("textdet op {i}: concat spatial sizes disagree")); }
                        channels = channels.checked_add(x.c).ok_or("textdet: concat channels overflow")?;
                    }
                    Shape::new(channels, first.h, first.w)?
                }
                Op::Activate { input, .. } => shapes[*input],
            };
            shapes.push(out);
            scratch_shapes.push(scratch);
        }

        let mut last: Vec<_> = (0..shapes.len()).collect();
        for (i, op) in self.ops.iter().enumerate() {
            for v in op.inputs() { last[v] = i + 1; }
        }
        last[self.ops.len()] = shapes.len();
        let mut retire = vec![Vec::new(); shapes.len()];
        for (v, end) in last.into_iter().enumerate() {
            if end < retire.len() { retire[end].push(v); }
        }
        let mut arena = Offsets::default();
        let mut values = Vec::with_capacity(shapes.len());
        let mut scratch = vec![None; self.ops.len()];
        for (i, shape) in shapes.into_iter().enumerate() {
            values.push(Value { shape, offset: arena.alloc(shape.bytes())? });
            if i > 0 {
                if let Some(shape) = scratch_shapes[i - 1] {
                    let value = Value { shape, offset: arena.alloc(shape.bytes())? };
                    scratch[i - 1] = Some(value);
                    arena.release(value);
                }
            }
            // Neither output nor scratch may overwrite a reader in this op.
            // The inter-op barrier makes reuse by the next operation safe.
            for &v in &retire[i] { arena.release(values[v]); }
        }
        Ok(Plan { values, scratch, bytes: arena.end })
    }

    pub fn run(&mut self, input: &Map) -> Result<Map> {
        let plan = self.plan(input)?;
        let first = plan.values[0];
        let output = *plan.values.last().unwrap();
        if self.arena.size < plan.bytes { self.arena = self.ctx.device.buffer(plan.bytes, BufferKind::Device)?; }
        let staging_bytes = first.shape.bytes().max(output.shape.bytes());
        if self.staging.size < staging_bytes { self.staging = self.ctx.device.buffer(staging_bytes, BufferKind::Staging)?; }
        if first.shape.plane() == first.shape.stride {
            self.ctx.device.upload(&self.staging, 0, bytes(&input.data))?;
        } else {
            let mut packed = vec![0.0; first.shape.len()];
            for (src, dst) in input.data.chunks(first.shape.plane()).zip(packed.chunks_mut(first.shape.stride)) {
                dst[..src.len()].copy_from_slice(src);
            }
            self.ctx.device.upload(&self.staging, 0, bytes(&packed))?;
        }
        self.stream.begin()?;
        self.stream.copy(&self.staging, 0, &self.arena, first.offset, first.shape.bytes());
        self.stream.barrier();
        for i in 0..self.ops.len() {
            self.record(i, &plan);
            self.stream.barrier();
        }
        self.stream.copy(&self.arena, output.offset, &self.staging, 0, output.shape.bytes());
        self.stream.submit_wait()?;
        let raw = self.ctx.device.download(&self.staging, 0, output.shape.bytes() as usize)?;
        let mut data = Vec::with_capacity(output.shape.c * output.shape.plane());
        for row in raw.chunks_exact(output.shape.stride * 4) {
            for v in row[..output.shape.plane() * 4].chunks_exact(4) {
                data.push(f32::from_ne_bytes(v.try_into().unwrap()));
            }
        }
        Ok(Map { c: output.shape.c, h: output.shape.h, w: output.shape.w, data })
    }

    fn gemm(&mut self, p: Parameters, x: Value, out: Value, act: Act, columns: Range<usize>) {
        let k = &self.ctx.kernels["textdet_gemm"];
        let dst = self.arena.address + out.offset;
        let args = with_act(k.args(), act)
            .buffer("a", self.weights.address + p.weight.offset)
            .buffer("b", self.arena.address + x.offset).buffer("dst", dst)
            .buffer("bias", p.bias.map_or(dst, |b| self.weights.address + b))
            .u32("m", out.shape.c as u32).u32("n", columns.len() as u32).u32("k", p.weight.cols as u32)
            .u32("astride", p.weight.stride as u32).u32("bstride", x.shape.stride as u32)
            .u32("dstride", out.shape.stride as u32).u32("col_offset", columns.start as u32)
            .u32("has_bias", u32::from(p.bias.is_some()));
        self.stream.dispatch(&args, groups(out.shape.c.div_ceil(32) * columns.len().div_ceil(32)));
    }

    fn record(&mut self, i: usize, plan: &Plan) {
        let out = plan.values[i + 1];
        let base = self.arena.address;
        let addr = |v: Value| base + v.offset;
        let dst = addr(out);
        match &self.ops[i] {
            Op::Conv { input, kernel, stride, pad, groups, act, .. } => {
                let x = plan.values[*input];
                let p = self.parameters[i].unwrap();
                if *groups != 1 {
                    let k = &self.ctx.kernels["textdet_depthwise"];
                    let args = with_act(convolution(maps(k.args(), x.shape, out.shape), *kernel, *stride, *pad), *act)
                        .buffer("x", addr(x)).buffer("dst", dst).buffer("w", self.weights.address + p.weight.offset)
                        .buffer("bias", p.bias.map_or(dst, |b| self.weights.address + b))
                        .u32("ws", p.weight.stride as u32).u32("multiplier", (out.shape.c / x.shape.c) as u32)
                        .u32("has_bias", u32::from(p.bias.is_some()));
                    self.stream.dispatch(&args, out.shape.groups());
                } else if let Some(patches) = plan.scratch[i] {
                    let (kernel, stride, pad, act) = (*kernel, *stride, *pad, *act);
                    for column in (0..out.shape.plane()).step_by(patches.shape.stride) {
                        // The preceding GEMM must finish reading scratch
                        // before the next im2col overwrites the same region.
                        if column != 0 { self.stream.barrier(); }
                        let width = patches.shape.stride.min(out.shape.stride - column);
                        let patches = Value { shape: Shape { w: width, stride: width, ..patches.shape }, ..patches };
                        let k = &self.ctx.kernels["textdet_im2col2d"];
                        let args = convolution(maps(k.args(), x.shape, out.shape), kernel, stride, pad)
                            .buffer("x", addr(x)).buffer("dst", addr(patches))
                            .u32("os", width as u32).u32("col_offset", column as u32);
                        self.stream.dispatch(&args, patches.shape.groups());
                        self.stream.barrier();
                        // Only the final chunk can have a partial vec4;
                        // GEMM clears its tail after applying the activation.
                        self.gemm(p, patches, out, act, column..(column + width).min(out.shape.plane()));
                    }
                } else {
                    self.gemm(p, x, out, *act, 0..out.shape.plane());
                }
            }
            Op::ConvTranspose { input, kernel, stride, act, .. } => {
                let x = plan.values[*input];
                let p = self.parameters[i].unwrap();
                let (kernel, stride, act) = (*kernel, *stride, *act);
                if let Some(taps) = plan.scratch[i] {
                    self.gemm(Parameters { bias: None, ..p }, x, taps, Act::None, 0..taps.shape.plane());
                    self.stream.barrier();
                    let k = &self.ctx.kernels["textdet_shuffle"];
                    let args = with_act(convolution(maps(k.args(), x.shape, out.shape), kernel, stride, [0; 4]), act)
                        .buffer("x", addr(taps)).buffer("dst", dst)
                        .buffer("bias", p.bias.map_or(dst, |b| self.weights.address + b))
                        .u32("has_bias", u32::from(p.bias.is_some()));
                    self.stream.dispatch(&args, out.shape.groups());
                } else {
                    let k = &self.ctx.kernels["textdet_transpose"];
                    let args = with_act(convolution(maps(k.args(), x.shape, out.shape), kernel, stride, [0; 4]), act)
                        .buffer("x", addr(x)).buffer("dst", dst).buffer("w", self.weights.address + p.weight.offset)
                        .buffer("bias", p.bias.map_or(dst, |b| self.weights.address + b))
                        .u32("ws", p.weight.stride as u32).u32("has_bias", u32::from(p.bias.is_some()));
                    self.stream.dispatch(&args, out.shape.groups());
                }
            }
            Op::Add { .. } | Op::Mul { .. } | Op::Activate { .. } => {
                let (x, other, mode, act) = match self.ops[i] {
                    Op::Add { a, b, act } => (a, b, 1, act),
                    Op::Mul { x, s } => (x, s, if plan.values[s].shape.plane() == 1 { 3 } else { 2 }, Act::None),
                    Op::Activate { input, act } => (input, input, 0, act),
                    _ => unreachable!(),
                };
                let k = &self.ctx.kernels["textdet_element"];
                let args = with_act(maps(k.args(), plan.values[x].shape, out.shape), act)
                    .buffer("x", addr(plan.values[x])).buffer("other", addr(plan.values[other])).buffer("dst", dst)
                    .u32("mode", mode);
                self.stream.dispatch(&args, out.shape.groups());
            }
            Op::GlobalAvgPool { input } => {
                let x = plan.values[*input];
                let k = &self.ctx.kernels["textdet_global_avg"];
                let args = k.args().buffer("x", addr(x)).buffer("dst", dst)
                    .u32("channels", x.shape.c as u32).u32("n", x.shape.plane() as u32).u32("xs", x.shape.stride as u32);
                self.stream.dispatch(&args, groups(x.shape.c));
            }
            Op::Pool { input, kernel, stride, pad, max } => {
                let x = plan.values[*input];
                let k = &self.ctx.kernels["textdet_pool"];
                let args = maps(k.args(), x.shape, out.shape).buffer("x", addr(x)).buffer("dst", dst)
                    .u32("kernel", *kernel as u32).u32("stride", *stride as u32).u32("pad", *pad as u32)
                    .u32("is_max", u32::from(*max)).f32("initial", if *max { f32::NEG_INFINITY } else { 0.0 });
                self.stream.dispatch(&args, out.shape.groups());
            }
            Op::Upsample { input, .. } | Op::UpsampleLike { input, .. } => {
                let x = plan.values[*input];
                let k = &self.ctx.kernels["textdet_nearest"];
                let args = maps(k.args(), x.shape, out.shape).buffer("x", addr(x)).buffer("dst", dst);
                self.stream.dispatch(&args, out.shape.groups());
            }
            Op::Concat { inputs } => {
                let mut offset = out.offset;
                for &v in inputs {
                    let x = plan.values[v];
                    self.stream.copy(&self.arena, x.offset, &self.arena, offset, x.shape.bytes());
                    offset += x.shape.bytes();
                }
            }
        }
    }
}
