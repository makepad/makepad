//! Oobleck decoder: one reusable Metal graph per padded tile shape. GEMMs
//! use half operands and f32 accumulation, with f32 Snake/residual states.
//! Transpose-convolution is two GEMMs plus an exact overlap-add/crop in IR.
//! Narrow output convolutions put time on the 64-wide GEMM axis and pad
//! weights to its 32-column tile, then crop whole contiguous channel rows.

use crate::backend::{metal, BufferStorageMode, DeviceGraphSession, DeviceRuntime, GraphDevice};
use crate::oobleck::YUE2_OOBLECK;
use crate::yue2::{YUE2_LATENT_DIM, YUE2_LATENT_HOP};
use crate::yue2_graph::{arena_context, bytes_f32, finish, leaf, padded, read_output, write_metal_tensor};
use crate::yue2_pipeline::{Yue2Progress, YUE2_VAE_CORE_FRAMES, YUE2_VAE_HALO_FRAMES};
use crate::{DiffusionError, Result};
use makepad_ai_common::sharded::ShardedSafetensors;
use makepad_ai_llm::{BufferUsage, Context, Graph, Op, TensorDesc, TensorId, TensorLayout, TensorType};
use makepad_ai_loader::quant::f32_to_f16_rn;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

const A: BufferUsage = BufferUsage::Activations;
// Includes weights, input, planned scratch and executor tails. A smaller
// core is selected BEFORE allocation if the planner exceeds this budget.
const ARENA_BYTES: usize = 1_500_000_000;
type IrResult<T> = std::result::Result<T, String>;

#[derive(Clone, Copy)]
enum Source {
    Conv { rows: usize },
    Up { input: usize, output: usize, stride: usize },
    Bias,
    Alpha,
    InvBeta,
}
struct Spec { name: String, shape: Vec<i64>, source: Source }

impl Spec {
    fn ty(&self) -> TensorType {
        match self.source {
            Source::Conv { .. } | Source::Up { .. } => TensorType::F16,
            _ => TensorType::F32,
        }
    }

    fn packed_shape(&self) -> Vec<i64> {
        let mut shape = self.shape.clone();
        if let Source::Conv { rows } = self.source {
            if rows < 64 {
                shape[2] = (rows.div_ceil(32) * 32) as i64;
            }
        }
        shape
    }

    fn bytes(&self) -> usize {
        let n = self.packed_shape().iter().product::<i64>() as usize;
        if matches!(self.source, Source::Up { .. }) {
            2 * padded(n) // two half-sized matrices, two bytes per element
        } else {
            padded(n * if self.ty() == TensorType::F16 { 2 } else { 4 })
        }
    }
}

fn specs() -> Vec<Spec> {
    let mut s = Vec::new();
    fn conv(s: &mut Vec<Spec>, p: &str, input: usize, output: usize, k: usize, transposed: bool, bias: bool) {
        let (shape, source) = if transposed {
            (vec![k as i64, output as i64, input as i64], Source::Up { input, output, stride: k / 2 })
        } else {
            (vec![k as i64, input as i64, output as i64], Source::Conv { rows: output })
        };
        s.push(Spec { name: format!("{p}.weight"), shape, source });
        if bias { s.push(Spec { name: format!("{p}.bias"), shape: vec![1, output as i64], source: Source::Bias }); }
    }
    fn snake(s: &mut Vec<Spec>, p: &str, ch: usize) {
        s.push(Spec { name: format!("{p}.alpha"), shape: vec![1, ch as i64], source: Source::Alpha });
        s.push(Spec { name: format!("{p}.beta"), shape: vec![1, ch as i64], source: Source::InvBeta });
    }
    let spec = YUE2_OOBLECK;
    conv(&mut s, "decoder.layers.0", spec.latent_dim, spec.channels * spec.mults[spec.strides.len()], 7, false, true);
    for (i, &stride) in spec.strides.iter().enumerate() {
        let input = spec.channels * spec.mults[spec.strides.len() - i];
        let output = spec.channels * spec.mults[spec.strides.len() - i - 1];
        let p = format!("decoder.layers.{}.layers", i + 1);
        snake(&mut s, &format!("{p}.0"), input);
        conv(&mut s, &format!("{p}.1"), input, output, 2 * stride, true, true);
        for u in 0..3 {
            let p = format!("{p}.{}.layers", u + 2);
            snake(&mut s, &format!("{p}.0"), output);
            conv(&mut s, &format!("{p}.1"), output, output, 7, false, true);
            snake(&mut s, &format!("{p}.2"), output);
            conv(&mut s, &format!("{p}.3"), output, output, 1, false, true);
        }
    }
    snake(&mut s, &format!("decoder.layers.{}", spec.strides.len() + 1), spec.channels);
    conv(&mut s, &format!("decoder.layers.{}", spec.strides.len() + 2), spec.channels, spec.audio_channels, 7, false, spec.out_bias);
    s
}

fn load_weights(state_bytes: usize) -> Result<(Context, HashMap<String, TensorId>)> {
    let specs = specs();
    let size: usize = specs.iter().map(Spec::bytes).sum();
    let mut ctx = arena_context(size + state_bytes, GraphDevice::Metal);
    let mut ids = HashMap::new();
    for s in specs {
        if let Source::Up { input, output, stride } = s.source {
            for part in ["lo", "hi"] {
                let name = format!("{}.{part}", s.name);
                let id = ctx.new_named_tensor(&name, TensorType::F16, 2,
                    &[input as i64, (stride * output) as i64], BufferUsage::Weights).map_err(DiffusionError::model)?;
                ids.insert(name, id);
            }
        } else {
            let shape = s.packed_shape();
            let id = ctx.new_named_tensor(&s.name, s.ty(), shape.len(), &shape, BufferUsage::Weights).map_err(DiffusionError::model)?;
            ids.insert(s.name, id);
        }
    }
    ctx.set_no_alloc(true);
    Ok((ctx, ids))
}

fn folded_weight(source: &ShardedSafetensors, s: &Spec, rows: usize) -> Result<Vec<f32>> {
    let p = s.name.strip_suffix(".weight").unwrap();
    let read = |old: &str, new: &str| -> Result<Vec<f32>> {
        let old = format!("{p}.{old}");
        let name = if source.has_tensor(&old) { old } else { format!("{p}.parametrizations.weight.{new}") };
        source.tensor_f32(&name, "yue2 graph VAE")
    };
    let g = read("weight_g", "original0")?;
    let mut v = read("weight_v", "original1")?;
    let expected = s.shape.iter().product::<i64>() as usize;
    if g.len() != rows || v.len() != expected {
        return Err(DiffusionError::model(format!("yue2 graph VAE {} weight-norm shape", s.name)));
    }
    // Identical f64 WN folding to OobleckDecoder, before packing either half.
    for (r, row) in v.chunks_exact_mut(expected / rows).enumerate() {
        let sum: f64 = row.iter().map(|&x| (x as f64) * (x as f64)).sum();
        let scale = g[r] as f64 / sum.sqrt().max(1e-30);
        for x in row { *x = (*x as f64 * scale) as f32; }
    }
    Ok(v)
}

fn half_bytes(values: impl Iterator<Item = f32>) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(values.size_hint().0 * 2);
    for value in values {
        let half = f32_to_f16_rn(value);
        if !value.is_finite() || half & 0x7c00 == 0x7c00 {
            return Err(DiffusionError::model("yue2 graph VAE nonfinite/overflowed half weight"));
        }
        bytes.extend_from_slice(&half.to_le_bytes());
    }
    Ok(bytes)
}

fn upload_weights(source: &ShardedSafetensors, ctx: &Context, ids: &HashMap<String, TensorId>, session: &DeviceGraphSession) -> Result<()> {
    for s in specs() {
        match s.source {
            Source::Up { input, output, stride } => {
                let v = folded_weight(source, &s, input)?;
                for (part, shift) in [("lo", 0), ("hi", stride)] {
                    // W_part[r*out_ch + o, c] = W[c, o, r + shift].
                    // GGML extents reverse the logical matrix dimensions.
                    let bytes = half_bytes((0..stride * output * input).map(|i| {
                        let c = i % input;
                        let o = i / input % output;
                        let r = i / (input * output);
                        v[(c * output + o) * (2 * stride) + r + shift]
                    }))?;
                    write_metal_tensor(session, ctx, ids[&format!("{}.{part}", s.name)], &bytes)?;
                }
            }
            Source::Conv { rows } => {
                let v = folded_weight(source, &s, rows)?;
                let mut bytes = half_bytes(v.into_iter())?;
                // Fold only real channels; dummy GEMM columns are exact zero.
                bytes.resize(s.packed_shape().iter().product::<i64>() as usize * 2, 0);
                write_metal_tensor(session, ctx, ids[&s.name], &bytes)?;
            }
            _ => {
                let mut v = source.tensor_f32(&s.name, "yue2 graph VAE")?;
                match s.source {
                    Source::Alpha => v.iter_mut().for_each(|x| *x = x.exp()),
                    Source::InvBeta => v.iter_mut().for_each(|x| *x = 1.0 / (x.exp() + 1e-9)),
                    _ => {}
                }
                write_metal_tensor(session, ctx, ids[&s.name], bytes_f32(&v))?;
            }
        }
    }
    Ok(())
}

fn up_length(frames: usize, stride: usize) -> usize {
    (frames - 1) * stride + 2 * stride - 2 * stride.div_ceil(2)
}

// One host write contains the padded latents and a validity row at every
// sample rate. Zeroing only the latents is insufficient: conv biases would
// create a fictitious continuation that leaks back across the true end.
struct TileInput { frames: usize, masks: Vec<(usize, usize)>, elements: usize }

impl TileInput {
    fn new(frames: usize) -> Self {
        // Even a short final/song tile must have ne11 > 8 for mul_mm.
        // Validity masks preserve the true boundary through every rate.
        let frames = frames.max(32);
        let mut len = frames;
        let mut elements = frames * YUE2_LATENT_DIM;
        let mut masks = vec![(elements, len)];
        elements += len;
        for &stride in YUE2_OOBLECK.strides {
            len = up_length(len, stride);
            // Binary kernels may load float4 when len is divisible by four.
            elements = padded(elements * 4) / 4;
            masks.push((elements, len));
            elements += len;
        }
        Self { frames, masks, elements }
    }

    fn pack(&self, latents: &[f32], packed: &mut [f32]) {
        packed.fill(0.0);
        packed[..latents.len()].copy_from_slice(latents);
        let mut valid = latents.len() / YUE2_LATENT_DIM;
        for (i, &(offset, len)) in self.masks.iter().enumerate() {
            if i != 0 { valid = up_length(valid, YUE2_OOBLECK.strides[i - 1]); }
            debug_assert!(valid <= len);
            packed[offset..offset + valid].fill(1.0);
        }
    }
}

struct Builder<'a> { ctx: &'a mut Context, ids: &'a HashMap<String, TensorId> }

impl Builder<'_> {
    fn id(&self, name: &str) -> IrResult<TensorId> { self.ids.get(name).copied().ok_or_else(|| format!("missing VAE graph weight {name}")) }

    fn unary(&mut self, x: TensorId, op: Op) -> IrResult<TensorId> {
        let shape = self.ctx.tensor(x).ok_or("missing VAE activation")?.ne;
        let desc = TensorDesc::new(TensorType::F32, TensorLayout::for_ggml(TensorType::F32, &shape)?, A);
        self.ctx.new_op_tensor(desc, op, &[x])
    }

    fn snake(&mut self, x: TensorId, p: &str) -> IrResult<TensorId> {
        let alpha = self.id(&format!("{p}.alpha"))?;
        let beta = self.id(&format!("{p}.beta"))?;
        let y = self.ctx.binary_like_a(Op::Mul, x, alpha, A)?;
        let y = self.unary(y, Op::Sin)?;
        let y = self.unary(y, Op::Sqr)?;
        let y = self.ctx.binary_like_a(Op::Mul, y, beta, A)?;
        self.ctx.binary_like_a(Op::Add, x, y, A)
    }

    fn matmul(&mut self, a: TensorId, b: TensorId, name: &str) -> IrResult<TensorId> {
        let src0 = self.ctx.tensor(a).ok_or("missing GEMM src0")?;
        let src1 = self.ctx.tensor(b).ok_or("missing GEMM src1")?;
        // metal_selector::program_mul_mat accepts BOTH f16 and f32 src1.
        // The shader instantiates mul_mm_f16_f16 as well as mul_mm_f16_f32;
        // both stage half operands and accumulate into f32. Keep half im2col
        // scratch, and check the actual shape/stride requirements here.
        if src0.ne[0] != src1.ne[0] || src0.ne[0] < 64 || src0.ne[0] % 32 != 0
            || src1.ne[1] <= 8 || !src0.is_contiguous() || !src1.is_contiguous()
            || src0.is_transposed() || src1.is_transposed()
            || src0.desc.ty != TensorType::F16
            || !matches!(src1.desc.ty, TensorType::F16 | TensorType::F32)
        {
            return Err(format!("VAE GEMM {name} violates mul_mm geometry: src0={} {:?} {:?}, src1={} {:?} {:?}",
                src0.desc.ty.name(), src0.ne, src0.nb, src1.desc.ty.name(), src1.ne, src1.nb));
        }
        let y = self.ctx.mul_mat(a, b, A)?;
        self.ctx.tensor_mut(y).unwrap().set_name(name);
        Ok(y)
    }

    fn conv(&mut self, x: TensorId, p: &str, channels: usize, dilation: usize, bias: bool, mask: TensorId) -> IrResult<TensorId> {
        let w = self.id(&format!("{p}.weight"))?;
        let shape = self.ctx.tensor(w).ok_or("missing convolution")?.ne;
        let cols = if shape[0] == 1 {
            // A pointwise convolution is a GEMM; no im2col slab is needed.
            let x = self.ctx.transpose(x)?;
            self.ctx.cont(x)?
        } else {
            // The existing simdgroup GEMM already rounds both operands to
            // half. Materialize columns at that precision to halve scratch.
            self.ctx.im2col(w, x, 1, 1, ((shape[0] - 1) * dilation as i64 / 2) as i32, 0,
                dilation as i32, 1, false, TensorType::F16, A)?
        };
        let w = self.ctx.reshape(w, &[shape[0] * shape[1], shape[2]])?;
        let mut y = if channels < 64 {
            // [K,T] x [K,32] -> [T,32]. Time fills the 64-wide axis;
            // stereo occupies two whole rows, so cropping needs no copy.
            // Using [K,2] as src1 would select mul_mv_ext instead of mul_mm.
            let y = self.matmul(cols, w, &format!("{p}.gemm.time_major"))?;
            let len = self.ctx.tensor(y).unwrap().ne[0];
            self.ctx.view_2d(y, len, channels as i64, len as usize * 4, 0)?
        } else {
            let y = self.matmul(w, cols, &format!("{p}.gemm"))?;
            let y = self.ctx.transpose(y)?;
            self.ctx.cont(y)?
        };
        if bias {
            let bias = self.id(&format!("{p}.bias"))?;
            y = self.ctx.binary_like_a(Op::Add, y, bias, A)?;
        }
        self.ctx.binary_like_a(Op::Mul, y, mask, A)
    }

    fn up(&mut self, x: TensorId, p: &str, stride: usize, mask: TensorId) -> IrResult<TensorId> {
        let w_lo = self.id(&format!("{p}.weight.lo"))?;
        let w_hi = self.id(&format!("{p}.weight.hi"))?;
        let len = self.ctx.tensor(x).ok_or("missing upsample input")?.ne[0];
        let ch = self.ctx.tensor(w_lo).ok_or("missing upsample weight")?.ne[1] / stride as i64;
        let x = self.ctx.transpose(x)?;
        let x = self.ctx.cont(x)?;
        let lo = self.matmul(w_lo, x, &format!("{p}.gemm.lo"))?;
        let hi = self.matmul(w_hi, x, &format!("{p}.gemm.hi"))?;
        let s = stride as i64;
        let lo = self.ctx.reshape(lo, &[ch, len * s])?;
        let hi = self.ctx.reshape(hi, &[ch, len * s])?;
        let row = ch as usize * 4;
        // Unpadded frame t*s+r = lo[t,r] + hi[t-1,r]. The first
        // stride has only lo, the last only hi; neither is discarded.
        let y = if len == 1 {
            self.ctx.concat(lo, hi, 1, A)?
        } else {
            let first = self.ctx.view_2d(lo, ch, s, row, 0)?;
            let next = self.ctx.view_2d(lo, ch, (len - 1) * s, row, stride * row)?;
            let prev = self.ctx.view_2d(hi, ch, (len - 1) * s, row, 0)?;
            let middle = self.ctx.binary_like_a(Op::Add, next, prev, A)?;
            let last = self.ctx.view_2d(hi, ch, s, row, (len as usize - 1) * stride * row)?;
            let y = self.ctx.concat(first, middle, 1, A)?;
            self.ctx.concat(y, last, 1, A)?
        };
        let padding = stride.div_ceil(2);
        let n = up_length(len as usize, stride);
        let y = self.ctx.view_2d(y, ch, n as i64, row, padding * row)?;
        let y = self.ctx.transpose(y)?;
        let y = self.ctx.cont(y)?;
        // Bias belongs to every retained sample, including the hi-only tail.
        let bias = self.id(&format!("{p}.bias"))?;
        let y = self.ctx.binary_like_a(Op::Add, y, bias, A)?;
        self.ctx.binary_like_a(Op::Mul, y, mask, A)
    }
}

struct TileGraph {
    graph: Graph,
    stages: Vec<(String, TensorId)>,
    input: TensorId,
    output: TensorId,
}

fn build_tile(ctx: &mut Context, ids: &HashMap<String, TensorId>, layout: &TileInput) -> IrResult<TileGraph> {
    let input = leaf(ctx, TensorType::F32, &[layout.elements as i64])?;
    let x = ctx.view_2d(input, YUE2_LATENT_DIM as i64, layout.frames as i64, YUE2_LATENT_DIM * 4, 0)?;
    let masks = layout.masks.iter().map(|&(offset, len)| {
        ctx.view_2d(input, len as i64, 1, len * 4, offset * 4)
    }).collect::<IrResult<Vec<_>>>()?;
    let mut b = Builder { ctx, ids };
    let mut stages = Vec::new();
    let x = b.ctx.transpose(x)?;
    let x = b.ctx.cont(x)?;
    let spec = YUE2_OOBLECK;
    let mut x = b.conv(x, "decoder.layers.0", spec.channels * spec.mults[spec.strides.len()], 1, true, masks[0])?;
    stages.push(("input conv".into(), x));
    for (i, &stride) in YUE2_OOBLECK.strides.iter().enumerate() {
        let p = format!("decoder.layers.{}.layers", i + 1);
        let channels = spec.channels * spec.mults[spec.strides.len() - i - 1];
        let mask = masks[i + 1];
        x = b.snake(x, &format!("{p}.0"))?;
        x = b.up(x, &format!("{p}.1"), stride, mask)?;
        for (u, dilation) in [1, 3, 9].into_iter().enumerate() {
            let p = format!("{p}.{}.layers", u + 2);
            let y = b.snake(x, &format!("{p}.0"))?;
            let y = b.conv(y, &format!("{p}.1"), channels, dilation, true, mask)?;
            let y = b.snake(y, &format!("{p}.2"))?;
            let y = b.conv(y, &format!("{p}.3"), channels, 1, true, mask)?;
            x = b.ctx.binary_like_a(Op::Add, x, y, A)?;
        }
        stages.push((format!("block={i} stride={stride}"), x));
    }
    x = b.snake(x, &format!("decoder.layers.{}", YUE2_OOBLECK.strides.len() + 1))?;
    x = b.conv(x, &format!("decoder.layers.{}", YUE2_OOBLECK.strides.len() + 2), spec.audio_channels, 1,
        YUE2_OOBLECK.out_bias, *masks.last().unwrap())?;
    x = b.ctx.transpose(x)?;
    let output = b.ctx.cont(x)?;
    stages.push(("output conv".into(), output));
    // Only the final audio is pinned. Stage endpoints are timing markers,
    // never readback leaves: their storage must be reusable by later blocks.
    let graph = finish(b.ctx, output)?;
    Ok(TileGraph { graph, stages, input, output })
}

fn audit_gemms(ctx: &Context, prepared: &metal::MetalPreparedGraph, verbose: bool) -> IrResult<()> {
    let mut count = 0;
    for node in &prepared.nodes {
        let tensor = ctx.tensor(node.node_id).ok_or("missing prepared VAE node")?;
        if tensor.op != Op::MulMat { continue; }
        count += 1;
        let kernel = node.stages.first().ok_or("missing VAE GEMM kernel")?;
        if node.stages.len() != 1 || !kernel.descriptor.base_name.starts_with("kernel_mul_mm_") {
            return Err(format!("VAE GEMM {} selected {} instead of mul_mm (simdgroup_mm={})",
                tensor.name().unwrap_or("<unnamed>"), kernel.descriptor.cache_name, prepared.features.has_simdgroup_mm));
        }
        if verbose {
            let a = ctx.tensor(tensor.src[0].ok_or("missing GEMM src0")?).ok_or("missing GEMM src0")?;
            let b = ctx.tensor(tensor.src[1].ok_or("missing GEMM src1")?).ok_or("missing GEMM src1")?;
            eprintln!("yue2 graph VAE GEMM {}: src0={} {:?} src1={} {:?} dst={} {:?} kernel={}",
                tensor.name().unwrap_or("<unnamed>"), a.desc.ty.name(), a.ne, b.desc.ty.name(), b.ne,
                tensor.desc.ty.name(), tensor.ne, kernel.descriptor.cache_name);
        }
    }
    eprintln!("yue2 graph VAE GEMM audit: {count}/{count} mul_mm, contiguous operands, K divisible by 32, src1 columns > 8");
    Ok(())
}

fn profile_tensor(ctx: &Context, id: TensorId) -> String {
    let Some(t) = ctx.tensor(id) else { return format!("{id}:<missing>"); };
    format!("{id}:{}:{} ne={:?} nb={:?} cont={} trans={}",
        t.name().unwrap_or("<unnamed>"), t.desc.ty.name(), t.ne, t.nb, t.is_contiguous(), t.is_transposed())
}

fn profile_tile(
    session: &metal::MetalGraphSession,
    ctx: &Context,
    writes: &[metal::MetalGraphTensorWrite<'_>],
    g: &TileGraph,
    stage_totals: &mut [(f64, f64)],
) -> IrResult<metal::MetalGraphExecution> {
    let nodes = &session.compiled().nodes;
    let ends: Vec<_> = nodes.iter().map(|n| n.output_id).collect();
    let mut stage = 0;
    let mut node_stages = Vec::with_capacity(nodes.len());
    for node in nodes {
        if stage >= g.stages.len() { return Err("VAE profile stage ends before graph".into()); }
        node_stages.push(stage);
        if node.output_id == g.stages[stage].1 { stage += 1; }
    }
    if stage != g.stages.len() { return Err("VAE profile stage endpoint was fused away".into()); }
    let mut timings = Vec::with_capacity(nodes.len());
    eprintln!("yue2 graph VAE tile=1 node profile: {} nodes, one completed command buffer per node; diagnostic timings, not throughput", nodes.len());
    // Same compiled nodes, buffers, reuse plan and ordering as execute.
    // Endpoints do not pin intermediates or add any activation readback.
    let result = session.execute_profiled(ctx, writes, &[g.output], &ends, &mut |_, wall, gpu| {
        let index = timings.len();
        stage_totals[node_stages[index]].0 += wall;
        stage_totals[node_stages[index]].1 += gpu;
        timings.push((index, wall, gpu));
    })?;
    for ((name, _), &(wall, gpu)) in g.stages.iter().zip(stage_totals.iter()) {
        eprintln!("yue2 graph VAE tile=1 {name} node-sum wall={wall:.3}s gpu={gpu:.3}s");
    }
    timings.sort_by(|a, b| b.2.total_cmp(&a.2));
    eprintln!("yue2 graph VAE tile=1 slowest 20 nodes (GPU ms; GGML fastest-first extents/byte strides):");
    for (rank, &(index, wall, gpu)) in timings.iter().take(20).enumerate() {
        let node = &nodes[index];
        let tensor = ctx.tensor(node.node_id).ok_or("missing profiled VAE node")?;
        let kernels = node.stages.iter().map(|s| s.descriptor.cache_name.as_str()).collect::<Vec<_>>().join(",");
        eprintln!("yue2 graph VAE slow #{} node={} output={} stage={} op={} name={} gpu_ms={:.3} wall_ms={:.3} kernels=[{kernels}]",
            rank + 1, node.node_id, node.output_id, g.stages[node_stages[index]].0, tensor.op.name(),
            tensor.name().unwrap_or("<unnamed>"), gpu * 1e3, wall * 1e3);
        for (slot, id) in tensor.src.iter().enumerate() {
            if let Some(id) = id { eprintln!("  src{slot} {}", profile_tensor(ctx, *id)); }
        }
        for &id in &node.fuse_src_ids { eprintln!("  fused-src {}", profile_tensor(ctx, id)); }
        eprintln!("  dst {}", profile_tensor(ctx, node.output_id));
    }
    Ok(result)
}

pub struct Yue2GraphVae { weights: ShardedSafetensors }

impl Yue2GraphVae {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let runtime = DeviceRuntime::new()?;
        if runtime.device() != GraphDevice::Metal {
            return Err(DiffusionError::model("yue2 graph VAE requires Metal (CUDA graph executor has no Im2col/Sin)"));
        }
        let weights = ShardedSafetensors::load(dir.as_ref().join("model.safetensors"), "yue2 graph VAE")?;
        Ok(Self { weights })
    }

    pub fn natural_output_length(frames: usize) -> usize {
        YUE2_OOBLECK.strides.iter().fold(frames, |n, &s| up_length(n, s))
    }

    pub fn decode_prefix(&self, latents: &[f32], output_frames: usize, progress: &mut dyn FnMut(Yue2Progress) -> Result<()>) -> Result<Vec<f32>> {
        let began = Instant::now();
        if latents.len() % YUE2_LATENT_DIM != 0 || output_frames == 0 || output_frames > latents.len() / YUE2_LATENT_DIM {
            return Err(DiffusionError::workflow("yue2 graph VAE latent shape"));
        }
        let frames = latents.len() / YUE2_LATENT_DIM;
        let halo = YUE2_VAE_HALO_FRAMES;
        let profile = matches!(std::env::var("MAKEPAD_YUE2_VAE_PROFILE").as_deref(), Ok("1"));
        // Keep the resident decoder Send; the runtime is owned by this call.
        let DeviceRuntime::Metal(runtime) = DeviceRuntime::new()? else {
            return Err(DiffusionError::model("yue2 graph VAE requires Metal"));
        };
        let mut core = YUE2_VAE_CORE_FRAMES.min(output_frames);
        let (ctx, ids, layout, g, prepared) = loop {
            let layout = TileInput::new(frames.min(core + 2 * halo));
            let (mut ctx, ids) = load_weights(padded(layout.elements * 4))?;
            let g = build_tile(&mut ctx, &ids, &layout).map_err(DiffusionError::model)?;
            let prepared = metal::prepare_graph_with_reuse_cooldown(&ctx, &g.graph, runtime.features(), 0)
                .map_err(DiffusionError::model)?;
            let bytes = prepared.main_buffer_size + prepared.tail_buffer_size;
            eprintln!("yue2 graph VAE plan: core={core} tile={} arena={bytes} budget={ARENA_BYTES} bytes nodes={}", layout.frames, prepared.nodes.len());
            if bytes <= ARENA_BYTES {
                break (ctx, ids, layout, g, prepared);
            }
            if core == 1 {
                return Err(DiffusionError::model(format!("yue2 graph VAE minimum tile arena {bytes} exceeds budget {ARENA_BYTES}")));
            }
            core = core.div_ceil(2);
            eprintln!("yue2 graph VAE: reducing core to {core} to reuse a smaller arena; all latents retained");
        };
        audit_gemms(&ctx, &prepared, profile).map_err(DiffusionError::model)?;
        let weights: usize = specs().iter().map(Spec::bytes).sum();
        let buffers = metal::MetalContextBuffers {
            ro_buffer: Some(runtime.create_buffer(weights, BufferStorageMode::Shared).map_err(DiffusionError::model)?),
            ro_split: weights,
            main_buffer: runtime.create_buffer(prepared.main_buffer_size - weights, BufferStorageMode::Shared)
                .map_err(DiffusionError::model)?,
        };
        let session = DeviceGraphSession::Metal(metal::MetalGraphSession::from_runtime_with_context_buffers(
            runtime, &prepared, &buffers, BufferStorageMode::Shared,
        ).map_err(DiffusionError::model)?);
        eprintln!("yue2 graph VAE compile/plan: {:.3}s weights={weights} scratch={} input={} bytes",
            began.elapsed().as_secs_f64(), prepared.main_buffer_size - ctx.used_mem(), layout.elements * 4);
        let upload = Instant::now();
        upload_weights(&self.weights, &ctx, &ids, &session)?;
        eprintln!("yue2 graph VAE weight fold/pack/upload: {:.3}s", upload.elapsed().as_secs_f64());
        let DeviceGraphSession::Metal(session) = session else { unreachable!() };
        let total = Self::natural_output_length(frames).min(output_frames * YUE2_LATENT_HOP);
        let mut audio = vec![0.0f32; total * 2];
        let mut packed = vec![0.0f32; layout.elements];
        let ends: Vec<_> = g.stages.iter().map(|(_, id)| *id).collect();
        let mut stage_totals = vec![(0.0f64, 0.0f64); ends.len()];
        let tiles = output_frames.div_ceil(core);
        let mut run_seconds = 0.0;
        eprintln!("yue2 graph VAE: {output_frames}/{frames} core/input latents, core={core} halo={halo}, {tiles} tiles, one graph, exact crops, profile={profile}");
        for (i, start) in (0..output_frames).step_by(core).enumerate() {
            let tile_began = Instant::now();
            let end = (start + core).min(output_frames);
            let left = start.saturating_sub(halo);
            let right = (end + halo).min(frames);
            layout.pack(&latents[left * YUE2_LATENT_DIM..right * YUE2_LATENT_DIM], &mut packed);
            let writes = [metal::MetalGraphTensorWrite { tensor_id: g.input, bytes: bytes_f32(&packed) }];
            let run_began = Instant::now();
            let result = if profile && i == 0 {
                profile_tile(&session, &ctx, &writes, &g, &mut stage_totals)
            } else if profile {
                session.execute_profiled(&ctx, &writes, &[g.output], &ends, &mut |id, wall, gpu| {
                    let stage = ends.iter().position(|&end| end == id).unwrap();
                    stage_totals[stage].0 += wall;
                    stage_totals[stage].1 += gpu;
                    eprintln!("yue2 graph VAE tile={}/{} {} wall={wall:.3}s gpu={gpu:.3}s", i + 1, tiles, g.stages[stage].0);
                })
            } else {
                session.execute(&ctx, &writes, &[g.output])
            }.map_err(DiffusionError::model)?;
            let run = run_began.elapsed().as_secs_f64();
            run_seconds += run;
            let tile = read_output(&result.outputs[&g.output]);
            let out_start = start * YUE2_LATENT_HOP;
            let out_end = (end * YUE2_LATENT_HOP).min(total);
            let crop = (start - left) * YUE2_LATENT_HOP;
            let count = out_end - out_start;
            if crop + count > Self::natural_output_length(right - left) || (crop + count) * 2 > tile.len() {
                return Err(DiffusionError::model("yue2 graph VAE tile does not cover core"));
            }
            for (dst, &src) in audio[out_start * 2..out_end * 2].iter_mut().zip(&tile[crop * 2..(crop + count) * 2]) {
                *dst = src.clamp(-1.0, 1.0);
            }
            eprintln!("yue2 graph VAE tile={}/{} core={start}..{end} input={} padded={} execute={run:.3}s total={:.3}s",
                i + 1, tiles, right - left, layout.frames, tile_began.elapsed().as_secs_f64());
            progress(Yue2Progress::Decode { done: i + 1, total: tiles })?;
        }
        if profile {
            for ((name, _), (wall, gpu)) in g.stages.iter().zip(stage_totals) {
                eprintln!("yue2 graph VAE stage total {name}: wall={wall:.3}s gpu={gpu:.3}s");
            }
        }
        eprintln!("yue2 graph VAE total: {:.3}s execute={run_seconds:.3}s tiles={tiles} latents={frames}", began.elapsed().as_secs_f64());
        Ok(audio)
    }
}
