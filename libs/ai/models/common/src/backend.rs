use crate::{DiffusionError, Result};
use makepad_ai_llm::metal_compiled::{self as backend_impl, MetalGraphSession, MetalPreparedGraph};
use makepad_ai_llm::{Context, Graph};

pub use crate::accel::*;
pub use crate::gpu as cuda;
pub use makepad_ai_cuda::llm_ops;
pub use makepad_ai_cuda::prof;
pub use makepad_ai_metal::{BackendCapabilities, BackendInfo, BackendKind};

pub mod metal {
    pub use makepad_ai_llm::metal_compiled::*;
    pub use makepad_ai_llm::metal_qmm::{affine_qmm_enabled, bench_steel_isolated, SteelBenchResult};
    pub use makepad_ai_metal::*;
}

pub use metal::{
    try_add_f32, try_attention_softmax_weighted_sum_f32, try_conv2d_planar_f32,
    try_flash_attn_f32_packed, try_gelu_f32, try_group_norm_planar_f32,
    try_layer_norm_mul_add_f32, try_matmul_nn_f32, try_matmul_nt_f32, try_mul_f32,
    try_rms_norm_mul_f32, try_silu_f32, BufferStorageMode,
    MetalGraphTensorWrite as GraphTensorWrite, MetalRuntime as Runtime,
};

/// Device-resident tensor API (CUDA today; Metal-backed stub elsewhere).
/// Activations stay on the GPU across a whole transformer step — see the
/// flux device path in flux_transformer.rs.
pub use crate::gpu::{
    gpu_act_f16_enabled, gpu_add, gpu_add_bf16, gpu_alias_snake_updown2x,
    gpu_attention_cross_fused_enabled,
    gpu_attention_gqa_decode_bf16, gpu_attention_gqa_decode_pair_bf16, gpu_attention_packed,
    gpu_attention_packed_bf16,
    gpu_attention_packed_causal, gpu_attention_packed_causal_bf16,
    gpu_attention_packed_causal_f16, gpu_attention_packed_causal_f32,
    gpu_attention_packed_causal_flash,
    gpu_attention_packed_flash_cross, gpu_attention_packed_flash_cross_bf16_rn,
    gpu_attention_packed_flash_cross_bf16pre_f16,
    gpu_attention_packed_composite_bf16, gpu_attention_packed_composite_f32,
    gpu_attention_packed_motion_text,
    gpu_attention_packed_cross, gpu_attention_packed_cross_bias, gpu_rpb_expand, gpu_sam3_sine_embed, gpu_sam3_rpb_axial, gpu_sam3_refine_boxes, gpu_attention_packed_cross_bf16,
    gpu_attention_packed_cross_composite_bf16, gpu_gather_rows_colblock,
    gpu_attention_packed_flash_bf16, gpu_attention_packed_flash2_d64,
    gpu_gelu_erf, gpu_rms_norm_mul_perhead,
    gpu_attention_planar_single,
    gpu_birefnet_broadcast, gpu_birefnet_deform_conv2d_cached,
    gpu_birefnet_global_avg_pool, gpu_birefnet_image_to_patches,
    gpu_birefnet_mul_sigmoid_mask, gpu_birefnet_relu, gpu_birefnet_resize_bilinear,
    gpu_birefnet_swin_attention, gpu_birefnet_tokens_to_planar,
    gpu_beam_cache_reorder_append, gpu_bf16_round, gpu_concat_cols, gpu_concat_rows,
    gpu_concat_rows_many,
    gpu_conv2d_planar_cached, gpu_conv2d_planar_strided, gpu_copy_into, gpu_device_available, gpu_download, gpu_gather_cols,
    gpu_gated_residual,
    gpu_gated_residual_mod, gpu_gated_residual_mod_round_bf16, gpu_gelu, gpu_gelu_bias_f16,
    gpu_graph_capture, gpu_graph_launch,
    gpu_group_norm_planar,
    gpu_layer_norm_mod, gpu_layer_norm_mod_f16,
    gpu_gated_residual_indexed, gpu_gemm_f16acc_enabled,
    gpu_layer_norm_mul_add, gpu_layer_norm_mul_add_grouped, gpu_layer_norm_pytorch,
    gpu_linear_f32_resident, gpu_mul,
    gpu_layer_norm_mod_to_bf16buf, gpu_bf16buf_slab_to_f32, gpu_rms_norm_mul_from_bf16_slab,
    gpu_swiglu_gate_first_from_bf16, gpu_concat_f32rn_bf16buf,
    gpu_linear_nt_cached, gpu_linear_nt_cached_bf16_bias_epilogue,
    gpu_linear_nt_cached_bf16_f32acc, gpu_linear_nt_cached_bf16_mm,
    gpu_linear_nt_cached_bf16_mm_from_buf, gpu_linear_nt_cached_bf16_mm_from_buf_to_buf,
    gpu_linear_nt_cached_f8_mm, gpu_linear_nt_cached_f8_mm_from_buf,
    gpu_linear_nt_cached_f8_mm_from_buf_to_buf,
    gpu_stream_ring_active, gpu_stream_ring_advance, gpu_stream_ring_prime,
    gpu_stream_ring_release_slots, gpu_stream_ring_setup,
    gpu_linear_nt_cached_f16_f32acc,
    gpu_linear_nt_cached_f16,
    gpu_perf_stats, gpu_pool_cap_override, gpu_pool_clear,
    gpu_rms_norm_mod_indexed, gpu_rms_norm_mul, gpu_rms_norm_mul_bf16, gpu_rms_norm_qwen3,
    gpu_sparse_conv27,
    gpu_skintokens_michelangelo_fourier,
    gpu_pixel_shuffle_planar, gpu_pixel_shuffle_planar_cached, gpu_reshape,
    gpu_realesrgan_alloc_f16, gpu_realesrgan_alloc_f32,
    gpu_realesrgan_bias_lrelu_f16,
    gpu_realesrgan_bias_lrelu_f32, gpu_realesrgan_conv3x3_f16, gpu_realesrgan_conv3x3_f32,
    gpu_realesrgan_lrelu, gpu_realesrgan_quantize_rgb8_f32,
    gpu_realesrgan_scale_add, gpu_realesrgan_spine_axpb,
    gpu_rife_conv_transpose2d, gpu_rife_fill, gpu_rife_merge_rgb8, gpu_rife_res_conv,
    gpu_rife_scale, gpu_rife_warp,
    gpu_rope_half, gpu_rope_half_bf16, gpu_rope_interleaved, gpu_silu, gpu_slice_cols, gpu_slice_rows,
    gpu_splat_repo3d_tables, gpu_splat_rope_pairs_per_head,
    gpu_swiglu_gate_first, gpu_swiglu_value_gate, gpu_to_f16, gpu_upload, gpu_wavenet_gate,
    gpu_quant_linear_type_supported,
    gpu_runtime_trim, gpu_upload_into, gpu_upload_u32, gpu_weight_cache_ensure,
    gpu_weight_cache_ensure_quant,
    gpu_weight_cache_evict_prefix, gpu_weight_cache_evict_prefix_if_loaded,
    gpu_weight_cache_protect_prefixes,
    gpu_upsample_nearest2x, GpuBf16Buf, GpuLinearPart, GpuPerfStats, GpuStepGraph, GpuTensor,
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

/// Release model-owned dense CUDA weight namespaces plus every reusable
/// activation/scratch buffer on the current thread. Teardown is deliberately
/// conditional: a cold/CPU backend never initializes CUDA just to unload.
/// All prefixes are attempted before an error is returned so one bad release
/// cannot strand the remaining model namespaces.
pub fn release_gpu_runtime_namespaces(prefixes: &[&str]) -> Result<usize> {
    let mut released = 0usize;
    let mut errors = Vec::new();
    for prefix in prefixes {
        match gpu_weight_cache_evict_prefix_if_loaded(prefix) {
            Ok(count) => released += count,
            Err(error) => errors.push(format!("evict {prefix:?}: {error}")),
        }
    }
    if let Err(error) = gpu_runtime_trim() {
        errors.push(format!("trim CUDA scratch: {error}"));
    }
    if errors.is_empty() {
        Ok(released)
    } else {
        Err(DiffusionError::model(errors.join("; ")))
    }
}
