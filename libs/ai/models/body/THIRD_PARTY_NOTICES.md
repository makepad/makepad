# Third-party notices — `makepad-ai-body`

The code in this crate is our own and is licensed MIT, like the rest of the
repository (see the `license` field in `Cargo.toml` and the repository
`LICENSE`). This file records the third-party work our port is *implemented
after*, and the attribution those upstreams ask for.

Nothing listed here is vendored. No third-party source file is copied into
this crate, and no model weights are redistributed by this repository —
operators pull the checkpoint at runtime under the checkpoint's own license.

---

## SAM 3D Body — `src/model.rs`, `src/decoder.rs`, `src/heads.rs`, `src/pose.rs`, `src/preprocess.rs`, `src/condition.rs`

**Architecture implemented after the published paper.** SAM 3D Body: Robust
Full-Body Human Mesh Recovery (Meta AI, arXiv:2602.15989,
<https://arxiv.org/abs/2602.15989>). The promptable decoder is described
there as a Segment-Anything-style transformer decoder over image tokens
with pose, prompt and keypoint tokens, refined layer by layer through the
rig; the crop, CLIFF-style camera conditioning, ray conditioning and
perspective camera follow the paper's description of the pipeline.

The reference inference code published with the model
(<https://github.com/facebookresearch/sam-3d-body>) is released under the
SAM License and is **not** part of this crate: it was not copied, adapted or
vendored. The port was written from an architecture specification (tensor
inventory, operation order, formulas) and validated against reference
intermediate dumps produced by running that reference code on its own,
outside the repository, on a public-domain photograph.

## DINOv3 ViT-H+/16 — `src/dino.rs`

**Architecture implemented after: Hugging Face `transformers`, Apache
License 2.0.**

> The vision backbone in `src/dino.rs` is implemented after the `DINOv3ViT`
> implementation in Hugging Face `transformers`,
> <https://github.com/huggingface/transformers/tree/main/src/transformers/models/dinov3_vit>
> (`modeling_dinov3_vit.py`, `configuration_dinov3_vit.py`), the same source
> the TRELLIS conditioner in `makepad-ai-trellis` follows, whose copyright
> header reads:
>
> > Copyright 2025 The Meta AI Authors and The HuggingFace Team. All rights reserved.
> > Licensed under the Apache License, Version 2.0 (the "License");
> > you may not use this file except in compliance with the License.
> > You may obtain a copy of the License at
> > <http://www.apache.org/licenses/LICENSE-2.0>

Every architectural constant of the backbone — patch 16, width 1280, depth
32, 20 heads, the SwiGLU feed-forward of width 5120, the four register
tokens, the axial rotate-half rotary embedding with base 100, layer scale on
both residuals, LayerNorm with eps 1e-5 — is present in that Apache-2.0
source. The checkpoint's tensor names follow the same implementation.

## Momentum Human Rig (MHR) — `src/mhr.rs`

**Rig semantics implemented after: facebookresearch/MHR, Apache License 2.0**
(<https://github.com/facebookresearch/MHR>), which describes the rig's
parameterisation (identity and expression blendshapes, the model-parameter
to joint-parameter transform, the joint hierarchy with per-joint translation
offsets, pre-rotations, XYZ euler rotations and log2 scales, the pose
corrective MLP, linear blend skinning against an inverse bind pose) and
whose downloadable rig assets are Apache-2.0. The kinematics follow
Momentum (<https://github.com/facebookresearch/momentum>, MIT). The rig data
this crate loads (`mhr.*`) ships inside the model checkpoint below.

## Weights — SAM License

The production checkpoint is the Comfy-Org repack `Comfy-Org/sam-3d-body`,
`detection/sam_3d_body_dinov3_bf16.safetensors`, pinned in the hub registry
by revision, byte size and SHA-256. It is released under the SAM License
(see the model card at <https://huggingface.co/Comfy-Org/sam-3d-body>);
operators must review and comply with those terms. This repository never
downloads `facebook/*` checkpoints and never redistributes weights.

### Reference numerics

The port reproduces the reference pipeline's numerics for the body decoder
path (`inference_type = "body"`): rig vertices within 1e-4 cm, keypoints
within 1e-6 m from identical rig parameters, and end to end from an image
within 2 mm on 3D keypoints and 0.5 px on 2D keypoints, the residue being
bf16 accumulation-order noise in the backbone.
