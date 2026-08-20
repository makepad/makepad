//! Native CUDA execution backend for the llama hybrid-decode graphs.
//!
//! Mirrors the Metal execution model exactly: the ggml `Context` arena is
//! reflected one-to-one into device memory — a read-only weights region
//! uploaded once (quantized tensors stay in their GGUF encoding on device;
//! Q4_K/Q5_K/Q6_K mat-vecs read the block stream directly) and a dirty
//! region holding KV/recurrent caches and planned graph activations. Every
//! `TensorId` resolves to `device_base + logical_offset`, so views,
//! permutes and strided reads behave identically to the Metal oracle.
//!
//! Fail-closed contract: `CudaExecRuntime::new()` verifies a usable device,
//! kernel-build/SASS compatibility and memory headroom; graph compilation
//! verifies every node's op/layout/tensor-type. Any gap is an error with
//! the precise reason — no CPU fallback, no silent dequant residency, no
//! shared-GPU-memory spill.
//!
//! The real implementation compiles behind `makepad_llama_cuda_kernels`
//! (set by llama build.rs when the CUDA toolkit is present). Kernel
//! objects live in `makepad-ggml` (`backend/cuda/llm/`) and link from
//! ggml's static lib. Elsewhere this module type-checks and fails
//! closed at runtime.

use crate::Context;

use crate::error::Result;
use crate::runtime::{
    HybridDecodeBatchLayout, HybridDecodeGraph, HybridDecodeRun, HybridDecodeSpec,
    HybridSharedCacheTensorIds, LogitsProbeInput,
};
use crate::weights::LoadedGgufWeights;

#[cfg(makepad_llama_cuda_kernels)]
#[path = "real.rs"]
mod imp;

#[cfg(not(makepad_llama_cuda_kernels))]
#[path = "stub.rs"]
mod imp;

/// Snapshot of the bound CUDA device, reported truthfully from the driver.
#[derive(Clone, Debug)]
pub struct CudaDeviceFeatures {
    pub device_name: String,
    pub compute_capability: (u32, u32),
    pub total_vram_bytes: u64,
    pub free_vram_bytes: u64,
    pub sm_count: u32,
    /// The SASS architecture this binary's kernels were compiled for.
    pub compiled_arch: &'static str,
}

/// The CUDA execution backend handle (device + stream + cublas identity).
pub struct CudaExecRuntime {
    imp: imp::Runtime,
}

/// Device mirror of a ggml `Context` arena.
pub struct CudaContextArena {
    imp: imp::Arena,
}

/// One compiled hybrid-decode graph shape on CUDA.
pub struct CompiledHybridDecodeCuda {
    imp: imp::Compiled,
}

/// A plain ggml `Graph` compiled once and executed many times on CUDA.
///
/// The CUDA sibling of `MetalGraphSession`: the caller owns the `Context`,
/// hands in one graph, and then writes inputs / reads outputs by `TensorId`
/// per execution. Weights are mirrored to the device once at construction and
/// activations are planned into a device buffer sized from the plan.
pub struct CudaRawGraphSession {
    imp: imp::RawSession,
}

impl CudaRawGraphSession {
    /// Execute the compiled graph. `writes` are uploaded (staged through
    /// pinned host memory), then every node is dispatched on the runtime's
    /// stream, then `wanted` is read back.
    pub fn execute(
        &self,
        ctx: &Context,
        writes: &[(crate::TensorId, &[u8])],
        wanted: &[crate::TensorId],
    ) -> Result<std::collections::BTreeMap<crate::TensorId, Vec<u8>>> {
        self.imp.execute(ctx, writes, wanted)
    }

    /// Total device bytes this session holds (weights + planned activations).
    pub fn device_bytes(&self) -> usize {
        self.imp.device_bytes()
    }

    /// Dispatches per execution, after fusion.
    pub fn node_count(&self) -> usize {
        self.imp.node_count()
    }
}

impl CudaExecRuntime {
    pub fn new() -> Result<Self> {
        Ok(Self {
            imp: imp::Runtime::new()?,
        })
    }

    pub fn features(&self) -> CudaDeviceFeatures {
        self.imp.features()
    }

    pub fn device_description(&self) -> String {
        let features = self.features();
        format!(
            "cuda:{} sm_{}{} vram {:.1}GiB (kernels sm_{})",
            features.device_name,
            features.compute_capability.0,
            features.compute_capability.1,
            features.total_vram_bytes as f64 / (1u64 << 30) as f64,
            features.compiled_arch,
        )
    }

    pub fn reserve_hybrid_decode_main_buffer_size(
        &self,
        weights: &LoadedGgufWeights,
        spec: &HybridDecodeSpec,
        shared_cache: Option<&HybridSharedCacheTensorIds>,
        n_tokens: usize,
        n_outputs: usize,
    ) -> Result<usize> {
        self.imp
            .reserve_main_buffer_size(weights, spec, shared_cache, n_tokens, n_outputs)
    }

    pub fn create_context_arena(&self, ctx: &Context) -> Result<CudaContextArena> {
        self.create_context_arena_with_progress(ctx, &mut |_, _| {})
    }

    pub fn create_context_arena_with_progress(
        &self,
        ctx: &Context,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<CudaContextArena> {
        Ok(CudaContextArena {
            imp: self.imp.create_context_arena_with_progress(ctx, progress)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn compile_hybrid_decode(
        &self,
        weights: &mut LoadedGgufWeights,
        spec: &HybridDecodeSpec,
        shared_cache: &HybridSharedCacheTensorIds,
        arena: &CudaContextArena,
        n_tokens: usize,
        n_outputs: usize,
        attention_key_count: usize,
    ) -> Result<CompiledHybridDecodeCuda> {
        Ok(CompiledHybridDecodeCuda {
            imp: self.imp.compile_hybrid_decode(
                weights,
                spec,
                shared_cache,
                &arena.imp,
                n_tokens,
                n_outputs,
                attention_key_count,
            )?,
        })
    }

    pub fn read_arena_bytes(
        &self,
        arena: &CudaContextArena,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>> {
        self.imp.read_arena_bytes(&arena.imp, offset, len)
    }

    /// Zero logical byte ranges in the mutable device arena. Session reset
    /// uses this to clear KV and recurrent caches without allocating and
    /// uploading a second copy of the model.
    pub fn clear_arena_ranges(
        &self,
        arena: &CudaContextArena,
        ranges: &[(usize, usize)],
    ) -> Result<()> {
        self.imp.clear_arena_ranges(&arena.imp, ranges)
    }

    /// Compile an arbitrary graph over `ctx` into a reusable session.
    ///
    /// `pinned` tensors stay alive to graph end (graph outputs, anything the
    /// caller intends to read back); everything else is free to have its
    /// storage recycled by the activation planner. Mirroring the context to
    /// the device happens here, once.
    pub fn create_raw_graph_session(
        &self,
        ctx: &Context,
        graph: &crate::Graph,
        pinned: &[crate::TensorId],
    ) -> Result<CudaRawGraphSession> {
        self.create_raw_graph_session_with_progress(ctx, graph, pinned, &mut |_, _| {})
    }

    pub fn create_raw_graph_session_with_progress(
        &self,
        ctx: &Context,
        graph: &crate::Graph,
        pinned: &[crate::TensorId],
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<CudaRawGraphSession> {
        Ok(CudaRawGraphSession {
            imp: self.imp.create_raw_session(ctx, graph, pinned, progress)?,
        })
    }

    /// Validation entry: execute an arbitrary graph over `ctx` through the
    /// exact planner/dispatch path the session uses. `pinned` tensors stay
    /// alive to graph end; `writes` upload input bytes; `wanted` tensors are
    /// read back. Used by the on-device op-correctness canaries.
    pub fn execute_raw_graph(
        &self,
        ctx: &Context,
        graph: &crate::Graph,
        pinned: &[crate::TensorId],
        writes: &[(crate::TensorId, Vec<u8>)],
        wanted: &[crate::TensorId],
    ) -> Result<std::collections::BTreeMap<crate::TensorId, Vec<u8>>> {
        self.imp.execute_raw_graph(ctx, graph, pinned, writes, wanted)
    }
}

impl CompiledHybridDecodeCuda {
    pub fn decode(&self) -> &HybridDecodeGraph {
        self.imp.decode()
    }

    pub fn execute_with_layout(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
    ) -> Result<HybridDecodeRun> {
        self.imp.execute(input, layout, true)
    }

    pub fn execute_logits_only_with_layout(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
    ) -> Result<HybridDecodeRun> {
        self.imp.execute(input, layout, false)
    }
}

/// llama-bench.cpp:2026 host-vs-GPU split (MAKEPAD_LLAMA_HOST_SPLIT=1).
#[cfg(makepad_llama_cuda_kernels)]
pub use imp::{host_split_reset, host_split_snapshot, HostSplit};

#[cfg(not(makepad_llama_cuda_kernels))]
#[derive(Default, Clone, Debug)]
pub struct HostSplit {
    pub tokens: u64,
    pub write_bytes: u64,
    pub host_pre_ms: f64,
    pub host_h2d_ms: f64,
    pub host_launch_cpu_ms: f64,
    pub gpu_graph_ms: f64,
    pub gpu_d2h_ms: f64,
    pub host_collect_ms: f64,
    pub wall_ms: f64,
}

#[cfg(not(makepad_llama_cuda_kernels))]
impl HostSplit {
    pub fn host_outside_gpu_ms(&self) -> f64 {
        0.0
    }
    pub fn report_line(&self) -> String {
        "host.split: unavailable (no CUDA kernels)".to_string()
    }
}

#[cfg(not(makepad_llama_cuda_kernels))]
pub fn host_split_reset() {}

#[cfg(not(makepad_llama_cuda_kernels))]
pub fn host_split_snapshot() -> HostSplit {
    HostSplit::default()
}
