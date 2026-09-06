# makepad-mesh-edit

A synchronous CPU polygon kernel for worker-owned asset documents. `Mesh` is
`Clone + Send + Sync`, has no shared locks, and can be published as `Arc<Mesh>`.
Vertices, faces and face corners live in flat arrays. A sorted endpoint-keyed
table holds explicit edge seam/crease attributes and loose edges. `adjacency`
builds a transient index on request; callers can cache it by document revision.

```rust
use makepad_mesh_edit::{Context, Mesh};

let mut context = Context::default();
let mut mesh = Mesh::cube([1.0, 1.0, 1.0], &mut context)?;
let front = mesh.faces()[1].id;
let extruded = mesh.extrude_face(front, [0.0, 0.0, 0.25], &mut context)?;
assert_eq!(extruded.cap, front);
let triangles = mesh.triangulate(&mut context)?;
let bytes = mesh.to_bytes(&mut context)?;
let reopened = Mesh::from_bytes(&bytes, &mut context)?;
assert_eq!(reopened.to_bytes(&mut context)?, bytes);
# Ok::<(), makepad_mesh_edit::MeshError>(())
```

Stable vertex, face and corner IDs share a monotonic allocation watermark.
Deleting an element does not make its ID reusable. IDs are local to a mesh;
the owning document supplies mesh/object/revision identity. Restoring an older
checkpoint restores its watermark, so selectors across undo branches require
the owning document's exact expected head. Immutable array
positions and corner ranges may change. `EdgeKey` consists of sorted stable
endpoint IDs, so changing either endpoint changes that edge's identity.

Every edit stages a candidate, checks its structure and geometry, and installs
it only on success. The original mesh and ID watermark survive errors,
cancellation and budget refusal exactly. `ChangeSet` reports created, retained,
deleted and explicitly remapped IDs; document checkpoints provide undo rather
than embedding another full mesh inside each change set.

Supported operations are affine selected-vertex transforms, single-face
extrusion, face deletion, whole-mesh reflection/duplication, selected-vertex
welding, strictly convex planar face inset, and weight/UV/normal/material/edge
attribute editing. Batch smoothing, grab brushes, planar UV projection and
Catmull–Clark subdivision support organic cages. Empty selections are empty,
never an implicit "all".
Import supports arbitrary simple polygon faces, independent corner UVs,
materials and normalized sparse weights. Cube and XZ plane constructors use
metres, Y-up, centered coordinates and outward winding.

Extrusion preserves the source face and corner IDs as its translated cap.
Copied vertices retain weights and the cap retains UVs. Side faces inherit the
material, receive a perimeter/height UV strip in object units, and use computed
normals. New rim edges inherit seam/crease values. Adjacent faces retain their
own discontinuous UVs. Inset preserves its cap identities and interpolates UVs
and weights from the source triangulation. Reflection duplicates the mesh,
reverses corner winding, and preserves reflected split normals and edge data.
It does not weld automatically. Weld preserves the lowest-ID representative
position, averages its group's weights, retains corner UV seams, combines
seams with OR and creases with max, and deletes collapsed faces. A surviving
pinched or self-crossing projected face refuses the entire edit.

Smoothing uses simultaneous uniform Laplacian updates, with optional fixed
boundary vertices and a maximum of 64 iterations. Grab brushes use smoothstep
radial falloff inside the requested sphere and an explicit displacement cap.
Both retain IDs, weights and UVs; affected split normals are invalidated.
Catmull–Clark supports one to three levels on oriented manifold surfaces,
retains updated original vertex IDs and maps replaced faces/corners/edges to
their children. Boundary vertices use the cubic boundary stencil. Geometry
and weights share stencils; face-local bilinear UV refinement preserves seam
discontinuities. Seam markers propagate, while nonzero geometric edge creases
are explicitly refused. Evaluated subdivision normals are averaged per vertex.

Triangulation uses checked, deterministic ear clipping with the existing CSG
adaptive-precision orientation predicate. Simple nonplanar faces are projected
using their Newell normal, allowing ordinary subdivision quads. Input winding is retained. Output
has one vertex per source corner, with position, normal, UV, weights, source
vertex/corner IDs and a source face/material per triangle. No partial fan is
returned on failure. Bounded local coordinates and normalized face projection
avoid large-coordinate arithmetic overflow. Coordinates and UVs must be
finite and at most `1e100` in magnitude. Relative face planarity tolerance is
`1e-8` for the planarity diagnostic and planar-only inset. Collapsed edges/area
below `1e-12` at the normalized face scale are rejected. Faces have one simple
boundary and no holes.

Validation distinguishes boundary edges from nonmanifold edges, checks vertex
fan connectivity (including bow ties), winding, isolated/loose elements and
individual face geometry. Open surfaces and nonmanifold neighborhoods can be
represented and inspected; extrusion requires a manifold local neighborhood.
`is_closed_manifold` describes locally valid, closed, oriented manifold topology.
**It does not certify global self-intersection freedom.** Ordinary local
validation reports `self_intersections: NotChecked`. Explicit `validate_global`
checks bounded triangle intersections, shell orientation and disconnected
components, and can establish solid validity. Nonplanarity remains a diagnostic.
The kernel includes bounded bevel, creased subdivision, UV unwrap/pack/relax,
loop editing, bridge/fill, region extrusion, deformers, sculpt and decimation.
Rig evaluation and rendering live in their respective engine crates; general
concave polygon inset remains an explicit refusal. See
[the modeling operation contract](../model/MESH_API.md) for supported domains.

`Context` applies caller-selected element, byte and work limits. Cancellation
uses an optional caller callback and does not start or wait for threads.
Work accounting is cumulative across calls; create a fresh context per job.
Working-storage estimates include candidate and operation scratch, and may
conservatively refuse an operation before reaching the configured element
limit. The default profile is provisional, not a device performance claim.

The canonical `MPMESH01` format records all attributes and the allocation
watermark in little-endian fields. Floating negative zero is encoded as zero.
The exact decoder bounds counts before allocation and rejects unknown formats,
duplicate IDs, noncanonical field ordering, malformed references, invalid
floats/flags/geometry, truncated input and trailing bytes. It does not silently
repair or reinterpret corrupt records.

Run focused validation from the repository workspace:

```sh
cargo test --release -p makepad-mesh-edit
```
