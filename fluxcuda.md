# FLUX CUDA Plan

Date: 2026-04-13

## Goal

Primary goal: make `makepad/libs/diffusion` on Linux + CUDA match the user's ComfyUI FLUX performance on this machine.

Current target from the user's observed ComfyUI run:

- about `0.6 s/step` in KSampler

Important constraint:

- matching `stable-diffusion.cpp` is not enough, because the measured CUDA C++ path here is still much slower than ComfyUI

## Verified Current State

### Assets and repos

- Makepad repo: `/home/playe/makepad`
- Diffusion crate: `/home/playe/makepad/libs/diffusion`
- Copied model root: `/home/playe/comfy/models`
- Stable Diffusion C++ repo: `/home/playe/stable-diffusion.cpp`
- Stable Diffusion C++ submodules were missing initially and were checked out with `git submodule update --init --recursive`

Verified model files used by the FLUX workflow:

- `/home/playe/comfy/models/unet/flux1-dev.safetensors`
- `/home/playe/comfy/models/vae/ae.safetensors`
- `/home/playe/comfy/models/text_encoders/clip_l.safetensors`
- `/home/playe/comfy/models/text_encoders/t5xxl_fp16.safetensors`

### End-to-end image generation status

- Rust `flux-smoke` can load the bundle layout
- Rust `flux-generate` does not run on Linux because the current pipeline hard-depends on Metal
- A reference CUDA image was generated through the `stable-diffusion.cpp` path:
  - output: `/tmp/flux-ref-test.png`

### Measured baselines

#### ComfyUI

- User-reported target: about `0.6 s/step`
- This is the performance bar that matters

#### Stable Diffusion C++ CUDA baseline

Measured with the Makepad wrapper on this machine:

- resolution: `256x256`
- steps: `4`
- text conditioning: `2.22s`
- sampling: `7.78s` total, about `1.94 s/step`
- total `generate_image`: `11.60s`
- model load: `28.51s`
- VRAM reported by the binary: `32119.43MB`

Conclusion:

- this path is useful as a CUDA correctness oracle
- it is not the performance target, because it is still far slower than ComfyUI

#### Native Rust baseline

`flux-generate` on Linux currently fails with:

```text
metal runtime init failed: Metal runtime is only available on macOS in this port
```

Lazy T5 was also tested separately:

- command used `FLUX_T5_MODE=lazy`
- GPU compute did not show up in `nvidia-smi`
- process stayed CPU-bound and was killed after about `105s`
- timing at kill:
  - elapsed `105.25s`
  - user `19.35s`
  - sys `86.42s`
  - max RSS about `9.8 GB`

Conclusion:

- even the "lazy" Linux path is still mostly CPU execution today

## Relevant Code Paths

### Makepad / Rust

Pipeline entry points and modules:

- `libs/diffusion/src/bin/flux_generate.rs`
- `libs/diffusion/src/flux_pipeline.rs`
- `libs/diffusion/src/flux_text.rs`
- `libs/diffusion/src/clip_l.rs`
- `libs/diffusion/src/flux_transformer.rs`
- `libs/diffusion/src/flux_vae.rs`
- `libs/diffusion/src/t5_encoder.rs`

Backend state:

- `libs/ggml/src/backend/mod.rs`
  - `graph_compute` is still a stub and returns failure
- `libs/ggml/src/backend/metal/compiled.rs`
  - only real compiled graph/session implementation
- `libs/ggml/src/backend/metal/compat.rs`
  - on non-macOS, many helpers return `None`
- `libs/ggml/src/backend/cuda/mod.rs`
  - already contains useful CUDA kernels and wrappers

### ComfyUI / Python

Actual ComfyUI source tree on this machine:

- `/mnt/c/Users/playe/AppData/Local/Programs/ComfyUI/resources/ComfyUI`

KSampler path:

- `nodes.py`
  - `common_ksampler(...)`
  - `KSampler`
- `comfy/sample.py`
- `comfy/samplers.py`
  - `KSAMPLER.sample(...)`
  - `ksampler(...)`
  - runtime `KSampler`
- `comfy/k_diffusion/sampling.py`
  - `sample_euler(...)`
  - `sample_euler_ancestral(...)`

Important note:

- `S&R KSampler` is not a different implementation
- `S&R` is workflow metadata ("search and replace"), not a sampler backend

FLUX and attention path:

- `comfy/model_base.py`
  - `BaseModel.apply_model(...)`
  - `BaseModel._apply_model(...)`
- `comfy/ops.py`
  - `scaled_dot_product_attention(...)` delegates to `torch.nn.functional.scaled_dot_product_attention`
  - `pick_operations(...)` selects optimized ops, including fp8-related paths
- `comfy/ldm/modules/attention.py`
  - can use PyTorch SDPA, xformers, or flash-attn paths
- `comfy/ldm/flux/model.py`
- `comfy/ldm/flux/layers.py`

## Main Findings

### 1. KSampler itself is not the performance bottleneck

The actual Euler loop in ComfyUI is simple:

- per step it does `denoised = model(...)`
- then applies the Euler update

That means the real gap is in the FLUX model call and the attention / MLP / norm implementation, not in the sampler wrapper.

### 2. The Rust FLUX pipeline is currently Metal-only

These modules import Metal runtime and compiled graph types directly:

- `clip_l.rs`
- `flux_transformer.rs`
- `flux_vae.rs`
- `flux_pipeline.rs`
- `flux_text.rs`
- `bin/flux_generate.rs`

They use `MetalRuntime`, `prepare_graph(...)`, and `MetalGraphSession`.

Practical consequence:

- Linux/CUDA cannot run the main FLUX denoiser + VAE path today

### 3. There is no generic Linux/CUDA graph executor yet

`libs/ggml/src/backend/mod.rs` still has:

- `graph_plan_create(...)` -> not implemented
- `graph_plan_compute(...)` -> failed
- `graph_compute(...)` -> failed

Only the Metal backend currently has a real compiled graph/session flow.

Practical consequence:

- this is not just "wire up CUDA"
- the current end-to-end execution model is missing on Linux

### 4. Lazy T5 only partially uses acceleration on Linux

In `t5_encoder.rs`:

- quantized linear projections can route through `backend::try_matmul_nt_ggml_bytes(...)`
- that path can already use CUDA through `backend/accel.rs`

But the lazy path also uses Metal compat helpers for many other heavy operations:

- `try_matmul_nt_f32`
- `try_matmul_nn_f32`
- `try_add_f32`
- `try_mul_f32`
- `try_gelu_f32`
- `try_rms_norm_mul_f32`

On non-macOS, those helpers currently return `None` in `backend/metal/compat.rs`, so execution falls back to CPU.

Practical consequence:

- text encoding is still dominated by CPU fallback even though some CUDA pieces already exist

### 5. Makepad already has several CUDA kernels that can be reused

Existing CUDA support already includes kernels/wrappers for:

- `add_f32`
- `rms_norm_rows_weighted_f32`
- `softmax_rows_f32`
- `attention_softmax_weighted_sum_f32`
- `attention_softmax_weighted_sum_f32_device_u32`
- quantized matmul entry points used by `backend/accel.rs`

This is enough to justify a first Linux/CUDA acceleration pass for the text stack without waiting for the whole FLUX graph path.

### 6. ComfyUI speed likely comes from optimized Torch execution, not sampler logic

ComfyUI's hot path uses:

- PyTorch model execution
- `torch.nn.functional.scaled_dot_product_attention`
- optional xformers / flash-attn paths
- dtype / device selection in `comfy.ops.pick_operations(...)`
- model/device management in `model_base.py` and related helpers

Practical consequence:

- reaching ComfyUI parity will probably require a CUDA-native hot path with persistent device residency and fused or near-fused ops
- a literal "Metal API port to CUDA" may functionally work but still miss the performance target

### 7. Current dtype policy is inconsistent and likely leaves performance on the table

Concrete examples from the current Rust code:

- `clip_l.rs`
  - `clip_target_tensor_type(...)` widens `F16`, `BF16`, and `F32` weights to `TensorType::F32`
- `flux_vae.rs`
  - `vae_target_tensor_type(...)` currently only accepts `TensorType::F32`
- `t5_encoder.rs`
  - preserves `F16` / `BF16` for non-rank1 weights
- `flux_transformer.rs`
  - preserves `F16` / `BF16` for non-rank1 weights unless `FLUX_FORCE_F32_WEIGHTS` is set
- `clip_l.rs` and `flux_transformer.rs`
  - both force flash attention precision to `Prec::F32`

Practical consequence:

- Makepad is not yet applying a consistent mixed-precision policy across the FLUX stack
- this is likely part of the gap versus ComfyUI, which selects ops and dtypes more dynamically

## What Also Applies To Metal

Short answer:

- yes, some of the ComfyUI findings should influence the Metal backend too
- no, the sampler wrapper itself does not need any special porting

### Things that do not need porting

- `KSampler` wrapper structure
- `S&R` workflow metadata

Those are not where the performance comes from.

### Things Metal already has

- compiled graph/session reuse
- flash-attention-style kernels in the Metal backend
- BF16 / F16 support in parts of the backend and model loaders

This means Metal is not missing the same basic primitives that Linux/CUDA is missing.

### Things that likely should be aligned on Metal too

- a backend-neutral dtype policy closer to ComfyUI's `pick_operations(...)`
- benchmarking whether forced `Prec::F32` attention is actually necessary
- reducing unnecessary F32 widening, especially in `clip_l.rs`
- checking whether `BufferStorageMode::Shared` is leaving performance on the table for GPU-only long-lived buffers
- keeping per-stage timing and profiling first-class on Metal as well as CUDA

### Recommendation

Treat the useful ComfyUI findings as backend-neutral optimization guidance:

- best available attention path
- best available dtype / mixed-precision policy
- persistent device residency
- low transfer overhead

That guidance should be applied to both CUDA and Metal, even though the concrete kernel implementations will differ.

## Recommended Plan

### Phase 0: Build an apples-to-apples benchmark harness

Purpose:

- remove ambiguity about where time is going
- compare the same workflow across ComfyUI, Rust, and `stable-diffusion.cpp`

Work:

- lock one prompt, one seed, one sampler, one step count, one resolution
- record separate timings for:
  - CLIP
  - T5
  - transformer per step
  - VAE decode
  - total wall clock
- add a small benchmark/report script or markdown table in the repo

Acceptance:

- a single benchmark page shows ComfyUI, Rust, and C++ numbers for the same run

### Phase 1: Make the Rust text stack actually use CUDA on Linux

Purpose:

- get the first substantial win with the smallest architectural change
- eliminate the current CPU-bound lazy T5 path

Work:

- add Linux/CUDA implementations for the operations currently routed through `backend/metal/compat.rs`
- either:
  - introduce a backend-neutral compat facade and implement CUDA under it, or
  - add a CUDA compat module and stop importing these helpers from `metal`
- cover at least:
  - `matmul_nt_f32`
  - `matmul_nn_f32`
  - `add`
  - `mul`
  - `gelu`
  - `rms_norm_mul`
- update `t5_encoder.rs` and `clip_l.rs` so Linux does not silently drop to CPU for the hot path

Acceptance:

- `flux-t5-smoke` shows sustained GPU activity
- CPU fallback is removed from the major lazy T5 blocks
- text conditioning time drops sharply from the current CPU-bound baseline

### Phase 2: Add a CUDA-native FLUX denoiser + VAE execution path

Recommendation:

- do not start by trying to genericize the entire Metal compiled graph stack
- build a CUDA-native execution path for the FLUX modules that matter first

Reason:

- the current compiled graph/session implementation is deeply Metal-specific
- a backend-agnostic graph rewrite is larger, riskier, and not obviously the fastest route to ComfyUI parity

Work:

- introduce runtime/backend selection above the current Metal-only pipeline
- make FLUX weights resident on the CUDA device across all steps
- create a CUDA execution path for:
  - FLUX transformer denoise step
  - VAE decode
- minimize host/device round-trips
- reuse per-step allocations and any graph/kernel launch structure that can be cached

Acceptance:

- Rust can generate a FLUX image end-to-end on Linux/CUDA
- denoiser and VAE stay on GPU for the full inference path

### Phase 3: Close the gap to ComfyUI

Purpose:

- move from "working on CUDA" to "matching ComfyUI"

Focus areas:

- fused attention path comparable to SDPA / flash attention behavior
- fused or reduced-overhead RMSNorm + modulation + MLP blocks
- BF16 / FP16 execution where numerically safe
- persistent CUDA buffers and scratch reuse
- fewer host synchronizations
- profile-driven kernel work instead of blanket rewrites
- carry the same dtype / attention-path improvements back to Metal so the backends do not diverge unnecessarily

Acceptance:

- per-step time is within striking distance of the user's ComfyUI run on the same workflow
- final target remains about `0.6 s/step` on this machine

### Phase 4: Correctness and regression protection

Purpose:

- avoid chasing speed with silent numerical drift

Work:

- keep `stable-diffusion.cpp` as a CUDA correctness oracle
- compare intermediate tensors where practical:
  - text embeddings
  - denoiser outputs on selected steps
  - final latent / decoded image stats
- add smoke tests for:
  - model loading
  - text encoding
  - single denoise step
  - VAE decode

Acceptance:

- each major CUDA optimization can be checked against a known-good output path

## Immediate Next Actions

1. Add a benchmark document or harness that measures ComfyUI, Rust, and C++ with the same settings.
2. Implement the missing Linux/CUDA compat ops used by lazy T5 and CLIP.
3. Refactor the diffusion pipeline to stop importing Metal types directly at the top level.
4. Prototype the FLUX transformer CUDA path with persistent device-side weights and buffers.
5. Use `stable-diffusion.cpp` only for correctness checks, not as the performance finish line.

## Risks

- The current architecture may need a real backend split, not a few conditional branches.
- A generic graph backend may still be needed later, even if it is not the fastest first move.
- ComfyUI may be benefiting from xformers, flash-attn, fp8 paths, or PyTorch kernel fusion that the current Rust stack does not yet mirror.
- The exact `0.6 s/step` target should be re-measured with the same workflow and warm/cold-start conditions before locking acceptance numbers.

## Bottom Line

The main issue is not KSampler. The main issue is that ComfyUI executes the FLUX model through highly optimized Torch attention and dtype/device paths, while the Rust codebase is still structurally tied to a Metal-only execution model and falls back to CPU on Linux for important text operations.

The fastest practical route is:

1. benchmark cleanly,
2. eliminate CPU fallback in the text stack,
3. add a CUDA-native FLUX denoiser/VAE path,
4. then optimize against ComfyUI's measured per-step time.
