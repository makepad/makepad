# Recipe mapping gaps

Lane F18 closes the creator-template and advertised-domain gaps that can be
expressed as a finite flow graph. The shipped registry now contains every
linear/looped Asset UI pipeline shape and at least one template for every
domain advertised by `libs/ai/hub/registry.json`.

## Filled in F18

- Structure-guided image generation is represented by
  `image-control.splash` (DA3 depth → FLUX depth control) and
  `image-control-canny.splash` (a supplied image → FLUX Canny control).
- Video post-processing has separate intent-level templates:
  `video-enhance.splash` uses the creator defaults (`upscale=2`,
  `interpolate=2`, `flow_map=true`), while `video-tween.splash` asks only for
  interpolation (`upscale=1`, `interpolate=2`, `flow_map=false`). Both route
  through the real `enhance` domain; no fictitious tween domain was added.
- The creator's direct, expanded, keyframed, looped, edit, sprite, mesh, PBR,
  splat, world, and playable-character pipeline variants now have templates.
  Image stages use 512×512 and model-default steps, video stages use
  640×352/39 frames/30 steps, music uses 180 seconds, and the quality-model
  pins match the preset definitions.
- The previously uncovered advertised domains now have typed prototypes and
  templates: `body` (`body-pose`), `segment` (`segment` and
  `prompt-to-segment`), `stt` (`speech-to-text`), `beats` (`audio-beats`),
  `stems` (`audio-stems`), and `notes` (`audio-notes`).
- Completed values use `Publish` whenever the asset-library type supports
  them. Companion opaque values such as MIDI and a stem bundle remain typed
  `Output` values because `Publish` deliberately does not accept `bytes`.

## Fan-out and choice

The two Asset UI fan-out presets, `fleet images → choose → video` and
`expand → fleet images → choose → video`, are `fleet-images-choose-video` and
`expanded-fleet-images-choose-video`:

- `Map{type many body over bind concurrency}` runs the job its `body` wire
  names (`body: image.image()`) once per element of `over`, with `bind`
  (`@seed`) set to the element, at most `concurrency` at once, and returns
  `items` in element order. A body with `on_fail: @skip` drops the elements
  it fails on; otherwise one failure fails the Map. The body runs only
  inside its Map (the run reports it skipped, "runs inside Map").
- `Choose{type many from pick question timeout}` parks the run with
  `NodeWaiting` (options: the candidate indices) until the caller sets its
  `pick` input to an index; a `pick` given in the source or for the run
  chooses without asking. An answer is that run's choice and is not kept
  for later runs.

The creator spreads its candidates over distinct fleet boxes. A flow asks
for one job per seed and lets the fleet router place them; with
`concurrency` at the number of boxes the jobs normally land on different
boxes, but the router, not the flow, decides.

## Intentional semantic adaptations

- There is no `image.enhance` kind. The creator's sprite-enhance pipeline is
  an `edit` request pinned to `flux2-dev`, so `sprite-enhance.splash` uses
  `ImageEdit` rather than inventing a routing domain.
- OCR is an advertised hub domain but not an Asset UI generation kind.
  `ocr.splash` therefore keeps the inline `Gen{domain: "ocr"}` idiom and
  carries the backend's HTML result as typed `text`.
- `text.expand` is a queued hub domain, while authored recipes intentionally
  use `Llm` so the system instruction is visible and editable. This does not
  expose the creator job's `target_domain`, `identity_anchor`, `style`, and
  `variants` fields as a dedicated node.
- `annotate.asset` mutates an existing asset annotation record rather than
  publishing a normal artifact. `annotate.splash` performs the value-producing
  portion (vision plus strict JSON parsing); the store mutation remains an
  application operation, not a hidden graph side effect.

## Parameter and post-processing boundaries

- The base `Image` prototype remains 1024×1024/8 steps for language
  compatibility. Creator-parity templates explicitly select 512×512 and omit
  `steps`, preserving the creator's model-default setting.
- `Upscale.factor=4` is translated to the hub request's `upscale` field, and
  `Image.negative` is translated to `negative_prompt` by the executor.
- Paired image dimensions and video `(frames, steps)` choices are coupled UI
  presets. A generic inspector must not turn them into a Cartesian product.
- Resize, thumbnail derivation, GLB inspection, media probing, catalog
  dressing, provenance, and rights processing remain importer/store work.
  Templates make generation and publication explicit without pretending those
  application steps are hub domains.
