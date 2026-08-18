# CUDA ports — campaign tracker

Updated: 2026-08-18. Owner: Fable (CUDA). Metal on `.162` is later.
Oracle rule: native must **match then beat** official warm time.
Faster-but-worse is forbidden.

## Now

1. **Flux2 Klein-4B CUDA — complete the path.** Denoise is under oracle
   (595 ms &lt; 671). TE + VAE encode + **VAE decode (~5 s oracle)** and
   end-to-end warm generate are **not** beaten. Teacher residual 0.05 is
   treated as unreachable (bf16 ulp pile-up); PNG lock stays.
2. **Then FlashWorld native CUDA** — only splat/world backend we have
   (Python today). Same rule: dumps, match, beat warm ~6 s.

Metal (`.162`, no oracle, as fast as possible) waits until CUDA for
that model is done.

## This campaign (image edit pipeline)

| Model | CUDA | vs oracle | Notes |
|---|---|---|---|
| ACE-Step 1.5 | **done** | warm 3.32 s &lt; 4.01 s, wav accepted | `62759e4b8` + `50d89dd73` |
| SAM 3.1 multiplex | **done** | detect 0.097 s &lt; 0.132 s, IoU 0.998 | passes 1–4 on `rik2` |
| Flux2 Klein-4B **edit** | **partial** | denoise 595 &lt; 671; **decode not beaten** | works on `.235`; not a product t2i |
| RealESRGAN x4 | **done** | 218 ms &lt; 249 ms, dump lock held | `07fff17bb` |
| Flux2-dev t2i | **scaffold only** | — | tokenizer/TE/schedule; no generate |
| FlashWorld splat | **Python only** | warm ~6 s / cold ~70 s | only `world` backend |

### Flux2 Klein — what “working” means

It **does** run on Windows CUDA (`.235`, overlay `C:\ai\flux2edit\`).
Instruction + reference PNG → edited PNG. Default `fa2bf16pre`.
Washed-out PNG bug fixed. Service id `flux2-klein-4b` is in the
registry. It is **not** Flux2-dev, **not** Metal, **not** done on
full-generate time (oracle decode **5.02 s**).

| stage (oracle 4090) | official | native last |
|---|---|---|
| load | 12.4 s | — |
| TE | 0.35 s | — |
| VAE encode | 0.38 s | — |
| denoise 4-step | **0.671 s** | **0.595 s** |
| VAE decode | **5.02 s** | not beaten |
| teacher residual | — | 0.20 f32 / 0.41 fa2 (0.05 is a floor, not a bug) |

### FlashWorld — is it the best splat option?

It is the **only** splat/world option in the registry. No TripoSplat,
no Luma, no native 3DGS trainer. Image/text → ~4 M splat PLY (~600 MB),
Wan2.2-TI2V-5B + 20.9 GB NC checkpoint, provisioned on `.169`. Native
port has not started.

## All backends

| id | domain | CUDA native | Metal | Oracle / notes |
|---|---|---|---|---|
| flux1-schnell / flux1-dev | image | **yes** (FP8, CUDA-gated) | **yes** compiled graphs (~2.50 s/step Q4 @256 on `.162`) | product image |
| flux2-klein-4b | image | **partial** (see above) | no | edit only |
| flux2-dev | image | scaffold | no | not a backend yet |
| realesrgan (not in registry yet) | — | **yes** 218 ms | no | pin Comfy-Org x4plus |
| sam3-1-multiplex | segment | **yes** | compile-only GpuTensor, untimed | Comfy-Org, no facebook/ |
| ace-step-1.5-xl | music | **yes** | no | wav accepted |
| minimax-music3 | music | **yes** BF16 | GGUF Q4 code exists, pack won't load | Q4 parked |
| birefnet-hr | matte | **yes** | no | |
| da3-metric-large | depth | **yes** | no | |
| trellis-2 | mesh | **yes** | no | |
| hunyuan3d-paint-2.1 | paint | **yes** | no | |
| skintokens | rig | **yes** | no | |
| hy-motion | motion | **yes** | no | |
| sa3-sfx / moss-sfx / woosh-sfx | audio | **yes** | no | |
| indextts-2.5 | speech | **yes** | no | |
| kokoro | speech | CPU + Metal GEMM offload | partial | |
| qwen3.6 / qwen3.8 | text | **yes** | **yes** compiled graphs | |
| minimax-h3 (+ quants) | video | **yes** | no | 24/32/96 GB tiers |
| flashworld | world | **Python** | no | only splat option |
| skintokens-oracle / hy-motion-oracle | rig/motion | Python | — | keep |
| testpattern / pbr-testpattern | image/paint | CPU | CPU | |

## Boxes

| box | GPU | this campaign |
|---|---|---|
| 10.0.0.100 | 4090 | SAM3 overlay `C:\ai\sam3\` |
| 10.0.0.203 | 4090 | ACE `C:\ai\ace\` |
| 10.0.0.235 | 4090 | Flux2 `C:\ai\flux2edit\`, RealESRGAN `C:\ai\realesrgan\` |
| 10.0.0.169 | RTX PRO 6000 96 GB | FlashWorld Python, H3 |
| 10.0.0.162 | M4 16 GB | Metal later |
| 10.0.0.217 | 5090 | **do not steal** |

`WIN_TUNNEL_ADDR=<ip>:8384`, always `--no-sync`, `MAKEPAD_GGML_CUDA_ARCH=89`
on 4090s. Splice-only ggml.

## Branches on rik2 (2026-08-18)

SAM3 speed `bb4487cac`…`3a0bdcc6a`, Flux2 Klein `953376ae6`…`0e5aab74d`,
RealESRGAN `07fff17bb`, Metal foothold `145f282b0`. Isolated worktrees
`sam3-fable`, `flux2-klein-fable`, `realesrgan-fable`, `metal162` remain
for Fable.

## Done when

- [x] ACE CUDA &gt; oracle, song accepted
- [x] SAM3 detect &gt; oracle, IoU held
- [x] RealESRGAN e2e &gt; oracle, dump lock held
- [ ] Flux2 Klein **full** warm generate &gt; oracle (decode included)
- [ ] FlashWorld native CUDA &gt; ~6 s warm
- [ ] Metal `.162` after each CUDA win (no oracle, as fast as possible)
