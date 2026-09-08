# Native TRELLIS.2 and Pixal3D

The `pixal3d` mesh backend runs image conditioning, sparse structure, low/high
resolution shape diffusion, sparse shape/texture decoders and PBR GLB export
through Rust and CUDA. It loads the combined Comfy-Org BF16 flow checkpoint by
component prefix, without extracting four copies. Checkpoints are downloaded
and SHA-256 verified through the hub registry. Production inference does not
start ComfyUI or Python.

The [Image to Pixal3D Flow](../../../flow/recipes/templates/image-to-pixal3d.splash)
connects matting, native mesh generation and the mesh output. Its JSON settings
input controls the reconstruction. The model's tensor stages remain inside the
hub backend so intermediate GPU tensors stay on the GPU worker.

## Request

Submit a mesh request to the hub with PNG input bytes in `input_b64`:

```json
{
  "model": "pixal3d",
  "input_b64": "<PNG base64>",
  "input_content_type": "image/png",
  "seed": 42,
  "texture": true,
  "remesh_resolution": 256,
  "decimation_target": 80000,
  "texture_size": 1024,
  "pixal": {
    "resolution": 1024,
    "camera_fov": 49.13,
    "structure_seed": 56,
    "texture_seed": 43,
    "shape_steps": 20
  }
}
```

`resolution` accepts 1024 or 1536. `camera_fov` is the horizontal field of view
in degrees (1–170); the default is 49.13. The main seed drives shape sampling.
Structure defaults to the main seed and texture to main seed + 1. Shape steps
control the high resolution stage (default 20, range 1–100). Native BiRefNet
segments opaque input; pre-segmented RGBA input skips that pass.

The defaults favor fast asset export: a 256 remesh grid, about 80,000 faces and
a 1024 texture atlas. Shape resolution and output mesh density are separate
controls. `remesh_resolution: 0` retains the existing raw mesh export path.

## Performance implementation

- NAF evaluates attention only at the four bilinear sample pixels consumed by
  each active voxel. It avoids the dense 1024 × 1024 × 1024 feature image
  (4 GiB at F32), dense unfolded neighborhoods and third-party NATTEN kernels.
- Image cross-attention K/V and up to 512 MiB of fixed projection outputs are
  cached across diffusion steps. Negative CFG preserves the projection bias.
- Guide transpose and 2D positional rotation are fused on the GPU. At 1024,
  this avoids constructing and uploading two 128 MiB CPU phase tables.
- Device weights reuse the existing native namespace cache; conditioning guide
  tensors are released before the corresponding diffusion stage.

This is currently a CUDA BF16 backend. The registry advertises a 24 GiB budget;
1536, larger images/meshes and device workspace behavior can require more.
Actual validation used an RTX PRO 6000 Blackwell with 96 GiB, not a 24 GiB card.

## Relationship to the ComfyUI workflow

The port follows Tencent's Pixal3D projection architecture and the Comfy-Org
combined checkpoint layout. It supports the 1024/1536 cascades and independent
sampling seeds. It is not a bit-for-bit reproduction of a ComfyUI seed:
Makepad uses its native Gaussian RNG and GPU math.

The downloaded workflow also uses features not implemented here: INT8 convrot
and 6 GiB offloading, MoGe automatic FOV, PEC UV unwrapping, and explicit normal
and AO texture baking. This backend uses manual FOV and the existing native
FaithC remesh, decimation, xatlas and color/metallic/roughness baking. Its
current shared decoders use the original Microsoft F16 checkpoints rather
than Comfy-Org's BF16 VAE repack. The downloaded 768 remesh / 700,000-face /
2048-atlas settings are not the fast defaults above (the native remesh grid is
currently capped at 512).

See [third-party notices](THIRD_PARTY_NOTICES.md) for pinned source identities
and licenses. Model and component licenses remain separate; the bundle is not
represented as uniformly MIT. ComfyUI GPL code is not vendored into this crate.

## Reproduce and check

From `libs/ai/hub`, on a CUDA build machine:

```sh
cargo build --release --no-default-features --features mesh --examples
```

Run the standalone release executable `pixal_generate` with:

```text
pixal_generate WEIGHTS INPUT.png OUTPUT.glb 1024 3
```

The optional final argument repeats generation with one loaded backend; each
trial writes `OUTPUT.glb.N.glb`. Trial 0 includes preparation/first use of model
weights. Later trials retain the device weight cache. The timer includes
matting, neural generation, mesh processing and GLB encoding; registry file
verification occurs before the timer. Weight filenames for this harness:

```text
pixal3d_bf16.safetensors
dino_naf.safetensors
ss-decoder.safetensors
shape-decoder.safetensors
texture-decoder.safetensors
native-matte.safetensors
```

All revisions, sizes and hashes come from `libs/ai/hub/registry.json`; the
harness only remaps local cache filenames. Standard hub operation uses the
ordinary registry cache paths.

Numerical checks:

```sh
# From libs/ai:
cargo test --release -p makepad-ai-trellis --lib

# Run native checkpoint-driven encoder fixture, then the independent oracle:
pixal_naf_check dino_naf.safetensors guide.f32
python pixal_guide_oracle.py dino_naf.safetensors guide.f32
```

The Python tests live in `tests/` and need PyTorch/safetensors. The complete
encoder fixture checks both cached and uncached native paths against ordinary
PyTorch convolution, group normalization, pooling, RoPE, neighborhood
attention and grid sampling. F16 tensor-core operands account for its looser
tolerance than the separate F32 sampling kernel test.

`tests/pixal_naf_oracle.py` accepts a shared library built from
`../../cuda/kernels/pixal.cu` with `nvcc -shared -O3` and the target GPU's
architecture. On Windows export `makepad_cuda_pixal_naf_sample_f32` from the
DLL. `--benchmark` measures sparse sampling only, excluding guide encoding and
the rest of the model.

## Measured run (2026-09-06)

RTX PRO 6000 Blackwell 96 GiB, CUDA 13.2, release build, Comfy's
`viking_wolf_rune_axe.png`, the 1024 request above:

| Trial | Through texture decode | Complete PBR GLB |
| --- | ---: | ---: |
| First use in a fresh process | 27.38 s | 47.76 s |
| Cached model weights, second call | 10.08 s | 32.40 s |
| Cached model weights, third call | 10.13 s | 30.49 s |

The final inspected GLB has 79,158 faces, valid UVs and two embedded 1024 PNG
textures. The 1536 cascade also completed in 68.33 s with the same fast export
settings, before the fused GPU phase change. Mesh cleanup and xatlas account
for most of the warm end-to-end time and vary between runs. These are native
measurements, not a speedup claim against an end-to-end ComfyUI baseline.

The separate F32 sparse NAF sampling oracle differed from dense PyTorch by at
most 1.1e-6, with roughly 7.3 ms sampling for 30,000 voxels. That kernel timing
excludes guide encoding. The complete checkpoint-driven encoder/NAF fixture
had maximum absolute error 0.000561 against F32 PyTorch; its native encoded
feature reuse and recomputation paths agreed exactly. A larger projection
cache and a persistent 1 GiB image encoding cache did not show a reliable
end-to-end benefit and were not retained in the production path.
