#[cfg(test)]
use crate::layer0_cached_case::ExactMetalGenerationCursor;
use crate::layer0_cached_case::{
    run_layer_sequence_from_inputs, CachedLayerInputs, ExactMetalGenerationGraph,
    ExactMetalGenerationStopReason, ExactMetalTextRuntimeSession, Layer0CachedArtifacts,
    Layer0CachedPlan, Layer0CachedStage,
};
use crate::GemmaKvCacheLayout;
#[cfg(test)]
use crate::{GemmaKvCacheSet, KvTensor, KvTensorShape};
use makepad_ggml::backend::metal::MetalRuntimeCounters;
#[cfg(test)]
use crate::{MlxDType, MlxGemmaMoeExpertOutput, MlxRouterTopKOutput};
use crate::{MlxGreedyToken, MlxIndexedSafetensors, MlxTokenizer};
use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

include!("text_runtime/api.rs");
include!("text_runtime/reference.rs");

#[cfg(test)]
#[path = "../tests/text_runtime.rs"]
mod tests;
