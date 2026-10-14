# Credits: FLUX weights the AI hub fetches

The FLUX code in this crate is our own. The hub downloads the weights below from their
publishers on first use (pinned revision, size and sha256 in `libs/ai/hub/registry.json`);
we do not host or redistribute them. Each is used under its licence.

## FLUX.1 [schnell] (`flux1-schnell`, `flux1-schnell-q4`)

- Authors: Black Forest Labs.
- Link: https://huggingface.co/black-forest-labs/FLUX.1-schnell
- Licence: Apache License 2.0. https://www.apache.org/licenses/LICENSE-2.0
- `flux1-schnell-q4` uses the GGUF Q4_K_S quantisation by city96
  (https://huggingface.co/city96/FLUX.1-schnell-gguf, Apache License 2.0). Modified by them:
  the transformer weights quantised to GGUF block formats; the weights themselves are FLUX.1 [schnell].
- `flux1-schnell` uses Comfy-Org's combined FP8 repack
  (https://huggingface.co/Comfy-Org/flux1-schnell).

## FLUX VAE (shared by the split FLUX.1 bundles)

- Authors: Black Forest Labs (`ae.safetensors` of FLUX.1).
- Source used: Kijai's BF16 cast, https://huggingface.co/Kijai/flux-fp8 (`flux-vae-bf16.safetensors`).
  Modified by Kijai: cast from F32 to BF16.
- Licence: Apache License 2.0 (as released with FLUX.1 [schnell]).

## Text encoders (shared by the split FLUX.1 bundles)

- Source used: https://huggingface.co/comfyanonymous/flux_text_encoders (Apache License 2.0 repackaging).
- CLIP ViT-L/14 text encoder (`clip_l.safetensors`): OpenAI, MIT License.
- T5 v1.1 XXL encoder (`t5xxl_fp8_e4m3fn.safetensors`): Google, Apache License 2.0. Modified by the
  repackager: cast to FP8 E4M3FN.
