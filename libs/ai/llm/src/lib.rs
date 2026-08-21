mod error;
pub mod context;
pub mod core;
pub mod graph;
pub mod mmap;
pub mod metal_compiled;
pub mod metal_qmm;
pub mod metal_selector;
pub mod op;
pub mod tensor;
pub mod cuda_exec;
pub mod exec;
pub mod gguf;
pub mod draft_vocab;
pub mod model;
pub mod plan;
pub mod qwen35;
pub mod qwen35_runtime;
pub mod qwen35moe;
pub mod qwen35moe_runtime;
pub mod runtime;
mod session;
pub mod lanes;
pub mod slots;
pub mod vision;
pub mod vocab;
pub mod weights;

pub use cuda_exec::{
    CompiledHybridDecodeCuda, CudaContextArena, CudaDeviceFeatures, CudaExecRuntime,
    CudaRawGraphSession,
};
pub use error::{LlamaError, Result};
pub use exec::{
    CompiledHybridDecode, ExecBackendKind, ExecContextBuffers, ExecFeatures, ExecRuntime,
};
pub use gguf::{
    GgufArray, GgufFile, GgufKeyValue, GgufString, GgufTensorInfo, GgufType, GgufValue,
};
pub use model::{LlamaArchitecture, LlamaModel, ModelGeneral, Qwen35Config, Qwen35MoeConfig};
pub use plan::{
    ModelExecutionPlan, ModelLayerInventory, ModelLayerRole, ModelTailProbePlan,
    ModelTensorInventory,
};
pub use qwen35::{
    Qwen35AttentionScales, Qwen35AttentionTensors, Qwen35DenseFfnScales, Qwen35DenseFfnTensors,
    Qwen35GlobalTensors, Qwen35LayerKind, Qwen35LayerTensors, Qwen35RecurrentScales,
    Qwen35RecurrentTensors, Qwen35Tensors,
};
pub use qwen35_runtime::{
    qwen35_attention_block_layout, qwen35_attention_block_spec, qwen35_attention_decode_spec,
    qwen35_delta_net_recurrent_decode_spec, qwen35_dense_ffn_layout, qwen35_dense_ffn_spec,
    qwen35_embedding_logits_probe_spec, qwen35_execution_plan, qwen35_first_attention_block_spec,
    qwen35_first_dense_ffn_spec, qwen35_first_recurrent_block_spec, qwen35_hybrid_cache_spec,
    qwen35_hybrid_cache_template, qwen35_hybrid_decode_spec, qwen35_recurrent_block_layout,
    qwen35_recurrent_block_spec, qwen35_token_logits_probe_spec, Qwen35Dims,
};
pub use qwen35moe::{
    Qwen35MoeAttentionScales, Qwen35MoeAttentionTensors, Qwen35MoeGlobalTensors,
    Qwen35MoeLayerKind, Qwen35MoeLayerTensors, Qwen35MoeMoeScales, Qwen35MoeMoeTensors,
    Qwen35MoeRecurrentScales, Qwen35MoeRecurrentTensors, Qwen35MoeTensors,
};
pub use qwen35moe_runtime::{
    qwen35moe_attention_block_layout, qwen35moe_attention_block_spec,
    qwen35moe_attention_decode_spec, qwen35moe_delta_net_recurrent_decode_spec,
    qwen35moe_embedding_logits_probe_spec, qwen35moe_execution_plan,
    qwen35moe_first_attention_block_spec, qwen35moe_first_moe_ffn_spec,
    qwen35moe_first_recurrent_block_spec, qwen35moe_hybrid_cache_spec,
    qwen35moe_hybrid_cache_template, qwen35moe_hybrid_decode_spec, qwen35moe_moe_ffn_layout,
    qwen35moe_moe_ffn_spec, qwen35moe_recurrent_block_layout, qwen35moe_recurrent_block_spec,
    qwen35moe_token_logits_probe_spec, Qwen35MoeDims,
};
pub use lanes::{
    LaneCounts, LaneEvent, LaneExecutor, LaneOutcome, LaneRequest, LaneScheduler, LaneStep,
    CHUNK_TOKENS,
};
pub use slots::{
    allocate_depths, draft_depth_for, draft_depth_for_budget, pad_batch_width, should_admit,
    Allocation, LaneDemand, SchedulerConfig, SchedulerObjective, Slot, SlotPhase, SlotStep,
    SlotTable, StepCostModel, StepPlan, BATCH_WIDTHS, COLUMN_BUDGET, SOLO_DRAFT_MAX,
};
pub use runtime::{
    allocate_hybrid_shared_cache_tensors, build_attention_block_graph,
    build_attention_decode_graph, build_attention_decode_graph_with_key_count,
    build_delta_net_recurrent_decode_graph, build_hybrid_decode_graph,
    build_hybrid_decode_graph_with_attention_key_count, build_hybrid_decode_graph_with_outputs,
    build_hybrid_decode_graph_with_sequences,
    build_logits_probe_graph, build_moe_ffn_graph, compile_attention_block_metal,
    compile_attention_decode_metal, compile_attention_decode_metal_with_key_count,
    compile_delta_net_recurrent_decode_metal, compile_hybrid_decode_metal,
    compile_hybrid_decode_metal_with_outputs,
    compile_hybrid_decode_metal_with_shared_runtime_and_state,
    compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs,
    compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count,
    compile_hybrid_decode_metal_with_shared_state,
    compile_hybrid_decode_metal_with_shared_state_and_outputs,
    compile_hybrid_prompt_processing_metal, compile_hybrid_prompt_processing_metal_with_outputs,
    compile_hybrid_prompt_processing_metal_with_shared_runtime_and_state,
    compile_hybrid_prompt_processing_metal_with_shared_runtime_and_state_and_outputs,
    compile_hybrid_prompt_processing_metal_with_shared_state,
    compile_hybrid_prompt_processing_metal_with_shared_state_and_outputs,
    compile_hybrid_token_generation_metal,
    compile_hybrid_token_generation_metal_with_shared_runtime_and_state,
    compile_hybrid_token_generation_metal_with_shared_state, compile_logits_probe_metal,
    compile_moe_ffn_metal, create_metal_context_buffer, create_metal_context_buffer_with_runtime,
    execute_attention_block_graph_metal, execute_attention_block_graph_metal_cached,
    execute_attention_decode_graph_metal, execute_attention_decode_graph_metal_cached,
    execute_delta_net_recurrent_decode_graph_metal,
    execute_delta_net_recurrent_decode_graph_metal_cached, execute_hybrid_decode_graph_metal,
    execute_hybrid_decode_graph_metal_cached, execute_hybrid_decode_graph_metal_cached_logits_only,
    execute_logits_probe_graph_metal, execute_logits_probe_graph_metal_cached,
    execute_logits_probe_metal, execute_moe_ffn_graph_metal, execute_moe_ffn_graph_metal_cached,
    execute_prepared_attention_block_metal, execute_prepared_attention_decode_metal,
    execute_prepared_attention_decode_metal_no_readback,
    execute_prepared_attention_decode_metal_no_readback_buffer_input,
    execute_prepared_attention_decode_metal_no_readback_prepared_input,
    execute_prepared_attention_decode_metal_no_readback_prepared_input_in_active_batch,
    execute_prepared_delta_net_recurrent_decode_metal, execute_prepared_hybrid_decode_metal,
    execute_prepared_logits_probe_metal, execute_prepared_moe_ffn_metal,
    prepare_attention_block_graph, prepare_attention_decode_graph,
    prepare_attention_decode_graph_with_key_count, prepare_delta_net_recurrent_decode_graph,
    prepare_hybrid_decode_graph, prepare_hybrid_decode_graph_with_attention_key_count,
    prepare_hybrid_decode_graph_with_outputs, prepare_logits_probe_graph, prepare_moe_ffn_graph,
    AttentionBlockGraph, AttentionBlockRun, AttentionBlockSpec, AttentionDecodeGraph,
    AttentionDecodeSpec, AttentionKvCacheSpec, AttentionQueryLayout, AttentionRopeSpec,
    CompiledAttentionBlockMetal, CompiledAttentionDecodeMetal,
    CompiledDeltaNetRecurrentDecodeMetal, CompiledHybridDecodeMetal, CompiledLogitsProbeMetal,
    CompiledMoeFfnMetal, DeltaNetRecurrentBlockSpec, DeltaNetRecurrentDecodeGraph,
    DeltaNetRecurrentDecodeSpec, DeltaNetRecurrentStateSpec, DenseGatedFfnSpec, DenseLayerFfnSpec,
    ExpertGatingFunc, GraphBatch, HybridAttentionCacheIds, HybridAttentionCacheView,
    HybridCacheLayout, HybridCacheShape, HybridCacheSpec, HybridCacheTemplate, HybridCacheTypes,
    HybridDecodeBatchLayout, HybridDecodeGraph, HybridDecodeOutputConfig, HybridDecodeRun,
    HybridDecodeSpec, HybridLayerFfnSpec, HybridLayerSpec, HybridRecurrentCacheIds,
    HybridSharedCacheTensorIds, LogitsProbeGraph, LogitsProbeInput, LogitsProbeRun,
    LogitsProbeSpec, MoeFfnGraph, MoeFfnRun, MoeFfnSpec, MoeSharedExpertSpec, ProbeInputKind,
    RmsNormSpec,
};
pub use draft_vocab::DraftVocab;
pub use session::{
    LlamaGeneration, LlamaSamplerState, LlamaSamplingParams, LlamaSession, LlamaSessionConfig,
    LlamaStopReason, SpeculativeStats,
};
pub use vision::{
    calc_size_preserved_ratio, preprocess_rgb8, vision_rope_positions, PreparedImage,
    VisionConfig, VisionTower,
};
pub use vocab::{LlamaTextDecoder, LlamaTokenizerKind, LlamaVocab};
pub use weights::{GgufWeightLayout, LoadedGgufWeights};
pub use context::Context;
pub use core::{
    ggml_pad, InitParams, LogLevel, ObjectType, PoolOp, ScaleMode, SortOrder, Status, TensorFlag,
    TriType, GGML_DEFAULT_GRAPH_SIZE, GGML_DEFAULT_N_THREADS, GGML_FILE_MAGIC, GGML_FILE_VERSION,
    GGML_MAX_DIMS, GGML_MAX_NAME, GGML_MAX_N_THREADS, GGML_MAX_OP_PARAMS, GGML_MAX_PARAMS,
    GGML_MAX_SRC, GGML_MEM_ALIGN, GGML_MROPE_SECTIONS, GGML_QNT_VERSION, GGML_QNT_VERSION_FACTOR,
    GGML_ROPE_TYPE_IMROPE, GGML_ROPE_TYPE_MROPE, GGML_ROPE_TYPE_NEOX, GGML_ROPE_TYPE_NORMAL,
    GGML_ROPE_TYPE_VISION, GGML_SCALE_FLAG_ALIGN_CORNERS, GGML_SCALE_FLAG_ANTIALIAS,
};
pub use graph::{Graph, GraphEvalOrder, NodeId};
pub use mmap::MappedRegion;
pub use op::{
    ggml_glu_op_name, ggml_op_name, ggml_op_symbol, ggml_unary_op_name, Ftype, GluOp, Op, Prec,
    UnaryOp,
};
pub use tensor::{
    ggml_blck_size_for_type, ggml_ftype_to_tensor_type, ggml_row_size_for_type,
    ggml_type_size_for_type, BufferUsage, Tensor, TensorDesc, TensorFlags, TensorId, TensorLayout,
    TensorType,
};
