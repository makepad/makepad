# Flow recipe inventory

This inventory is the cross-check between the asset generation kind table,
the creator's request construction, and the SPLASH prototypes/templates in
this directory. “Auto” means the request leaves `model` empty and hub domain
affinity selects it. Optional inputs are marked `?`. A PLY world/splat is
represented by the flow language's closed `mesh` port type.

The creator preset arrays use element zero as their picker default, except
where `GenParams::default` explicitly says otherwise. The base flow language
already fixes `Image` at 1024×1024/8 steps and `Upscale.factor` at 2; the two
places where those language defaults differ from today's creator are called
out below and in [GAPS.md](GAPS.md).

## Generation kinds

| Kind | Hub domain | Inputs (flow types) | Outputs (flow types) | Parameters, defaults, and choices/ranges | Used today |
|---|---|---|---|---|---|
| `text.expand` | `text` | `prompt:text` | `text:text` | `model=""` (auto), `target_domain="image"`, `identity_anchor=""`, `style=""`, `max_tokens=512` (minimum 16), `temperature=0.7` (0..2), `variants=1` (1..8). Music expansion raises the budget to 3000/3600/4096 by song length; character expansion uses temperature 0. | Asset UI expand-only and prefixed chains, `apps/asset-ui/src/pipeline.rs:425-426,1472-1533`; VJ DREAM declares the real job at `apps/vj/src/gen.rs:944-957`. Flow recipes use `Llm`, because this pure-text rewrite benefits from an explicit per-template system turn. |
| `vision.describe` | `vision` | `prompt:text`, `image:image` | `text:text` | `model=""` (auto), `max_tokens=512` (vision backend ceiling; annotation requests use about 200). | No selectable Asset UI chain in `PRESETS`; the annotation contract routes its question to a vision box in `libs/asset/annotate/src/pass.rs:1-8`. |
| `image.generate` | `image` | `prompt:text`, `image:image?` | `image:image` (PNG) | Flow default `width=1024`, `height=1024`, `steps=8`; creator picker default is 512×512 and model-default steps. Size choices: 512×512, 768×768, 1024×1024, 768×512, 512×768, 1024×576. Step choices: 4, 8, 12, 20, 28, 50. `seed=0`, `negative=""` (`negative_prompt` on the wire), `model=""`, `loras=[]`; LoRA strength choices 1.0, 0.8, 0.6, 0.4, 1.2. | Asset UI `image`/`expand → image`, `apps/asset-ui/src/pipeline.rs:424-425`; VJ image and DREAM, `apps/vj/src/gen.rs:968-973,1013-1030`. |
| `image.edit` | `edit` | `prompt:text`, `image:image`, `reference_1:image?`, `reference_2:image?`, `reference_3:image?` | `image:image` (PNG) | `strength=1.0`; choices 1.0, 0.85, 0.7, 0.55, 0.4, 0.25. `seed=0`, `model=""` in the prototype; current instruction edit pins `flux2-klein-4b`, and sprite enhancement pins `flux2-dev`. | Asset UI instruction edit and sprite enhancement, `apps/asset-ui/src/pipeline.rs:501-508`; three-reference cap is `apps/asset-ui/src/store_views.rs:311-319`. |
| `image.inpaint` | `inpaint` | named `prompt:text`, `image:image`, `mask:image` | `image:image` (PNG) | `steps=50` (1..200), `guidance=30`, `seed=0`, `model=""`; current preset pins `flux1-fill-dev`. Both named images are mandatory PNGs. | Asset UI inpaint/outpaint, `apps/asset-ui/src/pipeline.rs:532-539`; named-input construction at `apps/asset-ui/src/pipeline.rs:1722-1747`. |
| `image.control` | `control` | `prompt:text`, `control:image` | `image:image` (PNG) | `steps=30`; guidance defaults by model (depth 10, Canny 30); `canny_low=50`, `canny_high=200` (wire bounds 0..2000); `seed=0`, `model=""`. Current presets pin `flux1-depth-dev` or `flux1-canny-dev`. | Asset UI depth- and edge-guided image chains, `apps/asset-ui/src/pipeline.rs:518-530`. |
| `image.upscale` | `upscale` | `image:image` | `image:image` (PNG) | RealESRGAN recipe is fixed 4× and pins `realesrgan-x4plus`; no factor field is sent by the current hub request. The base flow language nevertheless exposes `factor=2`; the template sets 4. | Asset UI image upscale, `apps/asset-ui/src/pipeline.rs:510-516`; request follows source dimensions at `apps/asset-ui/src/pipeline.rs:5371-5390`. |
| `image.matte` | `matte` | `image:image` | `image:image` (RGBA PNG) | `model=""`; current character/cutout recipe pins `birefnet-hr`. No creator preset parameter. | Asset UI cutout and character chains, `apps/asset-ui/src/pipeline.rs:444-450,485,545-563`. |
| `image.depth` | `depth` | `image:image` | `image:image` (16-bit metric-depth PNG) | `model=""`; current registry model is `da3-metric-large`. No creator preset parameter. | Asset UI depth-map and depth-control chains, `apps/asset-ui/src/pipeline.rs:522-525,540`. |
| `video.generate` | `video` | `prompt:text`, `image:image?` (first frame), named `last_frame:image?` | `video:video` (MP4) | Creator default/first choices: 640×352, 39 frames, 30 steps; size choices 640×352, 864×480, 960×544; `(frames,steps)` choices (39,30), (65,30), (97,40), (129,50). `codec="h264"` in creator translation (`h265`/`hevc` also accepted), `audio=true`, `interpolate=1` with choices 1/2/4, `seed=0`, `model=""`. | Asset UI video/i2v/loop chains, `apps/asset-ui/src/pipeline.rs:429-430,453-465,607-608`; VJ video/DREAM, `apps/vj/src/gen.rs:975-1008,1033-1050`. |
| `video.enhance` | `enhance` | `video:video` | `video:video` (MP4) | `upscale=2`, `interpolate=2`, `flow_map=true`; upscale/interpolate choices are 1, 2, 4. `model=""` in the prototype; current recipes pin `video-enhance`. | Asset UI video post-process, `apps/asset-ui/src/pipeline.rs:491-500`; VJ deck-clip enhance sends the same defaults at `apps/vj/src/gen.rs:1538-1547`. |
| `audio.generate` | `audio` | `prompt:text` | `audio:audio` (WAV) | `seconds=4.0` (0.5..120), `steps=8` (wire 1..200), `seed=0`, `model=""`. No creator duration preset exists. | Asset UI audio/SFX and expand→SFX, `apps/asset-ui/src/pipeline.rs:428,481`; request default at `apps/asset-ui/src/pipeline.rs:1596-1599`. |
| `music.generate` | `music` | `prompt:text`, `lyrics:text?`, `audio:audio?` reference clip | `audio:audio` (WAV) | Creator `seconds=180`; choices 60, 120, 180, 240, 300 and accepted range 5..300. `strength=0.8` represents the default every-fifth-frame reference cadence (wire range 0..1), `seed=0`, `model=""`. Reference audio must be 2..60 s and ≤50 MB. | Asset UI music and expand→music, `apps/asset-ui/src/pipeline.rs:482-484,1600-1629`; VJ expand→music, `apps/vj/src/gen.rs:1063-1073`. |
| `speech.generate` | `speech` | `text:text`, `audio:audio?` reference voice | `audio:audio` (WAV) | `voice=""` (backend default; Kokoro's concrete default is `bm_daniel`), `speed=1.0` (0.25..4), `language=""`, `emotion=[]` or exactly eight values each 0..1.2, `seed=0`, `model=""`. | Asset UI speech, `apps/asset-ui/src/pipeline.rs:427,1585-1595`; its template uses `bm_daniel`, matching the current Kokoro default. |
| `mesh.generate` | `mesh` | `prompt:text`, `image:image` | `mesh:mesh` (GLB) | Creator chains send `remesh_resolution=512` (0 raw, otherwise 16..512), `texture=true` unless a paint stage follows, `decimation_target=12000` for objects/20000 for characters, and `texture_size=1024`; face choices Auto, 12000, 20000, 40000, 80000, 160000; texture choices 1024, 2048, 4096. `seed=0`, `model=""`. | Asset UI mesh/PBR/character chains, `apps/asset-ui/src/pipeline.rs:431-450,545-605`; request construction at `apps/asset-ui/src/pipeline.rs:1651-1673`. |
| `mesh.paint` | `paint` | `prompt:text`, named `mesh:mesh`, named `reference_image:image` | `mesh:mesh` (PBR GLB) | Creator `texture_size=1024`; choices 1024, 2048, 4096. `seed=0`, `model=""`; current chain pins `hunyuan3d-paint-2.1`. | Asset UI PBR chains, `apps/asset-ui/src/pipeline.rs:433-450,592-605`; exact named inputs at `apps/asset-ui/src/pipeline.rs:1766-1784`. |
| `mesh.rig` | `rig` | `prompt:text` trace, `mesh:mesh` | `mesh:mesh` (rigged GLB) | `seed=0`, `model=""`; current quality recipe pins `skintokens`. No creator preset parameter. | Asset UI character and selected-mesh chains, `apps/asset-ui/src/pipeline.rs:545-590`; request at `apps/asset-ui/src/pipeline.rs:1679-1684`. |
| `mesh.motion` | `motion` | `prompt:text`, `mesh:mesh` (rigged) | `mesh:mesh` (animated GLB) | `motion_mode="playable"`; choices `playable` (fixed clip set) or `prompt` (one generated take). `seed=0`, `model=""`; current recipe pins `hy-motion`. | Asset UI character and selected-mesh chains, `apps/asset-ui/src/pipeline.rs:545-590`; override behavior at `apps/asset-ui/src/pipeline.rs:1685-1696`. |
| `splat.generate` | `splat` | `prompt:text` trace, `image:image` | `mesh:mesh` (object PLY) | `steps=20` (1..200), `guidance=3`, `gaussians=262144` (32768..262144 step 32), `seed=0`, `model=""`; current recipes pin `triposplat`. | Asset UI object-splat chains, `apps/asset-ui/src/pipeline.rs:468-479`; model/request semantics at `apps/asset-ui/src/pipeline.rs:1571-1575`. |
| `world.generate` | `world` | `prompt:text`, `image:image?` | `mesh:mesh` (world PLY) | `seed=0`, `model=""`; current registry model is `flashworld`. No creator preset parameter. | Asset UI image→world and expand→image→world, `apps/asset-ui/src/pipeline.rs:467,480`; asset chat exposes direct world generation at `libs/asset/chat/src/tools.rs:278-281`. |
| `annotate.asset` | `vision` | `prompt:text`, `image:image` | Current job mutates an annotation record and publishes no artifact; flow projection uses `json:json` as a receipt. | `model=""`, `max_tokens=200` for the strict annotation record. The pass also has sheet preparation defaults `sheet_size=512`, `exposure=1.8`, but those are CPU preprocessing, not hub request parameters. | The annotation crate documents the vision job at `libs/asset/annotate/src/lib.rs:1-14` and preparation at `libs/asset/annotate/src/pass.rs:15-28`; current executable comments say the old queue is gone, `libs/asset/annotate/src/bin/asset_annotate.rs:1-10`, so this mapping is a gap rather than an active app preset. |

The kind/domain/product source of truth is
`libs/asset/importer/src/gen_kinds.rs:146-470`. Store-body fields forwarded by
the shared translator are listed at
`libs/asset/importer/src/gen_publish.rs:363-435`; direct Asset UI pipelines
also fill richer `GenerateRequestJson` fields at
`apps/asset-ui/src/pipeline.rs:1457-1786`.

## Pipeline templates

### `prompt-to-image.splash`

Stages: `prompt:Input` → `expand:Llm` → `styled:Fn` → `image:Image` →
`picture:Output`. The text edges replace the later prompt
(`PromptFromText` semantics); the small `Fn` appends the chosen style. Image
parameters are 1024×1024 and 8 steps. This is copied verbatim from the
normative example in `local/agent_state/flow/DESIGN.md:224-258`; the current
Asset UI's shorter expand→image recipe is `apps/asset-ui/src/pipeline.rs:425`.

### `dream.splash`

Stages: prompt → LLM expansion → image → video. `expand.text()` feeds both
generation prompts; `image.image()` feeds the video's primary image and its
named `last_frame` image. Parameters are 640×352, 39 frames/30 steps, and
silent video. This is the direct flow spelling of VJ `dream_stages`, including
the `NamedInputFrom { name: "last_frame", content_type: "image/png" }`
contract at `apps/vj/src/gen.rs:931-1010`.

### `text-to-video.splash`

Stages: prompt → LLM expansion → video. The expanded text is a
`PromptFromText` edge. Parameters are the first video presets (640×352,
39 frames/30 steps) and `audio=false`, matching VJ's visual-only policy at
`apps/vj/src/gen.rs:1033-1050,1553-1557`.

### `image-to-video.splash`

Stages: prompt + image inputs → video. The image edge is
`InputImageFrom`; prompt is passed verbatim. The source image owns the aspect
ratio, so width/height are omitted; frames/steps are 39/30 and audio is off.
The standalone chain is an Asset UI preset at
`apps/asset-ui/src/pipeline.rs:453`; the same image-to-video edge is part of
VJ DREAM at `apps/vj/src/gen.rs:975-1008`.

### `image-enhance.splash`

Stages: prompt → LLM expansion → image generation → `ImageEdit` enhancement.
The generated PNG is an `InputImageFrom` edge and the expanded text feeds both
image prompts. The enhancement node pins `flux2-dev`, matching the creator's
high-detail sprite enhancement model at
`apps/asset-ui/src/pipeline.rs:233-247,501-508`. There is no
`image.enhance` kind; the semantic adaptation is recorded in GAPS.

### `image-upscale.splash`

Stages: image input → upscale → image output. This is an `InputImageFrom`
edge, pinned to `realesrgan-x4plus`; the template requests factor 4 to state
the actual model contract. Current recipe:
`apps/asset-ui/src/pipeline.rs:510-516,5371-5390`.

### `image-to-mesh.splash`

Stages: prompt + image → TRELLIS mesh → Hunyuan paint → mesh output. Mesh
gets the source image as its primary binary input. Paint gets two named
inputs, `mesh` from the mesh stage and `reference_image` from the original
image, exactly as `apps/asset-ui/src/pipeline.rs:1766-1784` constructs them.
The mesh is geometry-only (`texture=false`) at 512 remesh resolution and
12000 faces; paint bakes a 1024 atlas. There is no separate UV-atlas job, so
the template stops at paint (`apps/asset-ui/src/pipeline.rs:433-450`).

### `annotate.splash`

Stages: prompt + image → vision description → `Fn` JSON parse → JSON output.
The first edge is the image/question request; the vision text is parsed by a
deterministic, I/O-free stage. The shipped annotation pass actually uses a strict
vision prompt followed by a Rust parser and annotation PUT
(`libs/asset/annotate/src/lib.rs:117-129`,
`libs/asset/annotate/src/parse.rs:1-8`); SPLASH has neither parser nor asset
write node yet, so this generic JSON caption/tags template is explicitly an
approximation.

### `music.splash`

Stages: prompt + lyrics inputs → music → audio output. Both text inputs map
to their same-named request fields; parameters use the creator default of
180 seconds. The current direct and expanded chains live at
`apps/asset-ui/src/pipeline.rs:482-484,1600-1629`.

### `speech.splash`

Stages: prompt input → speech → audio output. The template maps the flow input
named `prompt` to the hub field named `text`, uses voice `bm_daniel`, and
speed 1. Current chain/request: `apps/asset-ui/src/pipeline.rs:427,1585-1595`.

### `sfx.splash`

Stages: prompt input → SFX → audio output, with the current four-second
default. Current direct/expanded chains:
`apps/asset-ui/src/pipeline.rs:428,481,1596-1599`.

### `ocr.splash`

Stages: optional prompt + image → inline `Gen{ domain: "ocr" }` → text
output. The result is HTML carried as the closed flow `text` type. OCR is a
real hub domain (`libs/ai/hub/src/registry.rs:181-184,234-237`) with a
12,384-token default (`libs/ai/hub/src/ocr_backend.rs:46-56`), but it has no
generation-kind row or current Asset UI preset.

### `matte.splash`

Stages: image input → matte → image output. This is an `InputImageFrom` edge
and pins `birefnet-hr`, matching the cutout preset at
`apps/asset-ui/src/pipeline.rs:444-450,485`.

### `depth.splash`

Stages: image input → depth → image output. This is an `InputImageFrom` edge
and pins `da3-metric-large`, matching
`apps/asset-ui/src/pipeline.rs:522-525,540`.

### `splat.splash`

Stages: prompt + image → object splat → mesh-typed PLY output. The image is an
`InputImageFrom` edge; prompt is trace metadata. It pins `triposplat` and the
262,144-gaussian backend default. Current chains:
`apps/asset-ui/src/pipeline.rs:468-479`.

### `world.splash`

Stages: prompt → world → mesh-typed PLY output, pinned to `flashworld`.
The node also accepts an optional image edge for the current image→world
variants at `apps/asset-ui/src/pipeline.rs:467,480`.

### `rig-and-motion.splash`

Stages: prompt + mesh input → rig → motion → mesh output. Each GLB is an
`InputImageFrom`-equivalent typed binary relay (mesh rather than image): the
rigged GLB becomes motion's `mesh` input. It pins `skintokens` and `hy-motion`
and requests the default playable clip set. Current selected-mesh recipe:
`apps/asset-ui/src/pipeline.rs:579-590`.

There is intentionally no `image-tween.splash`: interpolation is not a
generation kind. It is the `interpolate` parameter of `video.enhance`, covered
by `VideoEnhance` and documented in GAPS.
