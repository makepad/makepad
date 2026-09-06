# Object scene and ordered modifiers

All operations are transactional and require the expected document head.
Scene transforms use local TRS; attachments may target root, an object, or a joint.

`object_node` replaces the entire node; omitted parent/link fields become null
and omitted TRS components become zero translation, identity rotation and unit
scale. `dimensions` sets that node's scale from evaluated local mesh extents; it
does not resize source vertices. Place/rotate with `object_node` BEFORE calling
`dimensions`, or preserve the existing complete TRS when repositioning a sized
part. `model.inspect` domain `scene_nodes` exposes the current node transforms.

| op | Additional fields |
|---|---|
| object_node | object, node: {parent?, linked_to?, transform?} |
| instance | object, source, transform? |
| duplicate | object, source |
| make_unique | object; bakes evaluated linked geometry and clears applied modifiers |
| join | object: new name, sources: distinct object names |
| separate | object: source, name: new name, faces: ID strings |
| pivot | object, position: local XYZ; relocate origin while preserving geometry, child transforms, sockets and lights in world space |
| snap | object, grid: positive metres |
| dimensions | object, size: positive local XYZ extents |
| light | name, attachment, transform?, kind: point/spot, color: linear RGB, intensity: candela, range: metres; spot adds inner/outer cone radians |
| delete_light | name |
| socket | name, attachment, transform |
| delete_socket | name |
| vehicle_wheel | object, connection, pivot: local XYZ, radius, width; see VEHICLE_API.md |
| delete_vehicle_wheel | object |
| lods | object, levels: [{target_faces,max_error,distance}]; 1–4 levels, face counts decrease and distances increase |
| collider | object, kind: box/sphere/mesh; mesh adds source object |

A transform is {translation:XYZ, rotation:normalized XYZW, scale:nonzero XYZ}; omitted transform is identity. An attachment is null (root), {object:name}, or {joint:ordinal}. Point/spot lights are distinct emitters, not emissive materials. Counts are bounded to 32 emitters and 128 sockets. Spotlights shine along local -Z; rotate 180 degrees around Y for a +Z-facing headlight. Range is metres and intensity is candela. Parent/link/modifier cycles are refused.

`modifier` sets or replaces a named ordered entry:

```json
{"op":"modifier","object":"body","name":"copies","enabled":true,
 "operation":{"op":"array","count":3,"offset":[2,0,0]}}
```

`enabled` defaults to true. Inner `object` may be omitted; if supplied it must
match the outer object. Each object allows 16 entries and a document allows 128.
Supported entries are subdivision, mirror, array, solidify, decimate, normals,
and deform/lattice/shrinkwrap with `vertices: []`. For a modifier, that empty
selection resolves all current evaluated vertices; an ordinary edit still
treats an empty selection as empty. Fixed element selections cannot be stored
in a stack because earlier modifiers may change topology.

Use `modifier_order {object,names:[...]}` with an exact permutation to reorder,
`remove_modifier {object,name}` to remove, and `apply_modifiers {object}` to bake
the evaluated result and clear the stack/link. A linked instance starts from its
source's evaluated mesh, then applies its own entries. Parent/link/enabled
modifier dependencies form one acyclic graph. Shared references use one immutable
cache per evaluation, and source cage meshes remain unchanged until baked.
Shrinkwrap references are evaluated once, then transformed by
`inverse(owner_world) * reference_world` into the owner's local coordinates.
Its offset and distance limit use that owner-local space.

`join` copies evaluated objects through their world transforms and preserves
UVs, weights, material IDs, split normals, seam/crease markers, and pins.
`separate` preserves those attributes and the selected element IDs in the new
object scope. Linked geometry and stacks must be made unique/baked before a
face selection is separated.

Exported LODs use hidden `MSFT_lod` geometry nodes and
`extras.MAKEPAD_lod_distances`. Sandbox prepares all variants on a worker and
selects one per instance by camera distance from the instance origin in world
metres. The runtime admits up to eight distinct thresholds across an asset;
all variants share one combined upload budget. Lower variants retain node IDs,
animation targets, material layers, and the skeleton. Collision and shadow
geometry use the highest-detail mesh; changing LOD does not change physics.
Hidden targets must be leaf geometry with the owner's skin, local transform,
and node weights. Shape keys must be baked/removed on objects whose LOD geometry
is generated, since decimation changes their vertex correspondence.

Pivot edits require unique geometry with its modifiers applied; shared sources
must first have their linked instances made unique. Morph deltas and vertex IDs
survive a pivot edit. LOD distances must be at most 1,000,000 metres; unreachable
reduction targets fail compilation with an explicit protected-topology error.
Box colliders require positive volume. Worker-cooked box, sphere and mesh
proxies provide collision independent of visual LOD.
