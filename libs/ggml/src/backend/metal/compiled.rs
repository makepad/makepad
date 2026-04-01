use std::collections::BTreeMap;

use crate::context::Context;
use crate::core::{ggml_pad, GGML_MEM_ALIGN};
use crate::graph::{Graph, NodeId};
use crate::op::Op;
use crate::tensor::{
    ggml_blck_size_for_type, ggml_type_size_for_type, BufferUsage, Tensor, TensorId, TensorType,
};

use super::selector::FC_SSM_CONV;
use super::{
    build_graph_plan, BufferStorageMode, FunctionConstantValue, MetalBufferBindingRef,
    MetalDeviceFeatures, MetalGraphNodePlan, MetalPipeline, MetalRuntime, MetalSize,
    MetalStageKind,
};

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsMulMm {
    ne00: i32,
    ne02: i32,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne12: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    ne0: i32,
    ne1: i32,
    r2: i16,
    r3: i16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsMulMv {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne10: i32,
    ne11: i32,
    ne12: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    ne0: i32,
    ne1: i32,
    nr0: i32,
    r2: i16,
    r3: i16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsMulMvExt {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne10: i32,
    ne11: i32,
    ne12: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    ne0: i32,
    ne1: i32,
    r2: i16,
    r3: i16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsCpy {
    nk0: i64,
    ne00: i64,
    ne01: i64,
    ne02: i64,
    ne03: i64,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne0: i64,
    ne1: i64,
    ne2: i64,
    ne3: i64,
    nb0: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsBin {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    ne03: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne10: i32,
    ne11: i32,
    ne12: i32,
    ne13: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    nb0: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
    offs: u64,
    o1: [u64; 8],
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsAddId {
    ne0: i64,
    ne1: i64,
    nb01: u64,
    nb02: u64,
    nb11: u64,
    nb21: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsConcat {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    ne03: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne10: i32,
    ne11: i32,
    ne12: i32,
    ne13: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    nb0: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
    dim: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsUnary {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    ne03: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    nb0: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
    slope: f32,
    scale: f32,
    bias: f32,
    val: f32,
    min: f32,
    max: f32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsSumRows {
    ne00: i64,
    ne01: i64,
    ne02: i64,
    ne03: i64,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne0: i64,
    ne1: i64,
    ne2: i64,
    ne3: i64,
    nb0: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsSoftMax {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne11: i32,
    ne12: i32,
    ne13: i32,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
    scale: f32,
    max_bias: f32,
    m0: f32,
    m1: f32,
    n_head_log2: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsL2Norm {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    ne03: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    nb0: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
    eps: f32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsNorm {
    ne00: i32,
    ne00_t: i32,
    nb1: u64,
    nb2: u64,
    nb3: u64,
    eps: f32,
    nef1: [i32; 3],
    nef2: [i32; 3],
    nef3: [i32; 3],
    nbf1: [u64; 3],
    nbf2: [u64; 3],
    nbf3: [u64; 3],
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsGetRows {
    ne00t: i32,
    ne00: i32,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne10: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsSetRows {
    nk0: i32,
    ne01: i32,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne11: i32,
    ne12: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsRope {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    ne03: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    nb0: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
    n_past: i32,
    n_dims: i32,
    n_ctx_orig: i32,
    freq_base: f32,
    freq_scale: f32,
    ext_factor: f32,
    attn_factor: f32,
    beta_fast: f32,
    beta_slow: f32,
    sect_0: i32,
    sect_1: i32,
    sect_2: i32,
    sect_3: i32,
    src2: bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsFlashAttnExtPad {
    ne11: i32,
    ne_12_2: i32,
    ne_12_3: i32,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    nb21: u64,
    nb22: u64,
    nb23: u64,
    ne31: i32,
    ne32: i32,
    ne33: i32,
    nb31: u64,
    nb32: u64,
    nb33: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsFlashAttnExtVec {
    ne01: i32,
    ne02: i32,
    ne03: i32,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne11: i32,
    ne_12_2: i32,
    ne_12_3: i32,
    ns10: i32,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    ns20: i32,
    nb21: u64,
    nb22: u64,
    nb23: u64,
    ne31: i32,
    ne32: i32,
    ne33: i32,
    nb31: u64,
    nb32: u64,
    nb33: u64,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    scale: f32,
    max_bias: f32,
    m0: f32,
    m1: f32,
    n_head_log2: i32,
    logit_softcap: f32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsFlashAttnExtVecReduce {
    nrows: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsSsmConv {
    ne00: i64,
    ne01: i64,
    ne02: i64,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    ne10: i64,
    ne11: i64,
    nb10: u64,
    nb11: u64,
    ne0: i64,
    ne1: i64,
    ne2: i64,
    nb0: u64,
    nb1: u64,
    nb2: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsMulMmIdMap0 {
    ne02: i32,
    ne10: i32,
    ne11: i32,
    nb11: u64,
    nb12: u64,
    ne21: i32,
    ne20: i32,
    nb21: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsMulMmId {
    ne00: i32,
    ne02: i32,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne11: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    ne20: i32,
    ne21: i32,
    ne0: i32,
    ne1: i32,
    r2: i16,
    r3: i16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsMulMvId {
    nei0: i32,
    nei1: i32,
    nbi1: u64,
    ne00: i32,
    ne01: i32,
    ne02: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    ne10: i32,
    ne11: i32,
    ne12: i32,
    ne13: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    ne0: i32,
    ne1: i32,
    nb1: u64,
    nr0: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsGatedDeltaNet {
    ne00: i32,
    ne01: i32,
    ne02: i32,
    ne03: i32,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne10: i32,
    ne11: i32,
    ne12: i32,
    ne13: i32,
    nb10: u64,
    nb11: u64,
    nb12: u64,
    nb13: u64,
    ne20: i32,
    ne21: i32,
    ne22: i32,
    ne23: i32,
    nb20: u64,
    nb21: u64,
    nb22: u64,
    nb23: u64,
    ns02: i32,
    ns12: i32,
    ns22: i32,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    nb0: u64,
    nb1: u64,
    nb2: u64,
    nb3: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsArgsort {
    ne00: i64,
    ne01: i64,
    ne02: i64,
    ne03: i64,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    top_k: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct KArgsArgsortMerge {
    ne00: i64,
    ne01: i64,
    ne02: i64,
    ne03: i64,
    nb00: u64,
    nb01: u64,
    nb02: u64,
    nb03: u64,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    top_k: i32,
    len: i32,
}

const FC_FLASH_ATTN_EXT_PAD: i32 = 100;
const OP_FLASH_ATTN_EXT_VEC_NQPSG: i32 = 1;
const OP_FLASH_ATTN_EXT_VEC_NCPSG: i32 = 32;

#[derive(Clone, Copy, Debug)]
pub struct MetalGraphTensorWrite<'a> {
    pub tensor_id: TensorId,
    pub bytes: &'a [u8],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetalTensorBinding {
    pub tensor_id: TensorId,
    pub usage: BufferUsage,
    pub offset_bytes: usize,
    pub size_bytes: usize,
    pub is_view: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetalPreparedStage {
    pub kind: MetalStageKind,
    pub descriptor: super::MetalPipelineDescriptor,
    pub c4: bool,
    pub cnt: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetalPreparedNode {
    pub node_id: NodeId,
    pub tail_offset_bytes: usize,
    pub stages: Vec<MetalPreparedStage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetalPreparedGraph {
    pub features: MetalDeviceFeatures,
    pub nodes: Vec<MetalPreparedNode>,
    pub bindings: BTreeMap<TensorId, MetalTensorBinding>,
    pub main_buffer_size: usize,
    pub tail_buffer_size: usize,
}

#[derive(Clone, Debug)]
pub struct MetalCompiledStage {
    pub kind: MetalStageKind,
    pub descriptor: super::MetalPipelineDescriptor,
    pub pipeline: MetalPipeline,
    pub c4: bool,
    pub cnt: bool,
}

#[derive(Clone, Debug)]
pub struct MetalCompiledNode {
    pub node_id: NodeId,
    pub tail_offset_bytes: usize,
    pub stages: Vec<MetalCompiledStage>,
}

#[derive(Clone, Debug)]
pub struct MetalCompiledGraph {
    pub features: MetalDeviceFeatures,
    pub nodes: Vec<MetalCompiledNode>,
    pub bindings: BTreeMap<TensorId, MetalTensorBinding>,
    pub main_buffer_size: usize,
    pub tail_buffer_size: usize,
    pub main_buffer: super::MetalBuffer,
    pub tail_buffer: Option<super::MetalBuffer>,
}

pub struct MetalGraphSession {
    runtime: MetalRuntime,
    compiled: MetalCompiledGraph,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetalGraphExecution {
    pub outputs: BTreeMap<TensorId, Vec<u8>>,
}

pub fn prepare_graph(
    ctx: &Context,
    graph: &Graph,
    features: MetalDeviceFeatures,
) -> Result<MetalPreparedGraph, String> {
    let graph_plan = build_graph_plan(ctx, graph, features)?;
    let bindings = collect_tensor_bindings(ctx, graph)?;
    let main_buffer_size = ggml_pad(ctx.used_mem(), GGML_MEM_ALIGN);
    let mut tail_cursor = 0usize;
    let mut nodes = Vec::with_capacity(graph_plan.nodes.len());
    for plan in &graph_plan.nodes {
        let tail_offset_bytes = if plan.program.resources.output_tail_bytes == 0 {
            0
        } else {
            ggml_pad(tail_cursor, GGML_MEM_ALIGN)
        };
        tail_cursor = tail_offset_bytes
            .checked_add(plan.program.resources.output_tail_bytes)
            .ok_or_else(|| "Metal graph tail offset overflow".to_string())?;
        nodes.push(prepared_node_from_plan(plan, tail_offset_bytes));
    }
    let tail_buffer_size = if graph_plan.total_output_tail_bytes == 0 {
        ggml_pad(tail_cursor, GGML_MEM_ALIGN)
    } else {
        ggml_pad(
            std::cmp::max(tail_cursor, graph_plan.total_output_tail_bytes),
            GGML_MEM_ALIGN,
        )
    };

    Ok(MetalPreparedGraph {
        features,
        nodes,
        bindings,
        main_buffer_size,
        tail_buffer_size,
    })
}

pub fn compile_prepared_graph(
    runtime: &MetalRuntime,
    ctx: &Context,
    prepared: &MetalPreparedGraph,
    main_storage: BufferStorageMode,
    tail_storage: BufferStorageMode,
) -> Result<MetalCompiledGraph, String> {
    let main_bytes = collect_main_buffer_bytes(ctx, prepared.main_buffer_size)?;
    let main_buffer = runtime.create_buffer_with_bytes(&main_bytes, main_storage)?;
    let tail_buffer = if prepared.tail_buffer_size > 0 {
        Some(runtime.create_buffer(prepared.tail_buffer_size, tail_storage)?)
    } else {
        None
    };

    let mut nodes = Vec::with_capacity(prepared.nodes.len());
    for node in &prepared.nodes {
        let mut stages = Vec::with_capacity(node.stages.len());
        for stage in &node.stages {
            let pipeline = runtime.get_or_compile_pipeline(&stage.descriptor)?;
            stages.push(MetalCompiledStage {
                kind: stage.kind,
                descriptor: stage.descriptor.clone(),
                pipeline,
                c4: stage.c4,
                cnt: stage.cnt,
            });
        }
        nodes.push(MetalCompiledNode {
            node_id: node.node_id,
            tail_offset_bytes: node.tail_offset_bytes,
            stages,
        });
    }

    Ok(MetalCompiledGraph {
        features: prepared.features,
        nodes,
        bindings: prepared.bindings.clone(),
        main_buffer_size: prepared.main_buffer_size,
        tail_buffer_size: prepared.tail_buffer_size,
        main_buffer,
        tail_buffer,
    })
}

pub fn compile_graph_session(
    ctx: &Context,
    prepared: &MetalPreparedGraph,
    main_storage: BufferStorageMode,
    tail_storage: BufferStorageMode,
) -> Result<MetalGraphSession, String> {
    let runtime = MetalRuntime::new()?;
    MetalGraphSession::from_runtime(runtime, ctx, prepared, main_storage, tail_storage)
}

impl MetalGraphSession {
    pub fn from_runtime(
        runtime: MetalRuntime,
        ctx: &Context,
        prepared: &MetalPreparedGraph,
        main_storage: BufferStorageMode,
        tail_storage: BufferStorageMode,
    ) -> Result<Self, String> {
        let compiled = compile_prepared_graph(&runtime, ctx, prepared, main_storage, tail_storage)?;
        Ok(Self { runtime, compiled })
    }

    pub fn runtime(&self) -> &MetalRuntime {
        &self.runtime
    }

    pub fn compiled(&self) -> &MetalCompiledGraph {
        &self.compiled
    }

    pub fn execute(
        &self,
        ctx: &Context,
        inputs: &[MetalGraphTensorWrite<'_>],
        outputs: &[TensorId],
    ) -> Result<MetalGraphExecution, String> {
        execute_compiled_graph(&self.runtime, ctx, &self.compiled, inputs, outputs)
    }
}

pub fn execute_compiled_graph(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    inputs: &[MetalGraphTensorWrite<'_>],
    outputs: &[TensorId],
) -> Result<MetalGraphExecution, String> {
    for input in inputs {
        let binding = binding(compiled, input.tensor_id)?;
        let tensor = ctx
            .tensor(input.tensor_id)
            .ok_or_else(|| format!("input references invalid tensor {}", input.tensor_id))?;
        if input.bytes.len() != tensor.nbytes() {
            return Err(format!(
                "input '{}' byte length mismatch: got {}, expected {}",
                tensor.name().unwrap_or("<unnamed>"),
                input.bytes.len(),
                tensor.nbytes()
            ));
        }
        runtime.write_buffer(&compiled.main_buffer, binding.offset_bytes, input.bytes)?;
    }

    for node in &compiled.nodes {
        let tensor = ctx
            .tensor(node.node_id)
            .ok_or_else(|| format!("compiled graph references invalid tensor {}", node.node_id))?;
        execute_node(runtime, ctx, compiled, tensor, node)?;
    }

    let mut execution = MetalGraphExecution::default();
    for &tensor_id in outputs {
        let binding = binding(compiled, tensor_id)?;
        execution.outputs.insert(
            tensor_id,
            runtime.read_buffer_range(
                &compiled.main_buffer,
                binding.offset_bytes,
                binding.size_bytes,
            )?,
        );
    }
    Ok(execution)
}

fn prepared_node_from_plan(
    plan: &MetalGraphNodePlan,
    tail_offset_bytes: usize,
) -> MetalPreparedNode {
    MetalPreparedNode {
        node_id: plan.node_id,
        tail_offset_bytes,
        stages: plan
            .program
            .stages
            .iter()
            .map(|stage| MetalPreparedStage {
                kind: stage.kind,
                descriptor: stage.descriptor.clone(),
                c4: stage.c4,
                cnt: stage.cnt,
            })
            .collect(),
    }
}

fn collect_tensor_bindings(
    ctx: &Context,
    graph: &Graph,
) -> Result<BTreeMap<TensorId, MetalTensorBinding>, String> {
    let tensors = ctx.tensors();
    let mut needed = graph.nodes.clone();
    needed.extend(graph.leafs.iter().copied());
    needed.sort_unstable();
    needed.dedup();

    let mut bindings = BTreeMap::new();
    for tensor_id in needed {
        let tensor = tensors
            .get(tensor_id)
            .ok_or_else(|| format!("graph references invalid tensor {}", tensor_id))?;
        let offset_bytes = resolve_tensor_offset(tensors, tensor)?;
        bindings.insert(
            tensor_id,
            MetalTensorBinding {
                tensor_id,
                usage: tensor.desc.usage,
                offset_bytes,
                size_bytes: tensor.nbytes(),
                is_view: tensor.is_view(),
            },
        );
    }
    Ok(bindings)
}

fn resolve_tensor_offset(tensors: &[Tensor], tensor: &Tensor) -> Result<usize, String> {
    if tensor.is_view() {
        let src_id = tensor
            .view_src
            .ok_or_else(|| format!("view tensor {} is missing view source", tensor.id))?;
        let src = tensors.get(src_id).ok_or_else(|| {
            format!(
                "tensor {} references invalid view source {}",
                tensor.id, src_id
            )
        })?;
        let src_offset = resolve_tensor_offset(tensors, src)?;
        return src_offset
            .checked_add(tensor.view_offs)
            .ok_or_else(|| format!("tensor {} view offset overflow", tensor.id));
    }

    tensor
        .data_offset
        .ok_or_else(|| format!("tensor {} has no allocated data offset", tensor.id))
}

fn collect_main_buffer_bytes(ctx: &Context, len: usize) -> Result<Vec<u8>, String> {
    let src = ctx.mem_buffer();
    let used = ctx.used_mem();
    if used > len {
        return Err(format!(
            "context memory image ({}) exceeds prepared main buffer size ({})",
            used, len
        ));
    }
    let mut bytes = vec![0u8; len.max(1)];
    bytes[..used].copy_from_slice(&src[..used]);
    Ok(bytes)
}

fn execute_node(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    match tensor.op {
        Op::Concat => dispatch_concat(runtime, ctx, compiled, tensor, node),
        Op::AddId => dispatch_add_id(runtime, ctx, compiled, tensor, node),
        Op::GetRows => dispatch_get_rows(runtime, ctx, compiled, tensor, node),
        Op::SetRows => dispatch_set_rows(runtime, ctx, compiled, tensor, node),
        Op::Add | Op::Sub | Op::Mul | Op::Div => dispatch_bin(runtime, ctx, compiled, tensor, node),
        Op::Scale
        | Op::Fill
        | Op::Clamp
        | Op::LeakyRelu
        | Op::Sqr
        | Op::Sqrt
        | Op::Sin
        | Op::Cos
        | Op::Log
        | Op::Unary => dispatch_unary(runtime, ctx, compiled, tensor, node),
        Op::SumRows | Op::Mean => dispatch_sum_rows(runtime, ctx, compiled, tensor, node),
        Op::SoftMax => dispatch_soft_max(runtime, ctx, compiled, tensor, node),
        Op::Norm => dispatch_norm(runtime, ctx, compiled, tensor, node, false),
        Op::RmsNorm => dispatch_norm(runtime, ctx, compiled, tensor, node, true),
        Op::L2Norm => dispatch_l2_norm(runtime, ctx, compiled, tensor, node),
        Op::Rope => dispatch_rope(runtime, ctx, compiled, tensor, node),
        Op::FlashAttnExt => dispatch_flash_attn_ext(runtime, ctx, compiled, tensor, node),
        Op::SsmConv => dispatch_ssm_conv(runtime, ctx, compiled, tensor, node),
        Op::GatedDeltaNet => dispatch_gated_delta_net(runtime, ctx, compiled, tensor, node),
        Op::MulMat => dispatch_mul_mat(runtime, ctx, compiled, tensor, node),
        Op::MulMatId => dispatch_mul_mat_id(runtime, ctx, compiled, tensor, node),
        Op::Argsort => dispatch_argsort_like(runtime, ctx, compiled, tensor, node, false),
        Op::TopK => dispatch_argsort_like(runtime, ctx, compiled, tensor, node, true),
        Op::Dup | Op::Cpy | Op::Cont => dispatch_cpy(runtime, ctx, compiled, tensor, node),
        Op::Set => dispatch_set(runtime, ctx, compiled, tensor, node),
        other => Err(format!(
            "Metal compiled executor does not support ggml op {} yet",
            other.name()
        )),
    }
}

fn dispatch_concat(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("concat src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("concat src1 {} is invalid", src1_id))?;
    let args = KArgsConcat {
        ne00: i32_dim(src0, 0)?,
        ne01: i32_dim(src0, 1)?,
        ne02: i32_dim(src0, 2)?,
        ne03: i32_dim(src0, 3)?,
        nb00: u64::try_from(src0.nb[0]).map_err(|_| "concat nb00 exceeds u64".to_string())?,
        nb01: u64::try_from(src0.nb[1]).map_err(|_| "concat nb01 exceeds u64".to_string())?,
        nb02: u64::try_from(src0.nb[2]).map_err(|_| "concat nb02 exceeds u64".to_string())?,
        nb03: u64::try_from(src0.nb[3]).map_err(|_| "concat nb03 exceeds u64".to_string())?,
        ne10: i32_dim(src1, 0)?,
        ne11: i32_dim(src1, 1)?,
        ne12: i32_dim(src1, 2)?,
        ne13: i32_dim(src1, 3)?,
        nb10: u64::try_from(src1.nb[0]).map_err(|_| "concat nb10 exceeds u64".to_string())?,
        nb11: u64::try_from(src1.nb[1]).map_err(|_| "concat nb11 exceeds u64".to_string())?,
        nb12: u64::try_from(src1.nb[2]).map_err(|_| "concat nb12 exceeds u64".to_string())?,
        nb13: u64::try_from(src1.nb[3]).map_err(|_| "concat nb13 exceeds u64".to_string())?,
        ne0: i32_dim(tensor, 0)?,
        ne1: i32_dim(tensor, 1)?,
        ne2: i32_dim(tensor, 2)?,
        ne3: i32_dim(tensor, 3)?,
        nb0: u64::try_from(tensor.nb[0]).map_err(|_| "concat nb0 exceeds u64".to_string())?,
        nb1: u64::try_from(tensor.nb[1]).map_err(|_| "concat nb1 exceeds u64".to_string())?,
        nb2: u64::try_from(tensor.nb[2]).map_err(|_| "concat nb2 exceeds u64".to_string())?,
        nb3: u64::try_from(tensor.nb[3]).map_err(|_| "concat nb3 exceeds u64".to_string())?,
        dim: tensor.op_param_i32(0),
    };

    let nth = std::cmp::min(1024_u64, tensor.ne[0].max(1) as u64);
    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, src1_id),
            buffer_ref(compiled, 3, tensor.id),
        ],
        &[],
        MetalSize {
            width: tensor.ne[1].max(1) as u64,
            height: tensor.ne[2].max(1) as u64,
            depth: tensor.ne[3].max(1) as u64,
        },
        MetalSize {
            width: nth,
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_add_id(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let ids_id = tensor_src(tensor, 2)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("add_id src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("add_id src1 {} is invalid", src1_id))?;
    let ids = ctx
        .tensor(ids_id)
        .ok_or_else(|| format!("add_id ids {} is invalid", ids_id))?;
    let args = KArgsAddId {
        ne0: i64_dim(src0, 0)?,
        ne1: i64_dim(src0, 1)?,
        nb01: u64::try_from(src0.nb[1]).map_err(|_| "add_id nb01 exceeds u64".to_string())?,
        nb02: u64::try_from(src0.nb[2]).map_err(|_| "add_id nb02 exceeds u64".to_string())?,
        nb11: u64::try_from(src1.nb[1]).map_err(|_| "add_id nb11 exceeds u64".to_string())?,
        nb21: u64::try_from(ids.nb[1]).map_err(|_| "add_id nb21 exceeds u64".to_string())?,
    };
    let nth = std::cmp::min(
        stage.pipeline.max_threads_per_threadgroup.max(1),
        src0.ne[0].max(1) as u64,
    );

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, src1_id),
            buffer_ref(compiled, 3, ids_id),
            buffer_ref(compiled, 4, tensor.id),
        ],
        &[],
        MetalSize {
            width: src0.ne[1].max(1) as u64,
            height: src0.ne[2].max(1) as u64,
            depth: 1,
        },
        MetalSize {
            width: nth,
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_get_rows(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("get_rows src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("get_rows src1 {} is invalid", src1_id))?;
    let src0_shape = shape4(src0)?;
    let src1_shape = shape4(src1)?;
    let dst_shape = shape4(tensor)?;

    let is_quantized = !matches!(
        src0.desc.ty,
        TensorType::F32 | TensorType::F16 | TensorType::BF16 | TensorType::I32
    );
    let ne00t = if is_quantized {
        src0_shape.ne[0] / 16
    } else {
        src0_shape.ne[0]
    };
    let args = KArgsGetRows {
        ne00t,
        ne00: src0_shape.ne[0],
        nb01: src0_shape.nb[1],
        nb02: src0_shape.nb[2],
        nb03: src0_shape.nb[3],
        ne10: src1_shape.ne[0],
        nb10: src1_shape.nb[0],
        nb11: src1_shape.nb[1],
        nb12: src1_shape.nb[2],
        nb1: dst_shape.nb[1],
        nb2: dst_shape.nb[2],
        nb3: dst_shape.nb[3],
    };

    let nth = std::cmp::min(
        args.ne00t.max(1) as u64,
        stage.pipeline.max_threads_per_threadgroup.max(1),
    );
    let nw0 = ((args.ne00t.max(1) as u64) + nth - 1) / nth;

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, src1_id),
            buffer_ref(compiled, 3, tensor.id),
        ],
        &[],
        MetalSize {
            width: nw0 * (src1_shape.ne[0] as u64),
            height: src1_shape.ne[1] as u64,
            depth: src1_shape.ne[2] as u64,
        },
        MetalSize {
            width: nth,
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_set_rows(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("set_rows src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("set_rows src1 {} is invalid", src1_id))?;

    let nk0 = i32::try_from(
        src0.ne[0]
            / i64::try_from(ggml_blck_size_for_type(tensor.desc.ty))
                .map_err(|_| "set_rows block size exceeds i64".to_string())?,
    )
    .map_err(|_| "set_rows nk0 exceeds i32".to_string())?;
    let ne01 = i32_dim(src0, 1)?;
    let ne02 = i32_dim(src0, 2)?;
    let ne03 = i32_dim(src0, 3)?;
    let ne11 = i32_dim(src1, 1)?;
    let ne12 = i32_dim(src1, 2)?;
    let args = KArgsSetRows {
        nk0,
        ne01,
        nb01: u64::try_from(src0.nb[1]).map_err(|_| "set_rows nb01 exceeds u64".to_string())?,
        nb02: u64::try_from(src0.nb[2]).map_err(|_| "set_rows nb02 exceeds u64".to_string())?,
        nb03: u64::try_from(src0.nb[3]).map_err(|_| "set_rows nb03 exceeds u64".to_string())?,
        ne11,
        ne12,
        nb10: u64::try_from(src1.nb[0]).map_err(|_| "set_rows nb10 exceeds u64".to_string())?,
        nb11: u64::try_from(src1.nb[1]).map_err(|_| "set_rows nb11 exceeds u64".to_string())?,
        nb12: u64::try_from(src1.nb[2]).map_err(|_| "set_rows nb12 exceeds u64".to_string())?,
        nb1: u64::try_from(tensor.nb[1]).map_err(|_| "set_rows nb1 exceeds u64".to_string())?,
        nb2: u64::try_from(tensor.nb[2]).map_err(|_| "set_rows nb2 exceeds u64".to_string())?,
        nb3: u64::try_from(tensor.nb[3]).map_err(|_| "set_rows nb3 exceeds u64".to_string())?,
    };

    let max_threads = stage.pipeline.max_threads_per_threadgroup.max(1);
    let mut nth = 32_u64;
    while nth < (nk0.max(1) as u64) && nth < max_threads {
        nth *= 2;
    }

    let mut nrptg = 1_u64;
    if nth > nk0.max(1) as u64 {
        nrptg = nth.div_ceil(nk0.max(1) as u64);
        nth = nk0.max(1) as u64;
        if nrptg
            .checked_mul(nth)
            .ok_or_else(|| "set_rows threads per tg overflow".to_string())?
            > max_threads
        {
            nrptg = nrptg.saturating_sub(1).max(1);
        }
    }
    nth = nth.min(nk0.max(1) as u64);

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, src1_id),
            buffer_ref(compiled, 3, tensor.id),
        ],
        &[],
        MetalSize {
            width: (ne01.max(1) as u64).div_ceil(nrptg),
            height: ne02.max(1) as u64,
            depth: ne03.max(1) as u64,
        },
        MetalSize {
            width: nth,
            height: nrptg,
            depth: 1,
        },
    )
}

fn dispatch_bin(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("binary src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("binary src1 {} is invalid", src1_id))?;
    let src0_shape = shape4(src0)?;
    let src1_shape = shape4(src1)?;

    for d in 0..4 {
        let b = src1_shape.ne[d];
        let a = src0_shape.ne[d];
        if b != 1 && b != a {
            return Err(format!(
                "binary broadcast mismatch at dim {}: lhs={}, rhs={}",
                d, a, b
            ));
        }
    }

    let is_c4 = src0_shape.ne[0] % 4 == 0 && src1_shape.ne[0] % 4 == 0;
    let is_rb = stage.cnt;

    let mut args = KArgsBin {
        ne00: src0_shape.ne[0],
        ne01: src0_shape.ne[1],
        ne02: src0_shape.ne[2],
        ne03: src0_shape.ne[3],
        nb00: src0_shape.nb[0],
        nb01: src0_shape.nb[1],
        nb02: src0_shape.nb[2],
        nb03: src0_shape.nb[3],
        ne10: src1_shape.ne[0],
        ne11: src1_shape.ne[1],
        ne12: src1_shape.ne[2],
        ne13: src1_shape.ne[3],
        nb10: src1_shape.nb[0],
        nb11: src1_shape.nb[1],
        nb12: src1_shape.nb[2],
        nb13: src1_shape.nb[3],
        ne0: src0_shape.ne[0],
        ne1: src0_shape.ne[1],
        ne2: src0_shape.ne[2],
        ne3: src0_shape.ne[3],
        nb0: src0_shape.nb[0],
        nb1: src0_shape.nb[1],
        nb2: src0_shape.nb[2],
        nb3: src0_shape.nb[3],
        offs: 0,
        o1: [0u64; 8],
    };

    if is_c4 {
        args.ne00 /= 4;
        args.ne10 /= 4;
        args.ne0 /= 4;
    }

    let (threadgroups, threads_per_threadgroup) = if is_rb {
        (
            MetalSize {
                width: args.ne0.max(1) as u64,
                height: nrows(&src0_shape) as u64,
                depth: 1,
            },
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
        )
    } else {
        let nth_max = std::cmp::min(256u64, stage.pipeline.max_threads_per_threadgroup).max(1);
        let mut nth = 1u64;
        while 2 * nth < args.ne0 as u64 && nth < nth_max {
            nth *= 2;
        }
        (
            MetalSize {
                width: src0_shape.ne[1] as u64,
                height: src0_shape.ne[2] as u64,
                depth: src0_shape.ne[3] as u64,
            },
            MetalSize {
                width: nth,
                height: 1,
                depth: 1,
            },
        )
    };

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, src1_id),
            buffer_ref(compiled, 3, tensor.id),
        ],
        &[],
        threadgroups,
        threads_per_threadgroup,
    )
}

fn dispatch_unary(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("unary src0 {} is invalid", src0_id))?;
    let src0_shape = shape4(src0)?;
    let dst_shape = shape4(tensor)?;

    let mut args = KArgsUnary {
        ne00: src0_shape.ne[0],
        ne01: src0_shape.ne[1],
        ne02: src0_shape.ne[2],
        ne03: src0_shape.ne[3],
        nb00: src0_shape.nb[0],
        nb01: src0_shape.nb[1],
        nb02: src0_shape.nb[2],
        nb03: src0_shape.nb[3],
        ne0: dst_shape.ne[0],
        ne1: dst_shape.ne[1],
        ne2: dst_shape.ne[2],
        ne3: dst_shape.ne[3],
        nb0: dst_shape.nb[0],
        nb1: dst_shape.nb[1],
        nb2: dst_shape.nb[2],
        nb3: dst_shape.nb[3],
        slope: tensor.op_param_f32(0),
        scale: tensor.op_param_f32(1),
        bias: tensor.op_param_f32(2),
        val: tensor.op_param_f32(3),
        min: tensor.op_param_f32(4),
        max: tensor.op_param_f32(5),
    };

    if stage.c4 {
        args.ne00 /= 4;
        args.ne0 /= 4;
    }

    let (threadgroups, threads_per_threadgroup) = if stage.cnt {
        let n = if stage.c4 {
            src0_shape.numel / 4
        } else {
            src0_shape.numel
        };
        (
            MetalSize {
                width: n as u64,
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
        )
    } else {
        let nth_max = std::cmp::min(256u64, stage.pipeline.max_threads_per_threadgroup).max(1);
        let mut nth = 1u64;
        while 2 * nth < args.ne0 as u64 && nth < nth_max {
            nth *= 2;
        }
        (
            MetalSize {
                width: src0_shape.ne[1] as u64,
                height: src0_shape.ne[2] as u64,
                depth: src0_shape.ne[3] as u64,
            },
            MetalSize {
                width: nth,
                height: 1,
                depth: 1,
            },
        )
    };

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, tensor.id),
        ],
        &[],
        threadgroups,
        threads_per_threadgroup,
    )
}

fn dispatch_soft_max(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src_opt(tensor, 1);
    let src2_id = tensor_src_opt(tensor, 2);
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("soft_max src0 {} is invalid", src0_id))?;
    let src1 = src1_id
        .map(|id| {
            ctx.tensor(id)
                .ok_or_else(|| format!("soft_max src1 {} is invalid", id))
        })
        .transpose()?;

    let scale = tensor.op_param_f32(0);
    let max_bias = tensor.op_param_f32(1);
    let n_head = src0.ne[2].max(1) as u32;
    let n_head_log2 = if n_head <= 1 {
        1_i32
    } else {
        let p = (u32::BITS - 1) - n_head.leading_zeros();
        (1u32 << p) as i32
    };
    let m0 = if max_bias != 0.0 {
        (2.0f32).powf(-(max_bias) / (n_head_log2 as f32))
    } else {
        1.0
    };
    let m1 = if max_bias != 0.0 {
        (2.0f32).powf(-(max_bias / 2.0) / (n_head_log2 as f32))
    } else {
        1.0
    };

    let src1_shape = src1.map(shape4).transpose()?.unwrap_or_default();
    let args = KArgsSoftMax {
        ne00: i32_dim(src0, 0)?,
        ne01: i32_dim(src0, 1)?,
        ne02: i32_dim(src0, 2)?,
        nb01: u64::try_from(src0.nb[1]).map_err(|_| "soft_max nb01 exceeds u64".to_string())?,
        nb02: u64::try_from(src0.nb[2]).map_err(|_| "soft_max nb02 exceeds u64".to_string())?,
        nb03: u64::try_from(src0.nb[3]).map_err(|_| "soft_max nb03 exceeds u64".to_string())?,
        ne11: src1_shape.ne[1],
        ne12: src1_shape.ne[2],
        ne13: src1_shape.ne[3],
        nb11: src1_shape.nb[1],
        nb12: src1_shape.nb[2],
        nb13: src1_shape.nb[3],
        nb1: u64::try_from(tensor.nb[1]).map_err(|_| "soft_max nb1 exceeds u64".to_string())?,
        nb2: u64::try_from(tensor.nb[2]).map_err(|_| "soft_max nb2 exceeds u64".to_string())?,
        nb3: u64::try_from(tensor.nb[3]).map_err(|_| "soft_max nb3 exceeds u64".to_string())?,
        scale,
        max_bias,
        m0,
        m1,
        n_head_log2,
    };

    let mut nth = 32_u64;
    let limit = if stage.c4 {
        (src0.ne[0] / 4).max(1) as u64
    } else {
        src0.ne[0].max(1) as u64
    };
    while nth < limit
        && nth
            .checked_mul(src0.ne[1].max(1) as u64)
            .and_then(|v| v.checked_mul(src0.ne[2].max(1) as u64))
            .and_then(|v| v.checked_mul(src0.ne[3].max(1) as u64))
            .unwrap_or(u64::MAX)
            < 256
    {
        nth *= 2;
    }
    nth = nth.min(stage.pipeline.max_threads_per_threadgroup.max(1));

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            src1_id
                .map(|id| buffer_ref(compiled, 2, id))
                .unwrap_or_else(|| buffer_ref(compiled, 2, src0_id)),
            src2_id
                .map(|id| buffer_ref(compiled, 3, id))
                .unwrap_or_else(|| buffer_ref(compiled, 3, src0_id)),
            buffer_ref(compiled, 4, tensor.id),
        ],
        &[(0, stage.pipeline.smem_bytes)],
        MetalSize {
            width: src0.ne[1].max(1) as u64,
            height: src0.ne[2].max(1) as u64,
            depth: src0.ne[3].max(1) as u64,
        },
        MetalSize {
            width: nth.max(1),
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_sum_rows(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("sum_rows src0 {} is invalid", src0_id))?;

    let mut args = KArgsSumRows {
        ne00: src0.ne[0],
        ne01: src0.ne[1],
        ne02: src0.ne[2],
        ne03: src0.ne[3],
        nb00: u64::try_from(src0.nb[0]).map_err(|_| "sum_rows nb00 exceeds u64".to_string())?,
        nb01: u64::try_from(src0.nb[1]).map_err(|_| "sum_rows nb01 exceeds u64".to_string())?,
        nb02: u64::try_from(src0.nb[2]).map_err(|_| "sum_rows nb02 exceeds u64".to_string())?,
        nb03: u64::try_from(src0.nb[3]).map_err(|_| "sum_rows nb03 exceeds u64".to_string())?,
        ne0: tensor.ne[0],
        ne1: tensor.ne[1],
        ne2: tensor.ne[2],
        ne3: tensor.ne[3],
        nb0: u64::try_from(tensor.nb[0]).map_err(|_| "sum_rows nb0 exceeds u64".to_string())?,
        nb1: u64::try_from(tensor.nb[1]).map_err(|_| "sum_rows nb1 exceeds u64".to_string())?,
        nb2: u64::try_from(tensor.nb[2]).map_err(|_| "sum_rows nb2 exceeds u64".to_string())?,
        nb3: u64::try_from(tensor.nb[3]).map_err(|_| "sum_rows nb3 exceeds u64".to_string())?,
    };

    if stage.c4 {
        args.ne00 /= 4;
        args.ne0 /= 4;
    }

    let max_threads = stage.pipeline.max_threads_per_threadgroup.max(1);
    let mut nth = 32_u64;
    while nth < (args.ne00.max(1) as u64) && nth < max_threads {
        nth *= 2;
    }
    nth = nth.min(max_threads).min(args.ne00.max(1) as u64);

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, tensor.id),
        ],
        &[(0, stage.pipeline.smem_bytes)],
        MetalSize {
            width: src0.ne[1].max(1) as u64,
            height: src0.ne[2].max(1) as u64,
            depth: src0.ne[3].max(1) as u64,
        },
        MetalSize {
            width: nth,
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_norm(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
    rms: bool,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("norm src0 {} is invalid", src0_id))?;
    let src0_shape = shape4(src0)?;
    let empty_shape = Shape4::default();
    let is_c4 = src0_shape.ne[0] % 4 == 0;
    let ne00_t = if is_c4 {
        src0_shape.ne[0] / 4
    } else {
        src0_shape.ne[0]
    };
    let args = KArgsNorm {
        ne00: src0_shape.ne[0],
        ne00_t,
        nb1: src0_shape.nb[1],
        nb2: src0_shape.nb[2],
        nb3: src0_shape.nb[3],
        eps: tensor.op_param_f32(0),
        nef1: [src0_shape.ne[1], empty_shape.ne[1], empty_shape.ne[1]],
        nef2: [src0_shape.ne[2], empty_shape.ne[2], empty_shape.ne[2]],
        nef3: [src0_shape.ne[3], empty_shape.ne[3], empty_shape.ne[3]],
        nbf1: [src0_shape.nb[1], empty_shape.nb[1], empty_shape.nb[1]],
        nbf2: [src0_shape.nb[2], empty_shape.nb[2], empty_shape.nb[2]],
        nbf3: [src0_shape.nb[3], empty_shape.nb[3], empty_shape.nb[3]],
    };

    let mut nth = 32u64;
    let nth_max = stage.pipeline.max_threads_per_threadgroup.max(1);
    while nth < args.ne00_t as u64 && nth < nth_max {
        nth *= 2;
    }
    nth = std::cmp::min(nth, nth_max);
    nth = std::cmp::min(nth, args.ne00_t.max(1) as u64);

    let buffers = if rms || tensor.op == Op::RmsNorm || tensor.op == Op::Norm {
        vec![
            buffer_ref(compiled, 1, src0_id),
            dummy_buffer_ref(2, &compiled.main_buffer),
            dummy_buffer_ref(3, &compiled.main_buffer),
            buffer_ref(compiled, 4, tensor.id),
        ]
    } else {
        unreachable!()
    };

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &buffers,
        &[(0, stage.pipeline.smem_bytes)],
        MetalSize {
            width: src0_shape.ne[1] as u64,
            height: src0_shape.ne[2] as u64,
            depth: src0_shape.ne[3] as u64,
        },
        MetalSize {
            width: nth,
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_l2_norm(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("l2_norm src0 {} is invalid", src0_id))?;
    if !src0.is_contiguous_rows() {
        return Err("l2_norm currently requires contiguous-row input".to_string());
    }

    let mut args = KArgsL2Norm {
        ne00: i32_dim(src0, 0)?,
        ne01: i32_dim(src0, 1)?,
        ne02: i32_dim(src0, 2)?,
        ne03: i32_dim(src0, 3)?,
        nb00: u64::try_from(src0.nb[0]).map_err(|_| "l2_norm nb00 exceeds u64".to_string())?,
        nb01: u64::try_from(src0.nb[1]).map_err(|_| "l2_norm nb01 exceeds u64".to_string())?,
        nb02: u64::try_from(src0.nb[2]).map_err(|_| "l2_norm nb02 exceeds u64".to_string())?,
        nb03: u64::try_from(src0.nb[3]).map_err(|_| "l2_norm nb03 exceeds u64".to_string())?,
        ne0: i32_dim(tensor, 0)?,
        ne1: i32_dim(tensor, 1)?,
        ne2: i32_dim(tensor, 2)?,
        ne3: i32_dim(tensor, 3)?,
        nb0: u64::try_from(tensor.nb[0]).map_err(|_| "l2_norm nb0 exceeds u64".to_string())?,
        nb1: u64::try_from(tensor.nb[1]).map_err(|_| "l2_norm nb1 exceeds u64".to_string())?,
        nb2: u64::try_from(tensor.nb[2]).map_err(|_| "l2_norm nb2 exceeds u64".to_string())?,
        nb3: u64::try_from(tensor.nb[3]).map_err(|_| "l2_norm nb3 exceeds u64".to_string())?,
        eps: tensor.op_param_f32(0),
    };
    if stage.c4 {
        args.ne00 /= 4;
        args.ne0 /= 4;
    }

    let mut nth = 32_u64;
    while nth < src0.ne[0].max(1) as u64 && nth < stage.pipeline.max_threads_per_threadgroup {
        nth *= 2;
    }
    nth = nth.min(stage.pipeline.max_threads_per_threadgroup.max(1));

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, tensor.id),
        ],
        &[(0, stage.pipeline.smem_bytes)],
        MetalSize {
            width: src0.ne[1].max(1) as u64,
            height: src0.ne[2].max(1) as u64,
            depth: src0.ne[3].max(1) as u64,
        },
        MetalSize {
            width: nth,
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_mul_mat(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("mul_mat src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("mul_mat src1 {} is invalid", src1_id))?;
    let base = stage.descriptor.base_name.as_str();

    if base.starts_with("kernel_mul_mm_") {
        let ne00 = i32_dim(src0, 0)?;
        let ne01 = i32_dim(src0, 1)?;
        let ne12 = i32_dim(src1, 1)?;
        let ne0 = i32_dim(tensor, 0)?;
        let ne1 = i32_dim(tensor, 1)?;
        let args = KArgsMulMm {
            ne00,
            ne02: 1,
            nb01: u64::try_from(src0.nb[1]).map_err(|_| "mul_mm nb01 exceeds u64".to_string())?,
            nb02: u64::try_from(src0.nb[1]).map_err(|_| "mul_mm nb02 exceeds u64".to_string())?
                * ne01 as u64,
            nb03: u64::try_from(src0.nb[1]).map_err(|_| "mul_mm nb03 exceeds u64".to_string())?
                * ne01 as u64,
            ne12,
            nb10: u64::try_from(src1.nb[0]).map_err(|_| "mul_mm nb10 exceeds u64".to_string())?,
            nb11: u64::try_from(src1.nb[1]).map_err(|_| "mul_mm nb11 exceeds u64".to_string())?,
            nb12: u64::try_from(src1.nb[1]).map_err(|_| "mul_mm nb12 exceeds u64".to_string())?
                * ne1 as u64,
            nb13: u64::try_from(src1.nb[1]).map_err(|_| "mul_mm nb13 exceeds u64".to_string())?
                * ne1 as u64,
            ne0,
            ne1,
            r2: 1,
            r3: 1,
        };
        return runtime.dispatch_compute(
            &stage.pipeline,
            bytes_of(&args),
            &[
                buffer_ref(compiled, 1, src0_id),
                buffer_ref(compiled, 2, src1_id),
                buffer_ref(compiled, 3, tensor.id),
            ],
            &[(0, stage.pipeline.smem_bytes)],
            MetalSize {
                width: ((ne1 + 31) / 32) as u64,
                height: ((ne01 + 63) / 64) as u64,
                depth: ne12 as u64,
            },
            MetalSize {
                width: 128,
                height: 1,
                depth: 1,
            },
        );
    }

    if base.starts_with("kernel_mul_mv_ext_") {
        let ne00 = i32_dim(src0, 0)?;
        let ne01 = i32_dim(src0, 1)?;
        let ne10 = i32_dim(src1, 0)?;
        let ne11 = i32_dim(src1, 1)?;
        let ne0 = i32_dim(tensor, 0)?;
        let ne1 = i32_dim(tensor, 1)?;
        let nsg = constant_i16(&stage.descriptor.constants, 601)? as i32;
        let nxpsg = stage
            .descriptor
            .constants
            .iter()
            .find(|constant| constant.idx == 602)
            .and_then(|constant| match constant.value {
                FunctionConstantValue::Int16(value) => Some(i32::from(value)),
                _ => None,
            })
            .unwrap_or_else(|| {
                if ne00 % 256 == 0 && ne11 < 3 {
                    16
                } else if ne00 % 128 == 0 {
                    8
                } else {
                    4
                }
            });
        let r1ptg = parse_trailing_i32(base, "_r1_")?;
        let r0ptg = nxpsg * nsg;
        let args = KArgsMulMvExt {
            ne00,
            ne01,
            ne02: 1,
            nb00: u64::try_from(src0.nb[0])
                .map_err(|_| "mul_mv_ext nb00 exceeds u64".to_string())?,
            nb01: u64::try_from(src0.nb[1])
                .map_err(|_| "mul_mv_ext nb01 exceeds u64".to_string())?,
            nb02: u64::try_from(src0.nb[1])
                .map_err(|_| "mul_mv_ext nb02 exceeds u64".to_string())?
                * ne01 as u64,
            nb03: u64::try_from(src0.nb[1])
                .map_err(|_| "mul_mv_ext nb03 exceeds u64".to_string())?
                * ne01 as u64,
            ne10,
            ne11,
            ne12: 1,
            nb10: u64::try_from(src1.nb[0])
                .map_err(|_| "mul_mv_ext nb10 exceeds u64".to_string())?,
            nb11: u64::try_from(src1.nb[1])
                .map_err(|_| "mul_mv_ext nb11 exceeds u64".to_string())?,
            nb12: u64::try_from(src1.nb[1])
                .map_err(|_| "mul_mv_ext nb12 exceeds u64".to_string())?
                * ne11 as u64,
            nb13: u64::try_from(src1.nb[1])
                .map_err(|_| "mul_mv_ext nb13 exceeds u64".to_string())?
                * ne11 as u64,
            ne0,
            ne1,
            r2: 1,
            r3: 1,
        };
        return runtime.dispatch_compute(
            &stage.pipeline,
            bytes_of(&args),
            &[
                buffer_ref(compiled, 1, src0_id),
                buffer_ref(compiled, 2, src1_id),
                buffer_ref(compiled, 3, tensor.id),
            ],
            &[],
            MetalSize {
                width: ((ne01 + r0ptg - 1) / r0ptg) as u64,
                height: ((ne11 + r1ptg - 1) / r1ptg) as u64,
                depth: 1,
            },
            MetalSize {
                width: 32,
                height: nsg as u64,
                depth: 1,
            },
        );
    }

    if base.starts_with("kernel_mul_mv_") {
        let ne00 = i32_dim(src0, 0)?;
        let ne01 = i32_dim(src0, 1)?;
        let ne10 = i32_dim(src1, 0)?;
        let ne11 = i32_dim(src1, 1)?;
        let ne0 = i32_dim(tensor, 0)?;
        let ne1 = i32_dim(tensor, 1)?;
        let args = KArgsMulMv {
            ne00,
            ne01,
            ne02: 1,
            nb00: u64::try_from(src0.nb[0]).map_err(|_| "mul_mv nb00 exceeds u64".to_string())?,
            nb01: u64::try_from(src0.nb[1]).map_err(|_| "mul_mv nb01 exceeds u64".to_string())?,
            nb02: u64::try_from(src0.nb[1]).map_err(|_| "mul_mv nb02 exceeds u64".to_string())?
                * ne01 as u64,
            nb03: u64::try_from(src0.nb[1]).map_err(|_| "mul_mv nb03 exceeds u64".to_string())?
                * ne01 as u64,
            ne10,
            ne11,
            ne12: 1,
            nb10: u64::try_from(src1.nb[0]).map_err(|_| "mul_mv nb10 exceeds u64".to_string())?,
            nb11: u64::try_from(src1.nb[1]).map_err(|_| "mul_mv nb11 exceeds u64".to_string())?,
            nb12: u64::try_from(src1.nb[1]).map_err(|_| "mul_mv nb12 exceeds u64".to_string())?
                * ne11 as u64,
            nb13: u64::try_from(src1.nb[1]).map_err(|_| "mul_mv nb13 exceeds u64".to_string())?
                * ne11 as u64,
            ne0,
            ne1,
            nr0: stage.pipeline.nr0,
            r2: 1,
            r3: 1,
        };

        let scalar_like = matches!(
            src0.desc.ty,
            TensorType::F32 | TensorType::F16 | TensorType::BF16 | TensorType::Q8_0
        );
        let tg_x = if scalar_like {
            (ne01 + stage.pipeline.nr0 - 1) / stage.pipeline.nr0
        } else {
            (ne01 + stage.pipeline.nr0 * stage.pipeline.nsg - 1)
                / (stage.pipeline.nr0 * stage.pipeline.nsg)
        };
        let tg_y = (ne11 + stage.pipeline.nr1 - 1) / stage.pipeline.nr1;
        let smem = if stage.pipeline.smem_bytes > 0 {
            vec![(0, stage.pipeline.smem_bytes)]
        } else {
            Vec::new()
        };

        return runtime.dispatch_compute(
            &stage.pipeline,
            bytes_of(&args),
            &[
                buffer_ref(compiled, 1, src0_id),
                buffer_ref(compiled, 2, src1_id),
                buffer_ref(compiled, 3, tensor.id),
            ],
            &smem,
            MetalSize {
                width: tg_x as u64,
                height: tg_y as u64,
                depth: 1,
            },
            MetalSize {
                width: 32,
                height: stage.pipeline.nsg as u64,
                depth: 1,
            },
        );
    }

    Err(format!(
        "unsupported mul_mat pipeline '{}'",
        stage.descriptor.base_name
    ))
}

fn dispatch_mul_mat_id(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src2_id = tensor_src(tensor, 2)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("mul_mat_id src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("mul_mat_id src1 {} is invalid", src1_id))?;
    let src2 = ctx
        .tensor(src2_id)
        .ok_or_else(|| format!("mul_mat_id src2 {} is invalid", src2_id))?;
    let stage = main_stage(node, tensor.op)?;
    let base = stage.descriptor.base_name.as_str();

    if base.starts_with("kernel_mul_mm_id_") {
        let map_stage = stage_kind(node, MetalStageKind::Aux, tensor.op)?;
        let tpe_bytes = mul_mat_id_extra_tpe_bytes(src0)?;
        let args_map0 = KArgsMulMmIdMap0 {
            ne02: i32_dim(src0, 2)?,
            ne10: i32_dim(src1, 0)?,
            ne11: i32_dim(src1, 1)?,
            nb11: u64::try_from(src1.nb[1])
                .map_err(|_| "mul_mm_id_map0 nb11 exceeds u64".to_string())?,
            nb12: u64::try_from(src1.nb[2])
                .map_err(|_| "mul_mm_id_map0 nb12 exceeds u64".to_string())?,
            ne21: i32_dim(src2, 1)?,
            ne20: i32_dim(src2, 0)?,
            nb21: u64::try_from(src2.nb[1])
                .map_err(|_| "mul_mm_id_map0 nb21 exceeds u64".to_string())?,
        };

        runtime.dispatch_compute(
            &map_stage.pipeline,
            bytes_of(&args_map0),
            &[
                buffer_ref(compiled, 1, src2_id),
                tail_node_buffer_ref(compiled, node, 2, 0)?,
                tail_node_buffer_ref(compiled, node, 3, tpe_bytes)?,
            ],
            &[(0, map_stage.pipeline.smem_bytes)],
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: src0.ne[2].max(1) as u64,
                height: 1,
                depth: 1,
            },
        )?;

        let args = KArgsMulMmId {
            ne00: i32_dim(src0, 0)?,
            ne02: i32_dim(src0, 2)?,
            nb01: u64::try_from(src0.nb[1])
                .map_err(|_| "mul_mm_id nb01 exceeds u64".to_string())?,
            nb02: u64::try_from(src0.nb[2])
                .map_err(|_| "mul_mm_id nb02 exceeds u64".to_string())?,
            nb03: u64::try_from(src0.nb[3])
                .map_err(|_| "mul_mm_id nb03 exceeds u64".to_string())?,
            ne11: i32_dim(src1, 1)?,
            nb10: u64::try_from(src1.nb[0])
                .map_err(|_| "mul_mm_id nb10 exceeds u64".to_string())?,
            nb11: u64::try_from(src1.nb[1])
                .map_err(|_| "mul_mm_id nb11 exceeds u64".to_string())?,
            nb12: u64::try_from(src1.nb[2])
                .map_err(|_| "mul_mm_id nb12 exceeds u64".to_string())?,
            nb13: u64::try_from(src1.nb[3])
                .map_err(|_| "mul_mm_id nb13 exceeds u64".to_string())?,
            ne20: i32_dim(src2, 0)?,
            ne21: i32_dim(src2, 1)?,
            ne0: i32_dim(tensor, 0)?,
            ne1: i32_dim(tensor, 1)?,
            r2: 1,
            r3: 1,
        };

        return runtime.dispatch_compute(
            &stage.pipeline,
            bytes_of(&args),
            &[
                buffer_ref(compiled, 1, src0_id),
                buffer_ref(compiled, 2, src1_id),
                tail_node_buffer_ref(compiled, node, 3, 0)?,
                tail_node_buffer_ref(compiled, node, 4, tpe_bytes)?,
                buffer_ref(compiled, 5, tensor.id),
            ],
            &[(0, stage.pipeline.smem_bytes)],
            MetalSize {
                width: ((src2.ne[1] + 31) / 32) as u64,
                height: ((src0.ne[1] + 63) / 64) as u64,
                depth: src0.ne[2].max(1) as u64,
            },
            MetalSize {
                width: 128,
                height: 1,
                depth: 1,
            },
        );
    }

    if base.starts_with("kernel_mul_mv_id_") {
        let args = KArgsMulMvId {
            nei0: i32_dim(src2, 0)?,
            nei1: i32_dim(src2, 1)?,
            nbi1: u64::try_from(src2.nb[1])
                .map_err(|_| "mul_mv_id nbi1 exceeds u64".to_string())?,
            ne00: i32_dim(src0, 0)?,
            ne01: i32_dim(src0, 1)?,
            ne02: i32_dim(src0, 2)?,
            nb00: u64::try_from(src0.nb[0])
                .map_err(|_| "mul_mv_id nb00 exceeds u64".to_string())?,
            nb01: u64::try_from(src0.nb[1])
                .map_err(|_| "mul_mv_id nb01 exceeds u64".to_string())?,
            nb02: u64::try_from(src0.nb[2])
                .map_err(|_| "mul_mv_id nb02 exceeds u64".to_string())?,
            ne10: i32_dim(src1, 0)?,
            ne11: i32_dim(src1, 1)?,
            ne12: i32_dim(src1, 2)?,
            ne13: i32_dim(src1, 3)?,
            nb10: u64::try_from(src1.nb[0])
                .map_err(|_| "mul_mv_id nb10 exceeds u64".to_string())?,
            nb11: u64::try_from(src1.nb[1])
                .map_err(|_| "mul_mv_id nb11 exceeds u64".to_string())?,
            nb12: u64::try_from(src1.nb[2])
                .map_err(|_| "mul_mv_id nb12 exceeds u64".to_string())?,
            ne0: i32_dim(tensor, 0)?,
            ne1: i32_dim(tensor, 1)?,
            nb1: u64::try_from(tensor.nb[1])
                .map_err(|_| "mul_mv_id nb1 exceeds u64".to_string())?,
            nr0: stage.pipeline.nr0,
        };

        let scalar_like = matches!(
            src0.desc.ty,
            TensorType::F32 | TensorType::F16 | TensorType::BF16 | TensorType::Q8_0
        );
        let tg_x = if scalar_like {
            (src0.ne[1] + stage.pipeline.nr0 as i64 - 1) / stage.pipeline.nr0 as i64
        } else {
            (src0.ne[1] + (stage.pipeline.nr0 * stage.pipeline.nsg) as i64 - 1)
                / (stage.pipeline.nr0 * stage.pipeline.nsg) as i64
        };
        let smem = if stage.pipeline.smem_bytes > 0 {
            vec![(0, stage.pipeline.smem_bytes)]
        } else {
            Vec::new()
        };

        return runtime.dispatch_compute(
            &stage.pipeline,
            bytes_of(&args),
            &[
                buffer_ref(compiled, 1, src0_id),
                buffer_ref(compiled, 2, src1_id),
                buffer_ref(compiled, 3, tensor.id),
                buffer_ref(compiled, 4, src2_id),
            ],
            &smem,
            MetalSize {
                width: tg_x.max(1) as u64,
                height: 1,
                depth: (src2.ne[0] * src2.ne[1]).max(1) as u64,
            },
            MetalSize {
                width: 32,
                height: stage.pipeline.nsg.max(1) as u64,
                depth: 1,
            },
        );
    }

    Err(format!(
        "unsupported mul_mat_id pipeline '{}'",
        stage.descriptor.base_name
    ))
}

fn dispatch_argsort_like(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
    top_k_mode: bool,
) -> Result<(), String> {
    let main_stage = main_stage(node, tensor.op)?;
    let merge_stage = stage_kind(node, MetalStageKind::Merge, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("argsort src0 {} is invalid", src0_id))?;

    let mut nth = 1_i32;
    while nth < src0.ne[0] as i32
        && 2 * nth
            <= i32::try_from(main_stage.pipeline.max_threads_per_threadgroup).unwrap_or(i32::MAX)
    {
        nth *= 2;
    }
    let npr = ((src0.ne[0] as i32) + nth - 1) / nth;
    let smem = ggml_pad(
        usize::try_from(nth).map_err(|_| "argsort nth overflow".to_string())?
            * std::mem::size_of::<i32>(),
        16,
    );

    let block_top_k = if top_k_mode {
        std::cmp::min(nth, i32_dim(tensor, 0)?).max(1)
    } else {
        nth.max(1)
    };
    let effective_ne0 = if top_k_mode && npr > 1 {
        (npr - 1) * block_top_k + std::cmp::min(src0.ne[0] as i32 - (npr - 1) * nth, block_top_k)
    } else if top_k_mode {
        block_top_k
    } else {
        i32_dim(tensor, 0)?
    };

    let src_nb1 = u64::try_from(src0.nb[1]).map_err(|_| "argsort nb01 exceeds u64".to_string())?;
    let src_nb2 = u64::try_from(src0.nb[2]).map_err(|_| "argsort nb02 exceeds u64".to_string())?;
    let src_nb3 = u64::try_from(src0.nb[3]).map_err(|_| "argsort nb03 exceeds u64".to_string())?;
    let dst_nb1 =
        u64::try_from(tensor.nb[1]).map_err(|_| "argsort dst nb1 exceeds u64".to_string())?;
    let dst_nb2 =
        u64::try_from(tensor.nb[2]).map_err(|_| "argsort dst nb2 exceeds u64".to_string())?;
    let dst_nb3 =
        u64::try_from(tensor.nb[3]).map_err(|_| "argsort dst nb3 exceeds u64".to_string())?;
    let tmp_row_bytes = ggml_type_size_for_type(TensorType::I32)
        * usize::try_from(src0.ne[0]).map_err(|_| "argsort temp row bytes overflow".to_string())?;

    for i03 in 0..src0.ne[3] {
        for i02 in 0..src0.ne[2] {
            for i01 in 0..src0.ne[1] {
                let row_index = usize::try_from(i01 + src0.ne[1] * (i02 + src0.ne[2] * i03))
                    .map_err(|_| "argsort row index overflow".to_string())?;
                let src_row_offset = usize::try_from(
                    i01 * src0.nb[1] as i64 + i02 * src0.nb[2] as i64 + i03 * src0.nb[3] as i64,
                )
                .map_err(|_| "argsort src row offset overflow".to_string())?;
                let dst_row_offset = usize::try_from(
                    i01 * tensor.nb[1] as i64
                        + i02 * tensor.nb[2] as i64
                        + i03 * tensor.nb[3] as i64,
                )
                .map_err(|_| "argsort dst row offset overflow".to_string())?;
                let tmp_row_offset = row_index
                    .checked_mul(tmp_row_bytes)
                    .ok_or_else(|| "argsort temp row offset overflow".to_string())?;

                let mut dst_binding =
                    buffer_ref_with_offset(compiled, 2, tensor.id, dst_row_offset)?;
                let mut tmp_binding = tail_node_buffer_ref(compiled, node, 3, tmp_row_offset)?;
                let mut merge_passes = 0usize;
                let mut parity_len = block_top_k;
                while parity_len < effective_ne0 {
                    merge_passes += 1;
                    parity_len <<= 1;
                }
                if merge_passes % 2 == 1 {
                    std::mem::swap(&mut dst_binding, &mut tmp_binding);
                }

                let args = KArgsArgsort {
                    ne00: i64_dim(src0, 0)?,
                    ne01: 1,
                    ne02: 1,
                    ne03: 1,
                    nb00: u64::try_from(src0.nb[0])
                        .map_err(|_| "argsort nb00 exceeds u64".to_string())?,
                    nb01: src_nb1,
                    nb02: src_nb2,
                    nb03: src_nb3,
                    ne0: effective_ne0,
                    ne1: 1,
                    ne2: 1,
                    ne3: 1,
                    top_k: block_top_k,
                };

                runtime.dispatch_compute(
                    &main_stage.pipeline,
                    bytes_of(&args),
                    &[
                        MetalBufferBindingRef {
                            index: 1,
                            buffer: &compiled.main_buffer,
                            offset_bytes: binding(compiled, src0_id)?
                                .offset_bytes
                                .checked_add(src_row_offset)
                                .ok_or_else(|| "argsort src binding offset overflow".to_string())?,
                        },
                        MetalBufferBindingRef {
                            index: 2,
                            buffer: dst_binding.buffer,
                            offset_bytes: dst_binding.offset_bytes,
                        },
                    ],
                    &[(0, smem)],
                    MetalSize {
                        width: npr.max(1) as u64,
                        height: 1,
                        depth: 1,
                    },
                    MetalSize {
                        width: nth.max(1) as u64,
                        height: 1,
                        depth: 1,
                    },
                )?;

                let mut len = block_top_k;
                while len < effective_ne0 {
                    let nm = (effective_ne0 + 2 * len - 1) / (2 * len);
                    let merge_top_k = if top_k_mode && nm == 1 {
                        i32_dim(tensor, 0)?
                    } else {
                        effective_ne0
                    };
                    let args_merge = KArgsArgsortMerge {
                        ne00: i64_dim(src0, 0)?,
                        ne01: 1,
                        ne02: 1,
                        ne03: 1,
                        nb00: u64::try_from(src0.nb[0])
                            .map_err(|_| "argsort_merge nb00 exceeds u64".to_string())?,
                        nb01: src_nb1,
                        nb02: src_nb2,
                        nb03: src_nb3,
                        ne0: effective_ne0,
                        ne1: 1,
                        ne2: 1,
                        ne3: 1,
                        top_k: merge_top_k,
                        len,
                    };
                    let merge_nth = if top_k_mode {
                        std::cmp::min(
                            512_u64,
                            std::cmp::min(
                                len.max(1) as u64,
                                merge_stage.pipeline.max_threads_per_threadgroup.max(1),
                            ),
                        )
                    } else {
                        std::cmp::min(
                            512_u64,
                            merge_stage.pipeline.max_threads_per_threadgroup.max(1),
                        )
                    };

                    runtime.dispatch_compute(
                        &merge_stage.pipeline,
                        bytes_of(&args_merge),
                        &[
                            MetalBufferBindingRef {
                                index: 1,
                                buffer: &compiled.main_buffer,
                                offset_bytes: binding(compiled, src0_id)?
                                    .offset_bytes
                                    .checked_add(src_row_offset)
                                    .ok_or_else(|| {
                                        "argsort src binding offset overflow".to_string()
                                    })?,
                            },
                            MetalBufferBindingRef {
                                index: 2,
                                buffer: dst_binding.buffer,
                                offset_bytes: dst_binding.offset_bytes,
                            },
                            MetalBufferBindingRef {
                                index: 3,
                                buffer: tmp_binding.buffer,
                                offset_bytes: tmp_binding.offset_bytes,
                            },
                        ],
                        &[],
                        MetalSize {
                            width: nm.max(1) as u64,
                            height: 1,
                            depth: 1,
                        },
                        MetalSize {
                            width: merge_nth.max(1),
                            height: 1,
                            depth: 1,
                        },
                    )?;

                    std::mem::swap(&mut dst_binding, &mut tmp_binding);
                    len <<= 1;
                }
            }
        }
    }

    Ok(())
}

fn dispatch_rope(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src2_id = tensor_src_opt(tensor, 2);
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("rope src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("rope src1 {} is invalid", src1_id))?;

    if src1.desc.ty != TensorType::I32 {
        return Err(format!(
            "rope positions tensor must be I32, got {}",
            src1.desc.ty.name()
        ));
    }

    let src0_shape = shape4(src0)?;
    let src1_shape = shape4(src1)?;
    let dst_shape = shape4(tensor)?;

    if src1_shape.ne[0] % src0_shape.ne[2] != 0 || src1_shape.ne[0] < src0_shape.ne[2] {
        return Err(format!(
            "rope positions shape is incompatible: ne10={} ne02={}",
            src1_shape.ne[0], src0_shape.ne[2]
        ));
    }

    let args = KArgsRope {
        ne00: src0_shape.ne[0],
        ne01: src0_shape.ne[1],
        ne02: src0_shape.ne[2],
        ne03: src0_shape.ne[3],
        nb00: src0_shape.nb[0],
        nb01: src0_shape.nb[1],
        nb02: src0_shape.nb[2],
        nb03: src0_shape.nb[3],
        ne0: dst_shape.ne[0],
        ne1: dst_shape.ne[1],
        ne2: dst_shape.ne[2],
        ne3: dst_shape.ne[3],
        nb0: dst_shape.nb[0],
        nb1: dst_shape.nb[1],
        nb2: dst_shape.nb[2],
        nb3: dst_shape.nb[3],
        n_past: tensor.op_param_i32(0),
        n_dims: tensor.op_param_i32(1),
        n_ctx_orig: tensor.op_param_i32(4),
        freq_base: tensor.op_param_f32(5),
        freq_scale: tensor.op_param_f32(6),
        ext_factor: tensor.op_param_f32(7),
        attn_factor: tensor.op_param_f32(8),
        beta_fast: tensor.op_param_f32(9),
        beta_slow: tensor.op_param_f32(10),
        sect_0: tensor.op_param_i32(11),
        sect_1: tensor.op_param_i32(12),
        sect_2: tensor.op_param_i32(13),
        sect_3: tensor.op_param_i32(14),
        src2: src2_id.is_some(),
    };

    let mut nth = std::cmp::min(1024_u64, src0_shape.ne[0].max(1) as u64);
    nth = std::cmp::min(nth, stage.pipeline.max_threads_per_threadgroup.max(1));

    let src2_binding = match src2_id {
        Some(src2_id) => buffer_ref(compiled, 3, src2_id),
        None => buffer_ref(compiled, 3, src0_id),
    };

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, src1_id),
            src2_binding,
            buffer_ref(compiled, 4, tensor.id),
        ],
        &[],
        MetalSize {
            width: src0_shape.ne[1] as u64,
            height: src0_shape.ne[2] as u64,
            depth: src0_shape.ne[3] as u64,
        },
        MetalSize {
            width: nth,
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_ssm_conv(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("ssm_conv src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("ssm_conv src1 {} is invalid", src1_id))?;
    let args = KArgsSsmConv {
        ne00: i64_dim(src0, 0)?,
        ne01: i64_dim(src0, 1)?,
        ne02: i64_dim(src0, 2)?,
        nb00: u64::try_from(src0.nb[0]).map_err(|_| "ssm_conv nb00 exceeds u64".to_string())?,
        nb01: u64::try_from(src0.nb[1]).map_err(|_| "ssm_conv nb01 exceeds u64".to_string())?,
        nb02: u64::try_from(src0.nb[2]).map_err(|_| "ssm_conv nb02 exceeds u64".to_string())?,
        ne10: i64_dim(src1, 0)?,
        ne11: i64_dim(src1, 1)?,
        nb10: u64::try_from(src1.nb[0]).map_err(|_| "ssm_conv nb10 exceeds u64".to_string())?,
        nb11: u64::try_from(src1.nb[1]).map_err(|_| "ssm_conv nb11 exceeds u64".to_string())?,
        ne0: i64_dim(tensor, 0)?,
        ne1: i64_dim(tensor, 1)?,
        ne2: i64_dim(tensor, 2)?,
        nb0: u64::try_from(tensor.nb[0]).map_err(|_| "ssm_conv nb0 exceeds u64".to_string())?,
        nb1: u64::try_from(tensor.nb[1]).map_err(|_| "ssm_conv nb1 exceeds u64".to_string())?,
        nb2: u64::try_from(tensor.nb[2]).map_err(|_| "ssm_conv nb2 exceeds u64".to_string())?,
    };

    let use_batched = tensor.ne[1] > 1;
    let threadgroups = if use_batched {
        let batch_size = u64::try_from(constant_i16(&stage.descriptor.constants, FC_SSM_CONV + 0)?)
            .map_err(|_| "ssm_conv batch size exceeds u64".to_string())?;
        let n_token_batches = (tensor.ne[1].max(1) as u64).div_ceil(batch_size.max(1));
        MetalSize {
            width: src0.ne[1].max(1) as u64,
            height: n_token_batches,
            depth: src0.ne[2].max(1) as u64,
        }
    } else {
        MetalSize {
            width: src0.ne[1].max(1) as u64,
            height: tensor.ne[1].max(1) as u64,
            depth: src0.ne[2].max(1) as u64,
        }
    };
    let threads = if use_batched {
        MetalSize {
            width: u64::try_from(constant_i16(&stage.descriptor.constants, FC_SSM_CONV + 0)?)
                .map_err(|_| "ssm_conv batch size exceeds u64".to_string())?,
            height: 1,
            depth: 1,
        }
    } else {
        MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        }
    };

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, src1_id),
            buffer_ref(compiled, 3, tensor.id),
        ],
        &[],
        threadgroups,
        threads,
    )
}

fn dispatch_gated_delta_net(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src2_id = tensor_src(tensor, 2)?;
    let src3_id = tensor_src(tensor, 3)?;
    let src4_id = tensor_src(tensor, 4)?;
    let src5_id = tensor_src(tensor, 5)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("gated_delta_net src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("gated_delta_net src1 {} is invalid", src1_id))?;
    let src2 = ctx
        .tensor(src2_id)
        .ok_or_else(|| format!("gated_delta_net src2 {} is invalid", src2_id))?;
    let args = KArgsGatedDeltaNet {
        ne00: i32_dim(src0, 0)?,
        ne01: i32_dim(src0, 1)?,
        ne02: i32_dim(src0, 2)?,
        ne03: i32_dim(src0, 3)?,
        nb00: u64::try_from(src0.nb[0]).map_err(|_| "gated_delta nb00 exceeds u64".to_string())?,
        nb01: u64::try_from(src0.nb[1]).map_err(|_| "gated_delta nb01 exceeds u64".to_string())?,
        nb02: u64::try_from(src0.nb[2]).map_err(|_| "gated_delta nb02 exceeds u64".to_string())?,
        nb03: u64::try_from(src0.nb[3]).map_err(|_| "gated_delta nb03 exceeds u64".to_string())?,
        ne10: i32_dim(src1, 0)?,
        ne11: i32_dim(src1, 1)?,
        ne12: i32_dim(src1, 2)?,
        ne13: i32_dim(src1, 3)?,
        nb10: u64::try_from(src1.nb[0]).map_err(|_| "gated_delta nb10 exceeds u64".to_string())?,
        nb11: u64::try_from(src1.nb[1]).map_err(|_| "gated_delta nb11 exceeds u64".to_string())?,
        nb12: u64::try_from(src1.nb[2]).map_err(|_| "gated_delta nb12 exceeds u64".to_string())?,
        nb13: u64::try_from(src1.nb[3]).map_err(|_| "gated_delta nb13 exceeds u64".to_string())?,
        ne20: i32_dim(src2, 0)?,
        ne21: i32_dim(src2, 1)?,
        ne22: i32_dim(src2, 2)?,
        ne23: i32_dim(src2, 3)?,
        nb20: u64::try_from(src2.nb[0]).map_err(|_| "gated_delta nb20 exceeds u64".to_string())?,
        nb21: u64::try_from(src2.nb[1]).map_err(|_| "gated_delta nb21 exceeds u64".to_string())?,
        nb22: u64::try_from(src2.nb[2]).map_err(|_| "gated_delta nb22 exceeds u64".to_string())?,
        nb23: u64::try_from(src2.nb[3]).map_err(|_| "gated_delta nb23 exceeds u64".to_string())?,
        ns02: i32::try_from(src0.nb[2] / std::mem::size_of::<f32>())
            .map_err(|_| "gated_delta ns02 exceeds i32".to_string())?,
        ns12: i32::try_from(src1.nb[2] / std::mem::size_of::<f32>())
            .map_err(|_| "gated_delta ns12 exceeds i32".to_string())?,
        ns22: i32::try_from(src2.nb[2] / std::mem::size_of::<f32>())
            .map_err(|_| "gated_delta ns22 exceeds i32".to_string())?,
        ne0: i32_dim(tensor, 0)?,
        ne1: i32_dim(tensor, 1)?,
        ne2: i32_dim(tensor, 2)?,
        ne3: i32_dim(tensor, 3)?,
        nb0: u64::try_from(tensor.nb[0]).map_err(|_| "gated_delta nb0 exceeds u64".to_string())?,
        nb1: u64::try_from(tensor.nb[1]).map_err(|_| "gated_delta nb1 exceeds u64".to_string())?,
        nb2: u64::try_from(tensor.nb[2]).map_err(|_| "gated_delta nb2 exceeds u64".to_string())?,
        nb3: u64::try_from(tensor.nb[3]).map_err(|_| "gated_delta nb3 exceeds u64".to_string())?,
    };

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src0_id),
            buffer_ref(compiled, 2, src1_id),
            buffer_ref(compiled, 3, src2_id),
            buffer_ref(compiled, 4, src3_id),
            buffer_ref(compiled, 5, src4_id),
            buffer_ref(compiled, 6, src5_id),
            buffer_ref(compiled, 7, tensor.id),
        ],
        &[],
        MetalSize {
            width: (src2.ne[0].max(1) as u64) / (stage.pipeline.nsg.max(1) as u64),
            height: src2.ne[1].max(1) as u64,
            depth: src2.ne[3].max(1) as u64,
        },
        MetalSize {
            width: 32,
            height: stage.pipeline.nsg.max(1) as u64,
            depth: 1,
        },
    )
}

fn dispatch_flash_attn_ext(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let q_id = tensor_src(tensor, 0)?;
    let k_id = tensor_src(tensor, 1)?;
    let v_id = tensor_src(tensor, 2)?;
    let mask_id = tensor_src_opt(tensor, 3);
    let sinks_id = tensor_src_opt(tensor, 4);

    let q = ctx
        .tensor(q_id)
        .ok_or_else(|| format!("flash_attn_ext q {} is invalid", q_id))?;
    let k = ctx
        .tensor(k_id)
        .ok_or_else(|| format!("flash_attn_ext k {} is invalid", k_id))?;
    let v = ctx
        .tensor(v_id)
        .ok_or_else(|| format!("flash_attn_ext v {} is invalid", v_id))?;
    let mask = mask_id
        .map(|id| {
            ctx.tensor(id)
                .ok_or_else(|| format!("flash_attn_ext mask {} is invalid", id))
        })
        .transpose()?;
    let sinks = sinks_id
        .map(|id| {
            ctx.tensor(id)
                .ok_or_else(|| format!("flash_attn_ext sinks {} is invalid", id))
        })
        .transpose()?;

    if q.desc.ty != TensorType::F32 {
        return Err(format!(
            "flash_attn_ext currently requires q to be F32, got {}",
            q.desc.ty.name()
        ));
    }
    if !flash_attn_supported_head_dim(
        usize::try_from(q.ne[0]).map_err(|_| "flash_attn head dim overflow".to_string())?,
    ) {
        return Err(format!("unsupported flash_attn_ext head dim {}", q.ne[0]));
    }
    if k.desc.ty != v.desc.ty {
        return Err(format!(
            "flash_attn_ext requires k/v to share a type, got {} and {}",
            k.desc.ty.name(),
            v.desc.ty.name()
        ));
    }

    let q_shape = shape4(q)?;
    let k_shape = shape4(k)?;
    let v_shape = shape4(v)?;
    let dst_shape = shape4(tensor)?;

    let has_mask = mask.is_some();
    let has_sinks = sinks.is_some();
    let max_bias = tensor.op_param_f32(1);
    let logit_softcap = tensor.op_param_f32(2);
    let has_bias = max_bias != 0.0;
    let has_scap = logit_softcap != 0.0;

    if !flash_attn_ext_use_vec(q) {
        return Err(
            "flash_attn_ext compiled executor currently only supports the Metal vec path"
                .to_string(),
        );
    }

    let stage_main = node
        .stages
        .iter()
        .find(|stage| {
            stage
                .descriptor
                .base_name
                .starts_with("kernel_flash_attn_ext_vec_")
        })
        .ok_or_else(|| "flash_attn_ext is missing vec main stage".to_string())?;
    let stage_reduce = node
        .stages
        .iter()
        .find(|stage| stage.descriptor.base_name == "kernel_flash_attn_ext_vec_reduce")
        .ok_or_else(|| "flash_attn_ext is missing vec reduce stage".to_string())?;
    let stage_pad = node
        .stages
        .iter()
        .find(|stage| stage.descriptor.base_name == "kernel_flash_attn_ext_pad");

    let pad_bytes = flash_attn_ext_extra_pad_bytes(q, k, v, mask)?;
    let blk_bytes = flash_attn_ext_extra_blk_bytes(q, mask)?;
    let tmp_bytes = flash_attn_ext_extra_tmp_bytes(q, v)?;

    let has_kvpad = k_shape.ne[1] % OP_FLASH_ATTN_EXT_VEC_NCPSG != 0;
    let nsg = flash_attn_ext_vec_nsg(k);
    let nwg = 32_i32;
    let n_head =
        usize::try_from(q_shape.ne[2]).map_err(|_| "flash_attn n_head overflow".to_string())?;
    let n_head_log2 = if n_head <= 1 {
        1_i32
    } else {
        let p = (usize::BITS - 1) - n_head.leading_zeros();
        (1usize << p) as i32
    };
    let m0 = (2.0f32).powf(-(max_bias) / (n_head_log2 as f32));
    let m1 = (2.0f32).powf(-(max_bias / 2.0) / (n_head_log2 as f32));
    let scale = if has_scap {
        tensor.op_param_f32(0) / logit_softcap
    } else {
        tensor.op_param_f32(0)
    };

    let default_mask = default_flash_mask_shape(&q_shape, &k_shape)?;
    let mask_shape = match mask {
        Some(mask) => shape4(mask)?,
        None => default_mask,
    };
    let ns10 = i32::try_from(k_shape.nb[1] / k_shape.nb[0])
        .map_err(|_| "flash_attn ns10 overflow".to_string())?;
    let ns20 = i32::try_from(v_shape.nb[1] / v_shape.nb[0])
        .map_err(|_| "flash_attn ns20 overflow".to_string())?;

    if has_kvpad {
        let stage_pad =
            stage_pad.ok_or_else(|| "flash_attn_ext is missing vec pad stage".to_string())?;
        let pad_ncpsg = constant_i32(&stage_pad.descriptor.constants, FC_FLASH_ATTN_EXT_PAD + 25)?;
        let args = KArgsFlashAttnExtPad {
            ne11: k_shape.ne[1],
            ne_12_2: k_shape.ne[2],
            ne_12_3: k_shape.ne[3],
            nb11: k_shape.nb[1],
            nb12: k_shape.nb[2],
            nb13: k_shape.nb[3],
            nb21: v_shape.nb[1],
            nb22: v_shape.nb[2],
            nb23: v_shape.nb[3],
            ne31: mask_shape.ne[1],
            ne32: mask_shape.ne[2],
            ne33: mask_shape.ne[3],
            nb31: mask_shape.nb[1],
            nb32: mask_shape.nb[2],
            nb33: mask_shape.nb[3],
        };

        runtime.dispatch_compute(
            &stage_pad.pipeline,
            bytes_of(&args),
            &[
                buffer_ref(compiled, 1, k_id),
                buffer_ref(compiled, 2, v_id),
                mask_id
                    .map(|id| buffer_ref(compiled, 3, id))
                    .unwrap_or_else(|| buffer_ref(compiled, 3, q_id)),
                tail_node_buffer_ref(compiled, node, 4, 0)?,
            ],
            &[],
            MetalSize {
                width: pad_ncpsg as u64,
                height: k_shape.ne[2].max(mask_shape.ne[2]) as u64,
                depth: k_shape.ne[3].max(mask_shape.ne[3]) as u64,
            },
            MetalSize {
                width: 32,
                height: 1,
                depth: 1,
            },
        )?;
    }

    let args = KArgsFlashAttnExtVec {
        ne01: q_shape.ne[1],
        ne02: q_shape.ne[2],
        ne03: q_shape.ne[3],
        nb01: q_shape.nb[1],
        nb02: q_shape.nb[2],
        nb03: q_shape.nb[3],
        ne11: k_shape.ne[1],
        ne_12_2: k_shape.ne[2],
        ne_12_3: k_shape.ne[3],
        ns10,
        nb11: k_shape.nb[1],
        nb12: k_shape.nb[2],
        nb13: k_shape.nb[3],
        ns20,
        nb21: v_shape.nb[1],
        nb22: v_shape.nb[2],
        nb23: v_shape.nb[3],
        ne31: mask_shape.ne[1],
        ne32: mask_shape.ne[2],
        ne33: mask_shape.ne[3],
        nb31: mask_shape.nb[1],
        nb32: mask_shape.nb[2],
        nb33: mask_shape.nb[3],
        ne1: dst_shape.ne[1],
        ne2: dst_shape.ne[2],
        ne3: dst_shape.ne[3],
        scale,
        max_bias,
        m0,
        m1,
        n_head_log2,
        logit_softcap,
    };

    runtime.dispatch_compute(
        &stage_main.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, q_id),
            buffer_ref(compiled, 2, k_id),
            buffer_ref(compiled, 3, v_id),
            mask_id
                .map(|id| buffer_ref(compiled, 4, id))
                .unwrap_or_else(|| buffer_ref(compiled, 4, q_id)),
            sinks_id
                .map(|id| buffer_ref(compiled, 5, id))
                .unwrap_or_else(|| buffer_ref(compiled, 5, q_id)),
            tail_node_buffer_ref(compiled, node, 6, 0)?,
            tail_node_buffer_ref(compiled, node, 7, pad_bytes + blk_bytes)?,
        ],
        &[(0, stage_main.pipeline.smem_bytes)],
        MetalSize {
            width: ((q_shape.ne[1] + OP_FLASH_ATTN_EXT_VEC_NQPSG - 1) / OP_FLASH_ATTN_EXT_VEC_NQPSG)
                as u64,
            height: q_shape.ne[2] as u64,
            depth: (q_shape.ne[3] * nwg) as u64,
        },
        MetalSize {
            width: 32,
            height: nsg as u64,
            depth: 1,
        },
    )?;

    let args_reduce = KArgsFlashAttnExtVecReduce {
        nrows: i32::try_from(
            usize::try_from(dst_shape.ne[1])
                .map_err(|_| "flash_attn reduce ne1 overflow".to_string())?
                .checked_mul(
                    usize::try_from(dst_shape.ne[2])
                        .map_err(|_| "flash_attn reduce ne2 overflow".to_string())?,
                )
                .and_then(|v| v.checked_mul(usize::try_from(dst_shape.ne[3]).ok()?))
                .ok_or_else(|| "flash_attn reduce nrows overflow".to_string())?,
        )
        .map_err(|_| "flash_attn reduce nrows exceeds i32".to_string())?,
    };

    let _ = tmp_bytes;
    let _ = has_mask;
    let _ = has_sinks;
    let _ = has_bias;

    runtime.dispatch_compute(
        &stage_reduce.pipeline,
        bytes_of(&args_reduce),
        &[
            tail_node_buffer_ref(compiled, node, 1, pad_bytes + blk_bytes)?,
            buffer_ref(compiled, 2, tensor.id),
        ],
        &[],
        MetalSize {
            width: args_reduce.nrows as u64,
            height: 1,
            depth: 1,
        },
        MetalSize {
            width: (32 * nwg) as u64,
            height: 1,
            depth: 1,
        },
    )
}

fn dispatch_cpy(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let stage = main_stage(node, tensor.op)?;
    let src0_id = tensor_src(tensor, 0)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("cpy src0 {} is invalid", src0_id))?;
    dispatch_cpy_stage(
        runtime,
        compiled,
        stage,
        src0,
        src0_id,
        tensor.id,
        tensor.desc.ty,
        0,
        Layout4::from_tensor(tensor)?,
    )
}

fn dispatch_set(
    runtime: &MetalRuntime,
    ctx: &Context,
    compiled: &MetalCompiledGraph,
    tensor: &Tensor,
    node: &MetalCompiledNode,
) -> Result<(), String> {
    let src0_id = tensor_src(tensor, 0)?;
    let src1_id = tensor_src(tensor, 1)?;
    let src0 = ctx
        .tensor(src0_id)
        .ok_or_else(|| format!("set src0 {} is invalid", src0_id))?;
    let src1 = ctx
        .tensor(src1_id)
        .ok_or_else(|| format!("set src1 {} is invalid", src1_id))?;

    let pnb1 = usize::try_from(tensor.op_param_i32(0))
        .map_err(|_| format!("set pnb1 is negative: {}", tensor.op_param_i32(0)))?;
    let pnb2 = usize::try_from(tensor.op_param_i32(1))
        .map_err(|_| format!("set pnb2 is negative: {}", tensor.op_param_i32(1)))?;
    let pnb3 = usize::try_from(tensor.op_param_i32(2))
        .map_err(|_| format!("set pnb3 is negative: {}", tensor.op_param_i32(2)))?;
    let offs = usize::try_from(tensor.op_param_i32(3))
        .map_err(|_| format!("set offs is negative: {}", tensor.op_param_i32(3)))?;
    let inplace = tensor.op_param_i32(4) != 0;

    let dst_layout = Layout4 {
        ne: [
            i64_dim(src1, 0)?,
            i64_dim(src1, 1)?,
            i64_dim(src1, 2)?,
            i64_dim(src1, 3)?,
        ],
        nb: [
            ggml_type_size_for_type(tensor.desc.ty) as u64,
            u64::try_from(pnb1).map_err(|_| "set pnb1 exceeds u64".to_string())?,
            u64::try_from(pnb2).map_err(|_| "set pnb2 exceeds u64".to_string())?,
            u64::try_from(pnb3).map_err(|_| "set pnb3 exceeds u64".to_string())?,
        ],
    };

    let mut stages = node.stages.iter();
    if !inplace {
        let stage = stages
            .next()
            .ok_or_else(|| "set op is missing copy stage".to_string())?;
        dispatch_cpy_stage(
            runtime,
            compiled,
            stage,
            src0,
            src0_id,
            tensor.id,
            tensor.desc.ty,
            0,
            Layout4::from_tensor(tensor)?,
        )?;
    }

    let stage = stages
        .next()
        .ok_or_else(|| "set op is missing main stage".to_string())?;
    dispatch_cpy_stage(
        runtime,
        compiled,
        stage,
        src1,
        src1_id,
        tensor.id,
        tensor.desc.ty,
        offs,
        dst_layout,
    )
}

fn dispatch_cpy_stage(
    runtime: &MetalRuntime,
    compiled: &MetalCompiledGraph,
    stage: &MetalCompiledStage,
    src: &Tensor,
    src_id: TensorId,
    dst_id: TensorId,
    dst_ty: TensorType,
    dst_extra_offset: usize,
    dst_layout: Layout4,
) -> Result<(), String> {
    let ne00 = i64_dim(src, 0)?;
    let ne01 = i64_dim(src, 1)?;
    let ne02 = i64_dim(src, 2)?;
    let ne03 = i64_dim(src, 3)?;
    let nb00 = u64::try_from(src.nb[0]).map_err(|_| "cpy nb00 exceeds u64".to_string())?;
    let nb01 = u64::try_from(src.nb[1]).map_err(|_| "cpy nb01 exceeds u64".to_string())?;
    let nb02 = u64::try_from(src.nb[2]).map_err(|_| "cpy nb02 exceeds u64".to_string())?;
    let nb03 = u64::try_from(src.nb[3]).map_err(|_| "cpy nb03 exceeds u64".to_string())?;

    let src_blck = ggml_blck_size_for_type(src.desc.ty) as i64;
    if ne00 % src_blck != 0 {
        return Err(format!(
            "cpy source dim0 {} is not divisible by block size {} for {}",
            ne00,
            src_blck,
            src.desc.ty.name()
        ));
    }

    let mut nk0 = ne00;
    if src.desc.ty.is_quantized() {
        nk0 = ne00 / 16;
    } else if dst_ty.is_quantized() {
        nk0 = ne00 / ggml_blck_size_for_type(dst_ty) as i64;
    }

    let max_threads = stage.pipeline.max_threads_per_threadgroup.max(1);
    let mut nth = std::cmp::min(nk0.max(1) as u64, max_threads);
    let mut nrptg = 1u64;

    if ggml_blck_size_for_type(src.desc.ty) == 1 && ggml_blck_size_for_type(dst_ty) == 1 {
        if nth > nk0 as u64 {
            nrptg = nth.div_ceil(nk0.max(1) as u64);
            nth = nk0.max(1) as u64;
            if nrptg * nth > max_threads {
                nrptg = nrptg.saturating_sub(1).max(1);
            }
        }
    }

    nth = std::cmp::min(nth, nk0.max(1) as u64);

    let args = KArgsCpy {
        nk0,
        ne00,
        ne01,
        ne02,
        ne03,
        nb00,
        nb01,
        nb02,
        nb03,
        ne0: dst_layout.ne[0],
        ne1: dst_layout.ne[1],
        ne2: dst_layout.ne[2],
        ne3: dst_layout.ne[3],
        nb0: dst_layout.nb[0],
        nb1: dst_layout.nb[1],
        nb2: dst_layout.nb[2],
        nb3: dst_layout.nb[3],
    };

    let nw0 = if nrptg == 1 {
        (nk0.max(1) as u64).div_ceil(nth)
    } else {
        1
    };
    let width = nw0
        .checked_mul((ne01.max(1) as u64).div_ceil(nrptg))
        .ok_or_else(|| "overflow computing cpy threadgroup width".to_string())?;

    runtime.dispatch_compute(
        &stage.pipeline,
        bytes_of(&args),
        &[
            buffer_ref(compiled, 1, src_id),
            buffer_ref_with_offset(compiled, 2, dst_id, dst_extra_offset)?,
        ],
        &[],
        MetalSize {
            width,
            height: ne02.max(1) as u64,
            depth: ne03.max(1) as u64,
        },
        MetalSize {
            width: nth,
            height: nrptg,
            depth: 1,
        },
    )
}

#[derive(Clone, Copy, Debug, Default)]
struct Shape4 {
    ne: [i32; 4],
    nb: [u64; 4],
    numel: usize,
}

#[derive(Clone, Copy, Debug)]
struct Layout4 {
    ne: [i64; 4],
    nb: [u64; 4],
}

impl Layout4 {
    fn from_tensor(tensor: &Tensor) -> Result<Self, String> {
        Ok(Self {
            ne: [
                i64_dim(tensor, 0)?,
                i64_dim(tensor, 1)?,
                i64_dim(tensor, 2)?,
                i64_dim(tensor, 3)?,
            ],
            nb: [
                u64::try_from(tensor.nb[0]).map_err(|_| "layout nb0 exceeds u64".to_string())?,
                u64::try_from(tensor.nb[1]).map_err(|_| "layout nb1 exceeds u64".to_string())?,
                u64::try_from(tensor.nb[2]).map_err(|_| "layout nb2 exceeds u64".to_string())?,
                u64::try_from(tensor.nb[3]).map_err(|_| "layout nb3 exceeds u64".to_string())?,
            ],
        })
    }
}

fn shape4(tensor: &Tensor) -> Result<Shape4, String> {
    let mut ne = [1i32; 4];
    let mut nb = [0u64; 4];
    for i in 0..4 {
        ne[i] = i32::try_from(tensor.ne[i]).map_err(|_| {
            format!(
                "tensor '{}' ne[{}] exceeds i32",
                tensor.name().unwrap_or("<unnamed>"),
                i
            )
        })?;
        nb[i] = u64::try_from(tensor.nb[i]).map_err(|_| {
            format!(
                "tensor '{}' nb[{}] exceeds u64",
                tensor.name().unwrap_or("<unnamed>"),
                i
            )
        })?;
    }
    let numel = usize::try_from(tensor.nelements()).map_err(|_| {
        format!(
            "tensor '{}' numel exceeds usize",
            tensor.name().unwrap_or("<unnamed>")
        )
    })?;
    Ok(Shape4 { ne, nb, numel })
}

fn nrows(shape: &Shape4) -> usize {
    (shape.ne[1] as usize)
        .saturating_mul(shape.ne[2] as usize)
        .saturating_mul(shape.ne[3] as usize)
}

fn binding(
    compiled: &MetalCompiledGraph,
    tensor_id: TensorId,
) -> Result<&MetalTensorBinding, String> {
    compiled
        .bindings
        .get(&tensor_id)
        .ok_or_else(|| format!("compiled graph has no binding for tensor {}", tensor_id))
}

fn buffer_ref<'a>(
    compiled: &'a MetalCompiledGraph,
    index: u64,
    tensor_id: TensorId,
) -> MetalBufferBindingRef<'a> {
    let binding = compiled.bindings.get(&tensor_id).unwrap();
    MetalBufferBindingRef {
        index,
        buffer: &compiled.main_buffer,
        offset_bytes: binding.offset_bytes,
    }
}

fn buffer_ref_with_offset<'a>(
    compiled: &'a MetalCompiledGraph,
    index: u64,
    tensor_id: TensorId,
    extra_offset: usize,
) -> Result<MetalBufferBindingRef<'a>, String> {
    let binding = binding(compiled, tensor_id)?;
    Ok(MetalBufferBindingRef {
        index,
        buffer: &compiled.main_buffer,
        offset_bytes: binding
            .offset_bytes
            .checked_add(extra_offset)
            .ok_or_else(|| format!("buffer binding offset overflow for tensor {}", tensor_id))?,
    })
}

fn tail_buffer_ref<'a>(
    compiled: &'a MetalCompiledGraph,
    index: u64,
    offset_bytes: usize,
) -> Result<MetalBufferBindingRef<'a>, String> {
    let buffer = compiled
        .tail_buffer
        .as_ref()
        .ok_or_else(|| "compiled graph is missing a tail buffer".to_string())?;
    if offset_bytes > compiled.tail_buffer_size {
        return Err(format!(
            "tail buffer offset {} exceeds size {}",
            offset_bytes, compiled.tail_buffer_size
        ));
    }
    Ok(MetalBufferBindingRef {
        index,
        buffer,
        offset_bytes,
    })
}

fn tail_node_buffer_ref<'a>(
    compiled: &'a MetalCompiledGraph,
    node: &MetalCompiledNode,
    index: u64,
    local_offset: usize,
) -> Result<MetalBufferBindingRef<'a>, String> {
    tail_buffer_ref(
        compiled,
        index,
        node.tail_offset_bytes
            .checked_add(local_offset)
            .ok_or_else(|| "tail node buffer offset overflow".to_string())?,
    )
}

fn dummy_buffer_ref<'a>(index: u64, buffer: &'a super::MetalBuffer) -> MetalBufferBindingRef<'a> {
    MetalBufferBindingRef {
        index,
        buffer,
        offset_bytes: 0,
    }
}

fn stage_kind<'a>(
    node: &'a MetalCompiledNode,
    kind: MetalStageKind,
    op: Op,
) -> Result<&'a MetalCompiledStage, String> {
    node.stages
        .iter()
        .find(|stage| stage.kind == kind)
        .ok_or_else(|| format!("compiled node for op {} has no {:?} stage", op.name(), kind))
}

fn main_stage<'a>(node: &'a MetalCompiledNode, op: Op) -> Result<&'a MetalCompiledStage, String> {
    stage_kind(node, MetalStageKind::Main, op)
}

fn tensor_src(tensor: &Tensor, index: usize) -> Result<TensorId, String> {
    tensor.src[index].ok_or_else(|| {
        format!(
            "tensor '{}' is missing source {}",
            tensor.name().unwrap_or("<unnamed>"),
            index
        )
    })
}

fn tensor_src_opt(tensor: &Tensor, index: usize) -> Option<TensorId> {
    tensor.src.get(index).and_then(|src| *src)
}

fn i32_dim(tensor: &Tensor, dim: usize) -> Result<i32, String> {
    i32::try_from(tensor.ne[dim]).map_err(|_| {
        format!(
            "tensor '{}' dim {} exceeds i32",
            tensor.name().unwrap_or("<unnamed>"),
            dim
        )
    })
}

fn i64_dim(tensor: &Tensor, dim: usize) -> Result<i64, String> {
    Ok(tensor.ne[dim])
}

fn constant_i16(constants: &[super::FunctionConstant], idx: i32) -> Result<i16, String> {
    constants
        .iter()
        .find(|constant| constant.idx == idx)
        .ok_or_else(|| format!("missing function constant {}", idx))
        .and_then(|constant| match constant.value {
            FunctionConstantValue::Int16(value) => Ok(value),
            _ => Err(format!("function constant {} is not i16", idx)),
        })
}

fn constant_i32(constants: &[super::FunctionConstant], idx: i32) -> Result<i32, String> {
    constants
        .iter()
        .find(|constant| constant.idx == idx)
        .ok_or_else(|| format!("missing function constant {}", idx))
        .and_then(|constant| match constant.value {
            FunctionConstantValue::Int32(value) => Ok(value),
            _ => Err(format!("function constant {} is not i32", idx)),
        })
}

fn flash_attn_supported_head_dim(d: usize) -> bool {
    matches!(
        d,
        32 | 40 | 48 | 64 | 72 | 80 | 96 | 112 | 128 | 192 | 256 | 576
    )
}

fn flash_attn_ext_use_vec(q: &Tensor) -> bool {
    q.ne[1] < 20 && q.ne[0] % 32 == 0
}

fn flash_attn_ext_vec_nsg(k: &Tensor) -> i32 {
    let mut nsg = 1_i32;
    while 2 * 32 * nsg as i64 * i64::from(OP_FLASH_ATTN_EXT_VEC_NCPSG) < k.ne[1] && nsg < 4 {
        nsg *= 2;
    }
    nsg
}

fn flash_attn_ext_extra_pad_bytes(
    q: &Tensor,
    k: &Tensor,
    v: &Tensor,
    mask: Option<&Tensor>,
) -> Result<usize, String> {
    let has_mask = mask.is_some();
    let mask_bytes = if has_mask {
        ggml_type_size_for_type(TensorType::F16)
            * usize::try_from(mask.unwrap().ne[1] * mask.unwrap().ne[2] * mask.unwrap().ne[3])
                .map_err(|_| "flash_attn pad mask bytes overflow".to_string())?
    } else {
        0
    };
    let ncpsg = if flash_attn_ext_use_vec(q) {
        OP_FLASH_ATTN_EXT_VEC_NCPSG
    } else {
        64
    };
    Ok(
        usize::try_from(ncpsg).map_err(|_| "flash_attn ncpsg overflow".to_string())?
            * (k.nb[1]
                * usize::try_from(k.ne[2] * k.ne[3])
                    .map_err(|_| "flash_attn k dims overflow".to_string())?
                + v.nb[1]
                    * usize::try_from(v.ne[2] * v.ne[3])
                        .map_err(|_| "flash_attn v dims overflow".to_string())?
                + mask_bytes),
    )
}

fn flash_attn_ext_extra_blk_bytes(q: &Tensor, mask: Option<&Tensor>) -> Result<usize, String> {
    let Some(mask) = mask else {
        return Ok(0);
    };
    let nqptg = if flash_attn_ext_use_vec(q) {
        1_i64
    } else {
        8_i64
    };
    let ncpsg = if flash_attn_ext_use_vec(q) {
        32_i64
    } else {
        64_i64
    };
    let ne1 = (q.ne[1] + nqptg - 1) / nqptg;
    let ne0 = (mask.ne[0] + ncpsg - 1) / ncpsg;
    let bytes = usize::try_from(ne0 * ne1 * mask.ne[2] * mask.ne[3])
        .map_err(|_| "flash_attn blk bytes overflow".to_string())?;
    Ok(ggml_pad(bytes * std::mem::size_of::<i8>(), 32))
}

fn flash_attn_ext_extra_tmp_bytes(q: &Tensor, v: &Tensor) -> Result<usize, String> {
    let nwg = 32_i64;
    let ne01_max = q.ne[1].min(32);
    let items = ne01_max * q.ne[2] * q.ne[3] * nwg * (v.ne[0] + 2);
    Ok(ggml_type_size_for_type(TensorType::F32)
        * usize::try_from(items).map_err(|_| "flash_attn tmp bytes overflow".to_string())?)
}

fn mul_mat_id_extra_tpe_bytes(src0: &Tensor) -> Result<usize, String> {
    Ok(ggml_type_size_for_type(TensorType::I32)
        * usize::try_from(src0.ne[2]).map_err(|_| "mul_mat_id tpe bytes overflow".to_string())?)
}

fn default_flash_mask_shape(q: &Shape4, k: &Shape4) -> Result<Shape4, String> {
    let nb31 = (u64::try_from(k.ne[1])
        .map_err(|_| "flash_attn default mask ne1 overflow".to_string())?)
    .checked_mul(ggml_type_size_for_type(TensorType::F16) as u64)
    .ok_or_else(|| "flash_attn default mask nb31 overflow".to_string())?;
    let nb32 = nb31
        .checked_mul(
            u64::try_from(q.ne[1]).map_err(|_| "flash_attn default mask q overflow".to_string())?,
        )
        .ok_or_else(|| "flash_attn default mask nb32 overflow".to_string())?;
    Ok(Shape4 {
        ne: [k.ne[1], q.ne[1], 1, 1],
        nb: [
            ggml_type_size_for_type(TensorType::F16) as u64,
            nb31,
            nb32,
            nb32,
        ],
        numel: usize::try_from(k.ne[1])
            .unwrap_or(0)
            .saturating_mul(usize::try_from(q.ne[1]).unwrap_or(0)),
    })
}

fn parse_trailing_i32(value: &str, marker: &str) -> Result<i32, String> {
    value
        .rsplit_once(marker)
        .ok_or_else(|| format!("'{}' does not contain '{}'", value, marker))
        .and_then(|(_, tail)| {
            tail.parse::<i32>()
                .map_err(|err| format!("failed to parse integer from '{}': {}", value, err))
        })
}

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::core::InitParams;
    use crate::graph::Graph;
    use crate::op::Op;
    use crate::tensor::{ggml_row_size_for_type, BufferUsage, TensorType};

    #[test]
    fn prepare_graph_collects_bindings_and_tail_bytes() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let a = ctx
            .new_tensor_2d(TensorType::F32, 257, 4, BufferUsage::Activations)
            .unwrap();
        let out = ctx
            .new_tensor_2d(TensorType::I32, 257, 4, BufferUsage::Activations)
            .unwrap();

        {
            let tensor = ctx.tensor_mut(out).unwrap();
            tensor.op = Op::Argsort;
            tensor.src[0] = Some(a);
            tensor.set_op_param_i32(0, 0);
        }

        let mut graph = Graph::new();
        graph.add_leaf(a);
        graph.add_node(out);

        let prepared = prepare_graph(&ctx, &graph, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(prepared.nodes.len(), 1);
        assert_eq!(prepared.bindings.len(), 2);
        assert!(prepared.tail_buffer_size >= ctx.tensor(out).unwrap().nbytes());
        assert!(prepared.main_buffer_size >= ctx.used_mem());
    }

    #[test]
    fn prepare_graph_assigns_distinct_tail_offsets_for_temp_nodes() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let a = ctx
            .new_tensor_2d(TensorType::F32, 257, 4, BufferUsage::Activations)
            .unwrap();
        let sort = ctx.argsort(a, BufferUsage::Activations).unwrap();
        let topk = ctx.top_k(a, 8, BufferUsage::Activations).unwrap();

        let mut graph = Graph::new();
        graph.add_leaf(a);
        graph.add_node(sort);
        graph.add_node(topk);

        let prepared = prepare_graph(&ctx, &graph, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(prepared.nodes.len(), 2);
        assert!(prepared.nodes[0].tail_offset_bytes < prepared.tail_buffer_size);
        assert!(prepared.nodes[1].tail_offset_bytes < prepared.tail_buffer_size);
        assert_ne!(
            prepared.nodes[0].tail_offset_bytes,
            prepared.nodes[1].tail_offset_bytes
        );
    }

    #[test]
    fn prepare_graph_resolves_view_offsets() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_2d(TensorType::F32, 16, 8, BufferUsage::Activations)
            .unwrap();
        let view = ctx
            .view(src, TensorType::F32, &[8, 8], &[4, 64], 32)
            .unwrap();
        {
            let tensor = ctx.tensor_mut(view).unwrap();
            tensor.op = Op::Cont;
            tensor.src[0] = Some(src);
        }

        let mut graph = Graph::new();
        graph.add_leaf(src);
        graph.add_node(view);

        let prepared = prepare_graph(&ctx, &graph, MetalDeviceFeatures::default()).unwrap();
        let src_offset = prepared.bindings.get(&src).unwrap().offset_bytes;
        let view_offset = prepared.bindings.get(&view).unwrap().offset_bytes;
        assert_eq!(view_offset, src_offset + 32);
    }

    #[test]
    fn executes_rope_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_3d(TensorType::F32, 4, 1, 2, BufferUsage::Activations)
            .unwrap();
        let pos = ctx
            .new_tensor_1d(TensorType::I32, 2, BufferUsage::Activations)
            .unwrap();
        let rope = ctx.rope(src, pos, 4, 0, BufferUsage::Activations).unwrap();

        let src_values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let pos_values = vec![0_i32, 1_i32];
        ctx.write_tensor_data(src, &f32s_to_bytes(&src_values))
            .unwrap();
        ctx.write_tensor_data(pos, &i32s_to_bytes(&pos_values))
            .unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, rope).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[rope]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&rope).unwrap());
        let expected = cpu_rope_norm_f32(&src_values, &pos_values, 4, 1, 2);

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 1.0e-4,
                "rope output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_rope_multi_single_token_consistently_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut full_ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let d = 128_i64;
        let n_head = 8_i64;
        let n_tokens = 2_i64;

        let src_full = full_ctx
            .new_tensor_3d(
                TensorType::F32,
                d,
                n_head,
                n_tokens,
                BufferUsage::Activations,
            )
            .unwrap();
        let pos_full = full_ctx
            .new_tensor_1d(TensorType::I32, n_tokens, BufferUsage::Activations)
            .unwrap();
        let rope_full = full_ctx
            .rope_multi(
                src_full,
                pos_full,
                None,
                32,
                [11, 11, 10, 0],
                crate::GGML_ROPE_TYPE_IMROPE,
                262_144,
                1_000_000.0,
                1.0,
                0.0,
                1.0,
                32.0,
                1.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let src_full_values = patterned_f32s((d * n_head * n_tokens) as usize, 0.05, -0.002);
        full_ctx
            .write_tensor_data(src_full, &f32s_to_bytes(&src_full_values))
            .unwrap();
        full_ctx
            .write_tensor_data(pos_full, &i32s_to_bytes(&[0, 1]))
            .unwrap();

        let mut full_graph = Graph::new();
        full_graph
            .build_forward_expand(&full_ctx, rope_full)
            .unwrap();

        let full_prepared = prepare_graph(&full_ctx, &full_graph, runtime.features()).unwrap();
        let full_session = MetalGraphSession::from_runtime(
            runtime,
            &full_ctx,
            &full_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let full_execution = full_session.execute(&full_ctx, &[], &[rope_full]).unwrap();
        let full_values = bytes_to_f32s(full_execution.outputs.get(&rope_full).unwrap());
        let token_width = (d * n_head) as usize;
        let full_last_token = full_values[full_values.len() - token_width..].to_vec();

        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };
        let mut step_ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let src_step = step_ctx
            .new_tensor_3d(TensorType::F32, d, n_head, 1, BufferUsage::Activations)
            .unwrap();
        let pos_step = step_ctx
            .new_tensor_1d(TensorType::I32, 1, BufferUsage::Activations)
            .unwrap();
        let rope_step = step_ctx
            .rope_multi(
                src_step,
                pos_step,
                None,
                32,
                [11, 11, 10, 0],
                crate::GGML_ROPE_TYPE_IMROPE,
                262_144,
                1_000_000.0,
                1.0,
                0.0,
                1.0,
                32.0,
                1.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let src_step_values = {
            let mut values = Vec::with_capacity(token_width);
            for head in 0..(n_head as usize) {
                let base_offset = token_width + head * (d as usize);
                values.extend_from_slice(&src_full_values[base_offset..base_offset + d as usize]);
            }
            values
        };
        step_ctx
            .write_tensor_data(src_step, &f32s_to_bytes(&src_step_values))
            .unwrap();
        step_ctx
            .write_tensor_data(pos_step, &i32s_to_bytes(&[1]))
            .unwrap();

        let mut step_graph = Graph::new();
        step_graph
            .build_forward_expand(&step_ctx, rope_step)
            .unwrap();

        let step_prepared = prepare_graph(&step_ctx, &step_graph, runtime.features()).unwrap();
        let step_session = MetalGraphSession::from_runtime(
            runtime,
            &step_ctx,
            &step_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let step_execution = step_session.execute(&step_ctx, &[], &[rope_step]).unwrap();
        let step_values = bytes_to_f32s(step_execution.outputs.get(&rope_step).unwrap());

        assert_eq!(step_values.len(), full_last_token.len());
        for (a, e) in step_values.iter().zip(full_last_token.iter()) {
            assert!(
                (a - e).abs() < 1.0e-4,
                "rope_multi token1 mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_rope_multi_kv_head_shape_consistently_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut full_ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let d = 128_i64;
        let n_head = 2_i64;
        let n_tokens = 2_i64;

        let src_full = full_ctx
            .new_tensor_3d(
                TensorType::F32,
                d,
                n_head,
                n_tokens,
                BufferUsage::Activations,
            )
            .unwrap();
        let pos_full = full_ctx
            .new_tensor_1d(TensorType::I32, n_tokens, BufferUsage::Activations)
            .unwrap();
        let rope_full = full_ctx
            .rope_multi(
                src_full,
                pos_full,
                None,
                32,
                [11, 11, 10, 0],
                crate::GGML_ROPE_TYPE_IMROPE,
                262_144,
                1_000_000.0,
                1.0,
                0.0,
                1.0,
                32.0,
                1.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let src_full_values = patterned_f32s((d * n_head * n_tokens) as usize, 0.02, 0.007);
        full_ctx
            .write_tensor_data(src_full, &f32s_to_bytes(&src_full_values))
            .unwrap();
        full_ctx
            .write_tensor_data(pos_full, &i32s_to_bytes(&[0, 1]))
            .unwrap();

        let mut full_graph = Graph::new();
        full_graph
            .build_forward_expand(&full_ctx, rope_full)
            .unwrap();

        let full_prepared = prepare_graph(&full_ctx, &full_graph, runtime.features()).unwrap();
        let full_session = MetalGraphSession::from_runtime(
            runtime,
            &full_ctx,
            &full_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let full_execution = full_session.execute(&full_ctx, &[], &[rope_full]).unwrap();
        let full_values = bytes_to_f32s(full_execution.outputs.get(&rope_full).unwrap());
        let token_width = (d * n_head) as usize;
        let full_last_token = full_values[full_values.len() - token_width..].to_vec();

        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };
        let mut step_ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let src_step = step_ctx
            .new_tensor_3d(TensorType::F32, d, n_head, 1, BufferUsage::Activations)
            .unwrap();
        let pos_step = step_ctx
            .new_tensor_1d(TensorType::I32, 1, BufferUsage::Activations)
            .unwrap();
        let rope_step = step_ctx
            .rope_multi(
                src_step,
                pos_step,
                None,
                32,
                [11, 11, 10, 0],
                crate::GGML_ROPE_TYPE_IMROPE,
                262_144,
                1_000_000.0,
                1.0,
                0.0,
                1.0,
                32.0,
                1.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let src_step_values = {
            let mut values = Vec::with_capacity(token_width);
            for head in 0..(n_head as usize) {
                let base_offset = token_width + head * (d as usize);
                values.extend_from_slice(&src_full_values[base_offset..base_offset + d as usize]);
            }
            values
        };
        step_ctx
            .write_tensor_data(src_step, &f32s_to_bytes(&src_step_values))
            .unwrap();
        step_ctx
            .write_tensor_data(pos_step, &i32s_to_bytes(&[1]))
            .unwrap();

        let mut step_graph = Graph::new();
        step_graph
            .build_forward_expand(&step_ctx, rope_step)
            .unwrap();

        let step_prepared = prepare_graph(&step_ctx, &step_graph, runtime.features()).unwrap();
        let step_session = MetalGraphSession::from_runtime(
            runtime,
            &step_ctx,
            &step_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let step_execution = step_session.execute(&step_ctx, &[], &[rope_step]).unwrap();
        let step_values = bytes_to_f32s(step_execution.outputs.get(&rope_step).unwrap());

        assert_eq!(step_values.len(), full_last_token.len());
        for (a, e) in step_values.iter().zip(full_last_token.iter()) {
            assert!(
                (a - e).abs() < 1.0e-4,
                "rope_multi kv-head token1 mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_rms_norm_mul_single_token_consistently_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut full_ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let d = 128_i64;
        let n_head = 8_i64;
        let n_tokens = 2_i64;
        let eps = 1.0e-6f32;

        let src_full = full_ctx
            .new_tensor_3d(
                TensorType::F32,
                d,
                n_head,
                n_tokens,
                BufferUsage::Activations,
            )
            .unwrap();
        let weight = full_ctx
            .new_tensor_2d(TensorType::F32, d, n_head, BufferUsage::Weights)
            .unwrap();
        let norm_full = full_ctx
            .rms_norm_eps(src_full, eps, BufferUsage::Activations)
            .unwrap();
        let scaled_full = full_ctx
            .binary_like_a(Op::Mul, norm_full, weight, BufferUsage::Activations)
            .unwrap();

        let src_full_values = patterned_f32s((d * n_head * n_tokens) as usize, -0.1, 0.003);
        let weight_values = patterned_f32s((d * n_head) as usize, 0.5, -0.001);
        full_ctx
            .write_tensor_data(src_full, &f32s_to_bytes(&src_full_values))
            .unwrap();
        full_ctx
            .write_tensor_data(weight, &f32s_to_bytes(&weight_values))
            .unwrap();

        let mut full_graph = Graph::new();
        full_graph
            .build_forward_expand(&full_ctx, scaled_full)
            .unwrap();

        let full_prepared = prepare_graph(&full_ctx, &full_graph, runtime.features()).unwrap();
        let full_session = MetalGraphSession::from_runtime(
            runtime,
            &full_ctx,
            &full_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let full_execution = full_session
            .execute(&full_ctx, &[], &[scaled_full])
            .unwrap();
        let full_values = bytes_to_f32s(full_execution.outputs.get(&scaled_full).unwrap());
        let token_width = (d * n_head) as usize;
        let full_last_token = full_values[full_values.len() - token_width..].to_vec();

        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };
        let mut step_ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let src_step = step_ctx
            .new_tensor_3d(TensorType::F32, d, n_head, 1, BufferUsage::Activations)
            .unwrap();
        let weight_step = step_ctx
            .new_tensor_2d(TensorType::F32, d, n_head, BufferUsage::Weights)
            .unwrap();
        let norm_step = step_ctx
            .rms_norm_eps(src_step, eps, BufferUsage::Activations)
            .unwrap();
        let scaled_step = step_ctx
            .binary_like_a(Op::Mul, norm_step, weight_step, BufferUsage::Activations)
            .unwrap();

        let src_step_values = {
            let mut values = Vec::with_capacity(token_width);
            for head in 0..(n_head as usize) {
                let base_offset = token_width + head * (d as usize);
                values.extend_from_slice(&src_full_values[base_offset..base_offset + d as usize]);
            }
            values
        };
        step_ctx
            .write_tensor_data(src_step, &f32s_to_bytes(&src_step_values))
            .unwrap();
        step_ctx
            .write_tensor_data(weight_step, &f32s_to_bytes(&weight_values))
            .unwrap();

        let mut step_graph = Graph::new();
        step_graph
            .build_forward_expand(&step_ctx, scaled_step)
            .unwrap();

        let step_prepared = prepare_graph(&step_ctx, &step_graph, runtime.features()).unwrap();
        let step_session = MetalGraphSession::from_runtime(
            runtime,
            &step_ctx,
            &step_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let step_execution = step_session
            .execute(&step_ctx, &[], &[scaled_step])
            .unwrap();
        let step_values = bytes_to_f32s(step_execution.outputs.get(&scaled_step).unwrap());

        assert_eq!(step_values.len(), full_last_token.len());
        for (a, e) in step_values.iter().zip(full_last_token.iter()) {
            assert!(
                (a - e).abs() < 1.0e-4,
                "rms_norm_mul token1 mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_flash_attn_vec_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let d = 32_i64;
        let n_q = 1_i64;
        let n_kv = 3_i64;

        let q = ctx
            .new_tensor_4d(TensorType::F32, d, n_q, 1, 1, BufferUsage::Activations)
            .unwrap();
        let k = ctx
            .new_tensor_4d(TensorType::F32, d, n_kv, 1, 1, BufferUsage::Activations)
            .unwrap();
        let v = ctx
            .new_tensor_4d(TensorType::F32, d, n_kv, 1, 1, BufferUsage::Activations)
            .unwrap();
        let attn = ctx
            .flash_attn_ext(
                q,
                k,
                v,
                None,
                1.0 / (d as f32).sqrt(),
                0.0,
                0.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let q_values = patterned_f32s(d as usize, 0.1, 0.01);
        let k_values = patterned_f32s((d * n_kv) as usize, -0.05, 0.02);
        let v_values = patterned_f32s((d * n_kv) as usize, 0.2, -0.015);
        ctx.write_tensor_data(q, &f32s_to_bytes(&q_values)).unwrap();
        ctx.write_tensor_data(k, &f32s_to_bytes(&k_values)).unwrap();
        ctx.write_tensor_data(v, &f32s_to_bytes(&v_values)).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, attn).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[attn]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&attn).unwrap());
        let expected = cpu_flash_attn_f32(
            &q_values,
            &k_values,
            &v_values,
            d as usize,
            n_q as usize,
            n_kv as usize,
        );

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 2.0e-3,
                "flash_attn output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_flash_attn_vec_from_strided_cache_view_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let d = 32_i64;
        let n_q = 1_i64;
        let n_kv = 2_i64;

        let q = ctx
            .new_tensor_4d(TensorType::F32, d, n_q, 1, 1, BufferUsage::Activations)
            .unwrap();
        let k_cache = ctx
            .new_tensor_3d(TensorType::F32, d, n_kv, 1, BufferUsage::State)
            .unwrap();
        let v_cache = ctx
            .new_tensor_3d(TensorType::F32, d, n_kv, 1, BufferUsage::State)
            .unwrap();
        let k_cur = ctx
            .new_tensor_2d(TensorType::F32, d, 1, BufferUsage::Activations)
            .unwrap();
        let v_cur = ctx
            .new_tensor_2d(TensorType::F32, d, 1, BufferUsage::Activations)
            .unwrap();
        let rows = ctx
            .new_tensor_1d(TensorType::I32, 1, BufferUsage::Activations)
            .unwrap();

        let k_written = ctx
            .set_rows(k_cache, k_cur, rows, BufferUsage::State)
            .unwrap();
        let v_written = ctx
            .set_rows(v_cache, v_cur, rows, BufferUsage::State)
            .unwrap();

        let k_view = ctx
            .view_4d(
                k_written,
                d,
                n_kv,
                1,
                1,
                ggml_row_size_for_type(TensorType::F32, d).unwrap(),
                ggml_row_size_for_type(TensorType::F32, d).unwrap(),
                ggml_row_size_for_type(TensorType::F32, d * n_kv).unwrap(),
                0,
            )
            .unwrap();
        let v_view = ctx
            .view_4d(
                v_written,
                d,
                n_kv,
                1,
                1,
                ggml_row_size_for_type(TensorType::F32, d).unwrap(),
                ggml_row_size_for_type(TensorType::F32, d).unwrap(),
                ggml_row_size_for_type(TensorType::F32, d * n_kv).unwrap(),
                0,
            )
            .unwrap();
        let attn = ctx
            .flash_attn_ext(
                q,
                k_view,
                v_view,
                None,
                1.0 / (d as f32).sqrt(),
                0.0,
                0.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let q_values = patterned_f32s(d as usize, 0.25, -0.01);
        let k0_values = patterned_f32s(d as usize, -0.15, 0.02);
        let k1_values = patterned_f32s(d as usize, 0.35, -0.015);
        let v0_values = patterned_f32s(d as usize, 0.05, 0.03);
        let v1_values = patterned_f32s(d as usize, -0.2, 0.01);

        let mut k_cache_values = vec![0.0f32; (d * n_kv) as usize];
        let mut v_cache_values = vec![0.0f32; (d * n_kv) as usize];
        k_cache_values[..d as usize].copy_from_slice(&k0_values);
        v_cache_values[..d as usize].copy_from_slice(&v0_values);

        ctx.write_tensor_data(q, &f32s_to_bytes(&q_values)).unwrap();
        ctx.write_tensor_data(k_cache, &f32s_to_bytes(&k_cache_values))
            .unwrap();
        ctx.write_tensor_data(v_cache, &f32s_to_bytes(&v_cache_values))
            .unwrap();
        ctx.write_tensor_data(k_cur, &f32s_to_bytes(&k1_values))
            .unwrap();
        ctx.write_tensor_data(v_cur, &f32s_to_bytes(&v1_values))
            .unwrap();
        ctx.write_tensor_data(rows, &i32s_to_bytes(&[1])).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, attn).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[attn]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&attn).unwrap());
        let mut expected_k = k0_values.clone();
        expected_k.extend_from_slice(&k1_values);
        let mut expected_v = v0_values.clone();
        expected_v.extend_from_slice(&v1_values);
        let expected = cpu_flash_attn_f32(
            &q_values,
            &expected_k,
            &expected_v,
            d as usize,
            1,
            n_kv as usize,
        );

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 2.0e-3,
                "flash_attn strided cache output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_flash_attn_vec_from_strided_gqa_cache_view_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut full_ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let d = 32_i64;
        let n_q = 2_i64;
        let n_head = 4_i64;
        let n_kv = 2_i64;
        let n_kv_head = 2_i64;

        let q_base = full_ctx
            .new_tensor_4d(TensorType::F32, d, n_head, n_q, 1, BufferUsage::Activations)
            .unwrap();
        let k_base = full_ctx
            .new_tensor_4d(
                TensorType::F32,
                d,
                n_kv_head,
                n_kv,
                1,
                BufferUsage::Activations,
            )
            .unwrap();
        let v_base = full_ctx
            .new_tensor_4d(
                TensorType::F32,
                d,
                n_kv_head,
                n_kv,
                1,
                BufferUsage::Activations,
            )
            .unwrap();
        let q_full = full_ctx.permute(q_base, [0, 2, 1, 3]).unwrap();
        let k_full = full_ctx.permute(k_base, [0, 2, 1, 3]).unwrap();
        let v_full = full_ctx.permute(v_base, [0, 2, 1, 3]).unwrap();
        let full_attn = full_ctx
            .flash_attn_ext(
                q_full,
                k_full,
                v_full,
                None,
                1.0 / (d as f32).sqrt(),
                0.0,
                0.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let q_base_values = patterned_f32s((d * n_q * n_head) as usize, -0.2, 0.004);
        let k_base_values = patterned_f32s((d * n_kv * n_kv_head) as usize, 0.15, -0.006);
        let v_base_values = patterned_f32s((d * n_kv * n_kv_head) as usize, -0.05, 0.008);

        full_ctx
            .write_tensor_data(q_base, &f32s_to_bytes(&q_base_values))
            .unwrap();
        full_ctx
            .write_tensor_data(k_base, &f32s_to_bytes(&k_base_values))
            .unwrap();
        full_ctx
            .write_tensor_data(v_base, &f32s_to_bytes(&v_base_values))
            .unwrap();

        let mut full_graph = Graph::new();
        full_graph
            .build_forward_expand(&full_ctx, full_attn)
            .unwrap();

        let full_prepared = prepare_graph(&full_ctx, &full_graph, runtime.features()).unwrap();
        let full_session = MetalGraphSession::from_runtime(
            runtime,
            &full_ctx,
            &full_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let full_execution = full_session.execute(&full_ctx, &[], &[full_attn]).unwrap();
        let full_values = bytes_to_f32s(full_execution.outputs.get(&full_attn).unwrap());
        let last_token_width = (d * n_head) as usize;
        let full_last_token = full_values[full_values.len() - last_token_width..].to_vec();

        let mut decode_ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let q_step = decode_ctx
            .new_tensor_4d(TensorType::F32, d, 1, n_head, 1, BufferUsage::Activations)
            .unwrap();
        let k_cache = decode_ctx
            .new_tensor_3d(TensorType::F32, d * n_kv_head, n_kv, 1, BufferUsage::State)
            .unwrap();
        let v_cache = decode_ctx
            .new_tensor_3d(TensorType::F32, d * n_kv_head, n_kv, 1, BufferUsage::State)
            .unwrap();
        let k_cur = decode_ctx
            .new_tensor_2d(TensorType::F32, d * n_kv_head, 1, BufferUsage::Activations)
            .unwrap();
        let v_cur = decode_ctx
            .new_tensor_2d(TensorType::F32, d * n_kv_head, 1, BufferUsage::Activations)
            .unwrap();
        let rows = decode_ctx
            .new_tensor_1d(TensorType::I32, 1, BufferUsage::Activations)
            .unwrap();

        let k_written = decode_ctx
            .set_rows(k_cache, k_cur, rows, BufferUsage::State)
            .unwrap();
        let v_written = decode_ctx
            .set_rows(v_cache, v_cur, rows, BufferUsage::State)
            .unwrap();
        let k_view = decode_ctx
            .view_4d(
                k_written,
                d,
                n_kv,
                n_kv_head,
                1,
                ggml_row_size_for_type(TensorType::F32, d * n_kv_head).unwrap(),
                ggml_row_size_for_type(TensorType::F32, d).unwrap(),
                ggml_row_size_for_type(TensorType::F32, d * n_kv_head * n_kv).unwrap(),
                0,
            )
            .unwrap();
        let v_view = decode_ctx
            .view_4d(
                v_written,
                d,
                n_kv,
                n_kv_head,
                1,
                ggml_row_size_for_type(TensorType::F32, d * n_kv_head).unwrap(),
                ggml_row_size_for_type(TensorType::F32, d).unwrap(),
                ggml_row_size_for_type(TensorType::F32, d * n_kv_head * n_kv).unwrap(),
                0,
            )
            .unwrap();
        let decode_attn = decode_ctx
            .flash_attn_ext(
                q_step,
                k_view,
                v_view,
                None,
                1.0 / (d as f32).sqrt(),
                0.0,
                0.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let q_step_values = {
            let mut values = Vec::with_capacity(last_token_width);
            for head in 0..(n_head as usize) {
                let base_offset = (d as usize) * (n_head as usize) + head * (d as usize);
                values.extend_from_slice(&q_base_values[base_offset..base_offset + d as usize]);
            }
            values
        };
        let token0_k = {
            let mut values = Vec::with_capacity((d * n_kv_head) as usize);
            for head in 0..(n_kv_head as usize) {
                let base_offset = head * (d as usize);
                values.extend_from_slice(&k_base_values[base_offset..base_offset + d as usize]);
            }
            values
        };
        let token1_k = {
            let mut values = Vec::with_capacity((d * n_kv_head) as usize);
            for head in 0..(n_kv_head as usize) {
                let base_offset = (d as usize) * (n_kv_head as usize) + head * (d as usize);
                values.extend_from_slice(&k_base_values[base_offset..base_offset + d as usize]);
            }
            values
        };
        let token0_v = {
            let mut values = Vec::with_capacity((d * n_kv_head) as usize);
            for head in 0..(n_kv_head as usize) {
                let base_offset = head * (d as usize);
                values.extend_from_slice(&v_base_values[base_offset..base_offset + d as usize]);
            }
            values
        };
        let token1_v = {
            let mut values = Vec::with_capacity((d * n_kv_head) as usize);
            for head in 0..(n_kv_head as usize) {
                let base_offset = (d as usize) * (n_kv_head as usize) + head * (d as usize);
                values.extend_from_slice(&v_base_values[base_offset..base_offset + d as usize]);
            }
            values
        };
        let mut k_cache_values = vec![0.0f32; (d * n_kv_head * n_kv) as usize];
        let mut v_cache_values = vec![0.0f32; (d * n_kv_head * n_kv) as usize];
        k_cache_values[..token0_k.len()].copy_from_slice(&token0_k);
        v_cache_values[..token0_v.len()].copy_from_slice(&token0_v);

        decode_ctx
            .write_tensor_data(q_step, &f32s_to_bytes(&q_step_values))
            .unwrap();
        decode_ctx
            .write_tensor_data(k_cache, &f32s_to_bytes(&k_cache_values))
            .unwrap();
        decode_ctx
            .write_tensor_data(v_cache, &f32s_to_bytes(&v_cache_values))
            .unwrap();
        decode_ctx
            .write_tensor_data(k_cur, &f32s_to_bytes(&token1_k))
            .unwrap();
        decode_ctx
            .write_tensor_data(v_cur, &f32s_to_bytes(&token1_v))
            .unwrap();
        decode_ctx
            .write_tensor_data(rows, &i32s_to_bytes(&[1]))
            .unwrap();

        let mut decode_graph = Graph::new();
        decode_graph
            .build_forward_expand(&decode_ctx, decode_attn)
            .unwrap();

        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };
        let decode_prepared =
            prepare_graph(&decode_ctx, &decode_graph, runtime.features()).unwrap();
        let decode_session = MetalGraphSession::from_runtime(
            runtime,
            &decode_ctx,
            &decode_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let decode_execution = decode_session
            .execute(&decode_ctx, &[], &[decode_attn])
            .unwrap();
        let decode_values = bytes_to_f32s(decode_execution.outputs.get(&decode_attn).unwrap());

        assert_eq!(decode_values.len(), full_last_token.len());
        for (a, e) in decode_values.iter().zip(full_last_token.iter()) {
            assert!(
                (a - e).abs() < 2.0e-3,
                "flash_attn GQA cache output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_set_rows_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let dst = ctx
            .new_tensor_2d(TensorType::F32, 4, 4, BufferUsage::State)
            .unwrap();
        let src = ctx
            .new_tensor_2d(TensorType::F32, 4, 2, BufferUsage::Activations)
            .unwrap();
        let rows = ctx
            .new_tensor_1d(TensorType::I32, 2, BufferUsage::Activations)
            .unwrap();
        let out = ctx.set_rows(dst, src, rows, BufferUsage::State).unwrap();

        ctx.write_tensor_data(dst, &f32s_to_bytes(&[0.0f32; 16]))
            .unwrap();
        ctx.write_tensor_data(
            src,
            &f32s_to_bytes(&[
                1.0, 2.0, 3.0, 4.0, //
                5.0, 6.0, 7.0, 8.0,
            ]),
        )
        .unwrap();
        ctx.write_tensor_data(rows, &i32s_to_bytes(&[1, 3]))
            .unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, out).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[out]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&out).unwrap());
        assert_eq!(
            actual,
            vec![
                0.0, 0.0, 0.0, 0.0, //
                1.0, 2.0, 3.0, 4.0, //
                0.0, 0.0, 0.0, 0.0, //
                5.0, 6.0, 7.0, 8.0,
            ]
        );
    }

    #[test]
    fn executes_add_sigmoid_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let a = ctx
            .new_tensor_2d(TensorType::F32, 8, 2, BufferUsage::Activations)
            .unwrap();
        let b = ctx
            .new_tensor_2d(TensorType::F32, 8, 2, BufferUsage::Activations)
            .unwrap();
        let add = ctx
            .binary_like_a(Op::Add, a, b, BufferUsage::Activations)
            .unwrap();
        let out = ctx
            .unary(add, crate::op::UnaryOp::Sigmoid, BufferUsage::Activations)
            .unwrap();

        let a_values = patterned_f32s(16, -0.3, 0.07);
        let b_values = patterned_f32s(16, 0.15, -0.05);
        ctx.write_tensor_data(a, &f32s_to_bytes(&a_values)).unwrap();
        ctx.write_tensor_data(b, &f32s_to_bytes(&b_values)).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, out).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[out]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&out).unwrap());
        let expected = a_values
            .iter()
            .zip(b_values.iter())
            .map(|(a, b)| 1.0f32 / (1.0 + (-(a + b)).exp()))
            .collect::<Vec<_>>();

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 1.0e-5,
                "add+sigmoid output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_softmax_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_2d(TensorType::F32, 6, 2, BufferUsage::Activations)
            .unwrap();
        let out = ctx.soft_max(src, BufferUsage::Activations).unwrap();
        let values = vec![
            0.2, -0.1, 0.8, -0.7, 0.3, 0.5, //
            -0.4, 1.2, 0.6, -0.2, 0.9, -1.0,
        ];
        ctx.write_tensor_data(src, &f32s_to_bytes(&values)).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, out).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[out]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&out).unwrap());
        let expected = cpu_softmax_rows_f32(&values, 6);

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 1.0e-5,
                "softmax output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_sum_rows_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_2d(TensorType::F32, 6, 3, BufferUsage::Activations)
            .unwrap();
        let out = ctx.sum_rows(src, BufferUsage::Activations).unwrap();
        let values = vec![
            0.2, -0.1, 0.8, -0.7, 0.3, 0.5, //
            -0.4, 1.2, 0.6, -0.2, 0.9, -1.0, //
            1.0, 0.5, -0.3, 0.4, -0.6, 0.7,
        ];
        ctx.write_tensor_data(src, &f32s_to_bytes(&values)).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, out).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[out]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&out).unwrap());
        let expected = cpu_sum_rows_f32(&values, 6);

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 1.0e-5,
                "sum_rows output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_top_k_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_2d(TensorType::F32, 8, 2, BufferUsage::Activations)
            .unwrap();
        let out = ctx.top_k(src, 3, BufferUsage::Activations).unwrap();
        let values = vec![
            0.1, 0.6, -0.4, 1.5, 0.9, 0.2, -0.7, 0.8, //
            1.1, -0.5, 0.4, 0.7, 0.3, 1.6, -0.2, 0.0,
        ];
        ctx.write_tensor_data(src, &f32s_to_bytes(&values)).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, out).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[out]).unwrap();
        let actual = bytes_to_i32s(execution.outputs.get(&out).unwrap());
        let expected = cpu_top_k_rows_i32(&values, 8, 3);
        assert_eq!(actual, expected);
    }

    #[test]
    fn executes_argsort_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_2d(TensorType::F32, 8, 2, BufferUsage::Activations)
            .unwrap();
        let out = ctx.argsort(src, BufferUsage::Activations).unwrap();
        let values = vec![
            0.1, 0.6, -0.4, 1.5, 0.9, 0.2, -0.7, 0.8, //
            1.1, -0.5, 0.4, 0.7, 0.3, 1.6, -0.2, 0.0,
        ];
        ctx.write_tensor_data(src, &f32s_to_bytes(&values)).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, out).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[out]).unwrap();
        let actual = bytes_to_i32s(execution.outputs.get(&out).unwrap());
        let expected = cpu_argsort_rows_i32(&values, 8);
        assert_eq!(actual, expected);
    }

    #[test]
    fn executes_add_id_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let src0 = ctx
            .new_tensor_3d(TensorType::F32, 4, 2, 2, BufferUsage::Activations)
            .unwrap();
        let bias = ctx
            .new_tensor_2d(TensorType::F32, 4, 4, BufferUsage::Weights)
            .unwrap();
        let ids = ctx
            .new_tensor_2d(TensorType::I32, 2, 2, BufferUsage::Activations)
            .unwrap();
        let out = ctx
            .add_id(src0, bias, ids, BufferUsage::Activations)
            .unwrap();

        let src0_values = vec![
            0.5, 0.7, 0.9, 1.1, //
            1.5, 1.7, 1.9, 2.1, //
            -0.5, -0.7, -0.9, -1.1, //
            -1.5, -1.7, -1.9, -2.1,
        ];
        let bias_values = vec![
            0.1, 0.2, 0.3, 0.4, //
            1.0, 1.1, 1.2, 1.3, //
            2.0, 2.1, 2.2, 2.3, //
            3.0, 3.1, 3.2, 3.3,
        ];
        let ids_values = vec![2, 1, 0, 3];
        ctx.write_tensor_data(src0, &f32s_to_bytes(&src0_values))
            .unwrap();
        ctx.write_tensor_data(bias, &f32s_to_bytes(&bias_values))
            .unwrap();
        ctx.write_tensor_data(ids, &i32s_to_bytes(&ids_values))
            .unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, out).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[out]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&out).unwrap());
        let expected = cpu_add_id_f32(&src0_values, &bias_values, &ids_values, 4, 2, 2, 4);

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 1.0e-5,
                "add_id output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_mul_mat_id_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 22,
            mem_buffer: None,
            no_alloc: false,
        });

        let experts = ctx
            .new_tensor_3d(TensorType::F32, 4, 3, 4, BufferUsage::Weights)
            .unwrap();
        let input = ctx
            .new_tensor_3d(TensorType::F32, 4, 1, 2, BufferUsage::Activations)
            .unwrap();
        let ids = ctx
            .new_tensor_2d(TensorType::I32, 2, 2, BufferUsage::Activations)
            .unwrap();
        let out = ctx
            .mul_mat_id(experts, input, ids, BufferUsage::Activations)
            .unwrap();

        let expert_values = patterned_f32s(4 * 3 * 4, -0.3, 0.05);
        let input_values = vec![
            0.2, -0.1, 0.4, 0.8, //
            -0.3, 0.5, -0.6, 0.7,
        ];
        let ids_values = vec![1, 3, 0, 2];
        ctx.write_tensor_data(experts, &f32s_to_bytes(&expert_values))
            .unwrap();
        ctx.write_tensor_data(input, &f32s_to_bytes(&input_values))
            .unwrap();
        ctx.write_tensor_data(ids, &i32s_to_bytes(&ids_values))
            .unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, out).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[out]).unwrap();
        let actual = bytes_to_f32s(execution.outputs.get(&out).unwrap());
        let expected =
            cpu_mul_mat_id_f32(&expert_values, &input_values, &ids_values, 4, 3, 4, 2, 2);

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 1.0e-5,
                "mul_mat_id output mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn executes_gated_delta_net_graph_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 22,
            mem_buffer: None,
            no_alloc: false,
        });

        let s_v = 32_i64;
        let h_k = 1_i64;
        let h_v = 2_i64;
        let n_tokens = 1_i64;
        let n_seqs = 1_i64;

        let q = ctx
            .new_tensor_4d(
                TensorType::F32,
                s_v,
                h_k,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let k = ctx
            .new_tensor_4d(
                TensorType::F32,
                s_v,
                h_k,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let v = ctx
            .new_tensor_4d(
                TensorType::F32,
                s_v,
                h_v,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let g = ctx
            .new_tensor_4d(
                TensorType::F32,
                1,
                h_v,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let beta = ctx
            .new_tensor_4d(
                TensorType::F32,
                1,
                h_v,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let state = ctx
            .new_tensor_4d(
                TensorType::F32,
                s_v,
                s_v,
                h_v,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let result = ctx
            .gated_delta_net(q, k, v, g, beta, state, BufferUsage::Activations)
            .unwrap();

        let output = ctx
            .view_4d(
                result,
                s_v,
                h_v,
                n_tokens,
                n_seqs,
                ggml_row_size_for_type(TensorType::F32, s_v).unwrap(),
                ggml_row_size_for_type(TensorType::F32, s_v * h_v).unwrap(),
                ggml_row_size_for_type(TensorType::F32, s_v * h_v * n_tokens).unwrap(),
                0,
            )
            .unwrap();
        let new_state = ctx
            .view_4d(
                result,
                s_v,
                s_v,
                h_v,
                n_seqs,
                ggml_row_size_for_type(TensorType::F32, s_v).unwrap(),
                ggml_row_size_for_type(TensorType::F32, s_v * s_v).unwrap(),
                ggml_row_size_for_type(TensorType::F32, s_v * s_v * h_v).unwrap(),
                ggml_row_size_for_type(TensorType::F32, s_v * h_v * n_tokens * n_seqs).unwrap(),
            )
            .unwrap();

        let q_values = patterned_f32s((s_v * h_k * n_tokens * n_seqs) as usize, -0.2, 0.01);
        let k_values = patterned_f32s((s_v * h_k * n_tokens * n_seqs) as usize, 0.15, -0.008);
        let v_values = patterned_f32s((s_v * h_v * n_tokens * n_seqs) as usize, 0.05, 0.006);
        let g_values = vec![-0.3, 0.2];
        let beta_values = vec![0.7, 0.4];
        let state_values = patterned_f32s((s_v * s_v * h_v * n_seqs) as usize, -0.05, 0.0005);
        ctx.write_tensor_data(q, &f32s_to_bytes(&q_values)).unwrap();
        ctx.write_tensor_data(k, &f32s_to_bytes(&k_values)).unwrap();
        ctx.write_tensor_data(v, &f32s_to_bytes(&v_values)).unwrap();
        ctx.write_tensor_data(g, &f32s_to_bytes(&g_values)).unwrap();
        ctx.write_tensor_data(beta, &f32s_to_bytes(&beta_values))
            .unwrap();
        ctx.write_tensor_data(state, &f32s_to_bytes(&state_values))
            .unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, output).unwrap();
        graph.build_forward_expand(&ctx, new_state).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();

        let execution = session.execute(&ctx, &[], &[output, new_state]).unwrap();
        let actual_output = bytes_to_f32s(execution.outputs.get(&output).unwrap());
        let actual_state = bytes_to_f32s(execution.outputs.get(&new_state).unwrap());
        let (expected_output, expected_state) = cpu_gated_delta_net_f32(
            &q_values,
            &k_values,
            &v_values,
            &g_values,
            &beta_values,
            &state_values,
            s_v as usize,
            h_k as usize,
            h_v as usize,
            n_tokens as usize,
            n_seqs as usize,
        );

        assert_eq!(actual_output.len(), expected_output.len());
        for (a, e) in actual_output.iter().zip(expected_output.iter()) {
            assert!(
                (a - e).abs() < 1.0e-4,
                "gated_delta_net output mismatch: actual={} expected={}",
                a,
                e
            );
        }

        assert_eq!(actual_state.len(), expected_state.len());
        for (a, e) in actual_state.iter().zip(expected_state.iter()) {
            assert!(
                (a - e).abs() < 1.0e-4,
                "gated_delta_net state mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    fn cpu_rope_norm_f32(
        src: &[f32],
        positions: &[i32],
        n_dims: usize,
        n_heads: usize,
        n_tokens: usize,
    ) -> Vec<f32> {
        let ne0 = n_dims;
        let mut dst = src.to_vec();
        for token in 0..n_tokens {
            let theta_base = positions[token] as f32;
            for head in 0..n_heads {
                let base = (token * n_heads + head) * ne0;
                let mut i0 = 0usize;
                while i0 < ne0 {
                    let theta = theta_base * 10_000f32.powf(-(i0 as f32) / (n_dims as f32));
                    let cos_theta = theta.cos();
                    let sin_theta = theta.sin();
                    let x0 = src[base + i0];
                    let x1 = src[base + i0 + 1];
                    dst[base + i0] = x0 * cos_theta - x1 * sin_theta;
                    dst[base + i0 + 1] = x0 * sin_theta + x1 * cos_theta;
                    i0 += 2;
                }
            }
        }
        dst
    }

    fn f32s_to_bytes(values: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    fn i32s_to_bytes(values: &[i32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(values.len() * std::mem::size_of::<i32>());
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    fn bytes_to_i32s(bytes: &[u8]) -> Vec<i32> {
        bytes
            .chunks_exact(std::mem::size_of::<i32>())
            .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()
    }

    fn bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(std::mem::size_of::<f32>())
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()
    }

    fn patterned_f32s(len: usize, base: f32, step: f32) -> Vec<f32> {
        (0..len).map(|i| base + step * (i as f32)).collect()
    }

    fn cpu_softmax_rows_f32(values: &[f32], row_len: usize) -> Vec<f32> {
        let mut out = vec![0.0; values.len()];
        for row in 0..(values.len() / row_len) {
            let slice = &values[row * row_len..(row + 1) * row_len];
            let max = slice
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, |acc, v| acc.max(v));
            let mut sum = 0.0f32;
            for (i, value) in slice.iter().enumerate() {
                let exp = (*value - max).exp();
                out[row * row_len + i] = exp;
                sum += exp;
            }
            for i in 0..row_len {
                out[row * row_len + i] /= sum;
            }
        }
        out
    }

    fn cpu_sum_rows_f32(values: &[f32], row_len: usize) -> Vec<f32> {
        let mut out = vec![0.0; values.len() / row_len];
        for row in 0..out.len() {
            out[row] = values[row * row_len..(row + 1) * row_len]
                .iter()
                .copied()
                .sum();
        }
        out
    }

    fn cpu_top_k_rows_i32(values: &[f32], row_len: usize, k: usize) -> Vec<i32> {
        let mut out = Vec::with_capacity((values.len() / row_len) * k);
        for row in 0..(values.len() / row_len) {
            let mut indices = (0..row_len).collect::<Vec<_>>();
            indices
                .sort_by(|&a, &b| values[row * row_len + b].total_cmp(&values[row * row_len + a]));
            out.extend(indices.into_iter().take(k).map(|idx| idx as i32));
        }
        out
    }

    fn cpu_argsort_rows_i32(values: &[f32], row_len: usize) -> Vec<i32> {
        let mut out = Vec::with_capacity(values.len());
        for row in 0..(values.len() / row_len) {
            let mut indices = (0..row_len).collect::<Vec<_>>();
            indices
                .sort_by(|&a, &b| values[row * row_len + a].total_cmp(&values[row * row_len + b]));
            out.extend(indices.into_iter().map(|idx| idx as i32));
        }
        out
    }

    fn cpu_add_id_f32(
        src0: &[f32],
        bias: &[f32],
        ids: &[i32],
        width: usize,
        used_experts: usize,
        tokens: usize,
        total_experts: usize,
    ) -> Vec<f32> {
        let mut out = src0.to_vec();
        for token in 0..tokens {
            for slot in 0..used_experts {
                let expert = ids[token * used_experts + slot] as usize;
                for i in 0..width {
                    let dst_idx = token * used_experts * width + slot * width + i;
                    out[dst_idx] += bias[expert * width + i];
                }
            }
        }
        let _ = total_experts;
        out
    }

    fn cpu_mul_mat_id_f32(
        experts: &[f32],
        input: &[f32],
        ids: &[i32],
        in_dim: usize,
        out_dim: usize,
        expert_count: usize,
        used_experts: usize,
        tokens: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; out_dim * used_experts * tokens];
        for token in 0..tokens {
            for slot in 0..used_experts {
                let expert = ids[token * used_experts + slot] as usize;
                for o in 0..out_dim {
                    let mut acc = 0.0f32;
                    for i in 0..in_dim {
                        let w_idx = expert * (in_dim * out_dim) + o * in_dim + i;
                        let x_idx = token * in_dim + i;
                        acc += experts[w_idx] * input[x_idx];
                    }
                    out[token * used_experts * out_dim + slot * out_dim + o] = acc;
                }
            }
        }
        let _ = expert_count;
        out
    }

    fn cpu_gated_delta_net_f32(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        g: &[f32],
        beta: &[f32],
        state: &[f32],
        s_v: usize,
        h_k: usize,
        h_v: usize,
        n_tokens: usize,
        n_seqs: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        assert_eq!(q.len(), s_v * h_k * n_tokens * n_seqs);
        assert_eq!(k.len(), s_v * h_k * n_tokens * n_seqs);
        assert_eq!(v.len(), s_v * h_v * n_tokens * n_seqs);
        assert_eq!(beta.len(), h_v * n_tokens * n_seqs);
        assert_eq!(state.len(), s_v * s_v * h_v * n_seqs);
        assert!(
            g.len() == h_v * n_tokens * n_seqs || g.len() == s_v * h_v * n_tokens * n_seqs,
            "gate tensor must be scalar or per-channel"
        );

        let kda = g.len() == s_v * h_v * n_tokens * n_seqs;
        let scale = 1.0f32 / (s_v as f32).sqrt();
        let mut attn_out = vec![0.0f32; s_v * h_v * n_tokens * n_seqs];
        let mut state_out = state.to_vec();
        let mut delta = vec![0.0f32; s_v];

        for seq in 0..n_seqs {
            for head in 0..h_v {
                let q_head = head % h_k;
                let k_head = head % h_k;
                let state_base = (seq * h_v + head) * s_v * s_v;

                for token in 0..n_tokens {
                    let q_base = ((seq * n_tokens + token) * h_k + q_head) * s_v;
                    let k_base = ((seq * n_tokens + token) * h_k + k_head) * s_v;
                    let v_base = ((seq * n_tokens + token) * h_v + head) * s_v;
                    let beta_idx = (seq * n_tokens + token) * h_v + head;
                    let beta_val = beta[beta_idx];

                    if kda {
                        let g_base = ((seq * n_tokens + token) * h_v + head) * s_v;
                        for row in 0..s_v {
                            let row_base = state_base + row * s_v;
                            for col in 0..s_v {
                                state_out[row_base + col] *= g[g_base + col].exp();
                            }
                        }
                    } else {
                        let g_exp = g[beta_idx].exp();
                        for idx in 0..(s_v * s_v) {
                            state_out[state_base + idx] *= g_exp;
                        }
                    }

                    for row in 0..s_v {
                        let row_base = state_base + row * s_v;
                        let mut sum = 0.0f32;
                        for col in 0..s_v {
                            sum += state_out[row_base + col] * k[k_base + col];
                        }
                        delta[row] = (v[v_base + row] - sum) * beta_val;
                    }

                    for row in 0..s_v {
                        let row_base = state_base + row * s_v;
                        for col in 0..s_v {
                            state_out[row_base + col] += k[k_base + col] * delta[row];
                        }
                    }

                    let out_base = ((seq * n_tokens + token) * h_v + head) * s_v;
                    for row in 0..s_v {
                        let row_base = state_base + row * s_v;
                        let mut sum = 0.0f32;
                        for col in 0..s_v {
                            sum += state_out[row_base + col] * q[q_base + col];
                        }
                        attn_out[out_base + row] = sum * scale;
                    }
                }
            }
        }

        (attn_out, state_out)
    }

    fn cpu_flash_attn_f32(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        d: usize,
        n_q: usize,
        n_kv: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; d * n_q];
        for iq in 0..n_q {
            let q_row = &q[iq * d..(iq + 1) * d];
            let mut scores = vec![0.0f32; n_kv];
            for ik in 0..n_kv {
                let k_row = &k[ik * d..(ik + 1) * d];
                let mut dot = 0.0f32;
                for i in 0..d {
                    dot += q_row[i] * k_row[i];
                }
                scores[ik] = dot / (d as f32).sqrt();
            }
            let max_score = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let mut sum = 0.0f32;
            for score in &mut scores {
                *score = (*score - max_score).exp();
                sum += *score;
            }
            for score in &mut scores {
                *score /= sum.max(f32::MIN_POSITIVE);
            }
            for ik in 0..n_kv {
                let v_row = &v[ik * d..(ik + 1) * d];
                let w = scores[ik];
                for i in 0..d {
                    out[iq * d + i] += w * v_row[i];
                }
            }
        }
        out
    }
}
