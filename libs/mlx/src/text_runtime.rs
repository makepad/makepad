#[cfg(test)]
use crate::layer0_cached_case::ExactMetalGenerationCursor;
use crate::layer0_cached_case::{
    run_layer_sequence_from_inputs, CachedLayerInputs, ExactMetalGenerationGraph,
    ExactMetalGenerationStopReason, ExactMetalTextRuntimeSession, Layer0CachedArtifacts,
    Layer0CachedPlan, Layer0CachedStage,
};
use crate::GemmaKvCacheLayout;
use crate::{GemmaKvCacheSet, KvTensor, KvTensorShape};
use crate::{MlxDType, MlxGemmaMoeExpertOutput, MlxRouterTopKOutput};
use crate::{MlxGreedyToken, MlxIndexedSafetensors, MlxTokenizer};
use makepad_ggml::backend::metal::MetalRuntimeCounters;
use makepad_ggml::backend::{
    try_affine_quantized_matmul_bf16, try_matmul_nt_ggml_bytes,
    try_matmul_nt_ggml_bytes_cached_bf16_words,
    AffineQuantizedMatmulSpec,
};
use makepad_ggml::quant::{vec_dot_nvfp4_f32, GGML_TYPE_BF16, GGML_TYPE_NVFP4};
use std::collections::BTreeSet;
use std::error::Error;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

mod cuda_exact;

const EMBED_TOKENS_WEIGHT_NAME: &str = "language_model.model.embed_tokens.weight";
const EMBED_TOKENS_SCALES_NAME: &str = "language_model.model.embed_tokens.scales";
const EMBED_TOKENS_BIASES_NAME: &str = "language_model.model.embed_tokens.biases";

include!("text_runtime/api.rs");
include!("text_runtime/reference.rs");

#[cfg(test)]
#[path = "../tests/text_runtime.rs"]
mod tests;
