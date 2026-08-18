# RealESRGAN x4 native CUDA — Fable handoff

Owner: Fable. Coordinator: Grok. Klein CUDA denoise is beaten
(594.8 ms < 671 ms). This is the next CUDA lane. Metal stays off.

## GATE BEATEN 2026-08-18 — 218.05 ms < 249.09 ms, all quality locks inside

Native fast mode on .235 (warm e2e u8→u8, median of 10): **218.05 ms**
(min 216.94) vs the 249.09 ms fp16 oracle — 12.5% faster.  Every fast-mode
dump-lock metric is inside the official fp16 envelope, most by 2-3x:

| metric vs `out_f32.bin` | official fp16 envelope (gate) | native fast |
|---|---|---|
| max_abs | <= 1.0406e-2 | **8.315e-3** |
| mae | <= 3.703e-4 | **1.293e-4** |
| cosine | >= 0.999999629 | **0.999999956** |
| 8-bit pixel diffs | <= 1038566/4194304 | **388755/4194304** |
| max 8-bit delta | 3 | **2** |

PNG eyeballed vs `oracle_fp16.png`: indistinguishable — no blur, no
checkerboard, zipper/seam/speckle detail identical.  Parity mode re-verified
byte-identical to the locked floor (cosine 0.999999995, max_abs 5.768e-4).

### Fast-path design (realesrgan_model.rs `forward_f16`)

- **Dense body f16** exactly like the official CUDA forward: all 345
  dense-block convs are cuDNN PSEUDO_HALF (f16 in/out, f32 accumulate,
  `prepare_nchw_strided_f16`) over one persistent 256-row planar f16 buffer.
  Batch-1 planar rows are contiguous NCHW, so every dense concat is a
  pointer offset; fused in-place bias+LeakyReLU epilogues; zero concat
  copies, zero im2col.
- **f32 residual spine** (`gpu_realesrgan_spine_axpb`): the RDB/RRDB
  residual chain and trunk accumulate in f32 (`x32`/`save32`/`fea32`),
  mirroring each result into the f16 conv-input view in the same kernel.
  Spine rounding never compounds across the 23 blocks.
- **conv_first and conv_body in true f32** (cuDNN FMA math, no TF32
  down-conversion, `prepare_nchw_strided_f32_fma`): their rounding is the
  most amplified in the graph (rides all 23 blocks / the trunk skip).  This
  was THE max_abs lever: 1.14e-2 -> 8.3e-3.  Costs ~1 ms.
- **Head (conv_up1..conv_last) in true f32** with exact bias/lrelu, nearest
  2x upsamples in f32; ~+5 ms.
- **Device RGB8 quantize** (`gpu_realesrgan_quantize_rgb8_f32`): warm path
  downloads only the final 12 MB of RGB8, never a 200 MB f32 tensor.
- Weights cache under `realesrgan::<conv>::nchw16/nchw32`, biases f32.

### Numerics findings (in fix order, each verified on the box)

1. **`quant.rs` `f32_to_f16` truncates** (no rounding).  Host-packed f16
   conv weights carried a systematic 0.5-ulp toward-zero bias -> fast-mode
   mae 8.4e-4 (2.3x the envelope).  Fix: new `f32_to_f16_rn`
   (round-to-nearest-even, matches `__float2half`/torch `.half()`, unit
   tests incl. full f16 roundtrip) used by the RealESRGAN weight pack.
   mae -> 3.6e-4, pix_diff -> 1.02M.  The truncating version stays for
   consumers whose locked dumps were recorded with it (Flux etc.) — worth
   revisiting repo-wide later.
2. **Worst-pixel error was conv_first/conv_body rounding**, not the head
   and not spine compounding: f32 head alone and f32 spine alone each
   improved mae/pix but left max_abs pinned at 1.14e-2.  f32-ing the two
   64-channel 512-res convs dropped it under the envelope instantly.
3. f32 head: mae 3.6->3.1e-4; f32 spine: 3.1->2.9e-4 (+17 ms — kept for
   quality margin on unseen content, we have 31 ms of gate headroom).

### Box facts

- `validate.ps1` needs `CUDNN_PATH=C:\ai\realesrgan\venv\Lib\site-packages\torch\lib\cudnn64_9.dll`
  (cuDNN 9 stub; `cudnn_ops64_9.dll` auto-resolves next to it).  Without it
  `cudnn::available()` is false and the fast path refuses to run.
- Build: `C:\ai\realesrgan\scripts\build.ps1` (cargo in
  `C:\ai\realesrgan\native\libs\diffusion`, target dir `native\target`).
- Validate: `scripts\validate.ps1 -Mode fast|f32|v1 -Bench N`.
- Warm timing history: v1 (f16-multiply GEMM, explicit concats) 842 ms;
  v2 f16 head 196.1 ms; +f32 head 201.5 ms; +f32 spine 218.2 ms;
  +f32 first/body 218.05 ms (final, all locks inside).

## You own this lane entirely. No limits.

Build the oracle, then a native CUDA port that **matches** then
**beats** oracle warm time. Faster-but-worse is forbidden.

## Model pin

- Comfy-Org safetensors: `Comfy-Org/Real-ESRGAN_repackaged`
  `RealESRGAN_x4plus.safetensors` (RRDBNet x4plus).
- Original: xinntao/Real-ESRGAN (BSD-3). No cloud backends.
- Pin immutable revision + size + sha256 in `registry.json` when you
  add the model. Fail-closed.
- This is a **general image x4 upscaler**, not Hunyuan PBR texel upscale.

## Quality lock

Pick one fixture PNG (the Flux2 jacket or a small natural photo),
x4 upscale, lock:

- pixel max_abs / cosine vs official/Comfy dump
- Do not accept a faster run that blurs, checkerboards, or drifts
  the dump beyond a tight gate you write down first
- Same dtypes as the official forward (no invented bf16 shortcuts
  unless you prove they match)

## Speed target

Oracle first (time a warm x4 on the 4090). Native must beat that
warm number. Record both in this file.

## Box

- **10.0.0.235** RTX 4090 24 GB, sm89. Flux2 overlay stays in
  `C:\ai\flux2edit\` — do not overwrite it.
- New overlay: `C:\ai\realesrgan\`
  (`weights\`, `dumps\`, `fixtures\`, `native\`, `STATUS.md`, `ORACLE.md`)
- `WIN_TUNNEL_ADDR=10.0.0.235:8384`
- Helpers: `/Users/admin/makepad/makepad/tools/{wincmd,winrun,winspawn}.sh`
  Binary: `/Users/admin/makepad/makepad/target/release/cargo-makepad`
- **ALWAYS `--no-sync`.** ARCH=89. CUDA 12.4 on PATH.
- Do not use .217 or .100. Do not kill makepad-remote.
- Relative tunnel push. Splice-only ggml.

## Tree

ONLY `/Users/admin/.grok/worktrees/makepad-makepad/realesrgan-fable`
branch `realesrgan-fable`.

- Do **not** edit `/Users/admin/makepad/makepad`.
- Do **not** edit flux2-klein-fable or sam3-fable.
- Commit proven wins on `realesrgan-fable`. Do not push. Do not merge.

## Oracle (recorded 2026-08-18, box .235 4090)

Pin: rev `ea19b4cd14f85a5b914eee8aa7ff77bc371039a0`, size 66857836, sha256
`37f9a931c215f040aa6d50f711f2cb115f713c46df1d0d6469a8bd7bfe9a60bb`
(verified on box and locally). All tensors F32, canonical basicsr keys
(`conv_first`, `body.{0..22}.rdb{1..3}.conv{1..5}`, `conv_body`,
`conv_up1/2`, `conv_hr`, `conv_last`).

Fixture: `C:\ai\realesrgan\fixtures\jacket512.png` = the Flux2 jacket edit
output (512x512) -> 2048x2048. Oracle = official basicsr RRDBNet inline,
strict state-dict load, torch 2.11.0+cu128 (spandrel cross-check skipped:
its torchvision wheel is broken on the box; strict-load + official arch is
authoritative). Dumps in `C:\ai\realesrgan\dumps\jacket\`.

Warm timings (median of 10, cuda-synchronized):

| variant | forward | end-to-end (u8 host -> u8 host) |
|---|---|---|
| f32 (Comfy default) | 355.22 ms | 366.57 ms |
| fp16 (official CUDA default) | 237.74 ms | **249.09 ms** |

**Speed gate: native warm e2e < 249.09 ms** (the fastest correct oracle).

## Quality gates (locked before any speed fuse)

vs `out_f32.bin` (pre-clamp CHW f32):

- **Parity mode** (`FLUX_VAE_CONV_GEMM=0`, pure-f32 convs): proves the
  architecture. Must be far inside the fp16 envelope; recorded metric
  becomes the regression floor once measured.
  **MEASURED 2026-08-18 (locked):** cosine=0.999999995,
  max_abs=5.768e-4, mae=5.70e-5, pix_diff=178459/4194304, max8=1 —
  pure accumulation-order noise.  Native-f32 vs the fp16 oracle dump
  reproduces the official envelope (cosine 0.999999620, max_abs
  1.045e-2, pix 1050457 vs official 1038566): the graph is right.
  Any future parity-mode run must stay at or inside these numbers.
- **Fast mode** (default / any fused path): must match the official fp16
  forward or better on every metric — max_abs <= 1.0406e-2,
  mae <= 3.703e-4, cosine >= 0.999999629, 8-bit pixel diffs
  <= 1038566/4194304 (fp16 oracle's own numbers vs f32). No blur, no
  checkerboard: eyeball the PNG on any fused change.

## First steps

1. Download the pinned Comfy-Org x4plus safetensors onto
   `C:\ai\realesrgan\weights\`. Confirm sha.
2. Official/Comfy Python oracle dump of one fixture x4 + warm time.
   Write `ORACLE.md` / `STATUS.md`.
3. Port RRDBNet (conv / upsample / residual dense blocks) on
   makepad-ggml CUDA. Look at existing `gpu_conv2d_planar_*` first.
4. Validator bin `realesrgan-validate`. Registry + backend only if
   the generate path is real.
5. Beat oracle warm time with the dump lock held.

## Do not

- Touch Flux2, SAM3, ACE, Music3
- Start Metal (coordinator, `.162`, after CUDA)
- Invent a new architecture
- Pause
