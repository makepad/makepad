//! Backend-neutral execution seam for `LlamaSession`.
//!
//! The graph builders in `runtime.rs` are backend-agnostic: they produce a
//! `crate::Graph` over a `Context` whose tensors (weights, caches,
//! activations) are byte offsets into one arena. This module is the single
//! place that binds those graphs to a GPU execution backend:
//!
//! - macOS/iOS: the existing Metal path. The Metal arms below call EXACTLY
//!   the functions `session.rs` called before this seam existed, in the same
//!   order with the same arguments — Metal behavior is the oracle and must
//!   not change.
//! - Windows/Linux: the native CUDA executor in `cuda_exec` (quantized
//!   weights resident on device, executing the same graphs).
//!
//! There is deliberately NO cross-backend or CPU fallback: a platform whose
//! native backend cannot run (no device, unsupported arch, missing kernels,
//! not enough memory) fails closed at `ExecRuntime::new()` or at graph
//! compile, with the reason in the error.

use crate::metal_compiled::MetalContextBuffers;
use makepad_ai_metal::{MetalDeviceFeatures, MetalRuntime};
use crate::Context;

use crate::cuda_exec::{
    CompiledHybridDecodeCuda, CudaContextArena, CudaDeviceFeatures, CudaExecRuntime,
};
use crate::error::{LlamaError, Result};
use crate::runtime::{
    compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count,
    create_metal_context_buffer_with_runtime, reserve_hybrid_decode_main_buffer_size,
    CompiledHybridDecodeMetal, HybridDecodeBatchLayout, HybridDecodeGraph, HybridDecodeRun,
    HybridDecodeSpec, HybridSharedCacheTensorIds, LogitsProbeInput,
};
use crate::weights::LoadedGgufWeights;

/// Which execution backend a session should bind to. `Native` picks the
/// platform's own GPU backend and is the only production mode; the explicit
/// variants exist for tests and probes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecBackendKind {
    Native,
    Metal,
    Cuda,
}

/// Device capability snapshot used for graph preparation and buffer sizing.
pub enum ExecFeatures {
    Metal(MetalDeviceFeatures),
    Cuda(CudaDeviceFeatures),
}

/// The per-backend GPU view of a ggml `Context` arena.
pub enum ExecContextBuffers {
    Metal(MetalContextBuffers),
    Cuda(CudaContextArena),
}

/// One compiled hybrid-decode graph shape, ready to execute repeatedly.
pub enum CompiledHybridDecode {
    Metal(CompiledHybridDecodeMetal),
    Cuda(CompiledHybridDecodeCuda),
}

/// The execution backend handle shared by every compiled graph of a session.
pub enum ExecRuntime {
    Metal(MetalRuntime),
    Cuda(CudaExecRuntime),
}

impl ExecRuntime {
    /// Bind the platform-native backend, failing closed when it cannot run.
    pub fn new() -> Result<Self> {
        Self::with_backend(ExecBackendKind::Native)
    }

    pub fn with_backend(kind: ExecBackendKind) -> Result<Self> {
        match kind {
            ExecBackendKind::Native => {
                if cfg!(any(target_os = "macos", target_os = "ios")) {
                    Self::with_backend(ExecBackendKind::Metal)
                } else if cfg!(any(target_os = "windows", target_os = "linux")) {
                    Self::with_backend(ExecBackendKind::Cuda)
                } else {
                    Err(LlamaError::unsupported(
                        "no native LLM execution backend for this platform (Metal on \
                         macOS/iOS, CUDA on Windows/Linux)",
                    ))
                }
            }
            ExecBackendKind::Metal => Ok(Self::Metal(
                MetalRuntime::new().map_err(LlamaError::unsupported)?,
            )),
            ExecBackendKind::Cuda => Ok(Self::Cuda(CudaExecRuntime::new()?)),
        }
    }

    pub fn backend_kind(&self) -> ExecBackendKind {
        match self {
            Self::Metal(_) => ExecBackendKind::Metal,
            Self::Cuda(_) => ExecBackendKind::Cuda,
        }
    }

    /// Human-readable device identity for logs and truthful status reports.
    pub fn device_description(&self) -> String {
        match self {
            Self::Metal(_) => "metal".to_string(),
            Self::Cuda(runtime) => runtime.device_description(),
        }
    }

    pub fn features(&self) -> ExecFeatures {
        match self {
            Self::Metal(runtime) => ExecFeatures::Metal(runtime.features()),
            Self::Cuda(runtime) => ExecFeatures::Cuda(runtime.features()),
        }
    }

    /// The main-buffer bytes the largest requested graph shape needs inside
    /// the context arena (weights context `mem_size` must cover it).
    pub fn reserve_hybrid_decode_main_buffer_size(
        &self,
        weights: &LoadedGgufWeights,
        spec: &HybridDecodeSpec,
        shared_cache: Option<&HybridSharedCacheTensorIds>,
        n_tokens: usize,
        n_outputs: usize,
    ) -> Result<usize> {
        match self {
            Self::Metal(runtime) => reserve_hybrid_decode_main_buffer_size(
                weights,
                spec,
                shared_cache,
                n_tokens,
                n_outputs,
                runtime.features(),
            ),
            Self::Cuda(runtime) => runtime.reserve_hybrid_decode_main_buffer_size(
                weights,
                spec,
                shared_cache,
                n_tokens,
                n_outputs,
            ),
        }
    }

    /// Create the backend's GPU view of the weights context arena. On Metal
    /// this wraps the host arena zero-copy; on CUDA it allocates device
    /// memory and uploads the weight region once.
    pub fn create_context_buffers(&self, ctx: &Context) -> Result<ExecContextBuffers> {
        self.create_context_buffers_with_progress(ctx, &mut |_, _| {})
    }

    pub fn create_context_buffers_with_progress(
        &self,
        ctx: &Context,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<ExecContextBuffers> {
        match self {
            Self::Metal(runtime) => Ok(ExecContextBuffers::Metal(
                create_metal_context_buffer_with_runtime(runtime, ctx)?,
            )),
            Self::Cuda(runtime) => Ok(ExecContextBuffers::Cuda(
                runtime.create_context_arena_with_progress(ctx, progress)?,
            )),
        }
    }

    /// Compile one hybrid-decode graph shape against the shared cache and
    /// shared buffers created by this runtime.
    #[allow(clippy::too_many_arguments)]
    pub fn compile_hybrid_decode(
        &self,
        weights: &mut LoadedGgufWeights,
        spec: &HybridDecodeSpec,
        shared_cache: &HybridSharedCacheTensorIds,
        shared_buffers: &ExecContextBuffers,
        n_tokens: usize,
        n_outputs: usize,
        attention_key_count: usize,
    ) -> Result<CompiledHybridDecode> {
        match (self, shared_buffers) {
            (Self::Metal(runtime), ExecContextBuffers::Metal(buffers)) => {
                Ok(CompiledHybridDecode::Metal(
                    compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count(
                        weights,
                        spec,
                        runtime,
                        shared_cache,
                        buffers,
                        n_tokens,
                        n_outputs,
                        attention_key_count,
                    )?,
                ))
            }
            (Self::Cuda(runtime), ExecContextBuffers::Cuda(arena)) => {
                Ok(CompiledHybridDecode::Cuda(runtime.compile_hybrid_decode(
                    weights,
                    spec,
                    shared_cache,
                    arena,
                    n_tokens,
                    n_outputs,
                    attention_key_count,
                )?))
            }
            _ => Err(LlamaError::format(
                "execution runtime and context buffers belong to different backends",
            )),
        }
    }

    /// Read back the current bytes of a state tensor (KV/recurrent cache)
    /// as the device sees them. Metal state lives in unified memory, so this
    /// is the host arena slice; CUDA downloads from the device arena.
    pub fn read_state_tensor_bytes(
        &self,
        ctx: &Context,
        buffers: &ExecContextBuffers,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>> {
        match (self, buffers) {
            (Self::Metal(_), ExecContextBuffers::Metal(_)) => Ok(ctx
                .data_at(offset, len)
                .map_err(LlamaError::format)?
                .to_vec()),
            (Self::Cuda(runtime), ExecContextBuffers::Cuda(arena)) => {
                runtime.read_arena_bytes(arena, offset, len)
            }
            _ => Err(LlamaError::format(
                "execution runtime and context buffers belong to different backends",
            )),
        }
    }

    /// Clear mutable state ranges in both the host context image and the
    /// backend buffer that compiled graphs reference. This is deliberately
    /// in-place: rebuilding a CUDA session while its old arena is resident
    /// transiently requires two full model allocations and cannot work on a
    /// card sized for one model.
    pub fn clear_state_ranges(
        &self,
        ctx: &mut Context,
        buffers: &ExecContextBuffers,
        ranges: &[(usize, usize)],
    ) -> Result<()> {
        for &(offset, len) in ranges {
            ctx.data_at_mut(offset, len)
                .map_err(LlamaError::format)?
                .fill(0);
        }

        match (self, buffers) {
            (Self::Metal(runtime), ExecContextBuffers::Metal(buffers)) => {
                const ZERO_CHUNK_BYTES: usize = 8 << 20;
                let max_len = ranges.iter().map(|(_, len)| *len).max().unwrap_or(0);
                let zeroes = vec![0u8; max_len.min(ZERO_CHUNK_BYTES)];
                for &(offset, len) in ranges {
                    let main_offset = offset.checked_sub(buffers.ro_split).ok_or_else(|| {
                        LlamaError::format(format!(
                            "refusing to clear read-only Metal state range [{}..+{}) below split {}",
                            offset, len, buffers.ro_split,
                        ))
                    })?;
                    let mut written = 0usize;
                    while written < len {
                        let chunk = (len - written).min(zeroes.len());
                        runtime
                            .write_buffer(
                                &buffers.main_buffer,
                                main_offset + written,
                                &zeroes[..chunk],
                            )
                            .map_err(LlamaError::format)?;
                        written += chunk;
                    }
                }
                runtime.wait_idle().map_err(LlamaError::format)
            }
            (Self::Cuda(runtime), ExecContextBuffers::Cuda(arena)) => {
                runtime.clear_arena_ranges(arena, ranges)
            }
            _ => Err(LlamaError::format(
                "execution runtime and context buffers belong to different backends",
            )),
        }
    }
}

impl CompiledHybridDecode {
    pub fn decode(&self) -> &HybridDecodeGraph {
        match self {
            Self::Metal(compiled) => compiled.decode(),
            Self::Cuda(compiled) => compiled.decode(),
        }
    }

    pub fn execute_logits_only_with_layout(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
    ) -> Result<HybridDecodeRun> {
        match self {
            Self::Metal(compiled) => compiled.execute_logits_only_with_layout(input, layout),
            Self::Cuda(compiled) => compiled.execute_logits_only_with_layout(input, layout),
        }
    }

    pub fn execute_with_layout(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
    ) -> Result<HybridDecodeRun> {
        match self {
            Self::Metal(compiled) => compiled.execute_with_layout(input, layout),
            Self::Cuda(compiled) => compiled.execute_with_layout(input, layout),
        }
    }
}
