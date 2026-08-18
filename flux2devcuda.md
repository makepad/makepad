# FLUX.2-dev text-to-image — Fable lane record

Owner: Fable. Box: 10.0.0.217 (RTX 5090 32GB, sm_120a, CUDA 13.2, 63GB RAM).
Branch `flux2-dev-fable` (worktree `.grok/worktrees/makepad-makepad/flux2-dev-fable`).
Klein edit lane (flux2cuda.md, branch flux2-klein-fable) is the parent pattern —
this lane adds the guidance-distilled 32B dev model as t2i, fp8, on a 32GB card.

## The official recipe (pinned 2026-08-18)

ComfyUI 0.22.0 desktop venv (torch 2.10.0+cu130, python 3.12.11), headless
`--port 8288`, models hardlinked from `C:\ai\flux2dev\`:

- DiT: `Comfy-Org/flux2-dev` `split_files/diffusion_models/flux2_dev_fp8mixed.safetensors`
  (35,455,599,592 bytes, 555 tensors: double mlps + single linear1/linear2
  F8_E4M3 with scalar f32 `weight_scale` + `input_scale` siblings, attention
  qkv/proj + norms + globals BF16)
- TE: `split_files/text_encoders/mistral_3_small_flux2_fp8.safetensors`
  (18,034,640,095 bytes, 693 tensors — Mistral-Small-3.2-24B PRUNED to the
  30 layers the [10,20,30] hidden-state taps need, `model.layers.N.*` names,
  all 7 projections/layer F8_E4M3 + per-tensor scales, embed + norms BF16)
- VAE: `split_files/vae/flux2-vae.safetensors` (336,213,556 bytes)
- Workflow (= the shipped `image_flux2_fp8` / `image_flux2_text_to_image`
  templates, t2i wiring): UNETLoader(default) → CLIPTextEncode →
  FluxGuidance **4.0** → BasicGuider; RandomNoise(seed 7) + KSamplerSelect
  **euler** + **Flux2Scheduler(20 steps, 1024x1024)** + EmptyFlux2LatentImage
  → SamplerCustomAdvanced → VAEDecode → SaveImage. Turbo-LoRA switch OFF.

Fixed prompt (everything in this lane uses it):

> A weathered lighthouse keeper's cottage on a basalt cliff at golden hour:
> whitewashed stone walls streaked with salt, a red tin roof, warm lamplight
> glowing in one round window, gulls circling the rusted lantern tower, waves
> bursting into spray on black rocks below, long shadows across wind-bent
> grass, thin volumetric sea mist, shot on 35mm film with fine grain.

## Official numbers (the bar)

Instrumented via `flux2_oracle_hook.py` custom node (env-gated, patches
`Flux.forward` / `VAE.decode` / `CLIP.encode_from_tokens_scheduled` with
cuda-synced walls; inert without `FLUX2_TIMING_FILE`/`FLUX2_ORACLE_DIR`):

| stage | official ms |
|---|---|
| TE encode (cold, 118 tokens) | 5466 |
| DiT step warm (median of 20) | 947 |
| denoise 20 steps warm | 19160 |
| VAE decode warm | 294–297 (cold 703) |
| **warm e2e wall (new seed, TE cached)** | **20122–21640** |
| cold e2e wall (first run incl. staging) | 47126 |

Warm = 2nd+ run on a resident server, same prompt (Comfy caches the
conditioning — a same-seed rerun is a full no-op, hence "new seed"). Comfy
stages 33,813MB for dynamic VRAM loading and streams weights per step with
2 async offload streams + 25GB pinned host pool.

**The official DiT step runs true fp8 tensor-core GEMMs**: the mixed-precision
loader wraps fp8 weights as QuantizedTensors, quantizes ACTIVATIONS to fp8
with the static `input_scale`, and `F.linear` dispatches to
`torch._scaled_mm` (per-tensor scales applied post-accumulate). ~947ms/step
at 1024² ≈ 307 effective TFLOPS — above the 5090's bf16 peak; only the fp8
path explains it. `comfy_quant` metadata carries no full-precision overrides.

## Reference semantics pinned from comfy source (0.22.0)

- Tokenizer: Tekken via `<s>` start token + template
  `[SYSTEM_PROMPT]{system}[/SYSTEM_PROMPT][INST]{prompt}[/INST]` — same
  system message as diffusers (the mid-sentence `\n` included). Seed-7 run:
  118 ids `[1, 17, … , 4]`.
- **No 512-token padding** (unlike diffusers): batch-1 window is exactly the
  real ids, TE attention is plain causal. `model_base.Flux2.extra_conds`
  then zero-LEFT-pads the conditioning ROWS to 512 before the DiT; txt_ids
  are arange(512) on the L axis; no text attention mask flows in t2i. The
  (512-L) zero rows attend with logit 0 — deterministic, reproduced.
- TE taps `layer=[10,20,30]`, `layer_norm_hidden_state=False`, pruned file =
  30 layers with `final_norm=False`; taps stacked feature-wise → (·, 15360).
- Latent: comfy Flux2 latent format is the PACKED 128ch/16x space,
  process_in = identity → the model x at sigma 1.0 IS the seed's noise.
- Guidance: `vec = time_in(temb) + guidance_in(timestep_embedding(g, 256))`,
  g = 4.0 (FluxGuidance), same 1000× time_factor as timesteps.
- Schedule: comfy `Flux2Scheduler` = the BFL empirical-mu formula already
  pinned in `flux2.rs::flux2_schedule` (oracle sigmas: 1.0, 0.99419,
  0.98781, … 0.3216, 0).

## Oracle dumps (`C:\ai\flux2dev\oracle\`, seed 7)

`context.npy` (1,512,15360) — DiT-side padded conditioning;
`te_cond.npy` (1,118,15360) unpadded; `te_ids.json`; `stepNNN_{x,pred}.npy`
(1,128,64,64) for all 20 steps (step000_x = the noise); `vae_in.npy` final
latent; `vae_out.npy` (1,1024,1024,3) decoded [0,1]; `oracle.png`
(SaveImage; byte-identical across seed-7 reruns — deterministic oracle).

## Native port (commit b930c65a0)

- `flux2_transformer.rs`: guidance_in MLPEmbedder wired (bf16-grid sum into
  the time vector, `gpu_add_bf16`), persistent-state path carries the
  guidance embedding; fp8-aware linear helpers — F8_E4M3 tensors stay
  1-byte resident in the weight cache (`::f8` keys), per-tensor
  `weight_scale` rides the f32 GEMM alpha (post-accumulate, the same
  structure as `_scaled_mm`'s epilogue).
- `ggml cuda mod.rs`: `gpu_linear_nt_cached_f8_mm{,_from_buf,_from_buf_to_buf}`
  — dequant into pooled bf16 scratch per call (existing exact e4m3→bf16
  kernel) + cublas bf16 GEMM f32-accumulate with alpha=scale.
- `flux2_dev_text.rs`: Mistral3 30-layer forward over the Comfy fp8 single
  file (no qk-norm, theta 1e9, GQA 32/8, SwiGLU 32768, eps 1e-5), UNPADDED
  causal window, fp8-resident projections via the f8 mm family,
  **evict-per-layer streaming** (weights are single-use per encode; peak
  ~1.1GB so encode works beside the resident DiT).
  `MAKEPAD_FLUX2_TE_RESIDENT=1` keeps layers cached.
- `flux2_pipeline.rs`: `Flux2DevPipeline` t2i — unpadded tokenize →
  TE → zero-left-pad conditioning to 512 → euler over
  `flux2_schedule(steps, gen_tokens)` with guidance → VAE decode → PNG.
  Conditioning cached per prompt (comfy-node-cache parity: warm same-prompt
  generates never re-run the TE).
- `flux2-dev-validate` bin: gates ids exact / TE cosine / step-0 pred
  (teacher noise + teacher context) / final latent / decoded PNG vs the
  oracle dumps + warm e2e vs the 20117ms official wall.
- asset-ai: registry id **`flux2-dev`** (backend `flux2`), files pinned to
  Comfy-Org revision + the UNGATED `mistralai/Mistral-Small-3.1-24B-Instruct-2503`
  `tokenizer.json` (byte-identical to the gated BFL tokenizer, 17,078,037
  bytes, verified by size+content against the BFL bundle download).
  `GenerateParams.guidance` became Option<f32> (per-model defaults: flux1
  3.5, flux2-dev 4.0).

## Results

First native pass (2026-08-18, 5090, `own_te=false`, seed 7, 1024² / 20 steps):

| gate | native | official | pass |
|---|---|---|---|
| input_ids | 118 | 117 | no |
| TE cosine | 0.999617 | — | no (max_abs 3.5) |
| step0 pred cosine | 0.999809 | — | yes-ish (max_abs 0.219) |
| final latents | cosine 0.939 / max_abs 5.47 | — | no |
| decoded image | cosine 0.9869 / u8_max 252 | — | no |
| denoise warm | 18870 ms | 19160 ms | yes |
| VAE decode warm | 2611 ms | 295 ms | no |
| warm e2e | 21734 ms | 20117 ms | no (`failed_gates=4`) |

Denoise already under official. VAE decode is the e2e hole (~9×). Dump lock is not held (latent cosine 0.94). PNGs at `local/flux2dev_{oracle,native}.png` — same scene, not pixel-locked.

Parked 2026-08-18 to land the port and refactor `libs/ai`. Resume: close dump lock + VAE decode, then re-run `flux2-dev-validate` until `failed_gates=0` and warm e2e ≤ 20117 ms.

## Open / next

- Dump lock: tokenizer off-by-one (118 vs 117) and latent drift after step 0.
- VAE decode ~2.6 s vs official 0.30 s (release ring slots first — done in the follow-up commit; still not enough).
- Do not restart the fp8 scaled_mm / stream-ring work; that is in.

## Box facts / gotchas

- `C:\ai\flux2dev\` = weights + oracle + logs; `C:\ai\flux2devbuild\` =
  isolated source tree + target (never share the music3/klein clones).
- ComfyUI desktop is installed for the user; the lane runs its OWN headless
  server on port 8288 and kills ONLY processes whose command line matches
  `port 8288`. The hook custom node in `Documents\ComfyUI\custom_nodes\`
  is inert without the env vars.
- `sdcpp_tier\models` (42.8GB of re-downloadable H3-tier Q4 GGUFs, closed
  experiment, results recorded in memory/h3-tiers) was deleted for disk;
  logs kept in `sdcpp_tier\out`.
- Comfy server run 2 with identical seed = 100% cache hit (0.008s, no
  exec) — warm official numbers MUST change the seed.
- The mac TLS proxy redacts 64-hex; all sha256/revision metadata for the
  registry was fetched ON THE BOX and pulled as a file.
- Pre-existing on this branch: 2 asset-ai `server::lifecycle_tests`
  admission failures (fail with all lane changes stashed too).

## Parked (do not redo)

- fp8 cuBLASLt scaled_mm (quantize activations with `input_scale`,
  fp8×fp8 → bf16-out) — landed in `88eede3b8`.
- Double-block stream ring (resident singles + pinned-host doubles) —
  landed in `88eede3b8`; slot release/re-prime around VAE decode is in
  the follow-up commit.
