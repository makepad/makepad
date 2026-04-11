# MLX Rotor Plan

## Goal

Add an optional Gemma 4 Metal KV-cache compression mode that reduces long-context memory use on smaller-memory Macs without changing the default runtime path.

## Scope

- Model family: Gemma 4 text tower on Metal
- Default: off
- First target: full-attention layers only
- First compressed tensor: K cache only
- V cache stays BF16 in phase 1

## Why This Scope

Gemma 4 text is mostly sliding attention. Only 5 of 30 layers are full-attention, so long-context KV growth is concentrated there. Compressing only the full-attention K cache gives a useful memory reduction while keeping the attention/output path close to the current exact backend.

## Phase 1

### Mode

- `RotorPlanar4FullAttentionK`
- PlanarQuant-style 2D rotation
- 4-bit packed centroid indices
- 1 BF16 norm per token/head
- On-device quantize on KV append
- On-device dequant inside the logits kernel

### Memory Model

Full-attention K storage per token:

- BF16 baseline: `5 layers * 2 kv heads * 512 dims * 2 bytes = 10,240 bytes/token`
- Planar4 K-only: `5 * 2 * (256 packed bytes + 2 norm bytes) = 2,580 bytes/token`
- Saved: `7,660 bytes/token`

Estimated total Gemma 4 text KV storage:

| Context | BF16 KV | Phase 1 KV | Saved |
|---|---:|---:|---:|
| 8K | 360.0 MB | 300.2 MB | 59.8 MB |
| 32K | 840.0 MB | 600.6 MB | 239.4 MB |
| 128K | 2760.0 MB | 1802.5 MB | 957.5 MB |

## Current State

Implemented:

- Public optional Metal backend config
- `--rotor-k-cache` flag in `mlx-cli`, `gemma_text_generate`, and `gemma_text_bench`
- Exact Metal full-attention K-cache can now store compressed Planar4 rows instead of BF16
- Metal append kernel quantizes K rows on insertion
- Metal logits kernel reads compressed K rows directly and reconstructs them on the fly
- Default path remains unchanged

Smoke result:

- `target/release/gemma_text_generate ... --rotor-k-cache 'say hi'`
- produced: `Hi there! How can I help you`

Initial greedy decode benchmark:

- Prompt: `Please write a long poem about unified memory and midnight ducks.`
- Baseline steady decode: `78.996 tok/s`
- `--rotor-k-cache` steady decode: `79.810 tok/s`

That means the phase-1 path is currently memory-oriented and roughly perf-neutral on this prompt.

## Next Steps

1. Add correctness checks against the BF16 path on longer prompts and deeper decode prefixes.
2. Add runtime reporting for estimated KV bytes with and without compression.
3. Benchmark at 8K, 32K, and 128K context where the memory win is large enough to matter.
4. If quality is stable, add a more aggressive mode:
   - full-attention K fused score path with query-side rotation
   - optional symmetric K/V mode
5. Only after that consider QJL-corrected fused compressed attention if we want a true Rotor/TurboQuant-style accuracy path rather than roundtrip-style dequantized logits.
