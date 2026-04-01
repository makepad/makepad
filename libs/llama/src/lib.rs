mod error;
pub mod gguf;
pub mod model;
pub mod plan;
pub mod qwen35moe;
pub mod qwen35moe_runtime;
pub mod runtime;
pub mod vocab;
pub mod weights;

pub use error::{LlamaError, Result};
pub use gguf::{GgufArray, GgufFile, GgufKeyValue, GgufString, GgufTensorInfo, GgufType, GgufValue};
pub use model::{LlamaArchitecture, LlamaModel, ModelGeneral, Qwen35MoeConfig};
pub use plan::{
    ModelExecutionPlan, ModelLayerInventory, ModelLayerRole, ModelTailProbePlan,
    ModelTensorInventory,
};
pub use qwen35moe::{
    Qwen35MoeAttentionScales, Qwen35MoeAttentionTensors, Qwen35MoeGlobalTensors,
    Qwen35MoeLayerKind, Qwen35MoeLayerTensors, Qwen35MoeMoeScales, Qwen35MoeMoeTensors,
    Qwen35MoeRecurrentScales, Qwen35MoeRecurrentTensors, Qwen35MoeTensors,
};
pub use qwen35moe_runtime::{
    qwen35moe_attention_block_layout, qwen35moe_attention_block_spec,
    qwen35moe_attention_decode_spec,
    qwen35moe_delta_net_recurrent_decode_spec,
    qwen35moe_embedding_logits_probe_spec, qwen35moe_first_attention_block_spec,
    qwen35moe_first_moe_ffn_spec, qwen35moe_first_recurrent_block_spec,
    qwen35moe_hybrid_cache_spec, qwen35moe_hybrid_cache_template,
    qwen35moe_hybrid_decode_spec,
    qwen35moe_moe_ffn_layout, qwen35moe_moe_ffn_spec,
    qwen35moe_recurrent_block_layout, qwen35moe_recurrent_block_spec,
    qwen35moe_token_logits_probe_spec, qwen35moe_execution_plan, Qwen35MoeDims,
};
pub use runtime::{
    build_attention_block_graph, build_attention_decode_graph, build_logits_probe_graph,
    build_delta_net_recurrent_decode_graph, build_moe_ffn_graph, compile_attention_block_metal,
    compile_attention_decode_metal, compile_delta_net_recurrent_decode_metal,
    compile_hybrid_decode_metal, compile_logits_probe_metal, compile_moe_ffn_metal,
    execute_attention_block_graph_metal, execute_attention_block_graph_metal_cached,
    execute_attention_decode_graph_metal, execute_attention_decode_graph_metal_cached,
    execute_delta_net_recurrent_decode_graph_metal,
    execute_delta_net_recurrent_decode_graph_metal_cached,
    execute_hybrid_decode_graph_metal, execute_hybrid_decode_graph_metal_cached,
    execute_logits_probe_graph_metal, execute_logits_probe_graph_metal_cached,
    execute_logits_probe_metal, execute_moe_ffn_graph_metal,
    execute_moe_ffn_graph_metal_cached, execute_prepared_attention_block_metal,
    execute_prepared_attention_decode_metal, execute_prepared_logits_probe_metal,
    execute_prepared_delta_net_recurrent_decode_metal, execute_prepared_hybrid_decode_metal,
    execute_prepared_moe_ffn_metal,
    prepare_attention_block_graph,
    prepare_attention_decode_graph, prepare_delta_net_recurrent_decode_graph,
    prepare_hybrid_decode_graph, prepare_logits_probe_graph, prepare_moe_ffn_graph, AttentionBlockGraph,
    AttentionBlockRun, AttentionBlockSpec,
    AttentionDecodeGraph, AttentionDecodeSpec, AttentionKvCacheSpec, AttentionQueryLayout,
    AttentionRopeSpec, CompiledAttentionBlockMetal, CompiledAttentionDecodeMetal,
    CompiledDeltaNetRecurrentDecodeMetal, CompiledHybridDecodeMetal, CompiledLogitsProbeMetal,
    CompiledMoeFfnMetal, DenseGatedFfnSpec, ExpertGatingFunc,
    DeltaNetRecurrentBlockSpec, DeltaNetRecurrentDecodeGraph, DeltaNetRecurrentDecodeSpec,
    DeltaNetRecurrentStateSpec, GraphBatch, HybridAttentionCacheView, HybridCacheLayout, HybridCacheShape,
    HybridCacheSpec, HybridCacheTemplate, HybridCacheTypes, LogitsProbeGraph,
    HybridDecodeGraph, HybridDecodeRun, HybridDecodeSpec, HybridLayerSpec, LogitsProbeInput,
    LogitsProbeRun, LogitsProbeSpec, MoeFfnGraph, MoeFfnRun, MoeFfnSpec, MoeSharedExpertSpec,
    ProbeInputKind, RmsNormSpec,
};
pub use vocab::LlamaVocab;
pub use weights::{GgufWeightLayout, LoadedGgufWeights};
