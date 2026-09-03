# Recipe mapping gaps

These are deliberate, visible mismatches between today's creator/store code
and the closed flow language in the design of record.

## Kind and executor gaps

- `Gen` identifies a hub `domain`, not an Asset Server job kind. That is
  sufficient for domains with one operation, but `vision.describe` and
  `annotate.asset` both use `vision` and have different products. The
  `Vision` and `Annotate` prototypes remain distinct by `type_name`, but a
  future executor needs an explicit, validated operation/job-kind field if it
  is to reproduce store semantics rather than merely call the same model.
- `annotate.asset` is not a normal value-producing hub call. It consumes an
  existing asset, parses a strict vision answer, and writes the asset's
  mutable annotation record; `Product::Annotation` publishes no artifact.
  The prototype exposes a `json` receipt so the graph remains typed, while
  `annotate.splash` parses a generic vision-produced JSON record only. Exact
  parity needs the F8 `Asset`/`Publish` capability plus either the existing
  parser as a node or a faithful SPLASH port of it. The requested
  `apps/asset-ui/src/annotate_queue.rs` is not present in this checkout, and
  `libs/asset/annotate/src/bin/asset_annotate.rs:1-10,361-364` says the old
  queue no longer runs.
- `text.expand` is a real queued `text` job today, but the requested recipe
  vocabulary explicitly reuses `Llm`. That is a better authoring surface for
  visible per-template system instructions, but it does not expose the job's
  `target_domain`, `identity_anchor`, `style`, or `variants` contract. Add an
  `Expand`-over-`Gen` prototype only if byte-for-byte job parity becomes more
  important than the “use `Llm`” design decision.
- OCR is a registered hub domain but has no row in `GEN_KINDS`, no catalog
  product mapping, and no Asset UI pipeline preset. `ocr.splash` therefore
  uses an inline `Gen{ domain: "ocr" }` node and returns HTML as `text`.
  Promote OCR into `GEN_KINDS` before exposing it through store-backed
  generation/publish paths.
- The shared `libs/asset/creator/src/runner.rs` domain parser currently routes
  only image, video, audio, mesh, text, speech, world, matte, and depth
  (`runner.rs:98-111`). Newer advertised kinds such as edit, upscale, control,
  inpaint, enhance, splat, vision, paint, rig, and motion are therefore not
  callable through `generate_bytes` even though Asset UI can call their hub
  domains directly. Flow's generic domain parser should use `Domain::parse`
  instead of copying that incomplete match.

## Recipe-shape gaps

- There is no `image.enhance` kind. Asset UI's “sprite → enhance (hi-res)” is
  `image.edit` pinned to `flux2-dev`, so `image-enhance.splash` uses
  `ImageEdit` for its final stage. A dedicated image-enhance prototype would
  lie about routing unless a new generation kind/domain is added.
- There is no tween kind and therefore no `image-tween.splash`. Frame
  interpolation is `video.enhance.interpolate` with choices 1/2/4; generated
  video has the same optional parameter. A focused video-tween template can
  later be a one-node `VideoEnhance{ upscale: 1 interpolate: 2 flow_map:
  false }` recipe without inventing a kind.
- UV atlasing is not a hub kind. TRELLIS creates xatlas UV0 as part of
  retopo, and Hunyuan paint unwraps/bakes inside `mesh.paint`; the
  image-to-mesh template correctly stops at `Paint`. If atlas generation ever
  becomes independently schedulable, add a domain/kind and a mesh→mesh node
  rather than exposing an inert stage.
- Fan-out image selection (`fleet images → choose → video`) has no template
  because F8's `Map` and the run-time `Ask` choice gate are not part of this
  lane's recipe prototype set. The linear image→video template covers the
  selected artifact after that gate.
- The closed port type vocabulary has no syntax for giving a second image
  input a wire-specific name. DREAM therefore feeds its generated keyframe
  through `Video.image` but cannot also send the same image as `last_frame`;
  exact loop closure needs typed named ports. `Paint.image` likewise maps to
  the hub's `reference_image` field in the executor.
- The same limitation means `ImageEdit` cannot expose `reference_1..3`, and
  `Music` cannot type a separate `lyrics` text port. The music template folds
  lyrics into its prompt with a pure `Fn`; edit retains its primary image.
  `Inpaint` was removed from the recipe prelude because both its source image
  and mask are mandatory and cannot be represented truthfully until typed
  named ports exist. `Control.image` maps to the hub's `control` field.
- The full playable-character recipes include matte, bounded image/mesh
  retries, rig/motion quality gates, and optional paint. The requested
  `rig-and-motion.splash` starts from an already accepted mesh and therefore
  does not encode those retry policies. They need control-flow/retry nodes,
  not hidden behavior in `Rig` or `Motion`.

## Parameter gaps

- The normative flow `Image` default (1024×1024, 8 steps) differs from the
  creator picker (`IMAGE_SIZES[0] = 512×512`, with `image_steps=None` so the
  model picks its default). The prototype keeps the language-of-record
  defaults and documents all picker choices; templates override explicitly.
- The normative `Upscale.factor=2` is not a field in
  `GenerateRequestJson`, and `image.upscale` is currently fixed at 4× by
  `realesrgan-x4plus`. `image-upscale.splash` writes `factor: 4` for human
  truth, but the executor must either translate/validate this alias or omit it
  from the wire. It must never silently promise 2× while returning 4×.
- The image prototype's `negative` spelling comes from DESIGN §2, while the
  hub wire field is `negative_prompt`. The generation executor needs this
  one explicit alias. All other recipe parameter names use wire spelling.
- Presets provide no choices for vision token budgets, SFX duration/steps,
  speech voice/speed/language/emotion, control guidance/Canny thresholds,
  mesh remesh resolution/texture switch, splat gaussian count, codecs, or
  `flow_map`. Their docs use protocol/backend validation ranges and the
  defaults actually constructed by Asset UI. If inspector choices are
  required, add shared preset arrays rather than duplicating more UI-local
  constants.
- The image size choices are paired tuples, but §2 doc-range metadata is
  scalar. `width` and `height` each document the paired list; a generic
  inspector must keep them paired rather than offering their Cartesian
  product. Video `(frames, steps)` choices have the same coupling.
- Dynamic edit references are named `reference_1..N` on the hub wire and
  Asset UI caps them at three. The recipe prototype cannot type those named
  image ports today. Supporting them requires typed named ports or a typed
  list-of-image port; the current closed `list` type does not carry an element
  type.
- `Music.strength=0.8` reproduces the backend's default every-fifth-frame
  reference cadence, but the wire's literal default is absence (`None`). An
  executor may omit 0.8 rather than serialize it; both resolve to the same
  current cadence.
- `World`, matte, depth, rig, and motion expose few creator-controlled
  parameters. Model-specific settings remain hub/backend policy and should
  not be guessed into the flow prelude.

## Non-hub pipeline steps

- Resize, thumbnail derivation, GLB inspection, media probing, catalog
  dressing, provenance, rights, and publish happen after generation in
  `libs/asset/importer/src/gen_publish.rs`; none is a hub kind. Flow values
  remain scratch values under DESIGN §5.5. Persistence must be an explicit
  future `Publish` node; previews belong to faces, not hidden graph stages.
- Annotation sheet framing (`sheet_size=512`, `exposure=1.8`), strict reply
  parsing, facet normalization, and the annotation PUT are CPU/store steps,
  not vision parameters. Exact annotation should expose them as visible
  `Fn`/asset-I/O nodes once the corresponding primitives exist.
- VJ tags (`loop`, `dream`, `enhanced`), `loop_closure`, source revisions,
  and publish namespaces are store metadata, not hub model inputs. Templates
  preserve the generation edges; a later explicit `Publish` node should own
  those fields.
