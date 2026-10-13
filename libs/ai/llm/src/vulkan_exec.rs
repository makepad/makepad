//! Vulkan execution backend for the llama hybrid-decode graphs
//! (libs/ai/vulkan/ONDEVICE.md): the same graphs the Metal and CUDA
//! executors run, on raw Vulkan compute with the Splash kernels of
//! `makepad-ai-vulkan` (kernels/llm.splash).
//!
//! The model mirrors the CUDA executor: the ggml `Context` arena is
//! reflected one-to-one into device memory — the read-only weight region
//! uploaded once (quantised tensors stay in their GGUF block encoding and
//! the mat-vec kernels decode them in registers) and a dirty region holding
//! the KV/recurrent caches and the planned activations. Every `TensorId`
//! resolves to `device_address + logical_offset`. A compiled graph shape is
//! recorded once into a command buffer (input copies, every node, output
//! copies) and that recording is submitted again for every step.
//!
//! Fail-closed like CUDA: `VulkanExecRuntime::new()` needs a usable device,
//! graph compilation needs every node's op/layout/type to have a kernel.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use makepad_ai_vulkan as vkx;

use crate::error::{LlamaError, Result};
use crate::runtime::{
    build_hybrid_decode_graph_with_attention_key_count, build_hybrid_decode_graph_with_sequences,
    build_hybrid_decode_writes, collect_hybrid_decode_run, import_hybrid_graph_context,
    validate_hybrid_decode_layout, HybridDecodeBatchLayout, HybridDecodeGraph,
    HybridDecodeOutputConfig, HybridDecodeRun, HybridDecodeSpec, HybridSharedCacheTensorIds,
    ImportedHybridGraphContext, LogitsProbeInput,
};
use crate::weights::LoadedGgufWeights;
use crate::{
    ggml_row_size_for_type, Context, Op, Tensor, TensorId, TensorType, GGML_ROPE_TYPE_IMROPE,
    GGML_ROPE_TYPE_MROPE,
};

pub use vkx::DeviceFeatures as VulkanDeviceFeatures;

fn vk(e: String) -> LlamaError {
    LlamaError::unsupported(format!("vulkan: {e}"))
}

struct State {
    // Drop pipelines before releasing the shared device reference.
    library: RefCell<vkx::Library>,
    device: Arc<vkx::Device>,
}

impl State {
    fn kernel(&self, name: &str) -> Result<Arc<vkx::Kernel>> {
        self.library.borrow_mut().get(&self.device, name).map_err(vk)
    }
}

/// The Vulkan execution backend handle (device + kernel library).
pub struct VulkanExecRuntime {
    state: Rc<State>,
}

/// Device mirror of a ggml `Context` arena.
pub struct VulkanContextArena {
    inner: Rc<Arena>,
}

/// A raw graph planned and mirrored once, with commands replayed for each
/// input set. Drop commands and buffers before their owning runtime.
pub struct VulkanRawGraphSession {
    recorded: RefCell<Option<Recorded>>,
    arena: Rc<Arena>,
    plan: Plan,
    pinned: Vec<TensorId>,
    inputs: Vec<TensorId>,
    state: Rc<State>,
}

struct Arena {
    /// The read-only weight region in pieces no bigger than one allocation
    /// may be, split at tensor boundaries: (logical start, length, buffer).
    ro: Vec<(usize, usize, vkx::Buffer)>,
    /// Weights repacked into the kernels' 16-aligned layouts (q6_K, q8_0),
    /// each in its own buffer: (logical start, logical bytes, logical row
    /// bytes, repacked row bytes, buffer).
    repacked: Vec<(usize, usize, usize, usize, vkx::Buffer)>,
    ro_split: usize,
    main: vkx::Buffer,
    main_size: usize,
    /// Keeps the device alive until the buffers above are freed.
    _state: Rc<State>,
}

impl Arena {
    /// The device address of logical bytes `[offset, offset + len)`.
    fn address(&self, offset: usize, len: usize) -> Result<u64> {
        for (start, size, row, packed_row, buffer) in &self.repacked {
            if offset >= *start && offset < start + size {
                let rel = offset - start;
                if rel % row != 0 {
                    return Err(LlamaError::format("a view into a repacked weight that does not start at a row"));
                }
                let _ = len;
                return Ok(buffer.address + ((rel / row) * packed_row) as u64);
            }
        }
        if offset < self.ro_split {
            for (start, size, buffer) in &self.ro {
                if offset >= *start && offset < start + size {
                    if offset + len > start + size {
                        return Err(LlamaError::format("device range straddles two weight buffers"));
                    }
                    return Ok(buffer.address + (offset - start) as u64);
                }
            }
            return Err(LlamaError::format(format!("weight offset {offset} has no device buffer")));
        }
        let rel = offset - self.ro_split;
        if rel + len > self.main_size {
            return Err(LlamaError::format(format!(
                "device range [{}..+{}) exceeds arena size {}",
                offset, len, self.main_size
            )));
        }
        Ok(self.main.address + rel as u64)
    }
}

/// One compiled hybrid-decode graph shape on Vulkan.
pub struct CompiledHybridDecodeVulkan {
    // Field order is drop order: the recording and the arena's buffers go
    // before the device they belong to.
    recorded: Option<Recorded>,
    arena: Rc<Arena>,
    spec: HybridDecodeSpec,
    ctx: Context,
    decode: HybridDecodeGraph,
    plan: Plan,
    state: Rc<State>,
}

/// A recorded step: the staging buffer the inputs go in and the outputs
/// come back through, and the command buffer that runs it all.
struct Recorded {
    stream: vkx::Stream,
    staging: vkx::Buffer,
    /// (tensor, staging offset, bytes) per input and per output.
    inputs: Vec<(TensorId, usize, usize)>,
    outputs: Vec<(TensorId, usize, usize)>,
    _q8: Option<vkx::Buffer>,
}

impl VulkanExecRuntime {
    pub fn new() -> Result<Self> {
        let device = vkx::Device::shared().map_err(vk)?;
        let library = vkx::Library::new(vkx::KERNEL_SOURCES).map_err(vk)?;
        Ok(Self { state: Rc::new(State { library: RefCell::new(library), device }) })
    }

    pub fn features(&self) -> VulkanDeviceFeatures {
        self.state.device.features.clone()
    }

    /// Compile an arbitrary graph. Unlike the validation-only one-shot
    /// entry, no_alloc contexts need no host allocation for activations.
    pub fn create_raw_graph_session(
        &self,
        ctx: &Context,
        graph: &crate::Graph,
        pinned: &[TensorId],
    ) -> Result<VulkanRawGraphSession> {
        let plan = plan_raw_graph(ctx, graph, pinned)?;
        let arena = self.mirror_raw_arena(ctx, plan.required_size)?;
        let inputs = graph.leafs.iter().copied().filter(|&id| {
            let t = &ctx.tensors()[id];
            t.data_offset.is_none() && t.view_src.is_none()
        }).collect();
        Ok(VulkanRawGraphSession {
            recorded: RefCell::new(None),
            arena,
            plan,
            pinned: pinned.to_vec(),
            inputs,
            state: self.state.clone(),
        })
    }

    fn mirror_raw_arena(&self, ctx: &Context, required_size: usize) -> Result<Rc<Arena>> {
        let device = &self.state.device;
        if ctx.ro_split() != 0 {
            // Reuse the existing segmented weight mirror. Only the dirty
            // region can grow; weight allocations and uploads are retained.
            let mirrored = self.create_context_arena_with_progress(ctx, &mut |_, _| {})?;
            if required_size <= ctx.mem_size() {
                return Ok(mirrored.inner);
            }
            let mut arena = Rc::try_unwrap(mirrored.inner).map_err(|_| LlamaError::format("raw arena unexpectedly shared"))?;
            let size = align_up(required_size - arena.ro_split).max(16);
            let main = device.buffer(size as u64, vkx::BufferKind::Device).map_err(vk)?;
            let mut stream = device.stream(0).map_err(vk)?;
            stream.begin().map_err(vk)?;
            stream.fill(&main, 0, size as u64, 0);
            stream.barrier();
            let used = ctx.used_mem().saturating_sub(arena.ro_split);
            if used > 0 {
                stream.copy(&arena.main, 0, &main, 0, used as u64);
            }
            stream.submit_wait().map_err(vk)?;
            arena.main = main;
            arena.main_size = size;
            return Ok(Rc::new(arena));
        }

        // Owned contexts (including the vision loader) hold weights in
        // their main region. Allocate the final planned size immediately,
        // avoiding a second full weight-sized allocation on the phone.
        let main_size = align_up(required_size.max(ctx.used_mem())).max(16);
        let main = device.buffer(main_size as u64, vkx::BufferKind::Device).map_err(vk)?;
        let mut stream = device.stream(0).map_err(vk)?;
        stream.begin().map_err(vk)?;
        stream.fill(&main, 0, main_size as u64, 0);
        stream.submit_wait().map_err(vk)?;
        let mut at = 0;
        while at < ctx.used_mem() {
            let n = (64 << 20).min(ctx.used_mem() - at);
            device.upload(&main, at as u64, ctx.data_at(at, n).map_err(LlamaError::format)?).map_err(vk)?;
            at += n;
        }
        let mut repacked = Vec::new();
        for t in ctx.tensors().iter().filter(|t| t.view_src.is_none()) {
            let (Some(kind), Some(offset)) = (repack_kind(t), t.data_offset) else { continue };
            let row = t.nb[1];
            let rows = t.nbytes() / row;
            let packed = repack(kind, ctx.data_at(offset, t.nbytes()).map_err(LlamaError::format)?, row, rows);
            let buffer = device.buffer(packed.len() as u64 + 16, vkx::BufferKind::Device).map_err(vk)?;
            device.upload(&buffer, 0, &packed).map_err(vk)?;
            repacked.push((offset, t.nbytes(), row, packed.len() / rows, buffer));
        }
        Ok(Rc::new(Arena { ro: Vec::new(), repacked, ro_split: 0, main, main_size, _state: self.state.clone() }))
    }

    pub fn device_description(&self) -> String {
        self.state.device.features.description()
    }

    pub fn reserve_hybrid_decode_main_buffer_size(
        &self,
        weights: &LoadedGgufWeights,
        spec: &HybridDecodeSpec,
        shared_cache: Option<&HybridSharedCacheTensorIds>,
        n_tokens: usize,
        n_outputs: usize,
    ) -> Result<usize> {
        let ImportedHybridGraphContext { mut ctx, tensor_ids, shared_cache } =
            import_hybrid_graph_context(weights, shared_cache, false)?;
        let key_count = default_reserve_key_count(spec);
        let decode = build_hybrid_decode_graph_with_attention_key_count(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            n_outputs,
            0,
            key_count,
        )?;
        let plan = plan_graph(&ctx, &decode)?;
        Ok(plan.required_size)
    }

    pub fn create_context_arena_with_progress(
        &self,
        ctx: &Context,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<VulkanContextArena> {
        let device = &self.state.device;
        let ro_split = ctx.ro_split();
        let main_size = ctx.mem_size() - ro_split;
        // Weight pieces: tensors in offset order, packed up to the largest
        // single allocation (a phone may refuse one 3 GB buffer).
        let limit = device.features.max_allocation_bytes.min(1 << 31).max(64 << 20) as usize;
        let mut spans: Vec<(usize, usize, bool)> = ctx
            .tensors()
            .iter()
            .filter(|t| t.view_src.is_none())
            .filter_map(|t| t.data_offset.map(|o| (o, t.nbytes(), repack_kind(t).is_some())))
            .filter(|(o, _, _)| *o < ro_split)
            .collect();
        spans.sort_unstable();
        // Pieces over the weights that stay as they are, cut between
        // tensors at the allocation limit and around repacked tensors.
        let mut covered: Vec<(usize, usize)> = Vec::new();
        let mut piece: Option<(usize, usize)> = None;
        for (o, n, repacked) in spans {
            if n > limit {
                return Err(LlamaError::unsupported(format!("a {n}-byte weight tensor exceeds the device's {limit}-byte allocation limit")));
            }
            if repacked {
                if let Some(p) = piece.take() {
                    covered.push(p);
                }
                continue;
            }
            piece = match piece {
                Some((s, e)) if o + n - s <= limit => Some((s, (o + n).max(e))),
                Some(p) => {
                    covered.push(p);
                    Some((o, o + n))
                }
                None => Some((o, o + n)),
            };
        }
        if let Some(p) = piece {
            covered.push(p);
        }
        let covered: Vec<(usize, usize)> = covered.into_iter().map(|(s, e)| (s, e - s)).collect();
        let mut repacked = Vec::new();
        for t in ctx.tensors() {
            let (Some(kind), Some(o)) = (repack_kind(t), t.data_offset) else { continue };
            if t.view_src.is_some() {
                continue;
            }
            let row = t.nb[1];
            let rows = t.nbytes() / row;
            let src = ctx.data_at(o, t.nbytes()).map_err(LlamaError::format)?;
            let packed = repack(kind, src, row, rows);
            ctx.release_ro_pages(o, t.nbytes());
            let packed_row = packed.len() / rows;
            let buffer = device.buffer(packed.len() as u64 + 16, vkx::BufferKind::Device).map_err(vk)?;
            device.upload(&buffer, 0, &packed).map_err(vk)?;
            repacked.push((o, t.nbytes(), row, packed_row, buffer));
        }
        let mut ro = Vec::new();
        progress(0, ro_split);
        let mut done = 0usize;
        for (start, len) in covered {
            // 16 bytes of slack: the kernels read whole words at block ends.
            let buffer = device.buffer(len as u64 + 16, vkx::BufferKind::Device).map_err(vk)?;
            const CHUNK: usize = 64 << 20;
            let mut at = 0usize;
            while at < len {
                let n = CHUNK.min(len - at);
                let host = ctx.data_at(start + at, n).map_err(LlamaError::format)?;
                device.upload(&buffer, at as u64, host).map_err(vk)?;
                // The device holds them now: the mapped file pages go.
                ctx.release_ro_pages(start + at, n);
                at += n;
                done += n;
                progress(done, ro_split);
            }
            ro.push((start, len, buffer));
        }
        let main = device.buffer(main_size.max(16) as u64, vkx::BufferKind::Device).map_err(vk)?;
        let zeros = vec![0u8; (16 << 20).min(main_size.max(16))];
        let mut at = 0usize;
        while at < main_size {
            let n = zeros.len().min(main_size - at);
            device.upload(&main, at as u64, &zeros[..n]).map_err(vk)?;
            at += n;
        }
        let dirty_used = ctx.used_mem().saturating_sub(ro_split);
        let mut at = 0usize;
        while at < dirty_used {
            let n = (64 << 20).min(dirty_used - at);
            let host = ctx.data_at(ro_split + at, n).map_err(LlamaError::format)?;
            device.upload(&main, at as u64, host).map_err(vk)?;
            at += n;
        }
        Ok(VulkanContextArena { inner: Rc::new(Arena { ro, repacked, ro_split, main, main_size, _state: self.state.clone() }) })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn compile_hybrid_decode(
        &self,
        weights: &mut LoadedGgufWeights,
        spec: &HybridDecodeSpec,
        shared_cache: &HybridSharedCacheTensorIds,
        arena: &VulkanContextArena,
        n_tokens: usize,
        n_outputs: usize,
        attention_key_base: usize,
        attention_key_count: usize,
        n_seqs: usize,
    ) -> Result<CompiledHybridDecodeVulkan> {
        let ImportedHybridGraphContext { mut ctx, tensor_ids, shared_cache } =
            import_hybrid_graph_context(weights, Some(shared_cache), false)?;
        let decode = build_hybrid_decode_graph_with_sequences(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            n_outputs,
            attention_key_base,
            attention_key_count,
            n_seqs,
        )?;
        let plan = plan_graph(&ctx, &decode)?;
        if plan.required_size > ctx.mem_size() {
            return Err(LlamaError::format(format!(
                "context out of memory allocating {} bytes for graph activations (context arena is {} bytes)",
                plan.required_size - ctx.used_mem(),
                ctx.mem_size(),
            )));
        }
        Ok(CompiledHybridDecodeVulkan {
            state: self.state.clone(),
            arena: arena.inner.clone(),
            spec: spec.clone(),
            ctx,
            decode,
            plan,
            recorded: None,
        })
    }

    /// Validation entry: execute an arbitrary graph over `ctx` through the
    /// exact planner/dispatch path the session uses (a private arena), and
    /// read back `wanted`. Used by the op checks against CPU references.
    pub fn execute_raw_graph(
        &self,
        ctx: &Context,
        graph: &crate::Graph,
        pinned: &[TensorId],
        writes: &[(TensorId, Vec<u8>)],
        wanted: &[TensorId],
    ) -> Result<BTreeMap<TensorId, Vec<u8>>> {
        let plan = plan_raw_graph(ctx, graph, pinned)?;
        if plan.required_size > ctx.mem_size() {
            return Err(LlamaError::format("context out of memory for graph activations"));
        }
        let arena = self.create_context_arena_with_progress(ctx, &mut |_, _| {})?;
        let device = &self.state.device;
        for (id, bytes) in writes {
            let offset = *plan.bindings.get(id).ok_or_else(|| LlamaError::format("write to an unbound tensor"))?;
            arena.inner.address(offset, bytes.len())?;
            device.upload(&arena.inner.main, (offset - arena.inner.ro_split) as u64, bytes).map_err(vk)?;
        }
        let mut s = device.stream(0).map_err(vk)?;
        s.begin().map_err(vk)?;
        let q8 = plan.q8_scratch(device)?;
        let view = ExecView { state: &self.state, arena: &arena.inner, ctx, plan: &plan, scratch: q8.as_ref().map(|(_, a, b)| (*a, *b)), hazards: Default::default(), q8_live: Default::default() };
        for index in 0..plan.nodes.len() {
            view.dispatch_node(&mut s, index).map_err(|err| LlamaError::format(format!("node {index}: {err:?}")))?;
        }
        s.barrier();
        s.submit_wait().map_err(vk)?;
        let mut out = BTreeMap::new();
        for id in wanted {
            let t = ctx.tensor(*id).ok_or_else(|| LlamaError::format("invalid output tensor"))?;
            let offset = *plan.bindings.get(id).ok_or_else(|| LlamaError::format("output has no binding"))?;
            let bytes = if offset < arena.inner.ro_split {
                ctx.data_at(offset, t.nbytes()).map_err(LlamaError::format)?.to_vec()
            } else {
                device.download(&arena.inner.main, (offset - arena.inner.ro_split) as u64, t.nbytes()).map_err(vk)?
            };
            out.insert(*id, bytes);
        }
        Ok(out)
    }

    pub fn read_arena_bytes(&self, arena: &VulkanContextArena, offset: usize, len: usize) -> Result<Vec<u8>> {
        let a = &arena.inner;
        if offset < a.ro_split {
            return Err(LlamaError::format("reading back the read-only weight region"));
        }
        a.address(offset, len)?;
        self.state.device.download(&a.main, (offset - a.ro_split) as u64, len).map_err(vk)
    }

    pub fn write_arena_bytes(&self, arena: &VulkanContextArena, offset: usize, bytes: &[u8]) -> Result<()> {
        let a = &arena.inner;
        if offset < a.ro_split {
            return Err(LlamaError::format("writing into the read-only weight region"));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        a.address(offset, bytes.len())?;
        self.state.device.upload(&a.main, (offset - a.ro_split) as u64, bytes).map_err(vk)
    }

    pub fn clear_arena_ranges(&self, arena: &VulkanContextArena, ranges: &[(usize, usize)]) -> Result<()> {
        let a = &arena.inner;
        let mut s = self.state.device.stream(0).map_err(vk)?;
        s.begin().map_err(vk)?;
        for &(offset, len) in ranges {
            if len == 0 {
                continue;
            }
            if offset < a.ro_split {
                return Err(LlamaError::format(format!(
                    "refusing to clear read-only Vulkan arena range [{}..+{}) below split {}",
                    offset, len, a.ro_split
                )));
            }
            a.address(offset, len)?;
            // vkCmdFillBuffer needs 4-byte multiples; cache rows always are.
            s.fill(&a.main, (offset - a.ro_split) as u64, (len as u64 + 3) & !3, 0);
        }
        s.submit_wait().map_err(vk)
    }
}

/// Which weights the kernels read in a repacked layout.
fn repack_kind(t: &Tensor) -> Option<Quant> {
    if t.op != Op::None || t.data_offset.is_none() || t.ne[0] % 256 != 0 {
        return None;
    }
    match t.desc.ty {
        TensorType::Q6K => Some(Quant::Q6K),
        TensorType::Q8_0 => Some(Quant::Q80),
        _ => None,
    }
}

/// `rows` rows of `row` bytes in the kernels' layout: q6_K blocks padded
/// from 210 to 224 bytes; q8_0 per 8 blocks the eight f16 scales first,
/// then the eight blocks' int8 (272 bytes either way).
fn repack(kind: Quant, src: &[u8], row: usize, rows: usize) -> Vec<u8> {
    match kind {
        Quant::Q6K => {
            let blocks = row / 210;
            let mut out = vec![0u8; rows * blocks * 224];
            for (i, b) in src.chunks_exact(210).enumerate() {
                out[i * 224..i * 224 + 210].copy_from_slice(b);
            }
            out
        }
        Quant::Q80 => {
            let mut out = vec![0u8; src.len()];
            for (u, unit) in src.chunks_exact(272).enumerate() {
                let o = &mut out[u * 272..(u + 1) * 272];
                for (i, b) in unit.chunks_exact(34).enumerate() {
                    o[2 * i..2 * i + 2].copy_from_slice(&b[..2]);
                    o[16 + 32 * i..16 + 32 * (i + 1)].copy_from_slice(&b[2..]);
                }
            }
            out
        }
        _ => src.to_vec(),
    }
}

/// The row bytes the kernels see for a weight of `ty` (repacked rows for
/// the repacked types).
fn kernel_row_bytes(ty: TensorType, ne0: i64) -> Result<usize> {
    let row = ggml_row_size_for_type(ty, ne0).map_err(LlamaError::format)?;
    Ok(match ty {
        TensorType::Q6K => row / 210 * 224,
        _ => row,
    })
}

fn default_reserve_key_count(spec: &HybridDecodeSpec) -> usize {
    use crate::runtime::HybridLayerSpec;
    for layer in &spec.layers {
        if let HybridLayerSpec::Attention { decode, .. } = layer {
            return decode.cache.max_context as usize;
        }
    }
    1
}

impl VulkanRawGraphSession {
    /// Upload full contiguous inputs, replay the graph, read pinned outputs.
    /// `ctx` must be the context used to create this session; adding other
    /// graph shapes to it is allowed because existing tensor IDs stay stable.
    pub fn execute(
        &self,
        ctx: &Context,
        writes: &[(TensorId, &[u8])],
        wanted: &[TensorId],
    ) -> Result<BTreeMap<TensorId, Vec<u8>>> {
        for id in &self.inputs {
            if !writes.iter().any(|(written, _)| written == id) {
                return Err(LlamaError::format(format!("raw graph input {id} has no data")));
            }
        }
        for (id, bytes) in writes {
            let t = ctx.tensor(*id).ok_or_else(|| LlamaError::format("invalid raw graph input"))?;
            if !t.is_contiguous() || bytes.len() != t.nbytes() {
                return Err(LlamaError::format("raw graph writes must cover one contiguous tensor"));
            }
            self.main_place(*id, bytes.len())?;
        }
        for id in wanted {
            if !self.pinned.contains(id) {
                return Err(LlamaError::format(format!("raw graph output {id} was not pinned at creation")));
            }
            let t = ctx.tensor(*id).ok_or_else(|| LlamaError::format("invalid raw graph output"))?;
            if !t.is_contiguous() {
                return Err(LlamaError::format("raw graph readback requires contiguous output"));
            }
            self.main_place(*id, t.nbytes())?;
        }
        let mut recorded = self.recorded.borrow_mut();
        let same = recorded.as_ref().is_some_and(|r| {
            r.inputs.len() == writes.len()
                && r.inputs.iter().zip(writes).all(|((id, _, len), (written, bytes))| id == written && *len == bytes.len())
                && r.outputs.iter().map(|o| o.0).eq(wanted.iter().copied())
        });
        if !same {
            *recorded = None;
            *recorded = Some(self.record(ctx, writes, wanted)?);
        }
        let rec = recorded.as_mut().unwrap();
        let mapped = rec.staging.mapped().ok_or_else(|| LlamaError::format("raw staging buffer is not mapped"))?;
        for ((_, at, len), (_, bytes)) in rec.inputs.iter().zip(writes) {
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.add(*at), *len) };
        }
        rec.stream.submit_wait().map_err(vk)?;
        let mut outputs = BTreeMap::new();
        for (id, at, len) in &rec.outputs {
            let mut bytes = vec![0; *len];
            unsafe { std::ptr::copy_nonoverlapping(mapped.add(*at), bytes.as_mut_ptr(), *len) };
            outputs.insert(*id, bytes);
        }
        if std::env::var_os("MAKEPAD_VULKAN_PROFILE").is_some() {
            let mut by = BTreeMap::<String, (f64, usize)>::new();
            for (label, ms) in rec.stream.timings().map_err(vk)? {
                if let Some(node) = label.parse::<usize>().ok().and_then(|i| self.plan.nodes.get(i)) {
                    let entry = by.entry(format!("{:?}", node.sel)).or_default();
                    entry.0 += ms;
                    entry.1 += 1;
                }
            }
            for (name, (ms, count)) in by {
                eprintln!("vulkan.raw.profile: {name} {ms:.3} ms / {count}");
            }
        }
        Ok(outputs)
    }

    pub fn device_bytes(&self) -> usize {
        self.arena.main_size + self.arena.ro.iter().map(|(_, _, b)| b.size as usize).sum::<usize>()
            + self.arena.repacked.iter().map(|(_, _, _, _, b)| b.size as usize).sum::<usize>()
    }

    pub fn node_count(&self) -> usize {
        self.plan.nodes.iter().filter(|n| n.sel != Sel::Skip).count()
    }

    fn main_place(&self, id: TensorId, len: usize) -> Result<(&vkx::Buffer, u64)> {
        let offset = *self.plan.bindings.get(&id).ok_or_else(|| LlamaError::format("raw tensor has no binding"))?;
        if offset < self.arena.ro_split {
            return Err(LlamaError::format("raw input/output in the read-only region"));
        }
        self.arena.address(offset, len)?;
        Ok((&self.arena.main, (offset - self.arena.ro_split) as u64))
    }

    fn record(&self, ctx: &Context, writes: &[(TensorId, &[u8])], wanted: &[TensorId]) -> Result<Recorded> {
        let device = &self.state.device;
        let mut size = 0;
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for (id, bytes) in writes {
            inputs.push((*id, size, bytes.len()));
            size += align_up(bytes.len());
        }
        for id in wanted {
            let len = ctx.tensor(*id).unwrap().nbytes();
            outputs.push((*id, size, len));
            size += align_up(len);
        }
        let staging = device.buffer(size.max(16) as u64, vkx::BufferKind::Staging).map_err(vk)?;
        let timed = std::env::var_os("MAKEPAD_VULKAN_PROFILE").is_some();
        let mut stream = device.stream(if timed { self.plan.nodes.len() as u32 + 1 } else { 0 }).map_err(vk)?;
        stream.begin().map_err(vk)?;
        for (id, at, len) in &inputs {
            if *len > 0 {
                let (buffer, offset) = self.main_place(*id, *len)?;
                stream.copy(&staging, *at as u64, buffer, offset, *len as u64);
            }
        }
        stream.barrier();
        let q8 = self.plan.q8_scratch(device)?;
        let view = ExecView { state: &self.state, arena: &self.arena, ctx, plan: &self.plan, scratch: q8.as_ref().map(|(_, a, b)| (*a, *b)), hazards: Default::default(), q8_live: Default::default() };
        for index in 0..self.plan.nodes.len() {
            if timed { stream.mark(&index.to_string()); }
            view.dispatch_node(&mut stream, index).map_err(|e| LlamaError::format(format!("raw node {index}: {e:?}")))?;
        }
        stream.barrier();
        if timed { stream.mark("end"); }
        for (id, at, len) in &outputs {
            if *len > 0 {
                let (buffer, offset) = self.main_place(*id, *len)?;
                stream.copy(buffer, offset, &staging, *at as u64, *len as u64);
            }
        }
        stream.end().map_err(vk)?;
        Ok(Recorded { stream, staging, inputs, outputs, _q8: q8.map(|(buffer, _, _)| buffer) })
    }
}

impl CompiledHybridDecodeVulkan {
    pub fn decode(&self) -> &HybridDecodeGraph {
        &self.decode
    }

    pub fn execute(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
        capture_hidden: bool,
        skip_logits_readback: bool,
    ) -> Result<HybridDecodeRun> {
        let t0 = std::time::Instant::now();
        validate_hybrid_decode_layout(&self.ctx, &self.spec, &self.decode, layout)?;
        let writes = build_hybrid_decode_writes(&self.ctx, &self.spec, &self.decode, input, layout)?;
        let t_writes = t0.elapsed();
        let mut wanted = vec![self.decode.result_logits];
        if capture_hidden {
            wanted.push(self.decode.result_hidden);
        }
        let skip_logits = !capture_hidden && skip_logits_readback;
        let wanted_read: Vec<TensorId> = if skip_logits { Vec::new() } else { wanted };
        let outputs = self.run(&writes, &wanted_read)?;
        let t_run = t0.elapsed();
        if skip_logits {
            return Ok(HybridDecodeRun {
                hidden: Vec::new(),
                logits: Vec::new(),
                n_tokens: layout.positions.len().max(1),
                hidden_size: 0,
                vocab_size: 0,
                selected_experts: Vec::new(),
            });
        }
        let config = if capture_hidden { HybridDecodeOutputConfig::FULL } else { HybridDecodeOutputConfig::LOGITS_ONLY };
        let run = collect_hybrid_decode_run(&self.ctx, &self.decode, config, &outputs);
        if std::env::var("MAKEPAD_VULKAN_PROFILE").is_ok() {
            let wb: usize = writes.iter().map(|w| w.1.len()).sum();
            eprintln!(
                "vulkan.host: writes {:.2} ms ({} B) run {:.2} ms collect {:.2} ms",
                t_writes.as_secs_f64() * 1e3,
                wb,
                (t_run - t_writes).as_secs_f64() * 1e3,
                (t0.elapsed() - t_run).as_secs_f64() * 1e3
            );
        }
        run
    }

    /// Writes the inputs, runs the recorded step (recording it first when
    /// the input set changed), reads the outputs.
    fn run(&mut self, writes: &[(TensorId, Vec<u8>)], wanted: &[TensorId]) -> Result<BTreeMap<TensorId, Vec<u8>>> {
        let same = self.recorded.as_ref().is_some_and(|r| {
            r.inputs.len() == writes.iter().filter(|(_, b)| !b.is_empty()).count()
                && r.inputs.iter().zip(writes.iter().filter(|(_, b)| !b.is_empty())).all(|(a, (id, b))| a.0 == *id && a.2 == b.len())
                && r.outputs.iter().map(|o| o.0).eq(wanted.iter().copied())
        });
        if !same {
            self.recorded = None;
            self.recorded = Some(self.record(writes, wanted)?);
        }
        let rec = self.recorded.as_mut().unwrap();
        let staging = rec.staging.mapped().ok_or_else(|| LlamaError::format("staging buffer is not mapped"))?;
        for ((_, at, len), (_, bytes)) in rec.inputs.iter().zip(writes.iter().filter(|(_, b)| !b.is_empty())) {
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), staging.add(*at), *len) };
        }
        let ts = std::time::Instant::now();
        rec.stream.submit_wait().map_err(vk)?;
        let t_gpu = ts.elapsed();
        let mut out = BTreeMap::new();
        for (id, at, len) in &rec.outputs {
            let mut host = vec![0u8; *len];
            unsafe { std::ptr::copy_nonoverlapping(staging.add(*at), host.as_mut_ptr(), *len) };
            out.insert(*id, host);
        }
        if std::env::var("MAKEPAD_VULKAN_PROFILE").is_ok() {
            eprintln!("vulkan.host: submit+wait {:.2} ms, readback {:.2} ms", t_gpu.as_secs_f64() * 1e3, (ts.elapsed() - t_gpu).as_secs_f64() * 1e3);
            self.report_profile()?;
        }
        Ok(out)
    }

    fn record(&self, writes: &[(TensorId, Vec<u8>)], wanted: &[TensorId]) -> Result<Recorded> {
        let device = &self.state.device;
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut size = 0usize;
        for (id, bytes) in writes.iter().filter(|(_, b)| !b.is_empty()) {
            inputs.push((*id, size, bytes.len()));
            size += (bytes.len() + 15) & !15;
        }
        for id in wanted {
            let len = self.ctx.tensor(*id).map(|t| t.nbytes()).unwrap_or(0);
            outputs.push((*id, size, len));
            size += (len + 15) & !15;
        }
        let staging = device.buffer(size.max(16) as u64, vkx::BufferKind::Staging).map_err(vk)?;
        let timed = std::env::var("MAKEPAD_VULKAN_PROFILE").is_ok();
        // `total`: only the whole step's GPU time (marks between nodes cost).
        let total_only = std::env::var("MAKEPAD_VULKAN_PROFILE").as_deref() == Ok("total");
        let mut stream = device.stream(if timed { self.plan.nodes.len() as u32 + 2 } else { 0 }).map_err(vk)?;
        stream.begin().map_err(vk)?;
        for (id, at, len) in &inputs {
            let (buffer, rel) = self.main_place(*id, *len)?;
            stream.copy(&staging, *at as u64, buffer, rel, *len as u64);
        }
        stream.barrier();
        let q8 = self.plan.q8_scratch(device)?;
        let view = ExecView { state: &self.state, arena: &self.arena, ctx: &self.ctx, plan: &self.plan, scratch: q8.as_ref().map(|(_, a, b)| (*a, *b)), hazards: Default::default(), q8_live: Default::default() };
        for index in 0..self.plan.nodes.len() {
            if timed && (!total_only || index == 0) {
                stream.mark(&format!("{index}"));
            }
            view.dispatch_node(&mut stream, index)
                .map_err(|err| LlamaError::format(format!("node {index}: {err:?}")))?;
        }
        stream.barrier();
        if timed {
            stream.mark("end");
        }
        for (id, at, len) in &outputs {
            let (buffer, rel) = self.main_place(*id, *len)?;
            stream.copy(buffer, rel, &staging, *at as u64, *len as u64);
        }
        stream.end().map_err(vk)?;
        Ok(Recorded { stream, staging, inputs, outputs, _q8: q8.map(|(b, _, _)| b) })
    }

    /// The dirty-region buffer and offset of a planned tensor.
    fn main_place(&self, id: TensorId, len: usize) -> Result<(&vkx::Buffer, u64)> {
        let offset = *self.plan.bindings.get(&id).ok_or_else(|| LlamaError::format(format!("tensor {id} has no device binding")))?;
        if offset < self.arena.ro_split {
            return Err(LlamaError::format("graph input/output in the read-only region"));
        }
        self.arena.address(offset, len)?;
        Ok((&self.arena.main, (offset - self.arena.ro_split) as u64))
    }

    /// GPU milliseconds of the last run by kernel, printed to stderr
    /// (MAKEPAD_VULKAN_PROFILE).
    fn report_profile(&self) -> Result<()> {
        let mut by: BTreeMap<String, (f64, usize)> = BTreeMap::new();
        let mut total = 0.0;
        let timings = self.node_timings()?;
        let mut slow: Vec<&(String, f64)> = timings.iter().collect();
        slow.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        for (what, ms) in slow.iter().take(6) {
            eprintln!("vulkan.slow: {ms:.3} ms {what}");
        }
        if std::env::var("MAKEPAD_VULKAN_PROFILE").as_deref() == Ok("seq") {
            for (what, ms) in timings.iter().filter(|(w, _)| !w.starts_with("Skip")).take(70) {
                eprintln!("vulkan.seq: {ms:.3} {what}");
            }
        }
        for (what, ms) in timings.iter().cloned() {
            let key = what.split(' ').next().unwrap_or("").to_string();
            let e = by.entry(key).or_insert((0.0, 0));
            e.0 += ms;
            e.1 += 1;
            total += ms;
        }
        let mut v: Vec<_> = by.into_iter().collect();
        v.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap());
        let parts: Vec<String> = v.iter().take(8).map(|(k, (ms, n))| format!("{k} {ms:.2}ms/{n}")).collect();
        eprintln!("vulkan.profile: {total:.2} ms | {}", parts.join(" | "));
        Ok(())
    }

    /// Per-node GPU milliseconds of the last run (MAKEPAD_VULKAN_PROFILE).
    pub fn node_timings(&self) -> Result<Vec<(String, f64)>> {
        let Some(rec) = &self.recorded else { return Ok(Vec::new()) };
        let raw = rec.stream.timings().map_err(vk)?;
        let tensors = self.ctx.tensors();
        Ok(raw
            .into_iter()
            .map(|(label, ms)| {
                let what = label
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| self.plan.nodes.get(i))
                    .map(|n| {
                        let t = &tensors[n.node_id];
                        let srcs: Vec<String> = t.src.iter().flatten().map(|s| format!("{:?}", tensors[*s].ne)).collect();
                        format!("{:?} {:?} {:?} <- {}", n.sel, t.op, t.ne, srcs.join(" "))
                    })
                    .unwrap_or(label);
                (what, ms)
            })
            .collect())
    }

    pub fn execute_with_layout(&mut self, input: LogitsProbeInput<'_>, layout: &HybridDecodeBatchLayout) -> Result<HybridDecodeRun> {
        self.execute(input, layout, true, false)
    }

    pub fn execute_logits_only_with_layout(&mut self, input: LogitsProbeInput<'_>, layout: &HybridDecodeBatchLayout) -> Result<HybridDecodeRun> {
        self.execute(input, layout, false, false)
    }

    pub fn execute_logits_only_with_layout_options(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
        skip_logits_readback: bool,
    ) -> Result<HybridDecodeRun> {
        self.execute(input, layout, false, skip_logits_readback)
    }
}

// ---------------------------------------------------------------------------
// Planning: kernel selection, then storage (lifetimes + offsets).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sel {
    Skip,
    Copy,
    Concat,
    GetRowsF32,
    GetRowsQ(Quant),
    SetRows { f16: bool },
    Norm { l2: bool },
    /// rms_norm(x) * w, the Mul node of a fused RmsNorm + Mul.
    NormMul,
    /// act(x) * g, the Mul node of a fused Unary + Mul.
    UnaryMul(u32),
    RopeMulti,
    Unary(u32),
    Glu(u32),
    Binary(u32),
    Gemv(Quant),
    SsmConv,
    GatedDeltaNet,
    Attn,
    FloatGemm { f16: bool },
    LayerNorm,
    Softmax,
    RopeVision,
    UpscaleBilinear { aa: bool },
    VisionAttn,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Quant {
    Q4K,
    Q5K,
    Q6K,
    Q80,
}

impl Quant {
    fn of(ty: TensorType) -> Option<Quant> {
        Some(match ty {
            TensorType::Q4K => Quant::Q4K,
            TensorType::Q5K => Quant::Q5K,
            TensorType::Q6K => Quant::Q6K,
            TensorType::Q8_0 => Quant::Q80,
            _ => return None,
        })
    }
    fn rows_kernel(self) -> &'static str {
        match self {
            Quant::Q4K => "get_rows_q4k",
            Quant::Q5K => "get_rows_q5k",
            Quant::Q6K => "get_rows_q6k",
            Quant::Q80 => "get_rows_q80",
        }
    }
    fn gemm_kernel(self) -> &'static str {
        match self {
            Quant::Q4K => "gemm_q4k",
            Quant::Q5K => "gemm_q5k",
            Quant::Q6K => "gemm_q6k",
            Quant::Q80 => "gemm_q80",
        }
    }
    fn kernel(self) -> &'static str {
        match self {
            Quant::Q4K => "gemv_q4k",
            Quant::Q5K => "gemv_q5k",
            Quant::Q6K => "gemv_q6k",
            Quant::Q80 => "gemv_q80",
        }
    }
}

struct PlannedNode {
    node_id: TensorId,
    sel: Sel,
}

struct Plan {
    nodes: Vec<PlannedNode>,
    bindings: BTreeMap<TensorId, usize>,
    required_size: usize,
    /// Each matmul's int8 activation slot: (node index, words offset,
    /// pairs offset), and the totals. Separate slots let independent
    /// matmuls run without a barrier between them.
    q8_slots: Vec<(usize, u64, u64)>,
    q8_words: u64,
    q8_pairs: u64,
}

impl Plan {
    /// One scratch buffer for every gemv's int8 activations: the words,
    /// then the (d, sum) pairs at a 16-byte boundary.
    fn q8_scratch(&self, device: &vkx::Device) -> Result<Option<(vkx::Buffer, u64, u64)>> {
        if self.q8_slots.is_empty() {
            return Ok(None);
        }
        let buffer = device.buffer(self.q8_words + self.q8_pairs + 16, vkx::BufferKind::Device).map_err(vk)?;
        let (xq, xs) = (buffer.address, buffer.address + self.q8_words);
        Ok(Some((buffer, xq, xs)))
    }

    fn q8_slot(&self, index: usize) -> Option<(u64, u64)> {
        self.q8_slots.iter().find(|s| s.0 == index).map(|s| (s.1, s.2))
    }
}

fn unsupported_node(t: &Tensor, why: &str) -> LlamaError {
    LlamaError::unsupported(format!(
        "Vulkan executor cannot run node '{}' ({:?}): {}",
        t.name().unwrap_or("<unnamed>"),
        t.op,
        why
    ))
}

fn select(tensors: &[Tensor], t: &Tensor) -> Result<Sel> {
    if let Some(sel) = select_vision(tensors, t)? {
        return Ok(sel);
    }
    let src = |i: usize| -> Result<&Tensor> {
        t.src[i].and_then(|id| tensors.get(id)).ok_or_else(|| unsupported_node(t, &format!("missing src{i}")))
    };
    Ok(match t.op {
        Op::None | Op::View | Op::Reshape | Op::Permute | Op::Transpose => Sel::Skip,
        Op::Cont | Op::Cpy | Op::Dup => {
            if src(0)?.desc.ty != t.desc.ty || t.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "copy that is not f32 -> f32"));
            }
            Sel::Copy
        }
        Op::Concat => {
            if t.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "non-f32 concat"));
            }
            Sel::Concat
        }
        Op::GetRows => {
            if src(1)?.desc.ty != TensorType::I32 {
                return Err(unsupported_node(t, "non-i32 row indices"));
            }
            match src(0)?.desc.ty {
                TensorType::F32 => Sel::GetRowsF32,
                ty => match Quant::of(ty) {
                    Some(q) => Sel::GetRowsQ(q),
                    None => return Err(unsupported_node(t, &format!("get_rows from {}", ty.name()))),
                },
            }
        }
        Op::SetRows => {
            if src(1)?.desc.ty != TensorType::I32 || src(0)?.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "set_rows needs f32 rows and i32 indices"));
            }
            if t.ne[2] != 1 || t.ne[3] != 1 {
                return Err(unsupported_node(t, "batched set_rows"));
            }
            match t.desc.ty {
                TensorType::F16 => Sel::SetRows { f16: true },
                TensorType::F32 => Sel::SetRows { f16: false },
                other => return Err(unsupported_node(t, &format!("set_rows into {}", other.name()))),
            }
        }
        Op::RmsNorm => Sel::Norm { l2: false },
        Op::L2Norm => Sel::Norm { l2: true },
        Op::Rope => {
            let mode = t.op_param_i32(2);
            if t.src[2].is_some() {
                return Err(unsupported_node(t, "rope freq_factors"));
            }
            if mode != GGML_ROPE_TYPE_IMROPE && (mode & GGML_ROPE_TYPE_MROPE) == 0 {
                return Err(unsupported_node(t, &format!("rope mode {mode}")));
            }
            Sel::RopeMulti
        }
        Op::Unary => {
            let op = t.op_param_i32(0);
            if !(0..=15).contains(&op) {
                return Err(unsupported_node(t, &format!("unary op {op}")));
            }
            if !src(0)?.is_contiguous() || !t.is_contiguous() {
                return Err(unsupported_node(t, "strided unary"));
            }
            Sel::Unary(op as u32)
        }
        Op::Glu => {
            if t.src[1].is_none() {
                return Err(unsupported_node(t, "fused-split glu"));
            }
            if !src(0)?.is_contiguous() || !src(1)?.is_contiguous() || !t.is_contiguous() {
                return Err(unsupported_node(t, "strided glu"));
            }
            Sel::Glu(t.op_param_i32(0) as u32)
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div => {
            if t.desc.ty != TensorType::F32 || src(0)?.desc.ty != TensorType::F32 || src(1)?.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "non-f32 binary op"));
            }
            Sel::Binary(match t.op {
                Op::Add => 0,
                Op::Sub => 1,
                Op::Mul => 2,
                _ => 3,
            })
        }
        Op::MulMat => {
            let a = src(0)?;
            let b = src(1)?;
            if b.desc.ty != TensorType::F32 || b.nb[0] != 4 {
                return Err(unsupported_node(t, "activations must be f32 with a contiguous dim 0"));
            }
            let q = Quant::of(a.desc.ty).ok_or_else(|| unsupported_node(t, &format!("matmul with {} weights", a.desc.ty.name())))?;
            if a.ne[2] != 1 || a.ne[3] != 1 || b.ne[2] != 1 || b.ne[3] != 1 {
                return Err(unsupported_node(t, "batched quantized matmul"));
            }
            if a.ne[0] % 256 != 0 {
                return Err(unsupported_node(t, "K not a multiple of 256"));
            }
            Sel::Gemv(q)
        }
        Op::SsmConv => Sel::SsmConv,
        Op::GatedDeltaNet => {
            let v = src(2)?;
            let sv = v.ne[0];
            if sv % 32 != 0 || sv > 256 {
                return Err(unsupported_node(t, &format!("gated_delta_net value dim {sv}")));
            }
            Sel::GatedDeltaNet
        }
        Op::FlashAttnExt => {
            let q = src(0)?;
            let k = src(1)?;
            let v = src(2)?;
            if t.src[4].is_some() || t.op_param_f32(1) != 0.0 || t.op_param_f32(2) != 0.0 {
                return Err(unsupported_node(t, "flash attention sinks/max_bias/softcap"));
            }
            let mask = src(3)?;
            if q.desc.ty != TensorType::F32 || k.desc.ty != TensorType::F16 || v.desc.ty != TensorType::F16 || mask.desc.ty != TensorType::F16 {
                return Err(unsupported_node(t, "attention needs f32 Q, f16 K/V and an f16 mask"));
            }
            if q.ne[0] > 256 || v.ne[0] > 256 || q.ne[3] != 1 {
                return Err(unsupported_node(t, "attention head dim over 256 or multi-stream"));
            }
            if q.ne[2] % k.ne[2] != 0 {
                return Err(unsupported_node(t, "non-integer GQA ratio"));
            }
            Sel::Attn
        }
        other => return Err(unsupported_node(t, &format!("op {other:?} has no Vulkan kernel"))),
    })
}

/// The next node after `i` that runs a kernel.
fn next_compute(nodes: &[PlannedNode], i: usize) -> Option<usize> {
    (i + 1..nodes.len()).find(|&j| nodes[j].sel != Sel::Skip)
}

/// Whether any node other than `except` reads `id` (or it is a graph output).
fn used_elsewhere(tensors: &[Tensor], graph_nodes: &[TensorId], pinned: &[TensorId], id: TensorId, except: TensorId) -> bool {
    pinned.contains(&id)
        || graph_nodes.iter().any(|&n| n != except && tensors[n].src.iter().flatten().any(|s| *s == id || tensors[*s].view_src == Some(id)))
}

/// Fewer, larger dispatches: RmsNorm + Mul(weight) and Unary + Mul(gate)
/// become one kernel each when the first result feeds only the second.
fn fuse_nodes(tensors: &[Tensor], graph_nodes: &[TensorId], pinned: &[TensorId], nodes: &mut [PlannedNode]) {
    let mut i = 0;
    while i < nodes.len() {
        let first = nodes[i].sel;
        let fusable = matches!(first, Sel::Norm { l2: false } | Sel::Unary(7) | Sel::Unary(10) | Sel::Unary(15));
        let Some(j) = next_compute(nodes, i).filter(|_| fusable) else {
            i += 1;
            continue;
        };
        if nodes[j].sel != Sel::Binary(2) {
            i += 1;
            continue;
        }
        let a_id = nodes[i].node_id;
        let mul = &tensors[nodes[j].node_id];
        let (Some(s0), Some(s1)) = (mul.src[0], mul.src[1]) else {
            i += 1;
            continue;
        };
        let other = if s0 == a_id { s1 } else if s1 == a_id { s0 } else {
            i += 1;
            continue;
        };
        let a = &tensors[a_id];
        let o = &tensors[other];
        let x = &tensors[a.src[0].unwrap()];
        let ok = match first {
            // The weight is one row broadcast over every row.
            Sel::Norm { .. } => o.ne[0] == a.ne[0] && o.ne[1] == 1 && o.ne[2] == 1 && o.ne[3] == 1 && o.nb[0] == 4 && a.nb[0] == 4 && x.nb[0] == 4,
            // The gate has the same shape, everything contiguous.
            _ => o.ne == a.ne && o.is_contiguous() && a.is_contiguous() && x.is_contiguous() && mul.is_contiguous(),
        };
        if !ok || used_elsewhere(tensors, graph_nodes, pinned, a_id, nodes[j].node_id) {
            i += 1;
            continue;
        }
        nodes[i].sel = Sel::Skip;
        nodes[j].sel = match first {
            Sel::Unary(op) => Sel::UnaryMul(op),
            _ => Sel::NormMul,
        };
        i = j + 1;
    }
}

/// Additive float/vision surface. Existing text graph selections stay in
/// `select`; in particular masked f16 attention keeps its existing kernel.
fn select_vision(tensors: &[Tensor], t: &Tensor) -> Result<Option<Sel>> {
    let src = |i: usize| -> Result<&Tensor> {
        t.src[i].and_then(|id| tensors.get(id)).ok_or_else(|| unsupported_node(t, &format!("missing src{i}")))
    };
    let f32_output = || -> Result<()> {
        if t.desc.ty != TensorType::F32 || !t.is_contiguous() {
            return Err(unsupported_node(t, "vision output must be contiguous f32"));
        }
        Ok(())
    };
    let selected = match t.op {
        Op::MulMat if matches!(src(0)?.desc.ty, TensorType::F32 | TensorType::F16) => {
            let a = src(0)?;
            let b = src(1)?;
            f32_output()?;
            if b.desc.ty != TensorType::F32 || a.ne[0] != b.ne[0]
                || a.ne.iter().chain(b.ne.iter()).any(|&d| d <= 0)
                || b.ne[2] % a.ne[2] != 0 || b.ne[3] % a.ne[3] != 0
            {
                return Err(unsupported_node(t, "float GEMM needs f32 activations and integer batch broadcast"));
            }
            Sel::FloatGemm { f16: a.desc.ty == TensorType::F16 }
        }
        Op::Norm => {
            f32_output()?;
            if src(0)?.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "LayerNorm needs f32 input"));
            }
            Sel::LayerNorm
        }
        Op::SoftMax => {
            f32_output()?;
            let x = src(0)?;
            if x.desc.ty != TensorType::F32 || x.nb[0] != 4
                || t.op_param_f32(1) != 0.0 || t.src[2].is_some()
            {
                return Err(unsupported_node(t, "softmax needs f32 rows without bias or sinks"));
            }
            if t.src[1].is_some() {
                let mask = src(1)?;
                if mask.desc.ty != TensorType::F32 || mask.nb[0] != 4 || mask.ne[0] != x.ne[0]
                    || mask.ne[1] <= 0 || mask.ne[2] != 1 || mask.ne[3] != 1
                {
                    return Err(unsupported_node(t, "softmax mask must be a broadcast f32 matrix"));
                }
            }
            Sel::Softmax
        }
        Op::Rope if t.op_param_i32(2) == crate::GGML_ROPE_TYPE_VISION => {
            f32_output()?;
            let x = src(0)?;
            let pos = src(1)?;
            let nd = t.op_param_i32(1);
            if x.desc.ty != TensorType::F32 || pos.desc.ty != TensorType::I32 || !pos.is_contiguous()
                || pos.ne.iter().product::<i64>() < 2 * x.ne[2]
                || t.src[2].is_some() || x.ne[0] % 2 != 0 || nd <= 0 || 2 * nd as i64 > x.ne[0]
                || t.op_param_i32(11) < 0 || t.op_param_i32(12) < 0
                || t.op_param_i32(11) + t.op_param_i32(12) <= 0
            {
                return Err(unsupported_node(t, "invalid f32 vision RoPE shape/sections/positions"));
            }
            Sel::RopeVision
        }
        Op::Upscale => {
            f32_output()?;
            let flags = t.op_param_i32(0);
            if src(0)?.desc.ty != TensorType::F32 || flags & 0xff != crate::ScaleMode::Bilinear as i32 {
                return Err(unsupported_node(t, "upscale needs f32 bilinear interpolation"));
            }
            Sel::UpscaleBilinear { aa: flags & crate::core::GGML_SCALE_FLAG_ANTIALIAS != 0 }
        }
        Op::FlashAttnExt if t.src[3].is_none() && src(1)?.desc.ty == TensorType::F32 => {
            f32_output()?;
            let q = src(0)?;
            let k = src(1)?;
            let v = src(2)?;
            if [q, k, v].iter().any(|x| x.desc.ty != TensorType::F32 || x.nb[0] != 4 || x.ne.iter().any(|&d| d <= 0))
                || q.ne[0] != k.ne[0] || k.ne[1] != v.ne[1] || k.ne[2] != v.ne[2]
                || q.ne[2] % k.ne[2] != 0 || q.ne[3] != k.ne[3] || q.ne[3] != v.ne[3]
                || t.src[4].is_some() || t.op_param_f32(1) != 0.0 || t.op_param_f32(2) != 0.0
            {
                return Err(unsupported_node(t, "maskless attention needs f32 Q/K/V with matching heads and batches, no bias/sinks/softcap"));
            }
            Sel::VisionAttn
        }
        _ => return Ok(None),
    };
    Ok(Some(selected))
}

fn resolve_root(tensors: &[Tensor], id: TensorId) -> Result<(TensorId, usize)> {
    let mut current = id;
    let mut extra = 0usize;
    loop {
        let t = tensors.get(current).ok_or_else(|| LlamaError::format(format!("invalid tensor id {current}")))?;
        if let Some(src) = t.view_src {
            extra += t.view_offs;
            current = src;
        } else {
            return Ok((current, extra));
        }
    }
}

fn align_up(v: usize) -> usize {
    (v + 255) & !255
}

fn plan_graph(ctx: &Context, decode: &HybridDecodeGraph) -> Result<Plan> {
    let mut pinned = vec![decode.result_logits, decode.result_hidden];
    pinned.extend(decode.moe_selected_experts.iter().map(|s| s.selected_experts));
    pinned.extend(decode.state_updates.iter().copied());
    plan_raw_graph(ctx, &decode.graph, &pinned)
}

/// Kernel selection (fails closed first), then storage: node outputs get
/// best-fit reuse of blocks whose last reader is done, leaf inputs are
/// allocated up front, resident tensors keep their logical offsets.
fn plan_raw_graph(ctx: &Context, graph: &crate::Graph, pinned: &[TensorId]) -> Result<Plan> {
    let tensors = ctx.tensors();
    let mut nodes = Vec::with_capacity(graph.nodes.len());
    for &node_id in &graph.nodes {
        let t = tensors.get(node_id).ok_or_else(|| LlamaError::format(format!("invalid graph node {node_id}")))?;
        nodes.push(PlannedNode { node_id, sel: select(tensors, t)? });
    }
    fuse_nodes(tensors, &graph.nodes, pinned, &mut nodes);
    let mut needed: Vec<TensorId> = graph.nodes.clone();
    needed.extend(graph.leafs.iter().copied());
    needed.sort_unstable();
    needed.dedup();
    let mut last_use: BTreeMap<TensorId, usize> = BTreeMap::new();
    let mut def: BTreeMap<TensorId, usize> = BTreeMap::new();
    for (index, &node_id) in graph.nodes.iter().enumerate() {
        let t = &tensors[node_id];
        for src in t.src.iter().flatten() {
            let (root, _) = resolve_root(tensors, *src)?;
            last_use.insert(root, index);
        }
        let (out_root, _) = resolve_root(tensors, node_id)?;
        def.entry(out_root).or_insert(index);
        last_use.insert(out_root, index.max(last_use.get(&out_root).copied().unwrap_or(0)));
    }
    let end = graph.nodes.len();
    for &pin in pinned {
        let (root, _) = resolve_root(tensors, pin)?;
        last_use.insert(root, end);
    }
    let base = align_up(ctx.used_mem());
    let mut cursor = base;
    let mut max_end = ctx.used_mem();
    let mut free: Vec<(usize, usize)> = Vec::new();
    let mut planned: BTreeMap<TensorId, usize> = BTreeMap::new();
    let allocate = |root: TensorId, size: usize, free: &mut Vec<(usize, usize)>, cursor: &mut usize, max_end: &mut usize, planned: &mut BTreeMap<TensorId, usize>| {
        let size = align_up(size);
        let mut best: Option<usize> = None;
        for (index, &(_, free_size)) in free.iter().enumerate() {
            if free_size >= size && best.map_or(true, |b| free[b].1 > free_size) {
                best = Some(index);
            }
        }
        let offset = if let Some(index) = best {
            let (offset, free_size) = free.remove(index);
            if free_size > size {
                free.push((offset + size, free_size - size));
            }
            offset
        } else {
            let offset = *cursor;
            *cursor += size;
            offset
        };
        *max_end = (*max_end).max(offset + size);
        planned.insert(root, offset);
    };
    for &leaf_id in &graph.leafs {
        let (root, _) = resolve_root(tensors, leaf_id)?;
        let root_t = &tensors[root];
        if root_t.data_offset.is_none() && !planned.contains_key(&root) {
            allocate(root, root_t.nbytes(), &mut free, &mut cursor, &mut max_end, &mut planned);
        }
    }
    let mut expiry: BTreeMap<usize, Vec<TensorId>> = BTreeMap::new();
    for (&root, &use_end) in &last_use {
        expiry.entry(use_end).or_default().push(root);
    }
    for (index, &node_id) in graph.nodes.iter().enumerate() {
        let (out_root, _) = resolve_root(tensors, node_id)?;
        let root_t = &tensors[out_root];
        if root_t.data_offset.is_none() && !planned.contains_key(&out_root) && def.get(&out_root) == Some(&index) {
            allocate(out_root, root_t.nbytes(), &mut free, &mut cursor, &mut max_end, &mut planned);
        }
        if let Some(dead) = expiry.get(&index) {
            for root in dead {
                if let Some(&offset) = planned.get(root) {
                    free.push((offset, align_up(tensors[*root].nbytes())));
                }
            }
        }
    }
    let mut bindings = BTreeMap::new();
    for tensor_id in needed {
        let t = &tensors[tensor_id];
        let (root, view_extra) = resolve_root(tensors, tensor_id)?;
        let root_t = &tensors[root];
        let root_offset = if let Some(offset) = root_t.data_offset {
            offset
        } else if let Some(&offset) = planned.get(&root) {
            offset
        } else {
            return Err(LlamaError::format(format!("tensor '{}' has no planned storage", t.name().unwrap_or("<unnamed>"))));
        };
        bindings.insert(tensor_id, root_offset + view_extra);
    }
    let mut q8_slots = Vec::new();
    let (mut q8_words, mut q8_pairs) = (0u64, 0u64);
    for (index, node) in nodes.iter().enumerate() {
        if let Sel::Gemv(_) = node.sel {
            let b = &tensors[tensors[node.node_id].src[1].unwrap()];
            let elems = (b.ne[0] * b.ne[1] * b.ne[2] * b.ne[3]) as u64;
            q8_slots.push((index, q8_words, q8_pairs));
            q8_words += elems.next_multiple_of(64);
            q8_pairs += (elems / 32 * 8).next_multiple_of(64);
        }
    }
    Ok(Plan { nodes, bindings, required_size: align_up(max_end), q8_slots, q8_words, q8_pairs })
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

struct ExecView<'a> {
    state: &'a State,
    arena: &'a Arena,
    ctx: &'a Context,
    plan: &'a Plan,
    /// Addresses of the int8 activation scratch (words, block scales) the
    /// gemv nodes quantise into, sized by `Plan::q8_scratch`.
    scratch: Option<(u64, u64)>,
    hazards: RefCell<Hazards>,
    /// Quantised activations still valid: (x address, k, m, column stride,
    /// words address, pairs address); a later matmul on the same x reuses
    /// them.
    q8_live: RefCell<Vec<(u64, u32, u32, u32, u64, u64)>>,
}

/// Device ranges read and written since the last barrier: a node that
/// reads what was written, or writes what was read or written, needs one
/// first; independent nodes run without.
#[derive(Default)]
struct Hazards {
    read: Vec<(u64, u64)>,
    written: Vec<(u64, u64)>,
}

fn overlaps(list: &[(u64, u64)], r: (u64, u64)) -> bool {
    list.iter().any(|&(a, b)| a < r.1 && r.0 < b)
}

impl Hazards {
    fn needs_barrier(&self, reads: &[(u64, u64)], writes: &[(u64, u64)]) -> bool {
        reads.iter().any(|r| overlaps(&self.written, *r))
            || writes.iter().any(|w| overlaps(&self.written, *w) || overlaps(&self.read, *w))
    }
    fn record(&mut self, reads: &[(u64, u64)], writes: &[(u64, u64)]) {
        self.read.extend_from_slice(reads);
        self.written.extend_from_slice(writes);
    }
    fn clear(&mut self) {
        self.read.clear();
        self.written.clear();
    }
}

/// Up to this many activation columns a matmul runs as the gemv; wider
/// (prefill batches) as the tiled matmul. Both on int8 activations.
pub const GEMV_MAX_COLUMNS: u32 = 4;

/// A byte stride in elements of `size` bytes, checked.
fn el(nb: usize, size: usize) -> Result<u32> {
    if nb % size != 0 {
        return Err(LlamaError::format(format!("stride {nb} is not a multiple of {size}")));
    }
    u32::try_from(nb / size).map_err(|_| LlamaError::format("stride exceeds u32"))
}

fn n32(v: i64) -> u32 {
    v as u32
}

/// Workgroups for `n` invocations of `wg`, folded into y past 65535.
fn groups_1d(n: usize, wg: u32) -> (u32, [u32; 3]) {
    let groups = n.div_ceil(wg as usize).max(1);
    let gx = groups.min(65535);
    let gy = groups.div_ceil(gx);
    (gx as u32, [gx as u32, gy as u32, 1])
}

impl ExecView<'_> {
    fn tensor(&self, id: TensorId) -> &Tensor {
        &self.ctx.tensors()[id]
    }

    fn addr(&self, id: TensorId) -> Result<u64> {
        let t = self.tensor(id);
        let offset = *self.plan.bindings.get(&id).ok_or_else(|| LlamaError::format(format!("tensor {id} has no device binding")))?;
        self.arena.address(offset, t.nbytes().max(1))
    }

    fn src_id(&self, t: &Tensor, i: usize) -> Result<TensorId> {
        t.src[i].ok_or_else(|| LlamaError::format(format!("node is missing src{i}")))
    }

    /// The device range a tensor occupies.
    fn range(&self, id: TensorId) -> Result<(u64, u64)> {
        let a = self.addr(id)?;
        Ok((a, a + self.tensor(id).nbytes().max(1) as u64))
    }

    fn dispatch_node(&self, s: &mut vkx::Stream, index: usize) -> Result<()> {
        let PlannedNode { node_id, sel } = self.plan.nodes[index];
        if sel == Sel::Skip {
            return Ok(());
        }
        let t = self.tensor(node_id).clone();
        let mut reads: Vec<(u64, u64)> = t.src.iter().flatten().map(|id| self.range(*id)).collect::<Result<_>>()?;
        if let Sel::NormMul | Sel::UnaryMul(_) = sel {
            // A fused node also reads its first node's input.
            let (first, _) = self.fused_parts(&t)?;
            let x = self.tensor(first).src[0].ok_or_else(|| LlamaError::format("fused node without input"))?;
            reads.push(self.range(x)?);
        }
        let writes = [self.range(node_id)?];
        if self.hazards.borrow().needs_barrier(&reads, &writes) {
            s.barrier();
            self.hazards.borrow_mut().clear();
        }
        self.dispatch_kernels(s, index, node_id, sel, &t)?;
        self.hazards.borrow_mut().record(&reads, &writes);
        // Quantised copies of what this node overwrote are stale.
        self.q8_live.borrow_mut().retain(|e| !overlaps(&writes, (e.0, e.0 + 1)));
        Ok(())
    }

    /// A fused Mul node's first node (the norm or activation) and the other
    /// Mul operand (the weight or gate).
    fn fused_parts(&self, t: &Tensor) -> Result<(TensorId, TensorId)> {
        let (s0, s1) = (self.src_id(t, 0)?, self.src_id(t, 1)?);
        let is_first = |id: TensorId| matches!(self.tensor(id).op, Op::RmsNorm | Op::Unary);
        if is_first(s0) {
            Ok((s0, s1))
        } else if is_first(s1) {
            Ok((s1, s0))
        } else {
            Err(LlamaError::format("fused Mul without its norm/activation"))
        }
    }

    /// A barrier inside one node's work (its dispatches depend on each
    /// other); it orders everything before it too.
    fn inner_barrier(&self, s: &mut vkx::Stream) {
        s.barrier();
        self.hazards.borrow_mut().clear();
    }

    fn dispatch_kernels(&self, s: &mut vkx::Stream, index: usize, node_id: TensorId, sel: Sel, t: &Tensor) -> Result<()> {
        let t = t.clone();
        match sel {
            Sel::Skip => return Ok(()),
            Sel::FloatGemm { f16 } => self.dispatch_float_gemm(s, node_id, f16)?,
            Sel::LayerNorm => self.dispatch_layer_norm(s, node_id)?,
            Sel::Softmax => self.dispatch_softmax(s, node_id)?,
            Sel::RopeVision => self.dispatch_vision_rope(s, node_id)?,
            Sel::UpscaleBilinear { aa } => self.dispatch_upscale(s, node_id, aa)?,
            Sel::VisionAttn => self.dispatch_vision_attention(s, node_id)?,
            Sel::Copy => {
                let sid = self.src_id(&t, 0)?;
                let src = self.tensor(sid).clone();
                self.copy_f32(s, self.addr(sid)?, &src.ne, &src.nb, self.addr(node_id)?, &t.ne, &t.nb)?;
            }
            Sel::Concat => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = self.tensor(a_id).clone();
                let b = self.tensor(b_id).clone();
                let axis = (0..4).find(|d| a.ne[*d] + b.ne[*d] == t.ne[*d]).ok_or_else(|| LlamaError::format("concat axis could not be derived"))?;
                let dst = self.addr(node_id)?;
                self.copy_f32(s, self.addr(a_id)?, &a.ne, &a.nb, dst, &a.ne, &t.nb)?;
                self.copy_f32(s, self.addr(b_id)?, &b.ne, &b.nb, dst + (a.ne[axis] as usize * t.nb[axis]) as u64, &b.ne, &t.nb)?;
            }
            Sel::GetRowsF32 => {
                let sid = self.src_id(&t, 0)?;
                let rid = self.src_id(&t, 1)?;
                let src = self.tensor(sid);
                let rows = self.tensor(rid);
                let nrows = (rows.ne[0] * rows.ne[1] * rows.ne[2]) as u32;
                let k = self.state.kernel("get_rows_f32")?;
                let args = k.args().buffer("src", self.addr(sid)?).buffer("rows", self.addr(rid)?).buffer("dst", self.addr(node_id)?)
                    .u32("ne0", n32(t.ne[0])).u32("src_row", el(src.nb[1], 4)?).u32("dst_row", el(t.nb[1], 4)?);
                s.dispatch(&args, [n32(t.ne[0]).div_ceil(256), nrows, 1]);
            }
            Sel::GetRowsQ(q) => {
                let sid = self.src_id(&t, 0)?;
                let rid = self.src_id(&t, 1)?;
                let src = self.tensor(sid);
                let rows = self.tensor(rid);
                if src.ne[0] % 256 != 0 {
                    return Err(LlamaError::unsupported("quantized rows not a multiple of 256"));
                }
                let nrows = (rows.ne[0] * rows.ne[1] * rows.ne[2]) as u32;
                let k = self.state.kernel(q.rows_kernel())?;
                let args = k.args().buffer("w", self.addr(sid)?).buffer("rows", self.addr(rid)?).buffer("dst", self.addr(node_id)?)
                    .u32("ne0", n32(t.ne[0])).u32("w_row", kernel_row_bytes(src.desc.ty, src.ne[0])? as u32).u32("dst_row", el(t.nb[1], 4)?);
                s.dispatch(&args, [n32(t.ne[0]).div_ceil(256), nrows, 1]);
            }
            Sel::SetRows { f16 } => {
                let sid = self.src_id(&t, 0)?;
                let rid = self.src_id(&t, 1)?;
                let src = self.tensor(sid);
                let k = self.state.kernel(if f16 { "set_rows_f16" } else { "set_rows_f32" })?;
                let args = k.args().buffer("src", self.addr(sid)?).buffer("rows", self.addr(rid)?).buffer("dst", self.addr(node_id)?)
                    .u32("ne0", n32(src.ne[0])).u32("src_row", el(src.nb[1], 4)?).u32("dst_row", el(t.nb[1], if f16 { 2 } else { 4 })?);
                s.dispatch(&args, [n32(src.ne[0]).div_ceil(256), n32(src.ne[1]), 1]);
            }
            Sel::Norm { l2 } => {
                let sid = self.src_id(&t, 0)?;
                let src = self.tensor(sid);
                if src.nb[0] != 4 || t.nb[0] != 4 {
                    return Err(LlamaError::unsupported("norm over a strided dim 0"));
                }
                let k = self.state.kernel("norm")?;
                let args = k.args().buffer("x", self.addr(sid)?).buffer("dst", self.addr(node_id)?)
                    .u32("ne0", n32(src.ne[0])).u32("ne1", n32(src.ne[1])).u32("ne2", n32(src.ne[2]))
                    .u32("x_nb1", el(src.nb[1], 4)?).u32("x_nb2", el(src.nb[2], 4)?).u32("x_nb3", el(src.nb[3], 4)?)
                    .u32("d_nb1", el(t.nb[1], 4)?).u32("d_nb2", el(t.nb[2], 4)?).u32("d_nb3", el(t.nb[3], 4)?)
                    .f32("eps", t.op_param_f32(0)).u32("l2", l2 as u32);
                s.dispatch(&args, [n32(src.ne[1] * src.ne[2] * src.ne[3]), 1, 1]);
            }
            Sel::NormMul => {
                let (rms_id, w_id) = self.fused_parts(&t)?;
                let rms = self.tensor(rms_id);
                let x_id = self.src_id(rms, 0)?;
                let src = self.tensor(x_id);
                let k = self.state.kernel("norm")?;
                let args = k.args().buffer("x", self.addr(x_id)?).buffer("dst", self.addr(node_id)?).buffer("w", self.addr(w_id)?)
                    .u32("ne0", n32(src.ne[0])).u32("ne1", n32(src.ne[1])).u32("ne2", n32(src.ne[2]))
                    .u32("x_nb1", el(src.nb[1], 4)?).u32("x_nb2", el(src.nb[2], 4)?).u32("x_nb3", el(src.nb[3], 4)?)
                    .u32("d_nb1", el(t.nb[1], 4)?).u32("d_nb2", el(t.nb[2], 4)?).u32("d_nb3", el(t.nb[3], 4)?)
                    .f32("eps", rms.op_param_f32(0)).u32("l2", 0).u32("w_mode", 1);
                s.dispatch(&args, [n32(src.ne[1] * src.ne[2] * src.ne[3]), 1, 1]);
            }
            Sel::UnaryMul(op) => {
                let (u_id, g_id) = self.fused_parts(&t)?;
                let x_id = self.src_id(self.tensor(u_id), 0)?;
                let n = t.ne.iter().product::<i64>() as usize;
                let k = self.state.kernel("unary")?;
                let (gx, groups) = groups_1d(n, 256);
                let args = k.args().buffer("x", self.addr(x_id)?).buffer("g", self.addr(g_id)?).buffer("dst", self.addr(node_id)?)
                    .u32("n", n as u32).u32("groups_x", gx).u32("op", op).u32("gated", 1);
                s.dispatch(&args, groups);
            }
            Sel::RopeMulti => {
                let sid = self.src_id(&t, 0)?;
                let pid = self.src_id(&t, 1)?;
                let src = self.tensor(sid);
                let n_dims = t.op_param_i32(1);
                let mode = t.op_param_i32(2);
                let n_ctx_orig = t.op_param_i32(4);
                let freq_base = t.op_param_f32(5);
                let beta_fast = t.op_param_f32(9);
                let beta_slow = t.op_param_f32(10);
                let corr = |n_rot: f32| -> f32 {
                    n_dims as f32 * ((n_ctx_orig as f32 / (n_rot * 2.0 * std::f32::consts::PI)).ln()) / (2.0 * freq_base.ln())
                };
                let corr0 = corr(beta_fast).floor().max(0.0);
                let corr1 = corr(beta_slow).ceil().min(n_dims as f32 - 1.0);
                let k = self.state.kernel("rope_multi")?;
                let args = k.args().buffer("src", self.addr(sid)?).buffer("pos", self.addr(pid)?).buffer("dst", self.addr(node_id)?)
                    .u32("ne0", n32(src.ne[0])).u32("ne2", n32(src.ne[2])).u32("imrope", (mode == GGML_ROPE_TYPE_IMROPE) as u32)
                    .u32("n_dims", n_dims as u32)
                    .u32("sect_0", t.op_param_i32(11) as u32).u32("sect_1", t.op_param_i32(12) as u32)
                    .u32("sect_2", t.op_param_i32(13) as u32).u32("sect_3", t.op_param_i32(14) as u32)
                    .f32("freq_base", freq_base).f32("freq_scale", t.op_param_f32(6))
                    .f32("ext_factor", t.op_param_f32(7)).f32("attn_factor", t.op_param_f32(8))
                    .f32("corr0", corr0).f32("corr1", corr1)
                    .u32("s_nb0", el(src.nb[0], 4)?).u32("s_nb1", el(src.nb[1], 4)?).u32("s_nb2", el(src.nb[2], 4)?).u32("s_nb3", el(src.nb[3], 4)?)
                    .u32("d_nb0", el(t.nb[0], 4)?).u32("d_nb1", el(t.nb[1], 4)?).u32("d_nb2", el(t.nb[2], 4)?).u32("d_nb3", el(t.nb[3], 4)?);
                s.dispatch(&args, [n32(src.ne[1]), n32(src.ne[2]), n32(src.ne[3])]);
            }
            Sel::Unary(op) => {
                let sid = self.src_id(&t, 0)?;
                let n = t.ne.iter().product::<i64>() as usize;
                let k = self.state.kernel("unary")?;
                let (gx, groups) = groups_1d(n, 256);
                let args = k.args().buffer("x", self.addr(sid)?).buffer("dst", self.addr(node_id)?)
                    .u32("n", n as u32).u32("groups_x", gx).u32("op", op);
                s.dispatch(&args, groups);
            }
            Sel::Glu(op) => {
                let a = self.src_id(&t, 0)?;
                let b = self.src_id(&t, 1)?;
                let n = t.ne.iter().product::<i64>() as usize;
                let k = self.state.kernel("glu")?;
                let (gx, groups) = groups_1d(n, 256);
                let args = k.args().buffer("a", self.addr(a)?).buffer("b", self.addr(b)?).buffer("dst", self.addr(node_id)?)
                    .u32("n", n as u32).u32("groups_x", gx).u32("op", op);
                s.dispatch(&args, groups);
            }
            Sel::Binary(op) => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = self.tensor(a_id);
                let b = self.tensor(b_id);
                let k = self.state.kernel("binary")?;
                let args = k.args().buffer("a", self.addr(a_id)?).buffer("b", self.addr(b_id)?).buffer("dst", self.addr(node_id)?)
                    .u32("op", op).u32("ne0", n32(t.ne[0])).u32("ne1", n32(t.ne[1])).u32("ne2", n32(t.ne[2]))
                    .u32("b_ne0", n32(b.ne[0])).u32("b_ne1", n32(b.ne[1])).u32("b_ne2", n32(b.ne[2])).u32("b_ne3", n32(b.ne[3]))
                    .u32("a_nb0", el(a.nb[0], 4)?).u32("a_nb1", el(a.nb[1], 4)?).u32("a_nb2", el(a.nb[2], 4)?).u32("a_nb3", el(a.nb[3], 4)?)
                    .u32("b_nb0", el(b.nb[0], 4)?).u32("b_nb1", el(b.nb[1], 4)?).u32("b_nb2", el(b.nb[2], 4)?).u32("b_nb3", el(b.nb[3], 4)?)
                    .u32("d_nb0", el(t.nb[0], 4)?).u32("d_nb1", el(t.nb[1], 4)?).u32("d_nb2", el(t.nb[2], 4)?).u32("d_nb3", el(t.nb[3], 4)?);
                s.dispatch(&args, [n32(t.ne[0]).div_ceil(256), n32(t.ne[1]), n32(t.ne[2] * t.ne[3])]);
            }
            Sel::Gemv(q) => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = self.tensor(a_id);
                let b = self.tensor(b_id);
                let k_dim = a.ne[0] as u32;
                let n = a.ne[1] as u32;
                let m = (b.ne[1] * b.ne[2] * b.ne[3]) as u32;
                if a.nb[1] != ggml_row_size_for_type(a.desc.ty, a.ne[0]).map_err(LlamaError::format)? {
                    return Err(LlamaError::unsupported("quantized weight rows are not packed"));
                }
                let row_bytes = kernel_row_bytes(a.desc.ty, a.ne[0])?;
                let (wa, xa) = (self.addr(a_id)?, self.addr(b_id)?);
                if wa % 16 != 0 || xa % 16 != 0 || b.nb[1] % 16 != 0 {
                    return Err(LlamaError::unsupported("gemv needs 16-byte aligned weights and activation columns"));
                }
                let (base_q, base_s) = self.scratch.ok_or_else(|| LlamaError::format("gemv without activation scratch"))?;
                let (off_q, off_s) = self.plan.q8_slot(index).ok_or_else(|| LlamaError::format("gemv without a scratch slot"))?;
                let x_col = el(b.nb[1], 4)?;
                let live = self.q8_live.borrow().iter().find(|e| (e.0, e.1, e.2, e.3) == (xa, k_dim, m, x_col)).map(|e| (e.4, e.5));
                let (xq, xs) = match live {
                    Some(slot) => slot,
                    None => {
                        let (xq, xs) = (base_q + off_q, base_s + off_s);
                        let blocks = k_dim / 32 * m;
                        let quant = self.state.kernel("quantize_q8")?;
                        let qargs = quant.args().buffer("x", xa).buffer("xq", xq).buffer("xs", xs)
                            .u32("blocks", blocks).u32("k", k_dim).u32("x_col", x_col);
                        s.dispatch(&qargs, [blocks.div_ceil(64), 1, 1]);
                        self.inner_barrier(s);
                        self.q8_live.borrow_mut().push((xa, k_dim, m, x_col, xq, xs));
                        (xq, xs)
                    }
                };
                if m > GEMV_MAX_COLUMNS {
                    // The 64x64 register tile (4x8 results per invocation)
                    // from 32 columns on; below that, and with
                    // MAKEPAD_LLM_PREFILL_GEMM=main, the 32x32 tile.
                    let main = m < 32 || std::env::var("MAKEPAD_LLM_PREFILL_GEMM").is_ok_and(|v| v == "main");
                    let (tile_n, tile_m) = if main { (32, 32) } else { (64, 64) };
                    let name = if main { q.gemm_kernel().to_string() } else { format!("{}_r8k16", q.gemm_kernel()) };
                    let kernel = self.state.kernel(&name)?;
                    let args = kernel.args().buffer("w", wa).buffer("w4", wa).buffer("xq", xq).buffer("xq4", xq).buffer("xs", xs).buffer("dst", self.addr(node_id)?)
                        .u32("k", k_dim).u32("n", n).u32("m", m).u32("w_row", row_bytes as u32).u32("d_col", el(t.nb[1], 4)?);
                    s.dispatch(&args, [n.div_ceil(tile_n), m.div_ceil(tile_m), 1]);
                    return Ok(());
                }
                let kernel = self.state.kernel(q.kernel())?;
                let args = kernel.args().buffer("w", wa).buffer("w4", wa).buffer("xq", xq).buffer("xq4", xq).buffer("xs", xs).buffer("xs4", xs).buffer("dst", self.addr(node_id)?)
                    .u32("k", k_dim).u32("n", n).u32("m", m).u32("w_row", row_bytes as u32).u32("d_col", el(t.nb[1], 4)?);
                // One row per 32-lane group, 8 per 256-invocation workgroup.
                s.dispatch(&args, [n.div_ceil(8), 1, 1]);
            }
            Sel::SsmConv => {
                let s_id = self.src_id(&t, 0)?;
                let w_id = self.src_id(&t, 1)?;
                let src = self.tensor(s_id);
                let w = self.tensor(w_id);
                if w.nb[0] != 4 {
                    return Err(LlamaError::unsupported("ssm_conv weights strided along the kernel"));
                }
                let k = self.state.kernel("ssm_conv")?;
                let args = k.args().buffer("src", self.addr(s_id)?).buffer("w", self.addr(w_id)?).buffer("dst", self.addr(node_id)?)
                    .u32("d_conv", n32(w.ne[0])).u32("d_inner", n32(w.ne[1]))
                    .u32("s_nb0", el(src.nb[0], 4)?).u32("s_nb1", el(src.nb[1], 4)?).u32("s_nb2", el(src.nb[2], 4)?)
                    .u32("w_nb1", el(w.nb[1], 4)?)
                    .u32("d_nb0", el(t.nb[0], 4)?).u32("d_nb1", el(t.nb[1], 4)?).u32("d_nb2", el(t.nb[2], 4)?);
                s.dispatch(&args, [n32(w.ne[1]).div_ceil(128), n32(t.ne[1]), n32(t.ne[2])]);
            }
            Sel::GatedDeltaNet => {
                let ids: Vec<TensorId> = (0..6).map(|i| self.src_id(&t, i)).collect::<Result<_>>()?;
                let q = self.tensor(ids[0]);
                let kk = self.tensor(ids[1]);
                let v = self.tensor(ids[2]);
                let g = self.tensor(ids[3]);
                let sv = v.ne[0] as u32;
                let (qa, ka, sa, da) = (self.addr(ids[0])?, self.addr(ids[1])?, self.addr(ids[5])?, self.addr(node_id)?);
                let qk_strides = [q.nb[1], q.nb[2], q.nb[3], kk.nb[1], kk.nb[2], kk.nb[3]];
                let v4 = sv == 128 && q.nb[0] == 4 && kk.nb[0] == 4
                    && qk_strides.iter().all(|nb| nb % 16 == 0) && [qa, ka, sa, da].iter().all(|a| a % 16 == 0);
                let k = self.state.kernel(match (v4, t.op_param_i32(0) != 0) {
                    (false, false) => "gated_delta_net",
                    (true, false) => "gated_delta_net_v4",
                    (false, true) => "gated_delta_net_checkpoints",
                    (true, true) => "gated_delta_net_v4_checkpoints",
                })?;
                let args = k.args()
                    .buffer("q", qa).buffer("k", ka).buffer("v", self.addr(ids[2])?)
                    .buffer("g", self.addr(ids[3])?).buffer("beta", self.addr(ids[4])?).buffer("state", sa)
                    .buffer("dst", da).buffer("q4", qa).buffer("k4", ka).buffer("state4", sa).buffer("dst4", da)
                    .u32("sv", sv).u32("h", n32(v.ne[1])).u32("n_tokens", n32(v.ne[2])).u32("n_seqs", n32(v.ne[3]))
                    .u32("gate_g", n32(g.ne[0])).u32("q_heads", n32(q.ne[1])).u32("k_heads", n32(kk.ne[1]))
                    .u32("q_h", el(q.nb[1], 4)?).u32("q_t", el(q.nb[2], 4)?).u32("q_s", el(q.nb[3], 4)?)
                    .u32("k_h", el(kk.nb[1], 4)?).u32("k_t", el(kk.nb[2], 4)?).u32("k_s", el(kk.nb[3], 4)?)
                    .u32("v_h", el(v.nb[1], 4)?).u32("v_t", el(v.nb[2], 4)?).u32("v_s", el(v.nb[3], 4)?);
                s.dispatch(&args, [sv.div_ceil(4), n32(v.ne[1]), n32(v.ne[3])]);
            }
            Sel::Attn => {
                let ids: Vec<TensorId> = (0..4).map(|i| self.src_id(&t, i)).collect::<Result<_>>()?;
                let q = self.tensor(ids[0]);
                let kk = self.tensor(ids[1]);
                let v = self.tensor(ids[2]);
                let mask = self.tensor(ids[3]);
                if q.nb[0] != 4 || kk.nb[0] != 2 || v.nb[0] != 2 {
                    return Err(LlamaError::unsupported("attention over strided head dims"));
                }
                let k = self.state.kernel("attn_decode")?;
                let args = k.args()
                    .buffer("q", self.addr(ids[0])?).buffer("k", self.addr(ids[1])?).buffer("v", self.addr(ids[2])?)
                    .buffer("mask", self.addr(ids[3])?).buffer("dst", self.addr(node_id)?)
                    .u32("d", n32(q.ne[0])).u32("dv", n32(v.ne[0])).u32("kc", n32(kk.ne[1]))
                    .u32("h", n32(q.ne[2])).u32("hkv", n32(kk.ne[2])).f32("scale", t.op_param_f32(0))
                    .u32("q_nb1", el(q.nb[1], 4)?).u32("q_nb2", el(q.nb[2], 4)?)
                    .u32("k_nb1", el(kk.nb[1], 2)?).u32("k_nb2", el(kk.nb[2], 2)?)
                    .u32("v_nb1", el(v.nb[1], 2)?).u32("v_nb2", el(v.nb[2], 2)?)
                    .u32("m_nb1", el(mask.nb[1], 2)?)
                    .u32("d_nb1", el(t.nb[1], 4)?).u32("d_nb2", el(t.nb[2], 4)?);
                s.dispatch(&args, [n32(q.ne[2]), n32(q.ne[1]), 1]);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_f32(&self, s: &mut vkx::Stream, src: u64, s_ne: &[i64; 4], s_nb: &[usize; 4], dst: u64, d_ne: &[i64; 4], d_nb: &[usize; 4]) -> Result<()> {
        let n = s_ne.iter().product::<i64>() as usize;
        let k = self.state.kernel("copy_f32")?;
        let (gx, groups) = groups_1d(n, 256);
        let args = k.args().buffer("src", src).buffer("dst", dst).u32("n", n as u32).u32("groups_x", gx)
            .u32("s_ne0", n32(s_ne[0])).u32("s_ne1", n32(s_ne[1])).u32("s_ne2", n32(s_ne[2]))
            .u32("s_nb0", el(s_nb[0], 4)?).u32("s_nb1", el(s_nb[1], 4)?).u32("s_nb2", el(s_nb[2], 4)?).u32("s_nb3", el(s_nb[3], 4)?)
            .u32("d_ne0", n32(d_ne[0])).u32("d_ne1", n32(d_ne[1])).u32("d_ne2", n32(d_ne[2]))
            .u32("d_nb0", el(d_nb[0], 4)?).u32("d_nb1", el(d_nb[1], 4)?).u32("d_nb2", el(d_nb[2], 4)?).u32("d_nb3", el(d_nb[3], 4)?);
        s.dispatch(&args, groups);
        Ok(())
    }

    fn dispatch_float_gemm(&self, s: &mut vkx::Stream, id: TensorId, f16: bool) -> Result<()> {
        let t = self.tensor(id);
        let aid = self.src_id(t, 0)?;
        let bid = self.src_id(t, 1)?;
        let a = self.tensor(aid);
        let b = self.tensor(bid);
        let size = if f16 { 2 } else { 4 };
        let weights = if f16 { vkx::ActivationType::F16 } else { vkx::ActivationType::F32 };
        // Contiguous, unbatched X[m,k] * W[n,k]^T. Strided views and batch
        // broadcasts keep the general kernel below. Pipeline specializations
        // stay in the library for the recorded graph's entire replay lifetime.
        if a.is_contiguous() && b.is_contiguous() && t.is_contiguous()
            && a.ne[2..] == [1, 1] && b.ne[2..] == [1, 1]
            && vkx::Gemm::supports(&self.state.device, vkx::ActivationType::F32,
                vkx::GemmMode::Register { rows: 4, weights }).is_ok()
        {
            let wa = self.addr(aid)?;
            let xa = self.addr(bid)?;
            let dst = self.addr(id)?;
            if wa % 16 == 0 && xa % 16 == 0 && dst % 16 == 0 {
                let gemm = vkx::Gemm::new_nt(&self.state.device, &mut self.state.library.borrow_mut(),
                    n32(a.ne[1]), n32(a.ne[0]), n32(b.ne[1]), weights, n32(a.ne[0])).map_err(vk)?;
                gemm.encode_addresses(s, wa, xa, dst, None).map_err(vk)?;
                return Ok(());
            }
        }
        let kernel = self.state.kernel(if f16 { "vision_gemm_f16" } else { "vision_gemm_f32" })?;
        let args = kernel.args().buffer("w", self.addr(aid)?).buffer("wh", self.addr(aid)?).buffer("x", self.addr(bid)?).buffer("dst", self.addr(id)?)
            .u32("k", n32(a.ne[0])).u32("n", n32(a.ne[1])).u32("m", n32(b.ne[1]))
            .u32("b2", n32(b.ne[2])).u32("ratio2", n32(b.ne[2] / a.ne[2])).u32("ratio3", n32(b.ne[3] / a.ne[3]))
            .u32("w0", el(a.nb[0], size)?).u32("w1", el(a.nb[1], size)?).u32("w2", el(a.nb[2], size)?).u32("w3", el(a.nb[3], size)?)
            .u32("x0", el(b.nb[0], 4)?).u32("x1", el(b.nb[1], 4)?).u32("x2", el(b.nb[2], 4)?).u32("x3", el(b.nb[3], 4)?);
        s.dispatch(&args, [n32(a.ne[1]).div_ceil(32), n32(b.ne[1]).div_ceil(32), n32(b.ne[2] * b.ne[3])]);
        Ok(())
    }

    fn dispatch_layer_norm(&self, s: &mut vkx::Stream, id: TensorId) -> Result<()> {
        let t = self.tensor(id);
        let sid = self.src_id(t, 0)?;
        let x = self.tensor(sid);
        let rows = (x.ne[1] * x.ne[2] * x.ne[3]) as usize;
        let (gx, groups) = groups_1d(rows, 1);
        let kernel = self.state.kernel("vision_layer_norm")?;
        let args = kernel.args().buffer("x", self.addr(sid)?).buffer("dst", self.addr(id)?)
            .u32("ne0", n32(x.ne[0])).u32("ne1", n32(x.ne[1])).u32("ne2", n32(x.ne[2]))
            .u32("rows", rows as u32).u32("groups_x", gx)
            .u32("x0", el(x.nb[0], 4)?).u32("x1", el(x.nb[1], 4)?).u32("x2", el(x.nb[2], 4)?).u32("x3", el(x.nb[3], 4)?)
            .f32("eps", t.op_param_f32(0));
        s.dispatch(&args, groups);
        Ok(())
    }

    fn dispatch_softmax(&self, s: &mut vkx::Stream, id: TensorId) -> Result<()> {
        let t = self.tensor(id);
        let sid = self.src_id(t, 0)?;
        let x = self.tensor(sid);
        let rows = (x.ne[1] * x.ne[2] * x.ne[3]) as usize;
        let (gx, groups) = groups_1d(rows, 1);
        let kernel = self.state.kernel("vision_softmax")?;
        let mut args = kernel.args().buffer("x", self.addr(sid)?).buffer("dst", self.addr(id)?)
            .u32("ne0", n32(x.ne[0])).u32("ne1", n32(x.ne[1])).u32("ne2", n32(x.ne[2]))
            .u32("rows", rows as u32).u32("groups_x", gx)
            .u32("x1", el(x.nb[1], 4)?).u32("x2", el(x.nb[2], 4)?).u32("x3", el(x.nb[3], 4)?)
            .f32("scale", t.op_param_f32(0)).u32("has_mask", t.src[1].is_some() as u32)
            .buffer("mask", self.addr(sid)?).u32("mask_rows", 1);
        if let Some(mid) = t.src[1] {
            let mask = self.tensor(mid);
            args = args.buffer("mask", self.addr(mid)?).u32("mask_rows", n32(mask.ne[1])).u32("mask_stride", el(mask.nb[1], 4)?);
        }
        s.dispatch(&args, groups);
        Ok(())
    }

    fn dispatch_vision_rope(&self, s: &mut vkx::Stream, id: TensorId) -> Result<()> {
        let t = self.tensor(id);
        let sid = self.src_id(t, 0)?;
        let pid = self.src_id(t, 1)?;
        let x = self.tensor(sid);
        let nd = t.op_param_i32(1) as f32;
        let freq_base = t.op_param_f32(5);
        let corr = |rot: f32| nd * (t.op_param_i32(4) as f32 / (rot * 2.0 * std::f32::consts::PI)).ln() / (2.0 * freq_base.ln());
        let kernel = self.state.kernel("vision_rope")?;
        let args = kernel.args().buffer("src", self.addr(sid)?).buffer("pos", self.addr(pid)?).buffer("dst", self.addr(id)?)
            .u32("ne0", n32(x.ne[0])).u32("ne1", n32(x.ne[1])).u32("ne2", n32(x.ne[2]))
            .u32("n_dims", t.op_param_i32(1) as u32).u32("sect0", t.op_param_i32(11) as u32).u32("sect1", t.op_param_i32(12) as u32)
            .f32("freq_base", freq_base).f32("freq_scale", t.op_param_f32(6)).f32("ext_factor", t.op_param_f32(7)).f32("attn_factor", t.op_param_f32(8))
            .f32("corr0", corr(t.op_param_f32(9)).floor().max(0.0)).f32("corr1", corr(t.op_param_f32(10)).ceil().min(nd - 1.0))
            .u32("s0", el(x.nb[0], 4)?).u32("s1", el(x.nb[1], 4)?).u32("s2", el(x.nb[2], 4)?).u32("s3", el(x.nb[3], 4)?);
        s.dispatch(&args, [n32(x.ne[1]), n32(x.ne[2]), n32(x.ne[3])]);
        Ok(())
    }

    fn dispatch_upscale(&self, s: &mut vkx::Stream, id: TensorId, aa: bool) -> Result<()> {
        let t = self.tensor(id);
        let sid = self.src_id(t, 0)?;
        let x = self.tensor(sid);
        let corners = t.op_param_i32(0) & crate::core::GGML_SCALE_FLAG_ALIGN_CORNERS != 0;
        let factor = |d: usize| {
            if corners && t.ne[d] > 1 && x.ne[d] > 1 {
                (t.ne[d] - 1) as f32 / (x.ne[d] - 1) as f32
            } else {
                t.ne[d] as f32 / x.ne[d] as f32
            }
        };
        let n = t.ne.iter().product::<i64>() as usize;
        let (gx, groups) = groups_1d(n, 256);
        let kernel = self.state.kernel("vision_upscale")?;
        let args = kernel.args().buffer("src", self.addr(sid)?).buffer("dst", self.addr(id)?)
            .u32("n", n as u32).u32("groups_x", gx).u32("sw", n32(x.ne[0])).u32("sh", n32(x.ne[1]))
            .u32("dw", n32(t.ne[0])).u32("dh", n32(t.ne[1])).u32("dc", n32(t.ne[2]))
            .u32("s0", el(x.nb[0], 4)?).u32("s1", el(x.nb[1], 4)?).u32("s2", el(x.nb[2], 4)?).u32("s3", el(x.nb[3], 4)?)
            .f32("sfx", factor(0)).f32("sfy", factor(1)).f32("sf2", t.ne[2] as f32 / x.ne[2] as f32).f32("sf3", t.ne[3] as f32 / x.ne[3] as f32)
            .f32("poffs", if corners { 0.0 } else { 0.5 }).u32("aa", aa as u32);
        s.dispatch(&args, groups);
        Ok(())
    }

    fn dispatch_vision_attention(&self, s: &mut vkx::Stream, id: TensorId) -> Result<()> {
        let t = self.tensor(id);
        let qid = self.src_id(t, 0)?;
        let kid = self.src_id(t, 1)?;
        let vid = self.src_id(t, 2)?;
        let q = self.tensor(qid);
        let k = self.tensor(kid);
        let v = self.tensor(vid);
        // 64-channel heads with vec4-aligned rows: the register-tiled
        // attention (64 queries per workgroup); anything else the panel kernel.
        let (qa, ka, va, da) = (self.addr(qid)?, self.addr(kid)?, self.addr(vid)?, self.addr(id)?);
        let strides = [q.nb[1], q.nb[2], k.nb[1], k.nb[2], v.nb[1], v.nb[2]];
        if q.ne[0] == 64 && v.ne[0] == 64 && k.ne[2] == q.ne[2] && q.ne[3] == 1 && q.nb[0] == 4 && k.nb[0] == 4 && v.nb[0] == 4
            && strides.iter().all(|nb| nb % 16 == 0) && [qa, ka, va, da].iter().all(|a| a % 16 == 0)
        {
            if let Ok(kernel) = self.state.kernel("attention_d64") {
                let h = n32(q.ne[2]);
                let args = kernel.args().buffer("q", qa).buffer("k", ka).buffer("v", va).buffer("dst", da)
                    .u32("nq", n32(q.ne[1])).u32("nk", n32(k.ne[1])).f32("scale", t.op_param_f32(0))
                    .u32("q_row", (q.nb[1] / 16) as u32).u32("q_head", (q.nb[2] / 16) as u32)
                    .u32("k_row", (k.nb[1] / 16) as u32).u32("k_head", (k.nb[2] / 16) as u32)
                    .u32("v_row", (v.nb[1] / 16) as u32).u32("v_head", (v.nb[2] / 16) as u32)
                    .u32("d_row", h * 16).u32("d_head", 16);
                s.dispatch(&args, [h, n32(q.ne[1]).div_ceil(64), 1]);
                return Ok(());
            }
        }
        let panels = n32(v.ne[0]).div_ceil(128);
        let kernel = self.state.kernel("vision_flash_f32")?;
        let args = kernel.args().buffer("q", self.addr(qid)?).buffer("k", self.addr(kid)?).buffer("v", self.addr(vid)?).buffer("dst", self.addr(id)?)
            .u32("d", n32(q.ne[0])).u32("dv", n32(v.ne[0])).u32("nq", n32(q.ne[1])).u32("nk", n32(k.ne[1]))
            .u32("h", n32(q.ne[2])).u32("hkv", n32(k.ne[2])).u32("panels", panels)
            .u32("q1", el(q.nb[1], 4)?).u32("q2", el(q.nb[2], 4)?).u32("q3", el(q.nb[3], 4)?)
            .u32("k1", el(k.nb[1], 4)?).u32("k2", el(k.nb[2], 4)?).u32("k3", el(k.nb[3], 4)?)
            .u32("v1", el(v.nb[1], 4)?).u32("v2", el(v.nb[2], 4)?).u32("v3", el(v.nb[3], 4)?)
            .f32("scale", t.op_param_f32(0));
        s.dispatch(&args, [n32(q.ne[1]).div_ceil(8), n32(q.ne[2]), n32(q.ne[3]) * panels]);
        Ok(())
    }
}
