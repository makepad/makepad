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

---

## BiRefNet — `src/birefnet.rs`, `src/birefnet_model.rs`

**Architecture implemented after: `ZhengPeng7/BiRefNet`, MIT License, with
its Swin Transformer backbone after `microsoft/Swin-Transformer`, MIT
License.**

> The Swin-L backbone, the two-scale input concatenation, the ASPP-deformable
> squeeze block and the decoder in `src/birefnet_model.rs` are implemented
> after BiRefNet, <https://github.com/ZhengPeng7/BiRefNet>, whose license
> reads "MIT License, Copyright (c) 2024 ZhengPeng", and the Swin
> Transformer, <https://github.com/microsoft/Swin-Transformer>, "MIT License,
> Copyright (c) Microsoft Corporation". Both grant use, copy, modification
> and distribution provided the copyright notice and permission notice are
> included; the full MIT text is the repository `LICENSE`'s.

---

## MapAnything — `src/mapanything.rs`, `src/mapanything_heads.rs`, `src/mapanything_prep.rs`

**Architecture implemented after: `facebookresearch/map-anything`, Apache
License 2.0, built from `castacks/UniCeption` 0.1.7, BSD 3-Clause License.
The dense and pose heads are written from papers only.**

> The graph in `src/mapanything.rs` and the input preparation and output
> geometry in `src/mapanything_prep.rs` are implemented after MapAnything,
> <https://github.com/facebookresearch/map-anything>
> (`mapanything/models/mapanything/model.py`, `mapanything/utils/geometry.py`,
> `mapanything/utils/inference.py`), whose copyright header reads:
>
> > Copyright (c) Meta Platforms, Inc. and affiliates.
> > This source code is licensed under the Apache License, Version 2.0
> > found in the LICENSE file in the root directory of this source tree.

The parts of that graph follow these sources:

| Part | Source | License, as the source states it |
| --- | --- | --- |
| Geometric input encoders, alternating-attention transformer, scale head | UniCeption 0.1.7, <https://github.com/castacks/UniCeption/tree/v0.1.7> | BSD 3-Clause, "Copyright (c) 2024, AirLab Stacks" |
| DINOv2 ViT-g/14 encoder | DINOv2, <https://github.com/facebookresearch/dinov2>, which UniCeption loads | Apache 2.0, "Copyright (c) Meta Platforms, Inc. and affiliates." |
| Dense head (`mapanything_heads.rs`) | Written from the papers: R. Ranftl, A. Bochkovskiy, V. Koltun, "Vision Transformers for Dense Prediction", ICCV 2021, arXiv:2103.13413; N. Keetha et al., "MapAnything: Universal Feed-Forward Metric 3D Reconstruction", arXiv:2509.13414 | Our own code |
| Pose head (`mapanything_heads.rs`) | Written from the papers: S. Dong et al., "Reloc3r: Large-Scale Training of Relative Camera Pose Regression for Generalizable, Fast, and Accurate Visual Localization", arXiv:2412.08376; S. Chen et al., "Map-Relative Pose Regression for Visual Re-Localization" (MaRePo), CVPR 2024, arXiv:2404.09884; MapAnything, above | Our own code |
| Quaternion conversion in `mapanything_prep.rs` (`rotation_to_quaternion`: the square-root-of-positive-part form and the sign standardisation) | map-anything `utils/geometry.py`, whose functions have the form of PyTorch3D's `matrix_to_quaternion` / `standardize_quaternion`, <https://github.com/facebookresearch/pytorch3d> | PyTorch3D: BSD 3-Clause, "Copyright (c) Meta Platforms, Inc. and affiliates." |

The dense and pose heads were written in a clean room: a specification written
from the papers above, the checkpoint's tensor names and shapes, and the
interface the rest of the graph gives the heads; an implementer who read only
that specification; and black-box comparison of outputs. No head code of
UniCeption, CroCo, DUSt3R or MultiMAE was read by either author.

### UniCeption license

> BSD 3-Clause License
>
> Copyright (c) 2024, AirLab Stacks
>
> Redistribution and use in source and binary forms, with or without
> modification, are permitted provided that the following conditions are met:
>
> 1. Redistributions of source code must retain the above copyright notice, this
>    list of conditions and the following disclaimer.
>
> 2. Redistributions in binary form must reproduce the above copyright notice,
>    this list of conditions and the following disclaimer in the documentation
>    and/or other materials provided with the distribution.
>
> 3. Neither the name of the copyright holder nor the names of its
>    contributors may be used to endorse or promote products derived from
>    this software without specific prior written permission.
>
> THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
> AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
> IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
> DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
> FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
> DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
> SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
> CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
> OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
> OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

### PyTorch3D license

> BSD License
>
> For PyTorch3D software
>
> Copyright (c) Meta Platforms, Inc. and affiliates. All rights reserved.
>
> Redistribution and use in source and binary forms, with or without modification,
> are permitted provided that the following conditions are met:
>
>  * Redistributions of source code must retain the above copyright notice, this
>    list of conditions and the following disclaimer.
>
>  * Redistributions in binary form must reproduce the above copyright notice,
>    this list of conditions and the following disclaimer in the documentation
>    and/or other materials provided with the distribution.
>
>  * Neither the name Meta nor the names of its contributors may be used to
>    endorse or promote products derived from this software without specific
>    prior written permission.
>
> THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
> ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
> WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
> DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
> ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
> (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
> LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
> ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
> (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
> SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

### Weights

This repository neither ships nor mirrors MapAnything weights. MapAnything
publishes two checkpoints, licensed by their training data:
`facebook/map-anything` (CC-BY-NC 4.0) and `facebook/map-anything-apache`
(Apache 2.0). The operator installs one and accepts its license.
