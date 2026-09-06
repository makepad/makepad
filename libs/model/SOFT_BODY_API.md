# Soft-body authoring

Open with `model.open {document:"yarn",max_joints:128}` before building or
reopening a soft-body source by alias. The default is 64 joints. A document's
explicit budget is immutable while open; close it and reopen its source to
change the budget. Other documents keep their own limits.

`soft_body_bind` is a worker-side atomic modeling operation:

```json
{
  "op": "soft_body_bind",
  "object": "body",
  "preset": "yarn_ball",
  "deform_objects": ["fuzz"],
  "attachments": [
    {"name":"face","objects":["sclera","iris","pupil"],"pivot":[0,0.8,-0.4]},
    {"name":"arm-frame","object":"arm","pivot":[0.4,0.7,0]}
  ],
  "config": {"mass":1,"friction":0.5}
}
```

- The body must be a closed manifold with three nonzero dimensions. It is
  preserved exactly at rest. The operation creates an enclosing ellipsoid cage
  with 43 particles and 80 tetrahedra, derived from an inflated icosphere shell.
- Every body/deform vertex is embedded into one tetrahedron and receives one
  full-weight affine skin joint. Additional deform objects enlarge the enclosure
  when necessary. Contact samples come only from original visible body vertices,
  never the cage or extra fiber meshes. At most 128 contacts are retained.
- Attachments use one rigid frame each, with a proper polar rotation and no
  inherited cage shear or stretch. Use either `object` or `objects` per group.
  Pivots are model-space coordinates; omitted pivots use the group's world-space
  bounding-box center. Group objects cannot appear in multiple bindings.
- A new document uses 81 joints plus one per attachment. Existing skeleton joints
  are preserved, then 80 cell joints and attachment frames are appended. The
  resulting total must fit the document budget and the 128-joint authoring cap.
  The result reports `attachment_joint:<name>` ordinals for subsequent rigging.
- Add ordinary child joints beneath the returned attachment frames for gaze,
  blink, and limb animation. Rebind an entire eye or limb with the compact
  `weight_edit {object,vertices:[],edit:{type:"assign",joint:N}}` operation.
  A positive locked weight on another joint rejects assignment. Animating
  solver-owned cell/frame joints is refused; animate their children.

The only supported optional preset is `yarn_ball`; the only supported optional
`cage` is `icosphere_1`. Config accepts partial overrides of current simulator
defaults: mass, edge_compliance, volume_compliance, pose_compliance, damping,
gravity, contact_radius, friction, substeps, iterations, max_speed, and
max_displacement. Unknown fields and unsupported values fail before mutation.
Substeps are 1..8 and iterations 1..16. Full bounds are enforced by the shared
metadata and physics settings validators.

Bound objects must be unique baked meshes without live modifiers, linked
instances, LODs, or shape keys. Finish geometry before binding. Later geometry,
transform, cell-weight, or frame-rest changes are rejected atomically. Use
`{"op":"soft_body_unbind"}` before changing those inputs, then call
`soft_body_bind` again with the desired groups/config. Both can surround edits
in one atomic transaction, or run as separate transactions. This also works
after reopening a published snapshot without any undo history.
Material and texture work remains editable while bound.
Rigid attachment weights may move only within that attachment's child hierarchy.
The skeleton root must retain identity rest rotation and scale.

Unbinding removes simulator metadata and keeps the ordinary rest skin, all joint
ordinals, child joints, clips, poses, constraints, locks, and joint attachments.
The cell joints are static identity skin frames until bound again. Rebinding
reuses the complete `__soft_body_tet_00` through `__soft_body_tet_79` block and
existing rigid attachment frames with matching names, so repeated binds do not
consume more joints. Those cell names are reserved; incomplete blocks or changed
cell rest frames are rejected, rather than silently replacing a user's rig.
Keep attachment names stable to preserve their frame identity. Renaming a group
requests a new frame and is subject to the ordinary joint budget.

Rebinding recomputes the cage/body weights and moves each reused rigid frame to
its requested or derived pivot. Local descendant rest/animation offsets and
weights inside that frame's hierarchy remain intact. New vertices without
weights in that hierarchy are bound to the rigid frame. A group omitted from the
new binding leaves its previous frame and descendants as ordinary static rig
joints, preserving references; adding the same named group back reuses it.
Authored animation on solver-owned frames must be removed before rebinding.

Every tetrahedron-influenced vertex, including newly added accessory objects,
must have one full-weight influence naming its enclosing cell. Invalid blended,
outside-cage or wrong-cell weights fail the authoring transaction before export.
Use a rigid frame or its child joint for ordinary accessories. Cage validation
and rest inverses are prepared once per embedding batch; weight checks remain
independent of the geometry fingerprint.

The portable GLB carries version 1 `MAKEPAD_soft_body` extras on its skin owner.
Metadata contains immutable rest positions, tetrahedra, contact bindings,
anchors, settings, cell-joint ordinals, rigid attachment bindings/pivots/groups,
and the source geometry fingerprint. It contains no live simulator state.
Every client creates its own retained worker-owned simulator. Cell output matrices
map rest-model positions directly to deformed-model positions; they are already
final skin matrices. Normals require inverse-transpose affine transformation.

Source reopening validates the fingerprint, geometry embeddings, rig frames, and
weights. GLB import validates the version, budgets, positive tetrahedra, normalized
bindings, palette ordinals, rest/inverse-bind frames, and affine vertex weights.
Malformed metadata fails before model publication. Static construction previews
omit live soft-body metadata; finished source and GLB retain it.

`examples/yarn_character.rs` creates an editable pink yarn body, deterministic
yarn material/normal/roughness, short fibers, large eyes, a recessed smile,
raised heart strand, thin limbs, rigid attachment frames, and independent
idle/walk gaze/blink tracks. Build it in release, run the resulting example binary
with an output directory, then publish the emitted source and GLB through the
normal asset-store path.
