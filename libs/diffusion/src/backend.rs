use crate::{DiffusionError, Result};
use makepad_ggml::backend::metal::{self as backend_impl, MetalGraphSession, MetalPreparedGraph};
use makepad_ggml::{Context, Graph};

pub use makepad_ggml::backend::metal::{
    try_add_f32, try_attention_softmax_weighted_sum_f32, try_flash_attn_f32_packed,
    try_gelu_f32, try_layer_norm_mul_add_f32, try_matmul_nn_f32, try_matmul_nt_f32, try_mul_f32,
    try_rms_norm_mul_f32, BufferStorageMode, MetalGraphTensorWrite as GraphTensorWrite,
    MetalRuntime as Runtime,
};

pub type GraphSession = MetalGraphSession;
pub type PreparedGraph = MetalPreparedGraph;

/// Import backend primitives through this module so diffusion stays agnostic
/// about whether ggml is driving the compiled graph through Metal, CUDA, or a
/// future backend-specific implementation detail.
pub fn new_runtime() -> Result<Runtime> {
    Runtime::new().map_err(DiffusionError::model)
}

pub fn runtime_available() -> bool {
    Runtime::is_available()
}

pub fn prepare_graph(runtime: &Runtime, ctx: &Context, graph: &Graph) -> Result<PreparedGraph> {
    backend_impl::prepare_graph(ctx, graph, runtime.features()).map_err(DiffusionError::model)
}

pub fn create_graph_session(
    runtime: &Runtime,
    ctx: &Context,
    prepared: &PreparedGraph,
    input_storage: BufferStorageMode,
    output_storage: BufferStorageMode,
) -> Result<GraphSession> {
    GraphSession::from_runtime(
        runtime.clone(),
        ctx,
        prepared,
        input_storage,
        output_storage,
    )
    .map_err(DiffusionError::model)
}

pub fn compile_graph_session(
    runtime: &Runtime,
    ctx: &Context,
    graph: &Graph,
    input_storage: BufferStorageMode,
    output_storage: BufferStorageMode,
) -> Result<GraphSession> {
    let prepared = prepare_graph(runtime, ctx, graph)?;
    create_graph_session(runtime, ctx, &prepared, input_storage, output_storage)
}
