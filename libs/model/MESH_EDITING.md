# Mesh editing and evaluated modifiers

All commands run on the owning worker, use the normal expected document head,
and participate in atomic batches, undo, retry receipts, and source checkpoints.
Element selectors are decimal strings scoped to that head and object name.
An edge is `[lower_vertex_id, higher_vertex_id]`; edge factors measure from the
lower ID. Distances use local metres, angles use radians, and axes are 0/1/2.

Every command below includes `op` and `object`. Selections must be unique.

| op | Additional fields and supported neighborhood |
|---|---|
| extrude_region | `faces`, `offset: XYZ`; manifold patches with simple boundary cycles; shared interior edges stay shared |
| split_edges | `edges`, `factor: (0,1)`; shares each new edge vertex and interpolates UVs independently per face |
| collapse_edge | `edge`, `factor: (0,1)`; unmarked manifold neighborhoods; refuses face reversal and topology pinches |
| dissolve_edge | `edge`; two coplanar, same-material faces with continuous edge UVs and no edge marker |
| fill_boundary | `vertices`, `material`; one ordered simple boundary cycle |
| bridge_boundaries | `a`, `b`, `material`; ordered, disjoint, equal-length boundary cycles; starting vertices determine correspondence |
| loop_cut | `edge`, `factor: (0,1)`; maximal opposite-edge quad strip; stops at boundaries and refuses crossed/non-quad strips |
| slide_vertices | `vertices`, `towards`, `factor: [-1,1]`; equal-length pairs must share edges |
| bevel_edges | `edges`, `width`; convex, planar, closed neighborhoods with trivalent endpoints; selected neighborhoods cannot share faces; width consumes less than half a flank edge |
| solidify | `thickness > 0`; copied surface offset along averaged vertex normals, reversed inner surface, and boundary walls |
| array | `count: 1–128`, `offset: XYZ`; count includes the original |
| bend, twist | `vertices`, `axis`, `range: [low,high]`, `angle: [-2π,2π]` |
| taper | `vertices`, `axis`, `range`, `scales: [start,end]`; positive scales |
| lattice | `vertices`, `bounds: [minXYZ,maxXYZ]`, `divisions: [nx,ny,nz]`, `displacements: XYZ[]`; 2–8 controls per axis, X fastest, selected points inside bounds |
| shrinkwrap | `vertices`, `reference: object name`, `max_distance`, `offset`; nearest reference triangle plus signed normal offset; reference mapped through `inverse(owner_world) × reference_world`, result/distance/offset in owner-local units; every selected point must match |
| decimate | `target_faces`, `max_error`; triangulates and reduces unmarked manifold edges while preserving UV/material/pin boundaries; result metrics report `achieved_faces` and `collapses` |
| sculpt | `vertices`, `kind: inflate/flatten/crease`, `center`, `normal`, `radius`, `strength`, `max_displacement`, `masks: [{vertex,value}]`, `symmetry: 0–7`; mask 1 protects, XYZ symmetry bits reflect through origin, overlapping symmetry strokes do not double displacement |
| uv_box | `faces`, `scale: UV`, `offset: UV`; chooses each face's dominant normal axis |
| uv_cylindrical | `faces`, `axis`, `scale`, `offset`; unwraps angular seams per face; vertices on the cylinder axis are refused |
| uv_unwrap | `faces`; cuts at seams/60-degree dihedrals, projects planar charts, harmonically maps disk charts, splits other charts per face |
| uv_pack | `faces`, `padding`; deterministic nonoverlapping grid atlas using a common scale; pinned or collapsed islands are refused |
| uv_relax | `faces`, `iterations: 1–128`, `factor: 0–1`; fixes boundaries/pins and never averages across UV seams |
| uv_pin | `corners`, `pinned`; pins persist in source and propagate through mapped topology copies |
| normals | `smooth`, `angle: 0–π`; crease/dihedral split normals |

`subdivide` also respects crease strengths: fractional values blend the smooth
and sharp stencils; fully sharp corners remain fixed. Pins protect projected UVs.
Pinned unwrap retains the existing boundary layout and relaxes its interior;
it refuses a collapsed constrained layout. `Mesh::uv_islands` exposes face/corner
membership, bounds, area, and pin counts for editor inspection.

Construction commands create a new object and keep input objects intact:

| op | Additional fields |
|---|---|
| sweep | `profile: XY[]` (3–128), `path: XYZ[]` (2–128), `caps`, `material`; transported local frames |
| lathe | `profile: [radius,height][]` (2–128), `axis`, `segments: 3–128`, `caps`, `material`; zero-radius endpoint poles supported |
| loft | `profiles: XYZ[][]` (2–128 equal-size rings), `closed`, `caps`, `material` |
| quad_strip | `a: XYZ[]`, `b: XYZ[]` (equal length 2–4096), `closed`, `material` |
| boolean | `a`, `b`, `mode: union/difference/intersection`; closed manifold inputs, bounded candidate pairs, unambiguous source-triangle UV/material/normal transfer |
| voxel_remesh | `source`, `resolution: 8/16/32`; bounded FaithC reconstruction with nearest-surface UV/material/weight reprojection |

Boolean and voxel operations refuse seam/crease/pin constraints they cannot map.
Booleans also refuse weighted intersection geometry; new intersections do not
have a unique rigging interpretation. Existing CSG/FaithC phases run sequentially
on the worker; cancellation is checked before/after those bounded phases, and
CSG inherits an installed cancellation token. Topology-changing output receives
new identities in the new object's scope. Local manifold validation does not
certify that arbitrary inputs or deformed shells have no global intersections.

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

`join` copies evaluated objects through their world transforms and preserves
UVs, weights, material IDs, split normals, seam/crease markers, and pins.
`separate` preserves those attributes and the selected element IDs in the new
object scope. Linked geometry and stacks must be made unique/baked before a
face selection is separated.
