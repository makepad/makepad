# TRELLIS.2 and Pixal3D provenance

The native implementations follow the released model architecture and
checkpoint contracts. Production execution uses Makepad's Rust/GPU stack.
Model weights are downloaded separately under their publishers' licenses.

Pixal3D projection and cascade math:
<https://github.com/TencentARC/Pixal3D/tree/f7cf38429b0bd264f1995f0f8743a88b1c728b94>
(`pixal3d/trainers/flow_matching/mixins/image_conditioned_proj.py`,
`pixal3d/pipelines/pixal3d_image_to_3d.py`).

MIT License

Copyright (c) 2026 Tencent.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

The reference workflow is ComfyUI's native Pixal3D graph. ComfyUI source
is GPL-3.0; commercial use does not make that source MIT-licensed. This
crate does not vendor ComfyUI code. Reference comparisons must record the
ComfyUI commit and checkpoint identities used.

NAF architecture and checkpoint layout:
<https://github.com/valeoai/NAF/tree/37f2dfc180f2de53d98bd601109c0da0dd6b0f43>
(`src/model/naf.py`, `src/layers/convolutions.py`, `src/layers/rope.py`,
`src/layers/attentions.py`). NAF is Apache-2.0; a copy is provided in
[LICENSE-NAF](LICENSE-NAF). The Rust implementation uses Makepad convolution
and normalization operations. The sparse CUDA sampler is a new implementation
of the reference attention and interpolation equations and needs no NATTEN.
