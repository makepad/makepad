# Third-party notices — `makepad-ai-vision`

The code in this crate is our own and is licensed MIT, like the rest of the
repository (see the `license` field in `Cargo.toml` and the repository
`LICENSE`). This file records the third-party work our model ports are
*implemented after*, and the attribution those upstreams ask for.

Nothing listed here is vendored. No third-party source file is copied into
this crate, and no model weights are redistributed by this repository —
operators pull checkpoints at runtime under the checkpoints' own licenses.

---

## SAM 3 / SAM 3.1 — `src/sam3.rs`, `src/sam3_model.rs`

**Architecture implemented after: Hugging Face `transformers`, Apache
License 2.0.**

> Portions of the SAM 3 architecture realised in `src/sam3.rs` and
> `src/sam3_model.rs` are implemented after the `Sam3` implementation in
> Hugging Face `transformers`,
> <https://github.com/huggingface/transformers/tree/main/src/transformers/models/sam3>
> (`modeling_sam3.py`, `configuration_sam3.py`, `image_processing_sam3.py`,
> `processing_sam3.py`), whose copyright header reads:
>
> > Copyright 2025 The Meta AI Authors and The HuggingFace Team. All rights reserved.
> > Licensed under the Apache License, Version 2.0 (the "License");
> > you may not use this file except in compliance with the License.
> > You may obtain a copy of the License at
> > <http://www.apache.org/licenses/LICENSE-2.0>
>
> The interactive / tracker mask-decoder path (`InteractiveSam`, the two-way
> attention blocks, the output hypernetworks, the random Fourier prompt
> encoding) follows the same project's Apache-2.0 `models/sam2`.

Every architectural constant in our port — 1008 input, patch 14, 24-token
windows with global attention at layers 7/15/23/31, ViT-L 1024/32/16/4736,
CLIP-L text tower 1024/24/16/4096 over a 32-token context and a 49408 vocab,
the 1024→256 resizer, the 3-level FPN, the 6-layer fusion encoder, the 6-layer
200-query DETR decoder with its presence token, learned reference points,
log-scaled box relative-position bias and sine query embedding, the dot-product
scoring head with its ±12 logit clamp, the GroupNorm(8) MaskFormer pixel
decoder and the mask-embedding head — is present in that Apache-2.0 source.

### Reference numerics

The port reproduces the exact numerics of the checkpoint pipeline it
targets (the Comfy-Org multiplex repack; validated against reference tensor
dumps of that pipeline). Several choices differ from the HF reference and
are deliberate, not bugs:

| Behaviour | HF reference | This port |
| --- | --- | --- |
| Input normalization | `Normalize(mean=0.5, std=0.5)` after rescale | none; plain `[0, 1]` RGB |
| Sine position encoding normalizer | `(pos + 1) / (N + 1e-6)` | `pos / (N - 1 + 1e-6)` |
| `inverse_sigmoid` | `log(clamp(x, 1e-3) / clamp(1 - x, 1e-3))` | `log(x / (1 - x + 1e-6) + 1e-6)` |
| LayerNorm epsilon | `1e-6` in the ViT config | `1e-5` (Torch default) |
| Post-detection mask refinement | not part of the model | crop-and-re-run pass, 2 iterations |

They are called out at their definition sites in `src/sam3.rs` and
`src/sam3_model.rs` so the divergence from the HF reference is never mistaken
for a bug. If reference parity is ever dropped, each row above should move
back to the HF form and every downstream tap re-pinned.

### Weights

Weights are pulled at runtime from the `Comfy-Org/sam3.1` repack
(`checkpoints/sam3.1_multiplex_fp16.safetensors`, pinned by revision and
SHA-256 in `src/sam3.rs`). SAM 3 model weights are covered by Meta's **SAM
License**; accepting it is the operator's act, at pull time. This repository
neither ships nor mirrors them; the loader is pinned to the Comfy-Org
multiplex header layout.
