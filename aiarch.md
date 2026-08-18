# AI model architecture — final state (plan of record)

2026-08-18. Supersedes the *sequence* section of `aicleanup.md` (Grok's crate
analysis stands; this file is the target we actually build). Scope = the
models the asset-ai system serves (registry.json, 30 entries) plus the llm
crate they share. Gemma, Apple-MLX, and everything only they used is dead.

Written to be executed **in one swoop**: every model has an automated gate
(existing validate bins + golden-output capture), run by cheap subagents
before and after. No behavioral change ships unverified, but we do not
tiptoe — moves, merges, and the loader/residency convergence land together.

---

## 1. Final crate tree

Everything model-related moves under `libs/ai/`. The names `ggml`, `mlx`,
`diffusion` disappear. `makepad-cuda` (driver) is absorbed into the CUDA
warehouse. One flat workspace — root membership preferred; keep it excluded
only if a root-build test shows the CUDA toolchain requirement is hostile.

```
libs/ai/
├── loader/        makepad-ai-loader    backend-neutral disk layer (no GPU deps)
│                    formats/safetensors.rs   ONE parser (union of mlx + pbr_paint + flux2)
│                    formats/gguf.rs          moved from llama (already the only one)
│                    formats/torch.rs         ONE zip+pickle reader (pbr_paint's, absorbs torch_pth + tts)
│                    formats/npy.rs           ONE tiny reader (4 copies today)
│                    weight_set.rs            WeightSet: mmap + tensor directory + shards +
│                                             prefix scope + row streaming + digest verify +
│                                             byte-progress + cancel
│                    quant.rs                 dtype tables, bf16/f16/fp8 views, NVFP4 meta
│                    mmap.rs                  MappedRegion (from ggml)
│
├── cuda/          makepad-ai-cuda      THE CUDA store (win/linux; absorbs libs/cuda)
│                    driver.rs                cudart/cublas/cublasLt/streams/graphs/events
│                    cudnn.rs                 conv paths (unchanged)
│                    kernels/                 all .cu, one nvcc build:
│                      ops.cu  diffusion_ops.cu  llm.cu(=llm/kernels.cu)  kquants.cu
│                      nvfp4.cu nvfp4_mmq.cu  gated_delta_net.cu  ssm_conv.cu
│                      paint.cu(=paint_extras) vision.cu(birefnet/sam3/realesrgan splits)
│                      [DELETED: qwen_ops.cu, affine.cu*]
│                    residency.rs             ONE WeightCache (merges the 3 thread-local caches)
│                                             + pool + OOM ladder + graph-capture pins + counters
│                    launch/                  cuda/mod.rs monolith (24.6k LOC) split by family:
│                      gemm.rs attention.rs conv.rs norm.rs rope.rs elementwise.rs shape.rs
│                      birefnet.rs sam3.rs realesrgan.rs paint.rs skintokens.rs llm_ops.rs
│
├── metal/         makepad-ai-metal     THE Metal store (macos)
│                    device.rs                ONE MTLDevice/queue/library owner
│                                             (merges compat.rs + runtime.rs duplicate stacks)
│                    shaders/                 ONE copy of ggml-metal.metal + impl headers
│                                             + model shim shaders  [voice2's fork deleted]
│                    residency.rs             named weight buffers + arena no-copy buffers
│                                             + NEW: evict_prefix/protect (parity with CUDA)
│                                             [pointer-keyed cache DELETED — it forced host copies]
│                    pool.rs                  transient pool + resident-activation machinery
│                                             (music3's laws, now shared infrastructure)
│                    shim.rs                  imperative ops (try_* + the gpu_tensor forwarders)
│
├── job/           makepad-ai-job       tiny; the ONE cross-model contract
│                    JobCtx { progress sink, cancel token }
│                    JobEvent { StageStart, Progress{stage,done,total}, Preview, Done, Error }
│
└── models/        one crate per family; each holds its OWN loading map and its
                   OWN execute paths. No shared runtime type. Where CUDA and
                   Metal genuinely mirror (small CV models) a #[cfg] alias picks
                   the store; where they don't (music3), execute_cuda.rs and
                   execute_metal.rs sit side by side and share nothing but kernels.
    llm/       makepad-ai-llm       ← libs/llama minus gemma4; keeps GGUF graph builders,
                                      exec.rs seam, cuda_exec/, Metal compiled-graph executor
                                      + selector + Context/Graph/plan (graph IR is llm-private;
                                      the graph IS this model — it borrows device+library from ai-metal)
    flux/      makepad-ai-flux      ← flux*, clip_l, t5_encoder, flux2*, tokenizers
                                      execute_device only; Compiled-Metal + Lazy modes DELETED
    h3/        makepad-ai-h3        ← h3* (bf16 / q4-gguf / nvfp4 tiers)
    trellis/   makepad-ai-trellis   ← trellis*
    music/     makepad-ai-music     ← music3* (CUDA + the live Metal port) + ace*
    sfx/       makepad-ai-sfx       ← sa3*, moss*, woosh*
    speech/    makepad-ai-speech    ← indextts*, kokoro (from libs/tts, incl. g2p + converter)
    vision/    makepad-ai-vision    ← sam3*, birefnet*, da3, realesrgan*
    rig/       makepad-ai-rig       ← skin_tokens*
    motion/    makepad-ai-motion    ← hy_motion*
    paint/     makepad-ai-paint     ← libs/pbr_paint
```

Control plane: `libs/asset/ai` (makepad-asset-ai) keeps its name and job —
registry, fleet, HTTP, python workers — and is UPDATED to consume the model
crates through `makepad-ai-job` plus per-target deps on the two stores for
capability probes and eviction. It never touches tensors.

Dependency arrows (strict):

```
asset-ai ──► job ──► models/* ──► cuda | metal (per target) ──► loader
                                   (stores never depend on models or loader-formats;
                                    loader depends on nothing GPU)
```

## 2. Model inventory — keep / chuck

KEEP (all 24 real backends serving the 30 registry entries):

| family crate | models (registry ids) | exec today | metal today |
|---|---|---|---|
| llm | qwen3.8-27b, qwen3.6-27b (GGUF, hybrid/MoE path — **qwen35moe in llama is LIVE**, verified via `hybrid_decode_spec` in session.rs) | Metal graph + CUDA exec | full |
| flux | flux1-schnell, flux1-dev (FP8 single-file), flux2-klein-4b | CUDA | none (modes deleted; future port is per-model gpu_* style) |
| h3 | minimax-h3 ×4 tiers | CUDA | none |
| trellis | trellis-2 | CUDA | none |
| music | minimax-music3, minimax-music3-q4, ace-step-1.5-xl | CUDA + **Metal (live, .162)** | music3-q4 yes |
| sfx | sa3-sfx, moss-sfx, woosh-sfx | CPU + CUDA-gated + Metal GEMM accel | accel-level |
| speech | kokoro, indextts-2.5 | CPU + Metal offload / per-component CUDA | kokoro yes |
| vision | sam3-1-multiplex, birefnet-hr, da3-metric-large, (+realesrgan — implemented, **register it** when the realesrgan-fable lane lands) | CUDA | sam3 shim best; birefnet partial |
| rig | skintokens | CUDA | none |
| motion | hy-motion | CUDA | none |
| paint | hunyuan3d-paint-2.1, pbr-testpattern | CUDA | none |
| (service) | testpattern, music3-python, flashworld, skintokens-oracle, hy-motion-oracle | CPU / python subprocess | n/a — untouched |

CHUCK (with receipts):

| what | size | why safe |
|---|---|---|
| libs/mlx — everything (runtimes, chat/KV, qwen35moe, layer0_cached_case, multimodal, 31 bins, cli, tools) | ~89k LOC + 4.3k tests | only 4 weight-I/O types (~700 LOC) have any consumer; they move to ai-loader. 26 files in diffusion need a 1-line import swap (3 also a fn-sig path swap) |
| qwen_ops.cu + its FFI/wrappers in cuda/mod.rs | 1,269 + ~1,000 | sole caller was mlx qwen runtime; removes an nvcc TU from every build |
| llama gemma4 (gemma4.rs, gemma4_runtime.rs, config) | — | no registry entry, no session reference |
| Backend trait family + runtime.rs + plan.rs in ggml | ~180 | zero impls, zero refs |
| diffusion/backend.rs re-export shim + backend/accel.rs | ~460 | 34 files re-point to the store directly (13 already do); asset-ai's 8 call sites go to the stores |
| flux Compiled-Metal + Lazy host modes | large | not live on .169; flux becomes fail-closed CUDA like flux2 |
| metal/affine.rs + metal/qmm.rs + mlx_qmm shaders; **cuda/affine.cu + CudaAffineBackend pending a call-site audit** | 628 + 945 + 2,535 | affine 4-bit path exists for mlx-style checkpoints; registry has no affine-quantized weights. Verify-then-delete during execution |
| libs/voice2 (unfinished whisper ggml-wrap + its own metallib fork) | 12.3k | zero dependents repo-wide. voice v1 stays as-is (live for converse; out of asset-ai scope) |
| duplicate parsers: 3 extra safetensors, 2 extra torch-pickle, 3 extra npy; sharded-dir wrapper ×2; 9 thin header wrappers; Flux-family loader quadruplicate (~1,020 LOC) | ~5k net | all callers move to ai-loader |
| host-retention paths: flux/t5/clip/vae host arenas (up to ~24 GB resident), pbr_paint f32 HashMaps (2× disk), music3 byte_cache + bf16 sidecar heap cache, metal gpu_tensor host-f32 global cache | — | replaced by WeightSet mmap views + device residency (§3); this is the heap-twin poison, removed at the root |
| stale: FleetQwen `qwen3.5-9b` preference entry (not in registry) | — | fix list or add entry — decide at execution |

## 3. The weight path — one architecture, per-backend optimized

The page-mapping and eviction machinery exists for real, measured reasons
(llama's zero-copy mmap; the FLUX 24 GB OOM ladder; music3's heap-twin churn).
The final state keeps every one of those wins and makes them THE path — same
concepts on both backends, separate implementations, no shared trait.

**Disk (ai-loader, shared):**
- `WeightSet::open(registry_entry)` → mmap every file once, parse directories
  into uniform `TensorMeta`. All reads are views into the mmap — file-backed
  clean pages, dropped by the OS for free. **No API exists that caches heap
  copies of weight bytes.** CPU transforms (bf16→f16 sidecar, NVFP4
  unswizzle, fp8 dequant-at-load for the one slow path) write into transient
  buffers that die after upload.
- Multi-file shards, prefix scoping, single-row streaming (embeddings /
  lm_head), sha256 verify, uniform byte-progress + cancel (pbr_paint's
  offset-sorted streaming plan generalized).

**Residency — two verbs, one cache, per store:**

| concept | ai-cuda | ai-metal |
|---|---|---|
| `ensure_arena(set)` — graph models | one contiguous device arena, 256 MiB-chunked async sweep from the mmap (llama's path, now shared) | `newBufferWithBytesNoCopy` over the mmap — zero upload, unified-memory page-in (llama-Metal's path, now shared) |
| `ensure(ns, name, ‖ view)` — imperative models | upload-once named buffer, packed quants verbatim (today's `gpu_weight_cache_ensure{,_quant}`) | upload-once named buffer (today's `named_weight_buffers`); pointer-keyed cache deleted |
| eviction | merge the 3 caches into one: prefix evict, protected prefixes, OOM ladder (pool → scratch → unprotected weights → retry), perf counters | **NEW**: same evict/protect API (today Metal leaks every prior model's buffers on switch) |
| scratch | pool + cap (6144 MB default) + graph-capture pins | pool + caps + resident-activation slots (music3's) |
| namespace switch | never auto-clears (dense-linear semantics win; the affine/ggml clear-on-switch just caused re-uploads) | same |

Model loading code shrinks to: name-map from `WeightSet` + `ensure_*` calls.
Warm second runs hit the named cache; model switches evict by prefix — the
exact calls asset-ai already makes, now on both backends.

**Capability probing:** `ai_cuda::available()` / `ai_metal::available()`,
and each model backend declares what it supports. This structurally fixes
the live bug where any Mac advertises flux/flux2/music3-CUDA as provisioned
(`compat::is_available()` == `cfg!(macos)` today) and fails at generate.

## 4. Kernel warehouses

One big store per backend, exactly as requested. Kernels are grouped in
model-named files where they are model-private (birefnet/sam3/realesrgan/
paint/skintokens) and family files where shared (gemm/attention/norm/...).
The launcher layer lives next to the kernels in the same crate. The
`mkllm_*` C symbols keep their names (no rename churn); they simply live in
the warehouse and are launched only from ai-llm. Metal: one metallib, one
device stack; llm's compiled-graph executor and every shim borrow it.

## 5. Per-model execution — the no-abstraction rule

- No `Runtime` trait, no graph IR for generators, no third tensor API.
- A model crate contains its own `execute_cuda.rs` / `execute_metal.rs`
  (or one body behind a `#[cfg]` store alias where the ops genuinely mirror,
  as the small CV models already prove out). Perf work stays hand-mapped
  per model per backend.
- The graph machinery (Context/Graph/plan/compiled-Metal executor) is
  llm-private: for the LLM the graph *is* the model.

## 6. The job contract (the one thing every model shares)

`makepad-ai-job`: `JobCtx { progress, cancel }` passed into every generate
and every load. Events: `StageStart(name)` / `Progress{stage, done, total}`
/ `Preview` / `Done` / `Error`. Cancel is a flag checked between denoise
steps, AR frames, decode chunks, and loader tensors. asset-ai's backend
trait, HTTP job routes (`/job/<id>`, `/cancel`), and the chat tools consume
this instead of today's per-backend ad-hoc progress strings. Load progress
("load unet 8.2/23.8GB") becomes uniform for free — the loader emits it.

## 7. Perf invariants (preserved by construction, asserted by the gate matrix)

- flux FP8: weights never expand in cache (24 GB tier depends on it); f32
  spine rules unchanged.
- music3-Metal: upload-once uncached loads, buffer pool, resident-layer
  decode, serial GQA — the 52.67 s fair-band laws.
- llama: mmap + no-copy (16384-padded arenas), 256 MiB chunked CUDA sweep,
  VRAM reserve gate, graph-capture scratch reservation.
- H3 staged residency namespaces; pool cap defaults; kokoro MIN_MACS gate.
- Load-time telemetry: keep PERF_WEIGHT_* counters; add per-model load-time
  to the gate matrix so the swoop cannot silently regress cold starts.

## 8. Migration — one swoop

1. **Freeze goldens** (cheap subagents): scripted generate per model on .169
   (CUDA set) and .162 (Metal set: music3-q4, sa3, woosh, kokoro, llm) +
   testpattern tiers. Record output hashes/cosines + load and generate
   wall-times. Python backends excluded (untouched).
2. **Moves** (compiler-verified): create `libs/ai/*`; lift the 4 loader
   types out of mlx; move gguf.rs; split the cuda monolith; merge the Metal
   stacks; dissolve diffusion into the family crates; absorb libs/cuda;
   delete the chuck list. Import swaps throughout (~40 files, mostly
   one-liners — exact list in the survey reports).
3. **Convergence** (behavioral, gated): WeightSet everywhere; unified
   residency + Metal eviction; host-retention deletions; capability probe
   fix; job contract in asset-ai.
4. **Re-run the gate matrix** (cheap subagents): outputs must match goldens
   (bit-exact where the model is deterministic, documented cosine bands
   where not); load times within noise or better. Per-model pass/fail table
   is the merge gate.
5. Land on rik2 as a commit series: moves separated from edits.

## 8b. Execution staging (2026-08-18, this swoop)

To keep every lane compile-gated, the swoop lands the structure in two
rings. Ring 1 (this pass, before the user's first model test): loader
crate + all parsers moved; dead code deleted; ai-cuda = driver + all .cu
kernels + nvcc build (ggml keeps the Rust launch surface behind its
existing cfg, fed by a links-metadata handshake); ai-metal = shaders +
metallib build (Rust metal modules likewise stay in ggml); model family
crates split out; asset-ai re-pointed. Ring 2 (post-test refinement):
move the Rust launch surfaces into the stores and split the cuda monolith
by family, merge the two Metal device stacks, merge the three CUDA weight
caches, WeightSet adoption + host-retention deletion, Metal eviction,
flux Compiled/Lazy deletion + affine verify-delete, job-contract wiring,
final ggml dissolution.

## 9. Out of scope

python worker scripts, asset-store/importer/chat surfaces (except the job
contract adoption), libs/voice (converse's whisper), tts users outside
asset-ai (converse/route keep working — kokoro's crate move keeps its API).
