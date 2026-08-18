# SAM 3.1 native CUDA — Fable handoff

Owner: Fable. Coordinator: Grok. Started 2026-08-17 after landing
`f67efe29e` on `rik2`.

## You own this lane entirely. No limits.

Keep going until warm detect **beats** the official Python oracle
**without** making the mask worse. Faster-but-worse is the wrong
direction (ACE lesson). Do not pause to ask permission.

## Quality lock (must hold)

- 05 refine IoU ~**0.998** vs `C:\ai\sam3\dumps\05_p0_cat_soft_masks.npy`
- 04 detect scores ~**0.9993**, kept IoU ~**0.9969**, detections=1
- Prompt `cat` on fixture `C:\ai\sam3\fixtures\cat_still.png`
- Never accept a speedup that drops 05 IoU below **0.99**
- Facebook TOS checkpoints (`facebook/sam3*`) are forbidden

## Speed target

| | native now | oracle to beat |
|---|---|---|
| WARM_MEDIAN (detect + 2-iter refine) | **0.190 s** | 0.132 s is trunk+detect only |
| detect_only phase | **0.097 s** | 0.132 s — **beaten** |
| cold load | — | 3.039 s |
| peak VRAM | — | 7179 MiB |

`sam3-validate` prints `WARM_MEDIAN_SECONDS` (detect+refine). Also
print/compare a detect-only phase so the 0.132s oracle is apples-to-apples.

## Fable campaign log (2026-08-17)

Important: WARM_MEDIAN_SECONDS covers detect **plus** the 2-iteration
refine (second full trunk pass on the crop). The handoff's "0.185 s"
was the `detect_only` phase line; the true baseline WARM_MEDIAN at
f67efe29e was **0.379 s** (IoU 0.998234). The oracle's 0.132 s is
"warm trunk + detect" — its stage-5 refine was never timed warm.

| commit | WARM_MEDIAN | 05 IoU | change |
|---|---|---|---|
| f67efe29e | 0.379 | 0.998234 | baseline re-measured |
| 7007805d5 | 0.268 | 0.998234 | const caches, GPU refine resizes, folded window attention |
| 596c6ced0 | 0.241 | 0.998225 | fusion self-attn via d64 flash, split in_proj, resident decoder |
| da79520c7 | 0.209 | 0.998254 | graphed trunks, resident pos embed, GPU mask downscale, f16acc trunk GEMMs |
| pass 4 | 0.190 | 0.998256 | FA2 register-tiled d64 flash (`flash2_d64`) in vision global/window + padded d32 decoder |

Pass 4 detail: `gpu_attention_packed_flash2_d64` is a dimensional port
of the proven d128 FA2 kernel (mma m16n8k16, cp.async K/V rings, f16
operands, f32 softmax/accum). Replaces the smem-tiled bf16 d64 flash at
all three call sites. detect_only 0.097s beats the 0.132s oracle.

Profile: trunk dense GEMMs ≈ 50ms ×2 still dominate. Pass 3 f16acc on
trunk only (decoder/score f16acc dropped IoU for ~2ms — rejected).
`FLUX_GEMM_F16ACC=0` restores f32 accumulate. Warm split now
detect_only ≈ 0.097 / refine ≈ 0.092 — the refine trunk pass is half
the remaining time.

## Box

- **10.0.0.100** RTX 4090 24 GB, sm89. **Not** .217 (5090 — do not steal).
- `WIN_TUNNEL_ADDR=10.0.0.100:8384`
- Helpers: `/Users/admin/makepad/makepad/tools/{wincmd,winrun,winspawn}.sh`
  and `/Users/admin/makepad/makepad/target/release/cargo-makepad`
- **ALWAYS `--no-sync`.** Never broad-sync the Mac tree onto Windows.
- Overlay: `C:\ai\sam3\native\`
- `MAKEPAD_GGML_CUDA_ARCH=89`
- CUDA 12.4 bin on PATH (see `C:\ai\sam3\native\build.ps1`)
- Do not kill `makepad-remote` / port 8384
- Tunnel `push` paths are **relative to the server cwd**. Absolute
  `C:\...` push paths get dropped. Push under a relative overlay dir
  (e.g. `_sam3_overlay\libs\...`) then copy into `C:\ai\sam3\native\`.
- **Splice-only ggml.** Do not copy a whole `libs/ggml` tree — that
  drops ACE/Music3 kernels. Only add/edit the kernels you need
  (`gpu_rpb_expand`, `gpu_attention_packed_cross_bias`, and whatever
  you fuse next).

## Weights / oracle

- Weights: `C:\ai\sam3\weights\checkpoints\sam3.1_multiplex_fp16.safetensors`
  (Comfy-Org/sam3.1 rev `f38cd62b71494b53ac2b56ca36e24f3c8d565581`)
- Dumps + ORACLE.md: `C:\ai\sam3\dumps\`
- Comfy source: `C:\ai\sam3\ref\ComfyUI\comfy\ldm\sam3\`
- Build: `C:\ai\sam3\native\build.ps1`
- Run: `C:\ai\sam3\native\generate.ps1`
  (`sam3-validate <weights> <dumps> cat 5`)

## Tree

Work **only** in this worktree:

`/Users/admin/.grok/worktrees/makepad-makepad/sam3-fable`  branch `sam3-fable`

- Do **not** edit `/Users/admin/makepad/makepad` (shared dirty `rik2`).
- You may commit on `sam3-fable`. Do not push. Do not merge to `rik2`.
- Landed sources: `libs/diffusion/src/sam3.rs`, `sam3_model.rs`,
  `bin/sam3_validate.rs`, `libs/game/asset-ai/src/segment_backend.rs`,
  ggml `gpu_rpb_expand` + `gpu_attention_packed_cross_bias`.

## First steps

1. Sync landed `f67efe29e` SAM3 + ggml splice files onto `C:\ai\sam3\native\`.
2. Release-build `sam3-validate` with ARCH=89. Confirm 05 IoU still ~0.998
   and record a fresh warm median. That is your baseline.
3. Profile (`MAKEPAD_GPU_PROF=1`). Vision + decode dominated last time
   (vision ~73ms, decode ~96ms of the 0.185s).
4. Cut host/PCIe and launch overhead first (ACE AdaLN lesson). Fuse
   only with a bitwise or IoU-lock proof.
5. Re-run the validator after every change. Keep the IoU lock.

## Do not

- Touch ACE, Music3, Flux2, or .217
- Invent a new math path that diverges from Comfy `sam.py` / `detector.py`
- Ship a faster run with a worse mask
- Pause
