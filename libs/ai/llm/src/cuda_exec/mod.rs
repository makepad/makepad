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

/// Pointer-stability epoch for the device scratch buffers.
///
/// A captured CUDA graph bakes device pointers into its nodes. The scratch
/// buffers (`GRAPH_ACT_SCRATCH` / `GRAPH_WEIGHT_SCRATCH`) live on the
/// `DeviceState` that **every** compiled graph shape shares, while the captured
/// graph is per-shape — and `Scratch::ensure` frees and re-mallocs on growth,
/// returning a different pointer. So a graph captured for shape A, followed by
/// shape B growing the scratch, would replay A against freed memory: silent
/// corruption, not a crash. Today the fixed pre-capture reserves happen to make
/// growth impossible; multiplying batch shapes is exactly what would break that.
///
/// Every realloc bumps this epoch. A captured graph records the epoch it was
/// captured at, and is thrown away and re-captured as soon as they differ.
///
/// (Kept here rather than in `real.rs` so its rule is testable on any host, not
/// only on a box with the CUDA toolkit.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(makepad_llama_cuda_kernels), allow(dead_code))]
pub(crate) struct ScratchEpoch(u64);

#[cfg_attr(not(makepad_llama_cuda_kernels), allow(dead_code))]
impl ScratchEpoch {
    /// One scratch buffer just moved: every previously captured graph is now
    /// holding a dangling pointer.
    pub(crate) fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }

    /// Is a graph captured at `captured_at` still safe to replay?
    pub(crate) fn is_stale(self, captured_at: ScratchEpoch) -> bool {
        self != captured_at
    }
}

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

/// The widest activation batch the quantized mat-vec route serves, and with it
/// the boundary between Q8_1-quantized activations and the bf16 GEMM slab.
/// Exported so the numerical canary can place cases on both sides of the line
/// the dispatcher actually draws.
#[cfg(makepad_llama_cuda_kernels)]
pub use imp::MMV_MAX_COLUMNS;

#[cfg(not(makepad_llama_cuda_kernels))]
pub const MMV_MAX_COLUMNS: usize = 8;

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

#[cfg(test)]
mod scratch_epoch_tests {
    use super::ScratchEpoch;

    /// The shared scratch buffer, modelled at the only level that matters:
    /// growing it moves the pointer.
    struct FakeScratch {
        ptr: usize,
        size: usize,
        next_ptr: usize,
        epoch: ScratchEpoch,
    }

    impl FakeScratch {
        fn new() -> Self {
            Self {
                ptr: 0x1000,
                size: 0,
                next_ptr: 0x1000,
                epoch: ScratchEpoch::default(),
            }
        }

        /// Mirrors `Scratch::ensure`: grow by free + malloc, which returns a
        /// DIFFERENT pointer and invalidates every captured graph.
        fn ensure(&mut self, size: usize) -> usize {
            if self.size < size {
                self.next_ptr += 0x1000;
                self.ptr = self.next_ptr;
                self.size = size;
                self.epoch.bump();
            }
            self.ptr
        }
    }

    /// One compiled graph shape: captures a pointer once, replays it after.
    struct FakeCompiled {
        captured_ptr: Option<usize>,
        captured_at: ScratchEpoch,
    }

    impl FakeCompiled {
        fn new() -> Self {
            Self {
                captured_ptr: None,
                captured_at: ScratchEpoch::default(),
            }
        }

        /// `execute_with_graph` with the epoch check: a stale capture is
        /// dropped and re-captured against the current pointer.
        fn execute(&mut self, scratch: &mut FakeScratch, reserve: usize, check_epoch: bool) -> usize {
            let live = scratch.ensure(reserve);
            if check_epoch {
                if self.captured_ptr.is_some() && scratch.epoch.is_stale(self.captured_at) {
                    self.captured_ptr = None;
                }
            }
            match self.captured_ptr {
                Some(ptr) => ptr,
                None => {
                    self.captured_ptr = Some(live);
                    self.captured_at = scratch.epoch;
                    live
                }
            }
        }
    }

    /// The §8.4b hazard, as a sequence: shape A captures, shape B grows the
    /// shared scratch, shape A replays. Without the epoch check A replays a
    /// pointer that no longer exists.
    #[test]
    fn a_graph_captured_before_a_scratch_growth_would_replay_a_freed_pointer() {
        const RESERVE: usize = 64 << 20;
        let mut scratch = FakeScratch::new();
        let mut shape_a = FakeCompiled::new();
        let mut shape_b = FakeCompiled::new();

        let a_first = shape_a.execute(&mut scratch, RESERVE, false);
        // A bigger batch shape needs more scratch than the pre-capture reserve.
        shape_b.execute(&mut scratch, RESERVE * 2, false);
        let a_replay = shape_a.execute(&mut scratch, RESERVE, false);

        assert_eq!(a_replay, a_first, "the unguarded replay reuses the old pointer");
        assert_ne!(
            a_replay, scratch.ptr,
            "...which is no longer the live scratch: that is the use-after-free"
        );
    }

    #[test]
    fn the_epoch_check_re_captures_instead_of_replaying_a_freed_pointer() {
        const RESERVE: usize = 64 << 20;
        let mut scratch = FakeScratch::new();
        let mut shape_a = FakeCompiled::new();
        let mut shape_b = FakeCompiled::new();

        shape_a.execute(&mut scratch, RESERVE, true);
        shape_b.execute(&mut scratch, RESERVE * 2, true);
        let a_replay = shape_a.execute(&mut scratch, RESERVE, true);

        assert_eq!(a_replay, scratch.ptr, "shape A must re-capture against the live scratch");
        assert_eq!(shape_a.captured_at, scratch.epoch);

        // And a steady state with no growth must NOT re-capture: the check is
        // free once the buffers have settled, which is every real decode step.
        let settled = scratch.epoch;
        for _ in 0..4 {
            assert_eq!(shape_a.execute(&mut scratch, RESERVE, true), scratch.ptr);
            assert_eq!(shape_b.execute(&mut scratch, RESERVE * 2, true), scratch.ptr);
            assert_eq!(scratch.epoch, settled, "a settled scratch must not move");
        }
    }
}
