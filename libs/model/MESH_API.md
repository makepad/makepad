# Mesh editing operations

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
| flip_faces | `faces`: face ID strings; reverse selected face winding and explicit normals while retaining IDs, UVs, pins, materials and weights |

`subdivide` also respects crease strengths: fractional values blend the smooth
and sharp stencils; fully sharp corners remain fixed. Pins protect projected UVs.
Pinned unwrap retains the existing boundary layout and relaxes its interior;
it refuses a collapsed constrained layout. `Mesh::uv_islands` exposes face/corner
membership, bounds, area, and pin counts for editor inspection.

New `sphere` and `cylinder` primitives default to `smooth:true`. Spheres store
analytic radial corner normals, including their poles; cylinder walls store
radial normals with separate flat cap normals. UV seams retain matching normals
on either side. `smooth:false` keeps faceted polygon shading. This creation
option changes shading only: positions, topology, IDs and UVs stay the same.
Saved editable meshes retain their stored normals when reopened; the new
default does not restyle existing assets.

For existing meshes, `normals {smooth:true,angle:1.0471975512}` derives normals
across edges below 60 degrees, keeping sharper boundaries. Crease edges still
split the result. This is separate from `smooth`, which moves vertices. Other
construction paths such as `polygon_mesh`, `sweep`, `lathe` and `loft` retain
their existing shading until normals are explicitly authored or recalculated.
Whole-object affine geometry transforms preserve normals with the inverse
transpose, including nonuniform scales. Partial geometry edits may invalidate
affected normals; recalculate them after shaping when needed.

Gloss is a material property: for example,
`surface_material {material:0,metallic:0,roughness:0.08}` gives a glossy
dielectric factor. Smooth normals do not add silhouette subdivisions, and low
roughness does not add a clearcoat layer, transmission or mirror reflections.
Roughness textures multiply the material factor. Lighting and reflection
support in the receiving renderer determine the visible highlight.

# Strong surface and solid inspection

`inspect {domain:"solid", object:...}` runs the explicit bounded global check.
The ordinary `validate` domain checks local faces and oriented topology and
leaves global intersections `NotChecked`; its solid flag is conservative false.
The strong check reports distinct intersecting face pairs (first 128 plus exact
total/omitted counts), connected shell count, and shell orientation errors.
Legal shared vertex/edge boundaries are excluded; geometric touching between
different element identities counts as an intersection. Closed embedded shells
must face outward at even nesting depth and inward around cavities. Disjoint
outward components are accepted. Cancellation, exhausted work/memory budgets,
or unsupported numerical ranges return an error without a certificate.

Rust: `Mesh::validate_global(&mut Context) -> Result<GlobalValidation>` returns
`local`, `candidate_pairs`, `tested_pairs`, `intersection_count`,
`intersections: Vec<FaceIntersection {a,b}>`, `intersections_omitted`,
`components`, `orientation_errors`, `is_embedded_surface`, `is_valid_solid`.
