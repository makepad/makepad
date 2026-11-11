# Credits: vision models the AI hub fetches

The model code in this crate is our own; `THIRD_PARTY_NOTICES.md` says which published code each port follows.
The hub downloads the weights below from their publishers when the operator installs them (pinned revision, size
and sha256 in `libs/ai/hub/registry.json`); we do not host or redistribute them. Each is used under its licence.

## MapAnything (`mapanything-apache`, `mapanything`)

- Authors: N. Keetha, N. Müller, J. Schönberger, L. Porzi, Y. Zhang, T. Fischer, A. Knapitsch, D. Zauss,
  E. Weber, N. Antunes, J. Luiten, M. Lopez-Antequera, S. Rota Bulò, C. Richardt, D. Ramanan, S. Scherer,
  P. Kontschieder.
- Paper: "MapAnything: Universal Feed-Forward Metric 3D Reconstruction", arXiv:2509.13414, 2025.
- Code: https://github.com/facebookresearch/map-anything (Apache License 2.0).
- `mapanything-apache`: https://huggingface.co/facebook/map-anything-apache, Apache License 2.0,
  https://www.apache.org/licenses/LICENSE-2.0. Unmodified.
- `mapanything`: https://huggingface.co/facebook/map-anything, Creative Commons Attribution-NonCommercial 4.0,
  https://creativecommons.org/licenses/by-nc/4.0/. Non-commercial use only; the hub marks it so. Unmodified.

## DINOv2 (the image encoder inside both MapAnything checkpoints)

- Authors: M. Oquab, T. Darcet, T. Moutakanni, H. Vo, M. Szafraniec, V. Khalidov, P. Fernandez, D. Haziza,
  F. Massa, A. El-Nouby, M. Assran, N. Ballas, W. Galuba, R. Howes, P.-Y. Huang, S.-W. Li, I. Misra, M. Rabbat,
  V. Sharma, G. Synnaeve, H. Xu, H. Jegou, J. Mairal, P. Labatut, A. Joulin, P. Bojanowski.
- Paper: "DINOv2: Learning Robust Visual Features without Supervision", arXiv:2304.07193, 2023.
- Code and weights: https://github.com/facebookresearch/dinov2, Apache License 2.0. Its ViT-g/14 weights are
  the starting point MapAnything trains from; they reach us only inside the MapAnything checkpoints.

## BiRefNet (`birefnet-hr`, and the matte of `triposplat` and `idol`)

- Authors: P. Zheng, D. Gao, D.-P. Fan, L. Liu, J. Laaksonen, W. Ouyang, N. Sebe.
- Paper: "Bilateral Reference for High-Resolution Dichotomous Image Segmentation", CAAI AIR, 2024,
  arXiv:2401.03407.
- Code: https://github.com/ZhengPeng7/BiRefNet (MIT License, "Copyright (c) 2024 ZhengPeng").
- `birefnet-hr`: https://huggingface.co/ZhengPeng7/BiRefNet_HR-matting, MIT License. Unmodified.
- The splat models' matte is the general checkpoint, https://huggingface.co/ZhengPeng7/BiRefNet (MIT), fetched
  as TripoSplat's unmodified copy.
