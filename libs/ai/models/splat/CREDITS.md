# Credits: image → splat models the AI hub fetches

The model code in this crate is our own; `THIRD_PARTY_NOTICES.md` says which published code each port follows.
The hub downloads the weights below from their publishers when the operator installs them (pinned revision, size
and sha256 in `libs/ai/hub/registry.json`); we do not host or redistribute them. Each is used under its licence.

## TripoSplat (`triposplat`)

- Authors: VAST (VAST-AI-Research).
- Code: https://github.com/VAST-AI-Research/TripoSplat (MIT License, "Copyright (c) 2026 VAST").
- Weights: https://huggingface.co/VAST-AI/TripoSplat, MIT License. The denoiser, the octree and Gaussian
  decoder, and the repacked DINOv3, FLUX.2 VAE and BiRefNet files are used from that repository unmodified.

## Idol (`idol`)

- Authors: Sperid Labs (speridlabs).
- Project: https://speridlabs.com/research/idol.
- Code: https://github.com/speridlabs/idol-sdk (Apache License 2.0).
- Weights: https://huggingface.co/speridlabs/idol, Apache License 2.0,
  https://www.apache.org/licenses/LICENSE-2.0. The denoiser, octree decoder and Gaussian decoder are used
  unmodified. Idol has TripoSplat's architecture; its image encoders and matte are the same tensors as
  TripoSplat's files above, so the hub shares those downloads.

## DINOv3 (the image encoder of both models)

- Authors: O. Siméoni, H. V. Vo, M. Seitzer, F. Baldassarre, M. Oquab, C. Jose, V. Khalidov, M. Szafraniec,
  S. Yi, M. Ramamonjisoa, F. Massa, D. Haziza, L. Wehrstedt, J. Wang, T. Darcet, T. Moutakanni, L. Sentana,
  C. Roberts, A. Vedaldi, J. Tolan, J. Brandt, C. Couprie, J. Mairal, H. Jégou, P. Labatut, P. Bojanowski.
- Paper: "DINOv3", arXiv:2508.10104, 2025.
- Weights: ViT-H+/16 (`facebook/dinov3-vith16plus-pretrain-lvd1689m`), under the DINOv3 License,
  https://ai.meta.com/resources/models-and-libraries/dinov3-license. They reach the hub as TripoSplat's bf16
  repack. Commercial use is permitted; the licence's trade-control terms exclude military, weapons, nuclear and
  espionage uses. The hub shows the licence before the download.

## FLUX.2 VAE (the second image encoder of both models)

- Authors: Black Forest Labs.
- The FLUX.2 autoencoder is released under the Apache License 2.0
  (https://github.com/black-forest-labs/flux2, "FLUX.2 autoencoder"). It reaches the hub as TripoSplat's copy.

## BiRefNet (background removal for both models)

- Authors: P. Zheng, D. Gao, D.-P. Fan, L. Liu, J. Laaksonen, W. Ouyang, N. Sebe.
- Paper: "Bilateral Reference for High-Resolution Dichotomous Image Segmentation", CAAI AIR, 2024,
  arXiv:2401.03407.
- Code and weights: https://github.com/ZhengPeng7/BiRefNet, https://huggingface.co/ZhengPeng7/BiRefNet,
  MIT License ("Copyright (c) 2024 ZhengPeng"). Run by the service's native BiRefNet port in
  `libs/ai/models/vision`.
