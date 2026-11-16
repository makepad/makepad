# Credits

The CORTAO modules derive from CORTAO (Circular Occluder Ray-Traced Ambient
Occlusion), reference implementation v1.0.2, by Daniel Isheden
(https://github.com/theagentd/CORTAO, commit `f1b18d7772`).

    MIT License

    Copyright (c) 2026 Daniel Isheden

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

Source mapping:

- `src/cortao/params.rs` derives from `src/CORTAOParameters.h`; its mip child
  rule and test model derive from `src/shaders/CORTAOPrepass.h`.
- `src/cortao/tables.rs` derives from
  `utils/generators/OccluderLookupTableGenerator.py` (whose header still says
  v1.0.1 in the v1.0.2 release).
- `src/cortao/shaders.rs` derives from `src/shaders/CORTAOBase.h`,
  `CORTAOPrepass.h`, `CORTAOMainPass.h`, and `CORTAOFilterPass.h`.
- The spiral taps in `src/cortao/pass.rs` derive from
  `src/shaders/CORTAOMainPass.h`.
- The temporal resolve (`DrawCortaoResolve` and its stage in `CortaoPass`) is
  not part of CORTAO.

Changes: translated to Rust, with CPU calculations in f64 and uniforms in
f32. Parameters derive from a decomposed camera (clip matrix RQ into
K[R|t]) instead of a projection matrix and its inverse, with one skew term
CORTAO lacks, y-down texture coordinates, and positive orthographic linear
depth. Singular, non-finite, and unrepresentable cameras return `None`,
including planar homographies with a varying denominator and an eye at
infinity, which cannot fit the orthographic parameterization.
Resolution is a pair of floats instead of packed 16-bit dimensions. The
orthographic mip bias includes the radius. The apparent-radius coefficient
is kept below one after conversion to f32, and maximum slope compensation
is clamped to at least one. The default world radius is 0.6. The README
recommends a backfaceBoost default of 0.0 while the configuration struct
uses 0.5; this port uses 0.5. An atlas layout and inverse mip child lookup
replace separate mip images. Tables are generated at runtime instead of
shipped files, with exact float masks for up to 16 rays and two exact 16-bit
float halves for up to 32 rays; file and DDS output and console messages are
omitted. Public dimensions, coordinate overflow, and ray counts are checked;
the configurable minimum mip size is at least two. Unit tests cover camera round trips,
reference parameter equivalence, atlas children, and table properties.

The GPU port uses three fullscreen Splash fragment passes, fixed device-pixel
targets, f32 arithmetic, and uniform tier, projection, and filter branches.
Only surface position and normal acquisition are host hooks; the default
reconstructs hardware depth and uses the closer neighbour on each axis (the
central difference where both neighbours step the same way by comparable
amounts: a one-pixel outlier such as z-fighting layers), with relative sliver
and zero-length guards. A frame-counter mix supplies exact
16-bit seed halves. The prepass walks the inverse child rule per atlas texel,
keeps the selected level-zero pixel's dither, clears unused atlas space, and
writes a second R32F target with 11+11-bit octahedral centre normals. Occluder
and filter normals retain the ten-bit spheremap encoding. Projection and
unprojection include skew and use explicit columns; both Frisvad bases and
singular spheremap operations are guarded, including quantization at the rim.

Spiral taps are generated in f64 on the CPU and read from a uniform array
(two vec4 per tap). Atlas reads reject borders before applying offsets and round/clamp
mip levels. The occluder LUT is a 2D stack of slices with exact float mask
halves and a clamped radius coordinate. Ray masks start at zero without the
descriptor preload; SWAR replaces bitCount. Bent normals, debug hooks, compute
dispatch ordering, and half precision are omitted. Empty pixels explicitly
write visibility one. Individual filter reads preserve the reference gather
quad order; off-image quad partners use centre depth and off-image taps weigh
zero. The pass owns its resources, supports shared tables and asynchronous
readiness/timing, and rejects invalid frame inputs. CPU helper tests and an
offscreen native-GPU scene test cover seeds, taps, planes, a crease, coplanar
layers, and orthographic projection.
