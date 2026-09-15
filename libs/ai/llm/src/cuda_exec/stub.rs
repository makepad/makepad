//! Fail-closed stand-in when the CUDA kernels are not part of this build
//! (macOS, or a Windows/Linux build without the CUDA toolkit). Everything
//! type-checks; `Runtime::new()` reports exactly why execution is
//! unavailable, and the unreachable types are empty enums.

use crate::Context;

use super::CudaDeviceFeatures;
use crate::error::{LlamaError, Result};
use crate::runtime::{
    HybridDecodeBatchLayout, HybridDecodeGraph, HybridDecodeRun, HybridDecodeSpec,
    HybridSharedCacheTensorIds, LogitsProbeInput,
};
use crate::weights::LoadedGgufWeights;

fn unavailable_reason() -> String {
    if cfg!(any(target_os = "windows", target_os = "linux")) {
        "the native CUDA llama executor is not compiled into this build \
         (ggml CUDA kernels / toolkit were unavailable at build time)"
            .to_string()
    } else {
        "the native CUDA llama executor only exists on Windows/Linux".to_string()
    }
}

pub(super) struct Runtime {}

pub(super) enum Arena {}

pub(super) enum Compiled {}

pub(super) enum RawSession {}

impl Runtime {
    pub(super) fn new() -> Result<Self> {
        Err(LlamaError::unsupported(unavailable_reason()))
    }

    pub(super) fn features(&self) -> CudaDeviceFeatures {
        CudaDeviceFeatures {
            device_name: "unavailable".to_string(),
            compute_capability: (0, 0),
            total_vram_bytes: 0,
            free_vram_bytes: 0,
            sm_count: 0,
            compiled_arch: "none",
        }
    }

    pub(super) fn reserve_main_buffer_size(
        &self,
        _weights: &LoadedGgufWeights,
        _spec: &HybridDecodeSpec,
        _shared_cache: Option<&HybridSharedCacheTensorIds>,
        _n_tokens: usize,
        _n_outputs: usize,
    ) -> Result<usize> {
        Err(LlamaError::unsupported(unavailable_reason()))
    }

    pub(super) fn create_context_arena(&self, _ctx: &Context) -> Result<Arena> {
        Err(LlamaError::unsupported(unavailable_reason()))
    }

    pub(super) fn create_context_arena_with_progress(
        &self,
        ctx: &Context,
        _progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Arena> {
        self.create_context_arena(ctx)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_hybrid_decode(
        &self,
        _weights: &mut LoadedGgufWeights,
        _spec: &HybridDecodeSpec,
        _shared_cache: &HybridSharedCacheTensorIds,
        _arena: &Arena,
        _n_tokens: usize,
        _n_outputs: usize,
        _attention_key_base: usize,
        _attention_key_count: usize,
        _n_seqs: usize,
    ) -> Result<Compiled> {
        Err(LlamaError::unsupported(unavailable_reason()))
    }

    pub(super) fn read_arena_bytes(
        &self,
        arena: &Arena,
        _offset: usize,
        _len: usize,
    ) -> Result<Vec<u8>> {
        match *arena {}
    }

    pub(super) fn clear_arena_ranges(
        &self,
        arena: &Arena,
        _ranges: &[(usize, usize)],
    ) -> Result<()> {
        match *arena {}
    }

    pub(super) fn execute_raw_graph(
        &self,
        _ctx: &Context,
        _graph: &crate::Graph,
        _pinned: &[crate::TensorId],
        _writes: &[(crate::TensorId, Vec<u8>)],
        _wanted: &[crate::TensorId],
    ) -> Result<std::collections::BTreeMap<crate::TensorId, Vec<u8>>> {
        Err(LlamaError::unsupported(unavailable_reason()))
    }

    pub(super) fn create_raw_session(
        &self,
        _ctx: &Context,
        _graph: &crate::Graph,
        _pinned: &[crate::TensorId],
        _options: super::CudaGraphOptions,
        _progress: &mut dyn FnMut(usize, usize),
    ) -> Result<RawSession> {
        Err(LlamaError::unsupported(unavailable_reason()))
    }
}

impl RawSession {
    pub(super) fn execute(
        &self,
        _ctx: &Context,
        _writes: &[(crate::TensorId, &[u8])],
        _wanted: &[crate::TensorId],
    ) -> Result<std::collections::BTreeMap<crate::TensorId, Vec<u8>>> {
        match *self {}
    }

    pub(super) fn device_bytes(&self) -> usize {
        match *self {}
    }

    pub(super) fn node_count(&self) -> usize {
        match *self {}
    }
}

impl Compiled {
    pub(super) fn decode(&self) -> &HybridDecodeGraph {
        match *self {}
    }

    pub(super) fn execute(
        &mut self,
        _input: LogitsProbeInput<'_>,
        _layout: &HybridDecodeBatchLayout,
        _capture_hidden: bool,
        _skip_logits_readback: bool,
    ) -> Result<HybridDecodeRun> {
        match *self {}
    }
}
