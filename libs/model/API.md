# Editable model worker API

`makepad-model` owns its state on one worker. The sandbox host handles bounded
jobs, verified asset fetch and publication through nonblocking queues.

Use metres, Y-up, local f64 authoring coordinates. Render export validates f32
range. Flat polygon faces/corners are authoritative; derived edge/radial queries
provide topology traversal. Stable element IDs and revision generations are
**decimal strings** in JSON. Material and joint ordinals are ordinary integers.

1. `model.open {"document":"robot"}` creates an empty document. The host can
   also open a stored binary source by alias. It must never replace an already
   open document without closing it explicitly.
2. Read `head: {"generation":"0","content":"<64 lowercase hex digits>"}`.
   Pass that exact head to every mutation. Generation always advances on
   edits/undo/redo; undo can restore a content hash but never an old head.
3. `model.apply {"document":"robot","request_id":"create-body",
   "expected":<head>,"operations":[{"op":"cube","object":"body",
   "size":[0.7,1.0,0.4]}]}` applies the entire batch or nothing. Results contain
   stable selections for subsequent edits. Repeating the same request ID and
   payload returns the original result; a reused ID with different payload fails.
   Result receipts retain at most 128 successful requests and 2 MiB by default;
   oldest receipts, including an oversized newest receipt, may be evicted;
   a persisted bounded request-ID ledger refuses reuse after receipt eviction.
   Exhausting the ledger is an explicit admission error, never silent reuse. Query the job when the host
   returns an accepted job ID: accepted does not mean completed or published.
4. `model.inspect` accepts `document`, optional `object`, `domain`, `offset`,
   and `limit` (1–128). Without `object`, use `overview`, `objects`, `materials`,
   `joints`, `clips`, or `clip_keys`. With `object`, use `overview`, `vertices`,
   `faces`, `corners`, `edges`, or `weights`. Face rows expose corner offsets/counts;
   page `corners` for their ordered vertex IDs, UVs and normals. Every reply is
   capped at 12 KiB of serialized JSON; follow the returned cursors described below.
   The `solid` domain performs bounded global intersection and orientation checks;
   ordinary local mesh validation explicitly leaves global solidity unchecked.
5. `model.history {"document":"robot","expected":<head>,"action":"undo"}`
   also supports redo and checkpoint. Checkpoint explicitly discards undo/redo
   history, advances generation, and preserves content and retry receipts.
6. `model.cancel {"job":"<accepted job id>"}` requests cancellation. Queued jobs
   are removed; running jobs cancel at checkpoints. Once publication commit
   begins, cancellation is too late and the result remains queryable.
7. `model.close {"document":"robot","expected":<head>}` releases the document.

Inspection pages return `items,total,offset,limit,count,omitted,next_offset,
byte_limited`. Follow the actual `next_offset` until null; restart if `head`
changes. Limits may stop pages early. `overview.inventories` supplies each
inventory's totals and preview cursor; object validation supplies
`issue_offset,issues_returned,issues_omitted,next_issue_offset`.

Vertex rows inline only a bounded weight prefix. Use `weight_offset` to page
`weights` (vertex, weight_index, joint, weight), ordered by vertex then joint.
Clip rows expose `keyframes,key_offset`; page `clip_keys` for clip, channel,
joint, path, key, time and value, ordered by clip name/channel/time. Offsets
refer to the entire domain; filter identities when crossing rows.

Apply replies retain current `head`, original `committed`, and `replayed`.
A shared 256-ID budget limits selections. `results_total,results_returned,
results_omitted,results_truncated` describe result rows. Rows contain
`result_index,object,face_count,vertex_count,selections_truncated`; full counts
include omitted IDs. `selection_face_total,selection_vertex_total,
selection_ids_returned,selection_ids_omitted` account for all results.
Selections refer to `committed`; inspection recovers surviving elements at
the current head, not intermediate elements deleted later in a batch.
Large replies never turn a successful commit into an error.

Supported operation objects (all include `op`; mesh edits also include `object`):

| op | Additional fields |
|---|---|
| cube | size: XYZ |
| plane | size: XZ |
| sphere | radius: metres, segments: 3–128, rings: 2–128, smooth?: boolean (default true); analytic radial normals, UV seam supplied |
| cylinder | radius, height: metres, segments: 3–128, smooth?: boolean (default true); centered along Y, smooth radial sides and hard caps |
| polygon_mesh | positions: XYZ arrays; polygons: [{vertices: zero-based input indices, uvs?: UV arrays, material?: integer}] |
| transform | vertices: ID strings, matrix: four row-major affine rows |
| extrude | face: ID string, offset: XYZ |
| inset | face: ID string, distance: metres; conservative planar convex inset |
| mirror | axis: 0/1/2, offset: reflection-plane coordinate; duplicates geometry |
| weld | vertices: ID strings, distance: metres |
| delete_faces | faces: ID strings |
| delete_object | none |
| uv | corner: ID string, uv: UV |
| corner_normal | corner: ID string, normal: XYZ or null to derive |
| edge_attributes | vertices: two ID strings, seam: boolean, crease: number |
| weights | vertex: ID string, weights: [{joint,weight}] |
| assign_material | faces: ID strings, material: integer |
| material | material: integer, color: linear RGB 0–1; replaces material/texture |
| texture_solid | material, width, height, color: RGB byte triple |
| paint_texture | material, center: normalized UV with top-left origin, radius: normalized distance, color: RGB byte triple |
| skeleton | joints: [{name,parent:null or earlier joint ordinal,translation: parent-local XYZ}] |
| auto_weights | object; initial rigid nearest-bone binding, editable afterward |
| subdivide | levels: 1–3; Catmull–Clark cage subdivision; preserves UV and marked edge seams; refuses creases and explicit loose-edge data |
| smooth | vertices: ID strings, iterations: 1–64, factor: 0–1, preserve_boundary: boolean |
| project_uv | faces: ID strings, axis: dropped 0/1/2 (YZ/ZX/XY), scale: UV, offset: UV |
| brush | vertices: ID strings, center: XYZ, radius: metres, delta: XYZ, max_displacement: metres; smooth grab falloff |
| clip | name, channels: [{joint,path,keys:[{time,value}]}] |
| delete_clip | name |

Skeletons have one root and parents precede children. Use `rig_rest` for arbitrary local rest translation, rotation and scale;
animation supports all three paths, FK/IK controls and portable baking.
Clip paths are `translation`, `rotation`, or `scale`. Key values have four lanes:
XYZ plus zero for translation/scale; normalized XYZW quaternion for rotation.
Times are increasing seconds. Skin export requires at most four known normalized
influences per rendered vertex and preserves each material/texture primitive.

Materials support layered PBR channels, masks, RGBA textures, alpha modes,
vertex color, emissive strength and bounded procedural/paint/bake operations.
PNG decoding and mip construction run on workers. Shape keys export real
position/normal targets and animated weights. Arbitrary shader graphs are
outside this portable material contract. Topology edits preserve corner
UV seams and vertex weights where the operation defines a mapping. Simple
nonplanar cage polygons use deterministic projected triangulation; nonplanarity
is reported, and planar inset refuses those faces. Selectors are scoped to the
expected document head and object name: an undone/recreated branch may reuse a
numeric element ID, so a selector must never be replayed against an unrelated head.

`Document::compile` produces GLB, bounds, counts and face provenance.
`Document::compile_preview` produces a static draft at the same source head,
including textures, current shape-key weights and rest-position lights while
wheel or skin binding is unfinished. The derived draft omits animation, skin,
wheel driving connections, sockets, collision and delivery LODs. It does not
modify source or relax the completion checks used for publication.
`to_bytes` stores versioned checkpoint, undo log and receipts; `from_bytes`
verifies canonical content. Publication uses `to_snapshot_bytes` to store
current editable state without cross-platform floating-point replay.
The working document retains its undo history. Transaction admission also bounds
aggregate replay work, including checkpoint decode, every retained transaction,
the most expensive undo cursor, and canonical source re-encoding. If another
edit would exceed that bound, it fails atomically with a checkpoint-required
error; call `checkpoint` explicitly to discard undo/redo before retrying. Undo
history is never silently truncated, and hostile source input keeps the same
aggregate work cap. Replay costs are measured from execution, not trusted fields
in serialized input.

The asset host publishes source and GLB in one existing `Prop` (static) or
`Character` (skinned) asset revision
(`Source/Bin`, `RenderGlb/Glb`). Alias publication requires an expected alias head
or explicit absence and a persisted asset identity. A separate expected document
head pins the intended edited content. New game clients fetch the compiled GLB;
they do not replay modeling operations. Old CSG `ModelProgram` source remains text.

Limits are provisional resource guards, not measured Quest 3 performance claims.
The default document admits 2048 objects and 512 materials, with at most 256
operations per batch. Working memory is 2 GiB, with 10 billion work units,
4,000,000 compiled triangles, 256 MiB source and 64 MiB transactions.
Undo history retains up to 4096 batches before an explicit checkpoint.
Sixteen open documents, one admitted authoring operation per host, bounded queues,
cancellation checkpoints and prepared render products prevent UI-side modeling
work. Global geometry operations have explicit resource and attribute admission rules.

The complete operation families and inspection domains are described in
[MESH_API.md](MESH_API.md), [CONSTRUCTION_API.md](CONSTRUCTION_API.md),
[SCENE_API.md](SCENE_API.md), [SURFACE_API.md](SURFACE_API.md),
[RIG_API.md](RIG_API.md), and [SELECTION_API.md](SELECTION_API.md).
`world.api` exposes these contracts to the AI. Use `surface_material` to modify
rich materials: legacy `material` refuses to discard an existing layer stack.
Deleting a clip removes its event/interpolation/morph options; deleting a morph
removes that morph's keys from all clips. Delete inbound attachments/references
in the same transaction before deleting an object they target.
