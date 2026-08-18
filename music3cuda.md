# Music3 CUDA handoff

**Updated:** 2026-08-17 20:26 +02
**Status:** PAUSED. Lyric A/B **PASS**. 169 product **SHIPPED** 30194176 19:46:30 (native Music3 + ACE). CUDA-graph decode rematch was bit-identical but slower (104s vs 100s) — **reverted** (`832e1a6d5`). User: complexity not needed, CUDA is fast enough. Fable speed shot stopped.
**Status note (stale 18:40, kept):** the **listen bar as written is MISCALIBRATED**: official Python free-runs at seeds 1/11 are themselves narrow-band at 5s (s11 official 3k+ = **0.00**; s1 sub-1k pile 7.75) — the bar fails the official model at 3/4 seeds. Per-seed native-vs-official band families are strikingly similar. The **real, robust deficit** is 3-8k at matched seed+duration: 60s seed-7 python **2.48** vs native **0.16** (15×). Fuse/DiT exonerated (replay of native codes reproduces the muffle; native stack renders official codes WITH highs). No sampler bug (audited vs encoders.py, constants match). Force-prefix experiment running to split per-step bias vs trajectory-entry.
**Owner now:** Fable background hammer (`claude --bg --model fable --name music3-cuda-hammer`). Keep this file current every stage.

If you are another AI: read this whole file, then `/tmp/music3_compare/LISTEN_BAR.txt`, `/tmp/music3_compare/FABLE_LISTEN_BAR.txt`, `/tmp/music3_compare/FABLE_QK_FA2.txt`, `/tmp/music3_compare/PATH_TO_CLOSE.txt`. Do not rediscover rejected rematches. Do not start another official-dtype rematch.

---

## Goal

Native MiniMax-Music3 on 169 CUDA = official Python ModularPipeline song, then warm 60s ≤ 1.05× Python (~45s).

**Ship bar (Fable, revised 2026-08-17 after QK rematch):**

- teacher-forced argmax 100% excluding near-ties (top-2 gap < ~0.25) over 60 frames
- RVQ[0..11] exact as regression tripwire (already holds)
- blind listen on ~4 seeds — coherent, artifact-free, official free-run family bands (1–3k dominant, 3–8k ~0.5–1.5, no sub-1k band >~5). **Currently FAIL.**
- official-code DiT replay SNR ≥ 25 dB (already ~24.5)
- then warm 60s ≤ 1.05× Python

Single-sample free-run SNR vs dump60 (−1.46 dB) is not the ship gate. The 4-seed listen **is** the gate and it **FAILED**: Fable 2026-08-17 says missing 3k+ on 4/4 is code-stream degeneracy, not a valid draw. First-flip ≳ f30 is unattainable without walking to eager 641.

**Token-exact to EOS is unattainable.** Official eager vs official SDPA already forks at RVQ[10] `641` vs `776`. RVQ[12,2] `654` vs `776` is a near-tie (gap 0.077).

---

## Hard constraints

- Fail-closed licensed pack import. No silent license substitution. No network pack fetch.
- Do **not** weaken `pack_import`.
- Do **not** edit `libs/content_chat/**` or `apps/ai-content/src/chat.rs`.
- Do **not** restart GPU remotes (.169 / .123 / .217) or kill `makepad-remote` unless a music deploy requires it.
- Do **not** commit unless asked. Do **not** push.
- Do **not** push local llama `cuda_exec/real.rs` (169 overlay untracked/diverged).
- 169 overlay `C:\Users\playe\makepad` is old **`dev`** — no `libs/game/asset-ai`. **Do not push asset-ai to 169.**
- **Do not rebuild product** until tokens stay in dump60 basin.
- Product exe `C:\Users\playe\makepad\local\aicurrent\target\release\makepad-ai-content.exe --port 8123 --cache-dir C:\ai\asset_node_cache --registry C:\Users\playe\makepad\local\aicontent-registry.json`
  - size **29428224** date **2026-08-17 11:01:34** — UNTOUCHED.
- Validate-only rebuilds of `music3-validate.exe` are OK.
- `tools/winrun.sh` takes a **script file**, not `powershell -Command`.
- Always `WIN_TUNNEL_ADDR=10.0.0.169:8384`.

---

## Machines / paths

| what | where |
|---|---|
| Local tree | `/Users/admin/makepad/makepad` (branch `rik2`, asset crate rename `5a004ec41`) |
| 169 overlay | `C:\Users\playe\makepad` git **dev** (old; no asset-ai) |
| Validate exe | `C:\Users\playe\makepad\local\aicurrent\target\release\music3-validate.exe` |
| Product exe | same `target\release\makepad-ai-content.exe` — do not rebuild |
| Weights | `C:\ai\asset_node_cache\music\MiniMax-Music3\` |
| Official dumps | `C:\ai\music3_compare\` and local `/tmp/music3_compare/` |
| Oracle dump | `C:\ai\music3_compare\python_dump_60s` seed 7 classical / `[Instrumental]` |
| Python | `C:\ai\music3venv\Scripts\python.exe` |
| GPU | 169 RTX PRO 6000 Blackwell 97887 MiB driver 591.86 |
| Tunnel | `WIN_TUNNEL_ADDR=10.0.0.169:8384 tools/winrun.sh <script>` |
| Push file | `target/release/cargo-makepad tunnel 10.0.0.169:8384 push <local> <remote-rel>` |
| Fable | `claude -p "$(cat /tmp/music3_fable_….txt)" --model fable --effort high --allowedTools Read,Grep --disallowedTools Edit,Write,Bash --output-format text` — **never Opus** |
| This doc | `/Users/admin/makepad/makepad/music3cuda.md` |
| Live notes | `/tmp/music3_compare/{NEXT,STEPS,PIPELINE,HARD_RULES,AGENT_CLOSE,PATH_TO_CLOSE,FABLE_*}.txt` |

---

## Token-best (KEEP unless rematch beats it)

Default path, **no** `MAKEPAD_MUSIC3_OFFICIAL`:

- FA2-bf16 prefill (`MAKEPAD_MUSIC3_ATTN` unset / `fa2bf16`)
- GQA decode (`MAKEPAD_MUSIC3_DECODE` unset)
- packed f32acc Linear (`gpu_linear_nt_cached_bf16_f32acc`)
- precise f32 RMS (`gpu_rms_norm_mul`)
- f32 residual (`gpu_add`)
- f32 rope (`gpu_rope_half`)
- identity KV (no bf16 round)

**Sampled result:**

- first token `152931` MATCH
- semantic first_mismatch **15** (`156729` vs official `155120`)
- **RVQ[0..11] EXACT** including RVQ[10]=`[449,800,776,755,689,972,3]`
- first fail **RVQ[12,2] `654` vs `776`**
- teacher-force: f0–f11 OK; f12 NEAR gap **−0.077**; f13–f16 OK; **f17 FLIP gap −4.71** (`166934` vs `153181`)
- last_hidden last-token: f0 `0.155` / f1 `0.323` / f12 `0.962` / f17 `0.872` (~1.5% rel, no new step)
- sampled 5s SNR vs python **−1.46 dB** (flute)
- official-code DiT+vocoder replay **24.53 dB** = real song
- last 60s native wall ~110s vs Python ~45s — **speed paused**

`MAKEPAD_MUSIC3_OFFICIAL=1` is gated in `music3_lm.rs` `official_py()` and **rejected** (full pack + MATH → eager 641, knorm 1.44).

---

## Oracle

Official ModularPipeline, dump60, seed 7, classical / `[Instrumental]`.

- Contract: `IM_START`+CAPTION+LYRICS+`IM_END`+`AUDIO_START`
- Specials: im 151644/45, audio_start 151669, audio_end 151670, AUDIO_CFG 151654
- CFG pair on `[2,T]`; dummy first decode **not emitted**; cond top-50 then guided
- `_sample_top_k` = nan_to_num, top-50, softmax, torch CUDA multinomial (Gumbel-max + Philox4x32-10)
- RVQ also m=2 CFG; stop on first `<|audio_end|>`
- DiT: invert_sigmas true, num_train_timesteps 1, shift 1, Euler dt positive, chunks 200/100 hop
- last_hidden bf16 SDPA
- Qwen3RMSNorm: f32 variance then `weight * xhat.to(input_dtype)` — **cast BEFORE *w**
- nn.Linear: bf16 A/B/C, f32 acc, CUBLAS tensor-op
- dump60 attention = torch SDPA **MATH** (`is_causal=True` prefill, `False` on q_len=1 decode, `attn_mask=None`, `enable_gqa=True`, scale `1/sqrt(128)`)
- Flash **not compiled** in 169 venv; Efficient fails GQA; cuDNN 0.002 off dump60
- Official eager vs SDPA **token fork** at RVQ[10,2] `641` vs `776`

---

## Official-input pipe (later stages are not the bug)

| stage | result |
|---|---|
| tokenize | EXACT |
| feedback embed | 0.000365 PASS |
| L0 last-token MLP | ALL PASS under 0.125 |
| official QKV → FA2+o_proj | maxabs **0.015** last 0.001 PASS (MATH 0.019 worse) |
| official k_proj → `w*xhat.to(bf16)` | **EXACT 0** |
| official k_proj → f32 fused knorm | 0.67 |
| free-running native knorm mid | **0.85** (first float fail in L0 Q/K) |
| official QKV → decode f1/f6/f12 | **0.000191 / 0.000154 / 0.000116** — decode is **not** the compounder |
| official SDPA vs numpy MATH | scores 5.7e-6, softmax 6.6e-7, PV 4.7e-8 ULP |
| L1 attn on official L0 | 0.024 PASS |
| L1 last-token | 0.097 PASS |
| L1 fullseq tok0 | 0.290 FAIL = MLP not attn |
| rvq / cond / dit / vocoder on official in | PASS |
| vocoder vs official.forward | maxabs 0.00995 PASS |
| lm_prefill last_hidden | FIRST FLOAT FAIL ~0.20 last / ~2 mid-seq accumulate |

**Meaning:** when native sees Python tensors, it produces the official song. Free-running last_hidden ~1% rel walks out of the 776 basin.

---

## Rejected rematches (do not retry)

All of these walked toward official **eager** RVQ[10]=`641` or worse (`984`), or made sem fail earlier than 15:

- isolated official Linear `bf16_mm`
- isolated official RMS `gpu_rms_norm_mul_bf16` (extra product round — **wrong kernel**)
- isolated `gpu_rms_norm_qwen3` on **f32** K (`MAKEPAD` qk rematch): knorm mid 0.85→**1.44**, RVQ[10]=641, sem fail 12
- official `q_proj`/`k_proj` (`bf16_mm`) + official knorm + FA2 (`MAKEPAD_MUSIC3_QK_OFFICIAL=1`): knorm mid 0.85→**1.50**, sem fail 12, RVQ[10,2]=776 but [10,4] 793 vs 689
- residual `gpu_add_bf16`
- KV-bf16 (RVQ[10]=984)
- rope-bf16
- FA2 decode (`MAKEPAD_MUSIC3_DECODE=fa2`)
- full official dtype pack + FA2
- `MAKEPAD_MUSIC3_ATTN=math` alone (sem 15→11, RVQ[10] 776→984)
- official+MATH coupled (`MAKEPAD_MUSIC3_OFFICIAL=1` + MATH): still 641, knorm 1.44
- enable_gqa rewrite (repeat_kv is the same)
- mask theory (official mask is plain causal)

Fable: official+MATH=641 had **broken knorm 1.44**. That did **not** falsify official knorm on official `k_proj` + FA2. That rematch then ran and **failed** (knorm 1.50, sem 12). Now rejected.

---

## Next experiment (live)

**Kernel rematches are CLOSED.** Fable 2026-08-17: stop. Official is not self-stable at these ties.

QK_OFFICIAL+FA2 REJECTED: knorm mid **1.50**, teacher f0 0.239 / f12 1.07, sem fail **12**, RVQ[10,2] stayed 776 but [10,4] 793 vs 689.

Sampled f15 free-run (token-best): official gap_ab **+1.50** (155120); native **−1.16** (156729) after RVQ[12,2] 654; last_hidden 16.14. Downstream of the 0.077 near-tie, not a new bug.

**Next (measurement only, Fable after listen FAIL):** dump seed-3 sampled semantic+RVQ (`MAKEPAD_MUSIC3_DUMP_SEMANTIC` / `_RVQ`) and replay through `music3_ar_replay` → DiT → vocoder (same recipe as `replay_ar_5s`). Muffled replay → sampling/feedback bug. Wideband replay → generate fuse/DiT handoff ≠ replay fuse. Histogram unique codes vs dump60. No kernel rematch. No product rebuild.

Do **not** re-ask mask / MATH-vs-FLASH / official+MATH=641 / official-QK+FA2 / isolated dtypes. Do not treat missing 3k+ as a valid draw.

Speed only after listen bar.

---

## Fable log (do not re-ask)

1. Unstuck: dump60 dtype pack *is* official-SDPA numerics; FA2 mid 0.025 is the seed; identify SDPA backend then bit-match; ship on first-flip ≳ f30 + listen.
2. After MATH rematch failed: mask audit. **Measured plain causal. Rejected.**
3. After official+MATH=641: teacher-force ops; reject enable_gqa rewrite; token-exact wrong gate.
4. On assembled close path (2026-08-17): **keep path, demote QK-dump**; coupled knorm+FA2 still live; next = q_len=1 decode. **Decode then measured PASS 0.00019.**
5. After decodeattn: stop kernel hunt; measure flip gaps; first real teacher flip f17 gap 4.71.
6. After QK_OFFICIAL+FA2=sem12: official Linear K + official knorm + FA2 is **rejected** (knorm 1.50).
7. After sampled f15: **stop kernel rematches**. First-flip 15 irreducible. Token-exact and single-sample SNR both wrong gates. New listen bar: teacher argmax excluding near-ties + RVQ[0..11] tripwire + ~4-seed listen.
8. After 4-seed listen FAIL: **not a valid draw**. Code-stream degeneracy (RVQ residual/on_off). Token-best **regressed highs** vs old `native_good_5s`. Next = replay native-sampled codes through official-code DiT path. Listen bar stays failed until official free-run family bands.

Consult / implement: `claude --bg --model fable --name music3-cuda-hammer` with full permissions. Never Opus. Update this file every stage.

---

## Key files

| file | role |
|---|---|
| `libs/diffusion/src/music3_lm.rs` | LM; `official_py()`, `qk_official()` (new), attn/RMS/Linear gates |
| `libs/diffusion/src/music3_ar.rs` | Philox, CFG, dummy skip |
| `libs/diffusion/src/music3_pipeline.rs` | generate |
| `libs/diffusion/src/bin/music3_validate.rs` | stages: tokenize, lm, rvq, cond, dit, vocoder, decode1, l0mlp, layer1, l0attn, decodeattn, teacher, sample |
| `libs/diffusion/src/music3_gguf.rs` | Metal path — sounds like music, slow; **169 product does not use it** |
| `libs/ggml/src/backend/cuda/ops.cu` | `makepad_ggml_cuda_rms_norm_qwen3` |
| `libs/ggml/src/backend/cuda/mod.rs` | `gpu_rms_norm_qwen3`, `gpu_attention_packed_causal_f32` |
| `libs/game/asset-ai/src/music3_backend.rs` | local product backend — **do not push to 169** |

---

## How to rematch on 169

Kernel rematches are **closed**. If you must run validate-only:

```bash
cd /Users/admin/makepad/makepad
# push edited rust
./target/release/cargo-makepad tunnel 10.0.0.169:8384 push \
  libs/diffusion/src/music3_lm.rs libs/diffusion/src/music3_lm.rs
# run a script file, not -Command
WIN_TUNNEL_ADDR=10.0.0.169:8384 tools/winrun.sh remote-jobs/<script.ps1>
```

Build **validate only** into `C:\Users\playe\makepad\local\aicurrent\target`. Check product size/date still 29428224 / 11:01:34 after every job.

Prompt / dump: `python_dump_60s`, weights as above, `--stage teacher` then `--stage sample`.

---

## Current work (update this section every stage)

- [x] Decode-step attn on official KV — PASS, not the compounder.
- [x] `MAKEPAD_MUSIC3_QK_OFFICIAL=1` + FA2 rematch — **REVERT** (sem 12, knorm 1.50). Gate stays env-only; default token-best.
- [x] Sampled free-run f15 logits: official gap_ab=+1.50 (155120); native −1.16 (156729) after RVQ[12] 654; last_hidden f15=16.14.
- [x] Fable: stop kernel rematches; first-flip 15 irreducible; listen bar revised.
- [x] Official RVQ top-2 gaps + 4-seed 5s generate (seeds 1,3,7,11) — **FAIL** all missing 3k+.
- [x] Fable listen: **not a valid draw**. Code-stream degeneracy. Token-best regressed highs vs `native_good_5s`.
- [x] Replay seed-3 native sampled codes: new `--stage replaywav` (`music3_ar_replay` → `music3_render_hiddens` = exact generate tail → wav). Seed-3 redump wav **SHA256 == listen_s3** (dumped codes = the heard audio). `replay_s3_native.wav` bands **0.08/9.45/0.41/0.05/0.01 == listen_s3 exactly** → generate inline fuse/DiT handoff **EXONERATED**; the code stream renders muffled through the official-code chain that renders official codes wideband.
- [x] Code histograms native s3 vs official 5s: **NOT collapsed**. sem unique 56 vs 54, max_run 10 vs 10 (tok 167467 vs 155120), repeat_frac 0.360 vs 0.360 (identical), rvq per-cb unique 83–103 vs 77–112, entropy 6.1–6.6 vs 6.0–6.7. Native draw is structurally normal — it sits in a narrow-band **content** region, all 4 seeds in different narrow bands.
- [x] Hybrid replays: control replay(official 5s codes, **seed-3 DiT noise**) = 0.02/0.49/**8.73/0.75**/0.00 wideband → stage validated, bands are **code-driven** (noise seed irrelevant). Hybrid A (off sem + nat rvq) = 2.97/2.50/**2.60/1.78**/0.14 highs present. Hybrid B (nat sem + off rvq) = 0.77/**6.28**/0.83/**2.01**/0.10 → the 250-1k pile follows the **native semantic stream**; highs return with official RVQ. Full native pair = no highs at all. Joint coherent native stream is what lacks high content.
- [x] Windowed bands (1s windows): all 4 native seeds narrow-band **from t=0** — no decay-into-drone; python starts quiet-low then blooms 3-8k at t=1s. Systematic content-selection bias from the start of free-run.
- [x] Sampler audit vs official `encoders.py` (pulled to /tmp/music3_compare/encoders.py): `_AR_CFG_TOP_K=50`, `_AR_SAMPLING_TOP_K=50`, native `sample_top_k` is a faithful port (nan_to_num→topk→softmax→Gumbel-max Philox), CFG formula and RVQ head sampling identical. **No mechanical sampler bug.**
- [x] Official Python free-runs seeds 1,3,11 5s (`python_dump_5s_s{1,3,11}` on 169, wavs local): s1 = 7.75/1.03/1.05/0.15/0.02, s3 = 0.18/0.33/8.98/0.23/0.29, s11 = 8.45/1.53/**0.02/0.00/0.00**. **Listen bar premise falsified** — official free-runs vary bandwidth by seed; official s11 has zero 3k+; the "no sub-1k >5 / 3-8k ≥0.5" bar fails the official model at 3/4 seeds. Per-seed pairing: native s11 8.28/1.49/0.11 ≈ official s11 8.45/1.53/0.02; native s1 and official s1 both bass-piled; s3 native one band lower (250-1k vs 1-3k); s7 both 1-3k dominant, native misses the 0.86 3-8k.
- [x] 60s matched comparison (seed 7): python 0.32/5.24/1.96/**2.48**/0.01 (EOS 31.9s, 3k+ blooms 5.82/4.16 in the 20-30s windows); native token-best 1.72/5.67/2.42/**0.16**/0.03 (53.4s, max 5s-window 3k+ = 1.23). 0-3k structure matches python closely; **3-8k is 15× low — this is the real deficit**, robust across seeds and durations. Old `native_good_5s` (pre-FA2 `packed_causal` f32 attn) had 3-8k=1.40.
- [x] Force-prefix runs (`MAKEPAD_MUSIC3_FORCE_SEM/_RVQ/_K` in the sampled loop; forced frames take official codes + `music3_rvq_depth_replay` hiddens, semantic RNG still advances). 3k+ per 5s window, 60s seed 7:
  - python: 0.82 0.47 1.20 0.14 **5.82 4.16** (EOS 31.9s)
  - K=550 (22s prefix): 0.83 0.41 1.37 0.21 **4.90** 0.33 0.30 (EOS 37.2s) — free-run **continued the bloom**, then decayed within ~5s.
  - K=125 (5s prefix): 0.78 **2.55 1.20** 0.27 0.04 0.02 0.02 0.11 0.04 (EOS 48.7s) — free-run **bloomed on its own** right after the prefix (brighter than python there), then decayed by 20s, never returned.
  - pure native: 0.05 0.03 0.17 0.44 0.12 0.07 0.06 1.23 0.22 0.01 (EOS 53.4s)
  **Verdict: slow trajectory decay into a low-band attractor.** Sampler can choose and sustain bright content ~5-15s past official history; accumulated self-generated history drags it down. f15 logits gap under forced history +1.5295 vs official +1.50 — per-step compute is official-grade on official history. EOS timing also tracks prefix length (31.9/37.2/48.7/53.4s).
- [x] Config archaeology: pre-FA2 era (e8ec673ee, `native_good_5s` 3-8k=1.40) LM stack differs from token-best in **exactly one kernel**: prefill attn `composite` (packed_causal cuBLAS) vs `fa2bf16`. Decode/Linear/RMS/rope identical. (AR-loop code also evolved since — suggestive, not proof.)
- [x] **Fable bar recalibration** (`/tmp/music3_compare/FABLE_BAR_RECAL.txt`) — old bar + "code-stream degeneracy" verdict **withdrawn**. New ship gate:
  - **Primary (blocking):** 60s matched-seed 3-8k sustain on seed 7 AND seed 3 (fresh seed-3 60s python oracle needed): (a) total-run 3-8k within **3×** same-seed python (s7: ≥ ~0.8); (b) bloom exists and sustains — ≥1 5s-window with 3k+ ≥ 2.0 AND two consecutive ≥ 1.0.
  - Secondary (non-blocking): per-seed 5s family pairing ≥3/4 (currently 3/4, s3 one band low). Final: blind listen on 60s outputs.
  - Tripwires unchanged: DiT replay ≥25 dB, replaywav band identity (keep the stage as permanent instrument), teacher argmax excluding near-ties. EOS time vs python = co-indicator, report not gate.
  - **Standing token-best rule FLIPPED:** trajectory gate + blind listen choose the default config; token metrics (RVQ[10]=776, sem-15) demoted to diagnostics + guardrail (they measure proximity to one backend's Philox coin-flip). `ATTN=math` token-gate rejection is **void**.
  - Battery decision rule: exactly one of composite/math passes → new default + re-baseline tripwires. Both → blind listen, tie → **math**. Neither → measure directional logit bias (bright-token depression vs official on same history); no more kernel rematches, no more force-prefix K-points.
- [x] ATTN battery: **neither passes the new gate.** `composite` 60s total 3-8k+ = 0.89 → passes (a) but bloom dies (windows 2.43/0.08/0.64/0.07, EOS 21.3s) → fails (b); 5s: s1 0.39 / s3 0.24 / s7 **1.55** / s11 0.32. `math` 60s total 0.32 (max window 0.96) fails both; 5s: s1 2.03 / s3 0.19 / s7 0.30 / s11 0.26. Proof of archaeology: `attn_composite_s7_5s.wav` **bit-identical** to `native_good_5s.wav`, and composite 60s bit-identical to `native_pair_60s.wav` — generate is deterministic per config, and prefill attn alone flips the free-run character, but no attn choice fixes the sustain. Attractor is not (only) attention-kernel-dependent.
- [x] **Directional bias measurement — NO BIAS.** Official PyTorch LM teacher-forced with the native s3 codes matches native logits at every depth: argmax 24/26, top-50 overlap **49.3/50**, cond maxabs flat 0.13–0.39 across 126 frames, **no drift growth on native history**. Given the native history, the official model would sample the same muffled continuation. (Brightness correlation moot — native-trajectory tokens barely overlap the official 60s token set.) Artifacts: `bias_logits.zip` local, `remote-jobs/bias_analysis.py`.
- [x] **Official 60s seed-3 oracle (`python_dump_60s_s3`): the OFFICIAL model fails the new gate.** Total 3-8k = **0.09** (windows 0.63 0.48 0.21 0.03 0.08 0.02, EOS 32.7s) — no bloom, no sustain, *below* native token-best's 0.16. **Bloom is a draw property, not a model property** (official s7 = bright piece, official s3 = mellow piece). Combined with no-bias: native free-runs now look like **valid draws**; the "3-8k deficit" was one native draw vs one luckily-bright official draw.
- [x] Distributional battery, 60s draws: **official 3k+ totals [0.09(s3), 0.27(s1), 0.39(s11), 2.48(s7)] — 1/4 bloom, EOS 22.1-32.7s. Native totals [0.01(s23), 0.03(s1), 0.16(s11), 0.19(s7), 0.24(s3), 0.29(s19), 0.39(s13)] — 0/7 bloom, EOS 15.0-53.4s.** Distributions overlap; official's non-bloom draws are indistinguishable from native draws; **the official model fails the recalibrated gate at 3/4 seeds** — bloom is a draw property. Bloom-rate difference (0/7 vs 1/4) not significant (P≈13%).
- [x] **Fable SHIP VERDICT (`FABLE_SHIP_VERDICT.txt`): CORRECT, provisionally.** The no-bias measurement is load-bearing: per-step conditionals match official on both official AND native history, sampler is a faithful port → valid draws by construction. Matched-seed sustain gate withdrawn (calibrated on n=1; official fails it 3/4).
  **Pre-registered reopen tests on dist2** (B = max 5s-window 3-8k, bloom ≡ B ≥ 2.0; 15 native vs 8 official): Fisher one-sided (official ≥ 3/8 with native 0/15 reopens, p=0.032) + Mann-Whitney one-sided on B (reopen p < 0.05). If reopened → next measurement = bright-history bias (teacher-force official bloom-region history, compare bright-token logits).
  **Final ship gate**: (1) pinned tripwires — RVQ[0..11] exact incl. RVQ[10]=[449,800,776,755,689,972,3]; sem first_mismatch=15 at the 0.077 near-tie; teacher argmax 100% ex-near-ties, first real flip ≥ f17; DiT replay 24.5±0.1 dB (re-baselined, regression detector); replaywav band-identity instrument; feedback ≤5e-4; decodeattn ≤5e-4. (2) distributional PASS per the pre-registered tests. (3) **USER blind listen**: 8 shuffled seed-paired 60s wavs (official+native s1/s3/s7/s11) — zero native flagged broken AND origin accuracy ≤ 6/8. (4) secondary non-blocking: 5s family pairing ≥3/4 (holds).
  **Speed protocol**: same box idle, seed 7, warm (load + 1 discard), 3 timed runs median, per-stage walls; primary gate = native prefill + native ms/frame × F_py + native render-rate × S_py ≤ 1.05 × python median wall (workload transfer — per-emitted-second the current gap is ~1.46×, not 2.4×). Product rebuild hold lifts after the full correctness gate (incl. blind listen) passes.
- [x] **dist2 + pre-registered tests: PASS. CORRECTNESS VERDICT STANDS — CORRECT.** Full sample (B = max 5s-window 3k+, bloom ≡ B≥2.0): official 8 draws **1/8 bloom**, B=[0.04, 0.09, 0.50, 0.63, 0.69, 0.74, 1.18, **5.82**(s7)], EOS 12.3-47.1s. Native 15 draws **1/15 bloom**, B=[0.01, 0.06, 0.06, 0.17, 0.32, 0.43, 0.51, 0.53, 0.57, 0.67, 0.83, 1.23, 1.32, **1.94**(s43), **3.34**(s53 BLOOM)], EOS 15.0-60.1s. **Native seed 53 blooms** — existence proof. Fisher one-sided p=0.585 PASS; Mann-Whitney one-sided p=0.373 PASS. Distributions interleave; no reopen.
- [x] Blind-listen set staged for the user: `/tmp/music3_compare/blind_listen/clip_01..08.wav` (official+native s1/s3/s7/s11 60s, shuffled) + `KEY_do_not_open_before_listening.txt`. **User action required**: listen, mark each valid/broken + guess origin; PASS = zero native flagged broken AND origin accuracy ≤6/8. (Bonus listen outside protocol: `native60_s53.wav` = the native bloom, `native60_s43.wav` near-bloom.)
- [x] Bench instrumentation shipped (validate-only): `MAKEPAD_MUSIC3_BENCH=1` per-stage walls (lm_load/prefill/ar+frames/render+samples), `MAKEPAD_MUSIC3_BENCH_RUNS=N` warm in-process median; `remote-jobs/py_bench_60s.py` python counterpart (load once, 1 discard + 3 timed).
- [x] **Speed bench (protocol run)**: tripwires HOLD at pinned values on the bench build (sem mismatch=15 at 156729/155120; RVQ flat 86 = [12,2] 654/776; RVQ[0..11] exact — the stage's exit-1 is the old token-exact criterion, not a regression). **Native warm median 100.200s** (1334 fr / 53.4 s draw; prefill 0.034 s, AR 77.88 s = **58.4 ms/frame**, render 22.17 s = **0.4151 s per audio-s**). **Python warm median 45.188s** (797 fr / 31.87 s draw). **Workload-transfer gate: 0.034 + 0.05838×797 + 0.4151×31.87 = 49.79 s vs 47.45 allowed → 1.102×, FAIL by ~5%** (~2.4 s to shave; AR term dominates at 46.5 s). AR head/rvq/step split instrumentation added (`BENCH ar_split`) + pushed; split bench queued behind the user's lyric A/B.
- [ ] **User lyric A/B in flight** (user-driven, 169): caption "warm acoustic pop, male vocal, acoustic guitar and light drums, 96 BPM" + real verse/chorus lyrics, seed 7, 60s, official then native → `/tmp/music3_compare/lyric_ab/{official,native}_60s.wav`. User verdict on the classical blind clips so far: official s1 and native s3 both "rumble — not music", so the `[Instrumental]` classical prompt itself renders as rumble on BOTH sides; the vocal-pop A/B is the meaningful ear test. Analyze bands + report when it lands. **No new 169 GPU jobs until lyric_ab.log finishes.**
- [x] AR-split bench (warm): **head 2.9 ms + rvq 12.4 ms + step 43.0 ms = 58.3 ms/frame.** The 36-layer pair decode step is **74%** of AR cost (~1.19 ms/layer — launch-overhead territory; CUDA-graph capture like the llama path is the lever), RVQ depth chain 21%, lm_head 5%. Perf order for the open 1.05× work: (1) graph-capture the decode step, (2) RVQ chain, (3) render (22.1 s / 53.4 audio-s).
- [x] **Lyric A/B PASS (user listened): "native is perfect"** vs official on 60s acoustic-pop + real lyrics, seed 7. Bands: official 2.55/5.85/1.43/0.15/0.02 (3k+ 0.16), native 5.37/3.55/0.78/0.12/0.18 (3k+ 0.30) — same family, native slightly brighter. The correctness listen leg is satisfied on meaningful content; classical `[Instrumental]` renders as rumble on BOTH sides (prompt/model property, not a port defect).
- [!] **SHIP BLOCKED — cross-lane tree skew, nothing disturbed.** Product rebuild fails at manifest load: overlay `apps\ai-content\Cargo.toml` depends on `libs/game/content` + underscore-era `libs/game/asset_client`, but `libs\game\content` is **missing on the 169 overlay**, and the local tree renamed those crates (`content`→`asset-data`, `asset_client`→`asset-client`, commit 5a004ec41) — the 11:01:34 exe is an asset/VJ-lane artifact from a source state matching **neither** tree today. Failed build wrote no artifacts: exe 29428224 / 11:01:34 intact, pid 11052 running, health OK, `jobs_pending=0`, trellis-2 loaded. **User options**: (a) have the asset/VJ lane rebuild the product from its consistent source set — the music3 changes are already in the overlay's `libs/diffusion` and validated, so any legitimate product rebuild picks them up; (b) full crate-sync push incl. `asset-ai` — forbidden by standing rule; (c) restore the pre-11:01 `apps\ai-content\Cargo.toml` + matching `libs\game` state on 169. Log: `remote-jobs\ship_product.log`.
- [ ] SPEED (open, non-blocking): 1.102× → 1.05× needs ~2.4 s at python workload. Lever order from ar_split: CUDA-graph the 36-layer pair decode step (43.0 of 58.3 ms/frame), then the RVQ depth chain (12.4 ms), then render (0.415 s/audio-s).
- [~] **CUDA-graph pair decode IN FLIGHT (2026-08-17 ~20:15, agent 63d942bb):** `Music3PairDecoder` — persistent `[2*cap,1024]` K/V per layer; new `gpu_kv_scatter_pair` (append row index from device f32 scalar) + `gpu_attention_gqa_decode_bf16_cached` (loop bound `*seq+1`, stride=cap; per-thread math byte-identical to the non-cached kernel); whole 36-layer step captured once after 2 warm frames via the flux/da3 `GpuStepGraph` infra, replayed per frame (4 tiny H2D + 1 launch + 1 D2H replace ~1300 WDDM launches AND the 6-per-layer O(seq) cache copies). Host rope per frame kept (exact same values as `rope_range`). Legacy `step_embeds_pair` path intact; auto-selected under `MAKEPAD_MUSIC3_GRAPH=0`, `MAKEPAD_MUSIC3_DECODE`, official/QK gates, `DUMP_F1`, or seq_lm; eager fallback if capture fails. Files: `music3_lm.rs`, `music3_ar.rs`, `diffusion_ops.cu`, `cuda/mod.rs`, `diffusion/backend.rs` (pushed to 169). Job `remote-jobs/graph_bench.log`: build + sample tripwire + warm median-of-3 60s s7 + **wav SHA256 vs pre-patch `bench_60s_s7.wav` (deterministic per config → must be bit-identical)**. KEEP iff tripwires hold AND workload-transfer ≤ 1.05×.

**Local band energy 2026-08-17 (rel×10, 0–250/250–1k/1–3k/3–8k/>8k, mono=mean L/R):**

| wav | peak | rms | bands | 3k+ |
|---|---|---|---|---|
| python_classical_5s | 0.590 | 0.075 | 0.03/0.58/8.51/0.86/0.02 | 0.88 |
| replay_ar_5s | 0.580 | 0.076 | 0.03/0.59/8.51/0.85/0.02 | 0.87 |
| native_good_5s (old) | 0.331 | 0.047 | 0.23/0.50/7.71/1.40/0.15 | 1.55 |
| native_fa2bf16_5s (token-best s7) | 0.219 | 0.032 | 0.37/1.08/8.53/**0.01/0.00** | 0.01 |
| listen_s1 | 0.334 | 0.074 | 5.48/4.38/0.12/**0.02/0.00** | 0.02 FAIL bass |
| listen_s3 | 0.738 | 0.128 | 0.08/9.45/0.41/**0.05/0.01** | 0.06 FAIL 250-1k |
| listen_s7 | 0.219 | 0.032 | 0.37/1.08/8.53/**0.01/0.00** | 0.01 FAIL = token-best |
| listen_s11 | 0.472 | 0.089 | 8.28/1.49/0.11/**0.11/0.00** | 0.11 FAIL sub-bass |
| native_classical_5s | 0.809 | 0.081 | 3.86/3.49/2.31/0.31/0.02 | 0.33 |

Listen bar **FAIL** 4/4. None look like python. `listen_s7` bit-identical to `native_fa2bf16_5s`. Seed 3 matched official semantic at f12 (gap +1.02) and is still muffled. Official-code replay still 3-8k=0.85.

SAMPLED_LOGITS: s1 left dump60 basin (a/b/c=-inf); s3/s11 f12 MATCH 155120; s7 f12 Philox 156729 then f15 flip after RVQ[12]=654.

Official dumped RVQ h2 top-2: f2 gap 0.1875; f12 gap 0.4375 (761 vs 433; 654=6.42 776=5.95 Philox 776). Generate log has 0 ARRVQ lines (trace off).

**Product:** 29428224 11:01:34 untouched.
**Wavs:** `/tmp/music3_compare/listen_s{1,3,7,11}.wav` and `C:\ai\music3_compare\`.
**Report:** `/tmp/music3_compare/LISTEN_BAR.txt`  Fable: `/tmp/music3_compare/FABLE_LISTEN_BAR.txt`
**Heartbeat:** `/tmp/music3_compare_heartbeat` (2 min).
**Do not claim 1.05× or “it’s a song”** without equal-duration warm rematch AND bands/SNR like `python_classical_5s` / `replay_ar_5s`.
