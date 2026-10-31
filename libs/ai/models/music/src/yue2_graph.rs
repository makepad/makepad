//! YuE2 MoT expressed in the ggml IR. The host owns sampling and the ODE;
//! graph families share one device arena for prefill, decode and NAR state.
//! No imperative `gpu_*` operation participates in this implementation.

use crate::backend::{metal, BufferStorageMode, DeviceGraphSession, DeviceRuntime, GraphDevice};
use crate::yue2::*;
use crate::{DiffusionError, Result};
use makepad_ai_llm::{
    BufferUsage, Context, GluOp, Graph, InitParams, Op, TensorDesc, TensorId,
    TensorLayout, TensorType, UnaryOp, GGML_MEM_ALIGN, GGML_ROPE_TYPE_NEOX,
};
use std::collections::HashMap;

type IrResult<T> = std::result::Result<T, String>;
const A: BufferUsage = BufferUsage::Activations;
const KV: usize = YUE2_KV_HEADS * YUE2_HEAD_DIM;
// A shape bucket, not a context limit. The model's actual limit is 24576.
const CACHE_BUCKET: usize = 256;
// Work sizes, never sequence limits. The last block keeps its real length.
const AR_BLOCK_ROWS: usize = 256;
const NAR_BLOCK_ROWS: usize = 512;
// Upload staging size, not a tensor limit; larger tensors stream in full.
const UPLOAD_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn bytes_f32(values: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

fn bytes_i32(values: &[i32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

pub(crate) fn read_output(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

pub(crate) fn bf16(v: f32) -> f32 {
    if !v.is_finite() { return v; }
    let bits = v.to_bits();
    f32::from_bits(bits.wrapping_add(0x7fff + ((bits >> 16) & 1)) & 0xffff_0000)
}

pub(crate) fn padded(bytes: usize) -> usize {
    bytes.div_ceil(GGML_MEM_ALIGN) * GGML_MEM_ALIGN
}

pub(crate) fn leaf(ctx: &mut Context, ty: TensorType, shape: &[i64]) -> IrResult<TensorId> {
    let old = ctx.get_no_alloc();
    ctx.set_no_alloc(false);
    let result = ctx.new_tensor(ty, shape.len(), shape, BufferUsage::State);
    ctx.set_no_alloc(old);
    result
}

pub(crate) fn cast(ctx: &mut Context, x: TensorId, ty: TensorType) -> IrResult<TensorId> {
    let t = ctx.tensor(x).ok_or("missing cast source")?;
    let layout = TensorLayout::for_ggml(ty, &t.ne)?;
    ctx.new_op_tensor(TensorDesc::new(ty, layout, A), Op::Cont, &[x])
}

pub(crate) fn finish(ctx: &Context, output: TensorId) -> IrResult<Graph> {
    let mut graph = Graph::new();
    graph.build_forward_expand(ctx, output)?;
    graph.add_leaf(output);
    Ok(graph)
}

/// A Metal context describes offsets only. Upload one tensor at a time
/// after compilation, avoiding a second resident copy of weights and KV.
pub(crate) fn arena_context(bytes: usize, device: GraphDevice) -> Context {
    Context::new(InitParams {
        mem_size: bytes,
        mem_buffer: (device == GraphDevice::Metal).then(Vec::new),
        no_alloc: false,
    })
}

pub(crate) fn write_metal_tensor(session: &DeviceGraphSession, ctx: &Context, id: TensorId, bytes: &[u8]) -> Result<()> {
    let DeviceGraphSession::Metal(session) = session else {
        return Err(DiffusionError::model("expected a Metal graph upload"));
    };
    let tensor = ctx.tensor(id).ok_or_else(|| DiffusionError::model("missing graph upload tensor"))?;
    if tensor.nbytes() != bytes.len() {
        return Err(DiffusionError::model("graph upload byte count mismatch"));
    }
    let offset = tensor.data_offset.ok_or_else(|| DiffusionError::model("graph upload needs an allocated leaf"))?;
    let compiled = session.compiled();
    let (buffer, offset) = if offset < compiled.ro_split {
        (compiled.ro_buffer.as_ref().ok_or_else(|| DiffusionError::model("missing graph weight buffer"))?, offset)
    } else { (&compiled.main_buffer, offset - compiled.ro_split) };
    for (i, chunk) in bytes.chunks(UPLOAD_BYTES).enumerate() {
        session.runtime().write_buffer(buffer, offset + i * UPLOAD_BYTES, chunk).map_err(DiffusionError::model)?;
        // Completed command buffers retain blit staging until the runtime
        // retires them. Do not accumulate a second checkpoint in staging.
        session.runtime().wait_idle().map_err(DiffusionError::model)?;
    }
    Ok(())
}

pub(crate) fn compile_family(runtime: &DeviceRuntime, ctx: &Context, graphs: &[(&str, &Graph, &[TensorId])]) -> Result<Vec<DeviceGraphSession>> {
    let weights: usize = ctx.tensors().iter().filter(|t| t.op == Op::None && t.desc.usage == BufferUsage::Weights).map(|t| padded(t.nbytes())).sum();
    match runtime {
        DeviceRuntime::Metal(runtime) => {
            let prepared = graphs.iter().map(|(_, g, outputs)| {
                let mut g = (*g).clone();
                for &id in *outputs { g.add_leaf(id); }
                metal::prepare_graph(ctx, &g, runtime.features()).map_err(DiffusionError::model)
            }).collect::<Result<Vec<_>>>()?;
            let logical_size = prepared.iter().map(|p| p.main_buffer_size).max().unwrap_or(0);
            let size = logical_size.saturating_sub(weights);
            let tails: usize = prepared.iter().map(|p| p.tail_buffer_size).sum();
            for ((name, _, _), p) in graphs.iter().zip(&prepared) {
                eprintln!("yue2 graph memory {name}: weights={weights} state={} scratch={} arena={} tail_buffer={} bytes",
                    ctx.used_mem().saturating_sub(weights), p.main_buffer_size.saturating_sub(ctx.used_mem()), p.main_buffer_size.saturating_sub(weights), p.tail_buffer_size);
            }
            eprintln!("yue2 graph family: shared_weights={weights} shared_main={size} tails={tails} device_total={} host_arena={} bytes", logical_size + tails, ctx.mem_buffer().len());
            if prepared.is_empty() { return Ok(Vec::new()); }
            let buffer = runtime.create_buffer(size.max(1), BufferStorageMode::Shared).map_err(DiffusionError::model)?;
            // The compiled address space is logical: read-only weights
            // below this split, mutable state/scratch above it. Both are
            // GPU-owned allocations; no host checkpoint is kept alive.
            let buffers = metal::MetalContextBuffers {
                ro_buffer: Some(runtime.create_buffer(weights, BufferStorageMode::Shared).map_err(DiffusionError::model)?),
                ro_split: weights,
                main_buffer: buffer,
            };
            // Leaves may include masked, unwritten cache rows. Initialize
            // them without ever allocating a host-sized cache/weight image.
            let zero = vec![0u8; ctx.used_mem().saturating_sub(weights).min(UPLOAD_BYTES)];
            if !zero.is_empty() {
                for offset in (weights..ctx.used_mem()).step_by(zero.len()) {
                    let len = zero.len().min(ctx.used_mem() - offset);
                    runtime.write_buffer(&buffers.main_buffer, offset - weights, &zero[..len]).map_err(DiffusionError::model)?;
                    runtime.wait_idle().map_err(DiffusionError::model)?;
                }
            }
            prepared.iter().map(|p| {
                metal::MetalGraphSession::from_runtime_with_context_buffers(runtime.clone(), p, &buffers, BufferStorageMode::Shared)
                    .map(DeviceGraphSession::Metal).map_err(DiffusionError::model)
            }).collect()
        }
        DeviceRuntime::Cuda(_) => {
            let family: Vec<_> = graphs.iter().map(|(_, g, o)| (*g, *o)).collect();
            let sessions = runtime.compile_graphs(ctx, &family)?;
            for ((name, _, _), session) in graphs.iter().zip(&sessions) {
                if let DeviceGraphSession::Cuda(s) = session {
                    eprintln!("yue2 graph memory {name}: shared_device={} weights={weights} state={} host_arena={} bytes, nodes={}",
                        s.device_bytes(), ctx.used_mem().saturating_sub(weights), ctx.mem_buffer().len(), s.node_count());
                }
            }
            Ok(sessions)
        }
    }
}

struct WeightSpec {
    name: String,
    shape: Vec<i64>,
    float: bool,
    q8: bool,
    rows: Option<(u64, u64)>,
}

fn weight_specs(nar_rows: usize, phase: Option<Yue2Phase>) -> Vec<WeightSpec> {
    let mut specs = Vec::new();
    let mut add = |name: String, shape: &[usize], float, q8| {
        specs.push(WeightSpec { name, shape: shape.iter().map(|&n| n as i64).collect(), float, q8, rows: None });
    };
    for nar in [false, true] {
        if nar && nar_rows == 0 { continue; }
        let (input, post, attn, mlp) = if nar {
            ("nar_input_layernorm", "nar_pre_mlp_layernorm", "nar_self_attn", "nar_mlp")
        } else { ("input_layernorm", "post_attention_layernorm", "self_attn", "mlp") };
        for i in 0..YUE2_LAYERS {
            let p = format!("model.layers.{i}");
            for norm in [input, post] { add(format!("{p}.{norm}.weight"), &[YUE2_HIDDEN], true, false); }
            for norm in ["q_norm", "k_norm"] { add(format!("{p}.{attn}.{norm}.weight"), &[YUE2_HEAD_DIM], true, false); }
            for (part, width, input) in [
                ("q_proj", YUE2_HIDDEN, YUE2_HIDDEN), ("k_proj", KV, YUE2_HIDDEN),
                ("v_proj", KV, YUE2_HIDDEN), ("o_proj", YUE2_HIDDEN, YUE2_HIDDEN),
            ] { add(format!("{p}.{attn}.{part}.weight"), &[input, width], false, !nar); }
            for (part, width, input) in [
                ("up_proj", YUE2_FF, YUE2_HIDDEN), ("gate_proj", YUE2_FF, YUE2_HIDDEN),
                ("down_proj", YUE2_HIDDEN, YUE2_FF),
            ] { add(format!("{p}.{mlp}.{part}.weight"), &[input, width], false, !nar); }
        }
    }
    add("model.embed_tokens.weight".into(), &[YUE2_HIDDEN, YUE2_VOCAB], false, false);
    add("model.norm.weight".into(), &[YUE2_HIDDEN], true, false);
    if nar_rows != 0 {
        for (name, input, output) in [
            ("vae2llm", YUE2_LATENT_DIM, YUE2_HIDDEN), ("llm2vae", YUE2_HIDDEN, YUE2_LATENT_DIM),
            ("time_embedder.mlp.0", YUE2_TIME_FREQ_DIM, YUE2_HIDDEN),
            ("time_embedder.mlp.2", YUE2_HIDDEN, YUE2_HIDDEN),
        ] {
            add(format!("{name}.weight"), &[input, output], false, false);
            add(format!("{name}.bias"), &[output], true, false);
        }
    }
    if nar_rows != 0 {
        specs.push(WeightSpec { name: "latent_pos_embed.pe".into(), shape: vec![YUE2_HIDDEN as i64, nar_rows as i64], float: false, q8: false, rows: Some((0, nar_rows as u64)) });
    }
    if let Some(phase) = phase {
        let (lo, rows) = phase.head_rows();
        specs.push(WeightSpec { name: "lm_head.weight".into(), shape: vec![YUE2_HIDDEN as i64, rows as i64], float: false, q8: true, rows: Some((lo as u64, rows as u64)) });
    }
    specs
}

struct Weights {
    ctx: Context,
    ids: HashMap<String, TensorId>,
    storage: TensorType,
    specs: Vec<WeightSpec>,
}

impl WeightSpec {
    fn ty(&self, storage: TensorType) -> TensorType {
        if self.float { TensorType::F32 } else if self.q8 { TensorType::Q8_0 } else { storage }
    }

    fn bytes(&self) -> usize {
        let n = self.shape.iter().product::<i64>() as usize;
        padded(if self.float { n * 4 } else if self.q8 { n / 32 * 34 } else { n * 2 })
    }

    fn read(&self, source: &Yue2Weights, storage: TensorType) -> Result<Vec<u8>> {
        if self.float { return Ok(bytes_f32(&source.tensor_f32(&self.name)?).to_vec()); }
        let mut bytes = if let Some((lo, rows)) = self.rows {
            source.tensor_row_range_bytes(&self.name, lo, rows)?
        } else { source.tensor_bytes(&self.name)? };
        if self.q8 {
            if self.shape[0] % 32 != 0 || bytes.len() % 64 != 0 {
                return Err(DiffusionError::model(format!("{} Q8_0 row alignment", self.name)));
            }
            // Pack in place: 64 BF16 bytes -> 34 Q8_0 bytes. This matches
            // q8.cu: f32 absmax/127, separate reciprocal and multiplication,
            // integer ties away from zero, f16 scale ties to even.
            for block in 0..bytes.len() / 64 {
                let mut values = [0.0f32; 32];
                let mut max = 0.0f32;
                for (j, value) in values.iter_mut().enumerate() {
                    let i = block * 64 + j * 2;
                    *value = f32::from_bits((u16::from_le_bytes([bytes[i], bytes[i + 1]]) as u32) << 16);
                    if !value.is_finite() { return Err(DiffusionError::model(format!("{} nonfinite Q8_0 weight", self.name))); }
                    max = max.max(value.abs());
                }
                let scale = max / 127.0;
                let inv = if scale == 0.0 { 0.0 } else { 1.0 / scale };
                let half = makepad_ai_loader::quant::f32_to_f16_rn(scale);
                if half & 0x7c00 == 0x7c00 { return Err(DiffusionError::model(format!("{} overflows Q8_0 scale", self.name))); }
                let out = &mut bytes[block * 34..(block + 1) * 34];
                out[..2].copy_from_slice(&half.to_le_bytes());
                for (q, value) in out[2..].iter_mut().zip(values) {
                    *q = ((value * inv).round().clamp(-127.0, 127.0) as i8) as u8;
                }
            }
            bytes.truncate(bytes.len() / 64 * 34);
        } else if storage == TensorType::F16 {
            for b in bytes.chunks_exact_mut(2) {
                let value = f32::from_bits((u16::from_le_bytes([b[0], b[1]]) as u32) << 16);
                let half = makepad_ai_loader::quant::f32_to_f16_rn(value);
                if value.is_finite() && half & 0x7c00 == 0x7c00 {
                    return Err(DiffusionError::model(format!("{} overflows f16 weight storage", self.name)));
                }
                b.copy_from_slice(&half.to_le_bytes());
            }
        }
        Ok(bytes)
    }
}

impl Weights {
    fn load(source: &Yue2Weights, storage: TensorType, device: GraphDevice, nar_rows: usize, phase: Option<Yue2Phase>, state_bytes: usize) -> Result<Self> {
        let specs = weight_specs(nar_rows, phase);
        let bytes: usize = specs.iter().map(WeightSpec::bytes).sum();
        let mut ctx = arena_context(bytes + state_bytes, device);
        let mut ids = HashMap::new();
        for s in &specs {
            let id = ctx.new_named_tensor(&s.name, s.ty(storage), s.shape.len(), &s.shape, BufferUsage::Weights).map_err(DiffusionError::model)?;
            if device != GraphDevice::Metal {
                ctx.write_tensor_data(id, &s.read(source, storage)?).map_err(DiffusionError::model)?;
            }
            ids.insert(s.name.clone(), id);
        }
        ctx.set_no_alloc(true);
        Ok(Self { ctx, ids, storage, specs })
    }

    fn compile(&mut self, model: &Yue2GraphModel, graphs: &[(&str, &Graph, &[TensorId])], initial: &[(TensorId, Vec<u8>)]) -> Result<Vec<DeviceGraphSession>> {
        if model.device != GraphDevice::Metal {
            for (id, bytes) in initial { self.ctx.write_tensor_data(*id, bytes).map_err(DiffusionError::model)?; }
        }
        let sessions = compile_family(&model.runtime()?, &self.ctx, graphs)?;
        if model.device == GraphDevice::Metal {
            for s in &self.specs { write_metal_tensor(&sessions[0], &self.ctx, self.ids[&s.name], &s.read(&model.weights, self.storage)?)?; }
            for (id, bytes) in initial { write_metal_tensor(&sessions[0], &self.ctx, *id, bytes)?; }
        }
        Ok(sessions)
    }
}

struct Builder<'a> {
    ctx: &'a mut Context,
    ids: &'a HashMap<String, TensorId>,
    storage: TensorType,
}

impl Builder<'_> {
    fn id(&self, name: &str) -> IrResult<TensorId> {
        self.ids.get(name).copied().ok_or_else(|| format!("missing graph weight {name}"))
    }

    fn round(&mut self, x: TensorId) -> IrResult<TensorId> {
        // BF16 is supported natively by Metal3 and the CUDA graph GEMM.
        // Older Metal devices use f16 weights/operands; report that choice.
        let half = cast(self.ctx, x, self.storage)?;
        cast(self.ctx, half, TensorType::F32)
    }

    fn linear(&mut self, x: TensorId, name: &str, bias: bool) -> IrResult<TensorId> {
        let weight = self.id(&format!("{name}.weight"))?;
        let x = self.round(x)?;
        let x = self.ctx.mul_mat(weight, x, A)?;
        if bias {
            let b = self.id(&format!("{name}.bias"))?;
            self.ctx.binary_like_a(Op::Add, x, b, A)
        } else { Ok(x) }
    }

    fn norm(&mut self, x: TensorId, name: &str) -> IrResult<TensorId> {
        let weight = self.id(&format!("{name}.weight"))?;
        let x = self.ctx.rms_norm_eps(x, YUE2_RMS_EPS, A)?;
        self.ctx.binary_like_a(Op::Mul, x, weight, A)
    }

    fn qkv(&mut self, x: TensorId, layer: usize, nar: bool, pos: TensorId, n: usize) -> IrResult<(TensorId, TensorId, TensorId)> {
        let p = format!("model.layers.{layer}");
        let attn = if nar { "nar_self_attn" } else { "self_attn" };
        let input = if nar { "nar_input_layernorm" } else { "input_layernorm" };
        let x = self.norm(x, &format!("{p}.{input}"))?;
        let q = self.linear(x, &format!("{p}.{attn}.q_proj"), false)?;
        let k = self.linear(x, &format!("{p}.{attn}.k_proj"), false)?;
        let v = self.linear(x, &format!("{p}.{attn}.v_proj"), false)?;
        let mut rope = |x, heads, norm: &str| -> IrResult<TensorId> {
            let x = self.ctx.reshape(x, &[YUE2_HEAD_DIM as i64, heads, n as i64])?;
            let x = self.norm(x, &format!("{p}.{attn}.{norm}"))?;
            let x = self.ctx.rope_ext(x, pos, None, YUE2_HEAD_DIM as i32, GGML_ROPE_TYPE_NEOX,
                0, YUE2_ROPE_THETA, 1.0, 0.0, 1.0, 0.0, 0.0, A)?;
            self.round(x)
        };
        let q = rope(q, YUE2_HEADS as i64, "q_norm")?;
        let k = rope(k, YUE2_KV_HEADS as i64, "k_norm")?;
        let k = self.ctx.reshape(k, &[KV as i64, n as i64])?;
        let v = self.round(v)?;
        Ok((q, k, v))
    }

    fn store(&mut self, cache: TensorId, x: TensorId, indices: TensorId, n: usize) -> IrResult<TensorId> {
        let x = self.ctx.reshape(x, &[YUE2_HEAD_DIM as i64, YUE2_KV_HEADS as i64, n as i64])?;
        let x = self.ctx.permute(x, [0, 2, 1, 3])?;
        let x = self.ctx.cont(x)?;
        let x = self.ctx.reshape(x, &[YUE2_HEAD_DIM as i64, (n * YUE2_KV_HEADS) as i64])?;
        self.ctx.set_rows(cache, x, indices, BufferUsage::State)
    }

    fn copy_prefix(&mut self, prefix: TensorId, combined: TensorId, prefix_capacity: usize, capacity: usize) -> IrResult<TensorId> {
        let prefix = self.ctx.reshape(prefix, &[YUE2_HEAD_DIM as i64, prefix_capacity as i64, YUE2_KV_HEADS as i64])?;
        let dst = self.ctx.view_3d(combined, YUE2_HEAD_DIM as i64, prefix_capacity as i64, YUE2_KV_HEADS as i64,
            YUE2_HEAD_DIM * 2, capacity * YUE2_HEAD_DIM * 2, 0)?;
        self.ctx.cpy(prefix, dst, BufferUsage::State)
    }

    fn attention(&mut self, q: TensorId, k: TensorId, v: TensorId, n: usize, keys: usize, capacity: usize, mask: TensorId) -> IrResult<TensorId> {
        let q = self.ctx.permute(q, [0, 2, 1, 3])?;
        // Cache is head-major [D, capacity, Hkv]. These views retain the
        // dependency on SetRows, so attention cannot move before the write.
        // Contiguous key rows also satisfy the Metal padding kernel's row
        // copy contract (nb1 bytes) and CUDA's vector/MMA alignment.
        let mut view = |x| self.ctx.view_3d(x, YUE2_HEAD_DIM as i64, keys as i64, YUE2_KV_HEADS as i64,
            YUE2_HEAD_DIM * 2, capacity * YUE2_HEAD_DIM * 2, 0);
        let k = view(k)?;
        let v = view(v)?;
        let x = self.ctx.flash_attn_ext(q, k, v, Some(mask),
            1.0 / (YUE2_HEAD_DIM as f32).sqrt(), 0.0, 0.0, A)?;
        self.ctx.reshape(x, &[YUE2_HIDDEN as i64, n as i64])
    }

    fn tail(&mut self, x: TensorId, attn: TensorId, layer: usize, nar: bool) -> IrResult<TensorId> {
        let p = format!("model.layers.{layer}");
        let (attn_name, post, mlp) = if nar {
            ("nar_self_attn", "nar_pre_mlp_layernorm", "nar_mlp")
        } else { ("self_attn", "post_attention_layernorm", "mlp") };
        let o = self.linear(attn, &format!("{p}.{attn_name}.o_proj"), false)?;
        let x = self.ctx.binary_like_a(Op::Add, x, o, A)?;
        let normed = self.norm(x, &format!("{p}.{post}"))?;
        let gate = self.linear(normed, &format!("{p}.{mlp}.gate_proj"), false)?;
        let up = self.linear(normed, &format!("{p}.{mlp}.up_proj"), false)?;
        let ff = self.ctx.glu_split(gate, up, GluOp::Swiglu, A)?;
        let down = self.linear(ff, &format!("{p}.{mlp}.down_proj"), false)?;
        self.ctx.binary_like_a(Op::Add, x, down, A)
    }
}

struct Cache { k: Vec<TensorId>, v: Vec<TensorId>, capacity: usize }

impl Cache {
    fn bytes(capacity: usize) -> usize { 2 * YUE2_LAYERS * padded(KV * capacity * 2) }
    fn new(ctx: &mut Context, capacity: usize) -> IrResult<Self> {
        let mut alloc = || (0..YUE2_LAYERS).map(|_| leaf(ctx, TensorType::F16, &[YUE2_HEAD_DIM as i64, (capacity * YUE2_KV_HEADS) as i64])).collect::<IrResult<Vec<_>>>();
        let k = alloc()?;
        let v = alloc()?;
        Ok(Self { k, v, capacity })
    }
}

struct ArGraph { graph: Graph, ids: TensorId, pos: TensorId, cache_rows: TensorId, mask: TensorId, output: TensorId, n: usize, keys: usize, capacity: usize }

fn ar_bytes(n: usize, keys: usize) -> usize {
    2 * padded(n * 4) + padded(n * YUE2_KV_HEADS * 4) + padded(keys * n.div_ceil(8) * 8 * 2)
}

fn block_lengths(rows: usize, block: usize) -> Vec<usize> {
    let mut lengths = vec![rows.min(block)];
    if rows > block && rows % block != 0 { lengths.push(rows % block); }
    lengths
}

fn ar_graph(w: &mut Weights, cache: &Cache, n: usize, keys: usize, head: bool) -> IrResult<ArGraph> {
    let ids = leaf(&mut w.ctx, TensorType::I32, &[n as i64])?;
    let pos = leaf(&mut w.ctx, TensorType::I32, &[n as i64])?;
    let cache_rows = leaf(&mut w.ctx, TensorType::I32, &[(n * YUE2_KV_HEADS) as i64])?;
    let mask = leaf(&mut w.ctx, TensorType::F16, &[keys as i64, (n.div_ceil(8) * 8) as i64])?;
    let mut b = Builder { ctx: &mut w.ctx, ids: &w.ids, storage: w.storage };
    let embed = b.id("model.embed_tokens.weight")?;
    let mut x = b.ctx.get_rows(embed, ids, A)?;
    for i in 0..YUE2_LAYERS {
        let (q, k, v) = b.qkv(x, i, false, pos, n)?;
        let k = b.store(cache.k[i], k, cache_rows, n)?;
        let v = b.store(cache.v[i], v, cache_rows, n)?;
        let attn = b.attention(q, k, v, n, keys, cache.capacity, mask)?;
        x = b.tail(x, attn, i, false)?;
    }
    let output = if head {
        let x = b.ctx.view_2d(x, YUE2_HIDDEN as i64, 1, YUE2_HIDDEN * 4, (n - 1) * YUE2_HIDDEN * 4)?;
        let x = b.norm(x, "model.norm")?;
        b.linear(x, "lm_head", false)?
    } else { x };
    Ok(ArGraph { graph: finish(b.ctx, output)?, ids, pos, cache_rows, mask, output, n, keys, capacity: cache.capacity })
}

fn cache_indices(start: usize, n: usize, capacity: usize) -> Vec<i32> {
    (0..YUE2_KV_HEADS).flat_map(|head| (start..start + n).map(move |p| (head * capacity + p) as i32)).collect()
}

fn mask_bytes(n: usize, keys: usize, start: usize) -> Vec<u8> {
    // Padded query rows are never read as real queries. Keep them masked.
    let mut mask = vec![0u8; keys * n.div_ceil(8) * 8 * 2];
    for (i, b) in mask.chunks_exact_mut(2).enumerate() {
        let q = i / keys;
        let k = i % keys;
        let value: u16 = if q < n && k <= start + q { 0 } else { 0xfc00 };
        b.copy_from_slice(&value.to_le_bytes());
    }
    mask
}

fn run_ar(ctx: &Context, session: &DeviceGraphSession, graph: &ArGraph, ids: &[u32], start: usize, read: bool) -> Result<Vec<f32>> {
    let ids: Vec<i32> = ids.iter().map(|&id| id as i32).collect();
    let pos: Vec<i32> = (start..start + ids.len()).map(|p| p as i32).collect();
    let mask = mask_bytes(graph.n, graph.keys, start);
    let cache_rows = cache_indices(start, ids.len(), graph.capacity);
    let wanted = if read { vec![graph.output] } else { Vec::new() };
    let result = session.execute(ctx, &[(graph.ids, bytes_i32(&ids)), (graph.pos, bytes_i32(&pos)),
        (graph.cache_rows, bytes_i32(&cache_rows)), (graph.mask, &mask)], &wanted)?;
    Ok(if read { read_output(&result.outputs[&graph.output]) } else { Vec::new() })
}

fn run_prefill(ctx: &Context, sessions: &[DeviceGraphSession], graphs: &[ArGraph], indices: &[usize], ids: &[u32], read: bool) -> Result<Vec<f32>> {
    let mut logits = Vec::new();
    for (i, block) in ids.chunks(AR_BLOCK_ROWS).enumerate() {
        let gi = *indices.iter().find(|&&gi| graphs[gi].n == block.len()).unwrap();
        let last = (i + 1) * AR_BLOCK_ROWS >= ids.len();
        logits = run_ar(ctx, &sessions[gi], &graphs[gi], block, i * AR_BLOCK_ROWS, read && last)?;
    }
    Ok(logits)
}

/// Weight source plus graph runtime. Each active phase owns one arena;
/// phase teardown releases it before the next phase loads its parameter set.
/// AR uses only AR weights, NAR uses both sets, VAE has its own arena.
pub struct Yue2GraphModel { weights: Yue2Weights, device: GraphDevice, storage: TensorType }

impl Yue2GraphModel {
    pub fn load(weights: Yue2Weights) -> Result<Self> {
        let runtime = DeviceRuntime::new()?;
        let storage = match &runtime {
            DeviceRuntime::Metal(m) if !m.features().has_bfloat => TensorType::F16,
            _ => TensorType::BF16,
        };
        eprintln!("yue2 graph: {}, AR linears Q8_0, other weights/operands {}, host sampling", runtime.description(), storage.name());
        // Runtimes/sessions are thread-affine. Keep the resident model Send
        // (the hub moves its backend to a worker), and create the runtime on
        // the thread that owns each phase's sessions.
        Ok(Self { weights, device: runtime.device(), storage })
    }

    pub fn device(&self) -> GraphDevice { self.device }

    fn runtime(&self) -> Result<DeviceRuntime> {
        let runtime = DeviceRuntime::new()?;
        if runtime.device() != self.device {
            return Err(DiffusionError::model("yue2 graph device changed after model load"));
        }
        Ok(runtime)
    }

    /// Prefill one or two CFG branches, then reuse each branch's one-token
    /// graph for the entire capacity bucket. Only ids/positions/masks change.
    pub fn ar_session(&self, prefix: &[u32], negative: Option<&[u32]>, max_tokens: usize, phase: Yue2Phase) -> Result<Yue2GraphAr> {
        let prefixes: Vec<&[u32]> = std::iter::once(prefix).chain(negative).collect();
        for ids in &prefixes {
            if ids.is_empty() || ids.iter().any(|&id| id as usize >= YUE2_VOCAB)
                || ids.len().checked_add(max_tokens).is_none_or(|n| n > YUE2_CONTEXT)
            { return Err(DiffusionError::workflow("yue2 graph AR prefix/budget outside model context")); }
        }
        let caps: Vec<usize> = prefixes.iter().map(|ids| (ids.len() + max_tokens).div_ceil(CACHE_BUCKET) * CACHE_BUCKET).collect();
        let state_bytes = prefixes.iter().zip(&caps).map(|(ids, &cap)| Cache::bytes(cap)
            + block_lengths(ids.len(), AR_BLOCK_ROWS).iter().map(|&n| ar_bytes(n, ids.len().div_ceil(CACHE_BUCKET) * CACHE_BUCKET)).sum::<usize>()
            + ar_bytes(1, cap)).sum();
        let mut w = Weights::load(&self.weights, self.storage, self.device, 0, Some(phase), state_bytes)?;
        let mut graphs = Vec::new();
        let mut names = Vec::new();
        let mut prefill_indices = Vec::new();
        let mut decode_indices = Vec::new();
        for (branch, (ids, &cap)) in prefixes.iter().zip(&caps).enumerate() {
            let cache = Cache::new(&mut w.ctx, cap).map_err(DiffusionError::model)?;
            let mut indices = Vec::new();
            for n in block_lengths(ids.len(), AR_BLOCK_ROWS) {
                indices.push(graphs.len());
                names.push(format!("AR {phase:?} branch={branch} prefill rows={n}"));
                graphs.push(ar_graph(&mut w, &cache, n, ids.len().div_ceil(CACHE_BUCKET) * CACHE_BUCKET, true).map_err(DiffusionError::model)?);
            }
            prefill_indices.push(indices);
            decode_indices.push(graphs.len());
            names.push(format!("AR {phase:?} branch={branch} decode"));
            graphs.push(ar_graph(&mut w, &cache, 1, cache.capacity, true).map_err(DiffusionError::model)?);
        }
        let outputs: Vec<[TensorId; 1]> = graphs.iter().map(|g| [g.output]).collect();
        let family: Vec<_> = graphs.iter().zip(&outputs).zip(&names).map(|((g, o), name)| (name.as_str(), &g.graph, o.as_slice())).collect();
        let sessions = w.compile(self, &family, &[])?;
        let mut logits = Vec::new();
        for (i, ids) in prefixes.iter().enumerate() {
            logits.push(run_prefill(&w.ctx, &sessions, &graphs, &prefill_indices[i], ids, true)?);
        }
        Ok(Yue2GraphAr { sessions, ctx: w.ctx, graphs, decode_indices, lengths: prefixes.iter().map(|p| p.len()).collect(), capacities: caps, logits, phase })
    }

    pub fn nar_chunk(&self, ar_tokens: &[u32], frames: usize, raw_times: &[f32]) -> Result<Yue2GraphNarChunk> {
        Yue2GraphNarChunk::prefill(self, ar_tokens, frames, raw_times)
    }
}

pub struct Yue2GraphAr {
    // Sessions drop before the Context they were compiled against.
    sessions: Vec<DeviceGraphSession>,
    ctx: Context,
    graphs: Vec<ArGraph>,
    decode_indices: Vec<usize>,
    lengths: Vec<usize>,
    capacities: Vec<usize>,
    logits: Vec<Vec<f32>>,
    phase: Yue2Phase,
}

impl Yue2GraphAr {
    pub fn logits(&self) -> (u32, &[f32], Option<&[f32]>) {
        (self.phase.head_rows().0, &self.logits[0], self.logits.get(1).map(Vec::as_slice))
    }

    pub fn decode(&mut self, token: u32) -> Result<()> {
        if token as usize >= YUE2_VOCAB || self.lengths.iter().zip(&self.capacities).any(|(&n, &cap)| n >= cap) {
            return Err(DiffusionError::workflow("yue2 graph decode token/cache outside model context"));
        }
        for i in 0..self.lengths.len() {
            let gi = self.decode_indices[i];
            self.logits[i] = run_ar(&self.ctx, &self.sessions[gi], &self.graphs[gi], &[token], self.lengths[i], true)?;
            self.lengths[i] += 1;
        }
        Ok(())
    }
}

pub struct Yue2GraphNarChunk {
    sessions: Vec<DeviceGraphSession>,
    ctx: Context,
    state: TensorId,
    time_index: TensorId,
    output: TensorId,
    frames: usize,
    times: usize,
    host_state: Vec<f32>,
    input_graph: usize,
    output_graph: usize,
    layers: Vec<NarLayer>,
    blocks: Vec<NarBlock>,
    host_rows: Vec<i32>,
}

struct NarLayer { qkv: usize, queries: Vec<usize> }
struct NarBlock { indices: TensorId, rows: usize }

fn time_sinusoid(raw_t: f32) -> Vec<f32> {
    let t = bf16(raw_t);
    let sig = bf16(1.0 / (1.0 + (-t).exp()));
    let shifted = bf16(YUE2_TIMESTEP_SHIFT * sig / (1.0 + (YUE2_TIMESTEP_SHIFT - 1.0) * sig));
    let half = YUE2_TIME_FREQ_DIM / 2;
    let mut values = vec![0.0; YUE2_TIME_FREQ_DIM];
    for j in 0..half {
        let freq = (-(10000.0f32).ln() * j as f32 / half as f32).exp();
        values[j] = bf16((shifted * freq).cos());
        values[half + j] = bf16((shifted * freq).sin());
    }
    values
}

impl Yue2GraphNarChunk {
    fn prefill(model: &Yue2GraphModel, ar: &[u32], frames: usize, times: &[f32]) -> Result<Self> {
        let rows = frames + 2;
        let total = ar.len() + rows;
        if ar.is_empty() || frames == 0 || times.is_empty() || total > YUE2_CONTEXT
            || ar.iter().any(|&id| id as usize >= YUE2_VOCAB)
        { return Err(DiffusionError::workflow("yue2 graph NAR prefix/state/schedule outside model context")); }
        let capacity = total.div_ceil(CACHE_BUCKET) * CACHE_BUCKET;
        let prefix_keys = ar.len().div_ceil(CACHE_BUCKET) * CACHE_BUCKET;
        let prefix_lengths = block_lengths(ar.len(), AR_BLOCK_ROWS);
        let query_lengths = block_lengths(rows, NAR_BLOCK_ROWS);
        eprintln!("yue2 NAR: prefix={} latent_rows={rows} query_block={NAR_BLOCK_ROWS}, full QKV before each layer's query blocks", ar.len());
        let mask_rows = rows.min(NAR_BLOCK_ROWS).div_ceil(8) * 8;
        let state_bytes = Cache::bytes(prefix_keys) + 2 * padded(KV * capacity * 2)
            + prefix_lengths.iter().map(|&n| ar_bytes(n, prefix_keys)).sum::<usize>()
            + padded(rows * YUE2_LATENT_DIM * 4) + 2 * padded(rows * 4) + padded(rows * YUE2_KV_HEADS * 4)
            + 2 * padded(rows * YUE2_HIDDEN * 4) + padded(capacity * mask_rows * 2)
            + query_lengths.iter().map(|&n| padded(n * 4)).sum::<usize>()
            + padded(times.len() * YUE2_HIDDEN * 4) + padded(YUE2_TIME_FREQ_DIM * 4) + 2 * padded(4);
        let mut w = Weights::load(&model.weights, model.storage, model.device, rows, None, state_bytes)?;
        let build = (|| -> IrResult<_> {
            let cache = Cache::new(&mut w.ctx, prefix_keys)?;
            let prefill = prefix_lengths.iter().map(|&n| ar_graph(&mut w, &cache, n, prefix_keys, false)).collect::<IrResult<Vec<_>>>()?;
            let combined_k = leaf(&mut w.ctx, TensorType::F16, &[YUE2_HEAD_DIM as i64, (capacity * YUE2_KV_HEADS) as i64])?;
            let combined_v = leaf(&mut w.ctx, TensorType::F16, &[YUE2_HEAD_DIM as i64, (capacity * YUE2_KV_HEADS) as i64])?;
            let state = leaf(&mut w.ctx, TensorType::F32, &[YUE2_LATENT_DIM as i64, rows as i64])?;
            let spine = leaf(&mut w.ctx, TensorType::F32, &[YUE2_HIDDEN as i64, rows as i64])?;
            let queries = leaf(&mut w.ctx, TensorType::F32, &[YUE2_HIDDEN as i64, rows as i64])?;
            let pos = leaf(&mut w.ctx, TensorType::I32, &[rows as i64])?;
            let pe_ids = leaf(&mut w.ctx, TensorType::I32, &[rows as i64])?;
            let cache_rows = leaf(&mut w.ctx, TensorType::I32, &[(rows * YUE2_KV_HEADS) as i64])?;
            let mask = leaf(&mut w.ctx, TensorType::F16, &[capacity as i64, mask_rows as i64])?;
            let table = leaf(&mut w.ctx, TensorType::F32, &[YUE2_HIDDEN as i64, times.len() as i64])?;
            let sinusoid = leaf(&mut w.ctx, TensorType::F32, &[YUE2_TIME_FREQ_DIM as i64, 1])?;
            let time_write_index = leaf(&mut w.ctx, TensorType::I32, &[1])?;
            let time_index = leaf(&mut w.ctx, TensorType::I32, &[1])?;
            let blocks = query_lengths.iter().map(|&n| Ok(NarBlock {
                indices: leaf(&mut w.ctx, TensorType::I32, &[n as i64])?, rows: n,
            })).collect::<IrResult<Vec<_>>>()?;
            let positions: Vec<i32> = (ar.len()..total).map(|i| i as i32).collect();
            let indices: Vec<i32> = (0..rows).map(|i| i as i32).collect();
            let stores: Vec<i32> = (0..rows).step_by(NAR_BLOCK_ROWS).flat_map(|start|
                cache_indices(ar.len() + start, (rows - start).min(NAR_BLOCK_ROWS), capacity)).collect();
            // All real AR/NAR keys are visible. Alignment padding is masked.
            let mut mask_data = vec![0u8; capacity * mask_rows * 2];
            for row in mask_data.chunks_exact_mut(capacity * 2) {
                for b in row[total * 2..].chunks_exact_mut(2) { b.copy_from_slice(&0xfc00u16.to_le_bytes()); }
            }
            let initial = vec![
                (pos, bytes_i32(&positions).to_vec()), (pe_ids, bytes_i32(&indices).to_vec()),
                (cache_rows, bytes_i32(&stores).to_vec()),
                (mask, mask_data),
            ];
            let mut graphs: Vec<Graph> = prefill.iter().map(|g| g.graph.clone()).collect();
            let mut names: Vec<String> = prefix_lengths.iter().map(|n| format!("NAR AR-prefix rows={n}")).collect();
            let mut b = Builder { ctx: &mut w.ctx, ids: &w.ids, storage: w.storage };
            let t = b.linear(sinusoid, "time_embedder.mlp.0", true)?;
            let t = b.ctx.unary(t, UnaryOp::Silu, A)?;
            let t = b.linear(t, "time_embedder.mlp.2", true)?;
            let t = b.ctx.set_rows(table, t, time_write_index, BufferUsage::State)?;
            let time_graph = graphs.len();
            graphs.push(finish(b.ctx, t)?);
            names.push("NAR time embedding".into());
            let time = b.ctx.get_rows(table, time_index, A)?;
            let pe = b.id("latent_pos_embed.pe")?;
            let pe = b.ctx.get_rows(pe, pe_ids, A)?;
            let mut x = b.linear(state, "vae2llm", true)?;
            x = b.ctx.binary_like_a(Op::Add, x, time, A)?;
            x = b.ctx.binary_like_a(Op::Add, x, pe, A)?;
            let x = b.ctx.cpy(x, spine, BufferUsage::State)?;
            let input_graph = graphs.len();
            graphs.push(finish(b.ctx, x)?);
            names.push("NAR input".into());
            let mut layers = Vec::new();
            for i in 0..YUE2_LAYERS {
                // Snapshot ALL layer inputs into Q/K/V before overwriting
                // even one spine block. Every block sees all NAR rows and
                // this layer's unchanged AR prefix, never partial/new KV.
                let k = b.copy_prefix(cache.k[i], combined_k, prefix_keys, capacity)?;
                let v = b.copy_prefix(cache.v[i], combined_v, prefix_keys, capacity)?;
                let mut graph = finish(b.ctx, k)?;
                graph.build_forward_expand(b.ctx, v)?;
                for start in (0..rows).step_by(NAR_BLOCK_ROWS) {
                    let n = (rows - start).min(NAR_BLOCK_ROWS);
                    let x = b.ctx.view_2d(spine, YUE2_HIDDEN as i64, n as i64, YUE2_HIDDEN * 4, start * YUE2_HIDDEN * 4)?;
                    let pos = b.ctx.view_1d(pos, n as i64, start * 4)?;
                    // Indices are packed block-major, then head-major.
                    let indices = b.ctx.view_1d(cache_rows, (n * YUE2_KV_HEADS) as i64, start * YUE2_KV_HEADS * 4)?;
                    let (q, k, v) = b.qkv(x, i, true, pos, n)?;
                    let k = b.store(combined_k, k, indices, n)?;
                    let v = b.store(combined_v, v, indices, n)?;
                    let q = b.ctx.reshape(q, &[YUE2_HIDDEN as i64, n as i64])?;
                    let dst = b.ctx.view_2d(queries, YUE2_HIDDEN as i64, n as i64, YUE2_HIDDEN * 4, start * YUE2_HIDDEN * 4)?;
                    let q = b.ctx.cpy(q, dst, BufferUsage::State)?;
                    graph.build_forward_expand(b.ctx, q)?;
                    graph.build_forward_expand(b.ctx, k)?;
                    graph.build_forward_expand(b.ctx, v)?;
                }
                let qkv = graphs.len();
                graphs.push(graph);
                names.push(format!("NAR layer={i} QKV rows={rows}"));
                let mut query_graphs = Vec::new();
                for block in &blocks {
                    let q = b.ctx.get_rows(queries, block.indices, A)?;
                    let q = b.ctx.reshape(q, &[YUE2_HEAD_DIM as i64, YUE2_HEADS as i64, block.rows as i64])?;
                    let x = b.ctx.get_rows(spine, block.indices, A)?;
                    let attn = b.attention(q, combined_k, combined_v, block.rows, capacity, capacity, mask)?;
                    let x = b.tail(x, attn, i, true)?;
                    let x = b.ctx.set_rows(spine, x, block.indices, BufferUsage::State)?;
                    query_graphs.push(graphs.len());
                    graphs.push(finish(b.ctx, x)?);
                    names.push(format!("NAR layer={i} attention+MLP rows={}", block.rows));
                }
                layers.push(NarLayer { qkv, queries: query_graphs });
            }
            let x = b.norm(spine, "model.norm")?;
            let x = b.ctx.view_2d(x, YUE2_HIDDEN as i64, frames as i64, YUE2_HIDDEN * 4, YUE2_HIDDEN * 4)?;
            let output = b.linear(x, "llm2vae", true)?;
            let output_graph = graphs.len();
            graphs.push(finish(b.ctx, output)?);
            names.push("NAR output".into());
            Ok((prefill, graphs, names, initial, time_graph, sinusoid, time_write_index,
                input_graph, output_graph, layers, blocks, state, time_index, output))
        })().map_err(DiffusionError::model)?;
        let (prefill, graphs, names, initial, time_graph, sinusoid, time_write_index,
            input_graph, output_graph, layers, blocks, state, time_index, output) = build;
        let outputs = [output];
        let family: Vec<_> = graphs.iter().zip(&names).enumerate().map(|(i, (g, name))|
            (name.as_str(), g, if i == output_graph { outputs.as_slice() } else { &[] })).collect();
        let sessions = w.compile(model, &family, &initial)?;
        let prefill_indices: Vec<usize> = (0..prefill.len()).collect();
        run_prefill(&w.ctx, &sessions, &prefill, &prefill_indices, ar, false)?;
        for (i, &raw) in times.iter().enumerate() {
            let values = time_sinusoid(raw);
            sessions[time_graph].execute(&w.ctx, &[(sinusoid, bytes_f32(&values)), (time_write_index, bytes_i32(&[i as i32]))], &[])?;
        }
        Ok(Self { sessions, ctx: w.ctx, state, time_index, output, frames, times: times.len(),
            host_state: vec![0.0; rows * YUE2_LATENT_DIM], host_rows: (0..rows).map(|i| i as i32).collect(),
            input_graph, output_graph, layers, blocks })
    }

    pub fn velocity(&mut self, state: &[f32], time_index: usize) -> Result<Vec<f32>> {
        if state.len() != self.frames * YUE2_LATENT_DIM || time_index >= self.times {
            return Err(DiffusionError::workflow("yue2 graph NAR state shape or timestep"));
        }
        for (dst, &value) in self.host_state[YUE2_LATENT_DIM..(self.frames + 1) * YUE2_LATENT_DIM].iter_mut().zip(state) {
            *dst = bf16(value);
        }
        self.sessions[self.input_graph].execute(&self.ctx,
            &[(self.state, bytes_f32(&self.host_state)), (self.time_index, bytes_i32(&[time_index as i32]))], &[])?;
        for layer in &self.layers {
            self.sessions[layer.qkv].execute(&self.ctx, &[], &[])?;
            for rows in self.host_rows.chunks(NAR_BLOCK_ROWS) {
                let bi = self.blocks.iter().position(|b| b.rows == rows.len()).unwrap();
                self.sessions[layer.queries[bi]].execute(&self.ctx,
                    &[(self.blocks[bi].indices, bytes_i32(rows))], &[])?;
            }
        }
        let result = self.sessions[self.output_graph].execute(&self.ctx, &[], &[self.output])?;
        Ok(read_output(&result.outputs[&self.output]))
    }
}
