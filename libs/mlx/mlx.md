# MLX CUDA Cleanup Plan

## Goal

Add CUDA support for `makepad/libs/mlx` without turning `mlx` itself into a backend-specific module.

The intended split is:

- `mlx`: model semantics, tensor naming, safetensors loading, Gemma runtime logic, prompt/generation flow.
- `ggml`: optional accelerated kernels and backend selection.
- `cuda`: minimal Rust FFI for the CUDA APIs required by the `ggml` CUDA path.

## Hard Constraints

This cleanup is not allowed to change the performance profile of the working paths while abstracting.

That means:

- no extra host-side buffer copies
- no extra device-side staging buffers unless the current path already requires them
- no extra tensor materialization caused only by the abstraction layer
- no extra synchronization points inserted just to satisfy a cleaner interface
- no change to cache/reuse behavior for uploaded weights, pipelines, or work buffers
- no change to rounding behavior or operation ordering in the math kernels
- no hidden fallback from a fast path to a slower generic path just because the API became cleaner

For this refactor, abstraction must be a structural change, not a behavioral change.

The backend-facing API must therefore be designed around:

- borrowed slices or existing buffers where possible
- lazy tensor-byte loaders instead of eager tensor reads
- stable cache keys for reusable uploaded weights
- preserving the current kernel math exactly for existing backends

## Current Problems

1. `mlx/src/text_runtime.rs` currently contains a large `MlxAffineMetalBackend` implementation.
   This is backend code living in the frontend/runtime layer.

2. The current acceleration hook is Metal-specific.
   The reference path calls `try_affine_quantized_matmul_tensor_metal(...)`, which leaks backend choice into `mlx`.

3. `GemmaTextRuntimeSession::load()` currently tries to build the exact Metal backend whenever the model shape matches.
   On Linux/CUDA, that can fail before the reference path even starts.

4. The exact backend path and the reference path are mixed together in a way that makes Linux support harder than it needs to be.

5. The local validation model is `gemma4-31b-nvfp4-mlx`, not an affine `group_size=64` checkpoint.
   That means affine CUDA cleanup is necessary but not sufficient for end-to-end validation on the model we actually have on disk.

## Target Architecture

### 1. Keep `mlx` backend-agnostic

`mlx` should only know that an optional accelerator exists for:

- affine packed BF16 input x U32 packed weights x BF16 scales/biases
- rank-2 and rank-3-plane variants

It should not know whether that accelerator is Metal or CUDA.

### 2. Put the accel boundary in `ggml`

Add a small backend-neutral API in `makepad/libs/ggml` for the MLX affine packed matmul case.

Suggested shape:

- `makepad_ggml::backend::AffineQuantizedMatmulSpec`
- `makepad_ggml::backend::try_affine_quantized_matmul_bf16(...)`

Important detail:

- this API should accept cache keys plus lazy byte loaders, not eagerly loaded tensor bytes
- that preserves the current "load once, upload once, reuse many times" behavior
- it must not introduce new copies just because the call site moved from `mlx` to `ggml`

### 3. Put CUDA-specific code under `ggml/backend/cuda`

`ggml/backend/cuda` should own:

- CUDA runtime/context setup
- device buffer caching
- input/output staging buffers
- the CUDA kernel wrapper for affine packed qmv

This is the right layer for backend policy and reuse.

### 4. Keep `libs/cuda` very small

`makepad/libs/cuda` should only contain:

- `extern "C"` bindings for the CUDA runtime APIs we need
- small safe wrappers for status handling, streams, malloc/free, memcpy

It should not contain MLX logic.

## Refactor Plan

### Phase 1. Fix the abstraction boundary

1. Add a backend-neutral affine accel API in `ggml`.
2. Move the current Metal affine qmv helper out of `mlx/src/text_runtime.rs`.
3. Replace the MLX Metal-specific hooks with calls into the new `ggml` accel API.
4. Keep the reference math in `mlx` unchanged.
5. Preserve current buffer ownership, upload reuse, dispatch shape, and math ordering exactly.

Result:

- `mlx` stops choosing Metal directly
- backend selection happens below the frontend layer

### Phase 2. Make Linux load cleanly

1. Gate `supports_exact_backend()` so it only enables the exact Metal path when Metal is actually available.
2. Ensure Linux goes through the reference path by default.
3. Let the reference path pick up CUDA acceleration only through the new `ggml` hook.

Result:

- Linux/CUDA works without needing the exact Metal runtime
- Metal remains untouched for macOS

### Phase 3. Add CUDA qmv support

1. Add `ggml/backend/cuda`.
2. Implement a cached CUDA context for the affine qmv case.
3. Compile a small `.cu` translation unit from `ggml/build.rs`.
4. Support:
   - BF16 inputs
   - 4-bit affine packed weights
   - 8-bit affine packed weights
   - group size 64

Result:

- the current MLX reference path gets a fast backend on Linux
- no CUDA code has to leak back into `mlx`

### Phase 3b. Handle the local NVFP4 checkpoint

The local model used for validation is `mode=nvfp4` with `group_size=16`.

That requires separate work from the affine cleanup:

1. Relax MLX model validation so `nvfp4` checkpoints are accepted.
2. Add or reuse NVFP4 tensor math paths in the MLX/reference layer.
3. Add CUDA NVFP4 qmv support by following the existing llama.cpp/ggml `modelopt` path.
4. Keep the affine and NVFP4 paths structurally separate so one does not distort the other.

Result:

- the codebase can validate and run the checkpoint we actually have
- CUDA verification can be done on the real target model instead of a format mismatch

### Phase 4. Clean up file ownership in `mlx`

Split responsibilities more clearly:

- `text_runtime.rs`
  only high-level wiring and shared imports
- `text_runtime/reference.rs`
  pure reference math and accel hook call sites
- `text_runtime/api.rs`
  public runtime/generation API
- optional future file: `text_runtime/backend.rs`
  only if a tiny frontend-facing backend bridge is still needed after the `ggml` move

The preferred outcome is to avoid a new MLX backend layer if `ggml` already provides a clean boundary.

### Phase 5. CUDA device-resident decode path

The current CUDA reference acceleration path is useful for bring-up, but it still pays too much per-op CPU overhead:

- repeated host-side activation quantization
- repeated host-to-device uploads for the same layer input
- repeated device-to-host readbacks between projections and elementwise ops
- no persistent per-layer workspace for decode

That means matvec kernel work alone will not be enough to close the gap with `llama.cpp`.

The next serious CUDA phase should follow the same broad idea that made the Metal fast path competitive:

1. preallocate persistent per-layer CUDA work buffers
2. concatenate hot projections that share an input:
   - q/k/v
   - gate/up
3. keep decode intermediates resident on device across the whole layer step
4. move simple elementwise stages onto CUDA as part of the same layer execution path:
   - RMS norm
   - rope application
   - residual/add
   - GeGLU
   - final norm / logits path as needed
5. only read back what the frontend actually needs

For CUDA, the target should be a non-Metal exact/runtime path or a thin backend-runtime layer under `ggml`, not more one-off backend branches in `mlx`.

This phase must still obey the same invariants:

- no extra copies introduced by the abstraction
- no changed math ordering relative to the chosen fast path
- no forced fallback to a slower reference path because of cleaner layering

## Concrete Work Items

1. Remove `MlxAffineMetalBackend` from `mlx/src/text_runtime.rs`.
2. Add `ggml` affine accel abstraction with lazy loaders and cache keys.
3. Add `ggml/backend/cuda` for the affine qmv path.
4. Gate exact backend creation to Metal-capable targets only.
5. Keep `try_matmul_nt_ggml_bytes(...)` and the dense/reference code paths untouched unless needed.
6. Treat performance parity and math parity as blocking requirements for the abstraction itself.
7. Add an apples-to-apples decode benchmark mode so CUDA can be compared against `llama-bench` without sampler overhead.
8. Verify:
   - macOS Metal path still builds
   - Linux loads the model without trying Metal
   - CUDA qmv path works on the local RTX 5090
   - CUDA sampled and greedy decode measurements are both understood
   - the next bottleneck after matvec parity is measured, not guessed

## Verification Requirements

Abstraction work is only acceptable if we verify both structure and invariants:

- same kernel launch geometry or equivalent backend dispatch shape for preserved paths
- same persistent/reusable buffer behavior for hot weights and scratch buffers
- no increase in copy count along the hot path
- same output bits, or the exact same BF16-rounded math where bitwise identity is not otherwise possible
- no regression in prompt prefill or decode throughput attributable to the abstraction layer

## Non-Goals For The First Pass

- Port the full exact Metal runtime to CUDA.
- Rebuild all `layer0_cached_case` kernels on CUDA immediately.
- Generalize all of `ggml` CUDA at once.

First pass should focus on the smallest clean abstraction that gets Linux/CUDA inference moving through the reference path.

## Success Criteria

- `mlx` no longer contains backend-specific affine matmul implementation code.
- Linux does not attempt to instantiate the exact Metal backend.
- CUDA acceleration is selected below `mlx`.
- Metal behavior is preserved on macOS.
- The new structure makes later CUDA work on the exact runtime possible without another cleanup pass first.
