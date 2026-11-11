# Third-party notices — `makepad-ai-splat`

The code in this crate is our own and is licensed MIT, like the rest of the
repository (see the `license` field in `Cargo.toml` and the repository
`LICENSE`). This file records the third-party work our model port is
*implemented after*, and the attribution those upstreams ask for.

Nothing listed here is vendored. No third-party source file is copied into
this crate, and no model weights are redistributed by this repository —
operators pull checkpoints at runtime under the checkpoints' own licenses.

---

## TripoSplat — the whole pipeline (`src/*.rs`)

**Implemented after: VAST-AI-Research/TripoSplat, MIT License.**

> The preprocessing, the DINOv3 conditioner, the FLUX.2 VAE condition, the
> `LatentSeqMMFlowModel` denoiser with its RePo rotary layers, the flow
> sampler, the octree and Gaussian decoders and the PLY layout are
> transcribed from the released `model.py` / `triposplat.py` of
> <https://github.com/VAST-AI-Research/TripoSplat>, whose license reads:
>
> > MIT License
> >
> > Copyright (c) 2026 VAST
> >
> > Permission is hereby granted, free of charge, to any person obtaining a copy
> > of this software and associated documentation files (the "Software"), to deal
> > in the Software without restriction, including without limitation the rights
> > to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> > copies of the Software, and to permit persons to whom the Software is
> > furnished to do so, subject to the following conditions:
> >
> > The above copyright notice and this permission notice shall be included in all
> > copies or substantial portions of the Software.
> >
> > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> > IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> > FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> > AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> > LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> > OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> > SOFTWARE.

The DINOv3 conditioner follows TripoSplat's `DinoV3ViT`, which has the
semantics of Hugging Face `transformers`' `modeling_dinov3_vit` (Apache
License 2.0).

## Idol — the `SplatModel::Idol` differences (`src/splat.rs`, `src/splat_flow.rs`, `src/splat_decoder.rs`, `src/splat_pipeline.rs`)

**Implemented after: speridlabs/idol-sdk, Apache License 2.0.**

> Idol runs TripoSplat's architecture. Where its released inference code
> differs — the latent anchor positions read from the checkpoint's `pos_pe`
> buffer, the FLUX.2 VAE standardisation with the VAE's own batch-norm
> epsilon, the `(1 + w)·cond − w·uncond` guidance (our `s = 1 + w`), its
> sampling defaults and its Gaussian budget — the port follows
> <https://github.com/speridlabs/idol-sdk> (commit `8db70034`), licensed under
> the Apache License, Version 2.0,
> <http://www.apache.org/licenses/LICENSE-2.0>. Its denoiser file
> (`idol/models/dit.py`) states that it is adapted from Hugging Face
> `diffusers`' `transformer_wan.py`:
>
> > Copyright 2025 The Wan Team and The HuggingFace Team. All rights reserved.
> > Licensed under the Apache License, Version 2.0 (the "License");
> > you may not use this file except in compliance with the License.
> > You may obtain a copy of the License at
> > <http://www.apache.org/licenses/LICENSE-2.0>
>
> Changes: transcribed from Python/PyTorch to Rust on the Makepad CUDA
> stack; one pipeline serves both releases, with the differences above as
> per-model data.

## BiRefNet — background removal

The matte runs through `libs/ai/models/vision`'s native BiRefNet; its
notice is in that crate's `THIRD_PARTY_NOTICES.md`.
