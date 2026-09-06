# Mesh construction operations

Construction commands create a new object and keep input objects intact:

| op | Additional fields |
|---|---|
| sphere | `radius`, `segments: 3–128`, `rings: 2–128`, `smooth?: boolean` (default true); radial normals across UV seams, including poles |
| cylinder | `radius`, `height`, `segments: 3–128`, `smooth?: boolean` (default true); centered on Y, radial side normals, flat caps with a hard rim |
| sweep | `profile: XY[]` (3–128), `path: XYZ[]` (2–128), `caps`, `material`; transported local frames |
| lathe | `profile: [radius,height][]` (2–128), `axis`, `segments: 3–128`, `caps`, `material`; zero-radius endpoint poles supported |
| loft | `profiles: XYZ[][]` (2–128 equal-size rings), `closed`, `caps`, `material` |
| quad_strip | `a: XYZ[]`, `b: XYZ[]` (equal length 2–4096), `closed`, `material` |
| boolean | `a`, `b`, `mode: union/difference/intersection`; closed manifold inputs, bounded candidate pairs, unambiguous source-triangle UV/material/normal transfer |
| voxel_remesh | `source`, `resolution: 8/16/32`; bounded FaithC reconstruction with nearest-surface UV/material/weight reprojection |

Boolean and voxel inputs are transformed by their complete scene-node world
matrices (including parents); the new output object has an identity frame.
Input geometry and nodes stay unchanged. Make linked sources unique and apply
enabled modifiers before these direct construction commands.

Boolean and voxel operations refuse seam/crease/pin constraints they cannot map.
Booleans also refuse weighted intersection geometry; new intersections do not
have a unique rigging interpretation. Existing CSG/FaithC phases run sequentially
on the worker; cancellation is checked before/after those bounded phases, and
CSG inherits an installed cancellation token. Topology-changing output receives
new identities in the new object's scope. Local manifold validation does not
certify that arbitrary inputs or deformed shells have no global intersections.

Sphere/cylinder `smooth:false` selects faceted polygon shading. Smooth shading
does not increase geometry density or round a coarse silhouette. Existing
saved sources keep their stored normals. Sweep/lathe/loft and custom polygon
meshes can use a subsequent `normals` operation to derive smooth shading with
an explicit crease angle; see [Mesh editing operations](MESH_API.md).

For a glossy surface, author a low `surface_material.roughness` along with the
desired color and metallic factor; see [Surface authoring](SURFACE_API.md).
This uses the existing PBR path and does not require image generation.

## Surface fibers

`fiber_shell {object,source,count,length,width,seed,material}` creates a new
mesh of tapered closed fibers sampled by source triangle area. Counts1–4096,
length0.0001–1m, width0.00001–0.1m and at most half the length, u32 seed;
source input at most16384 triangles. Geometry, normals, root UV/color sampling
and inherited skin weights are deterministic. Fibers use source mesh coordinates;
copy its object transform to the new object when required. `length:0.012`,
`width:0.0006`, `count:1400` is a starting yarn-ball surface. This creates
ordinary geometry, not a volumetric fur shader or a strand physics simulation.
For soft bodies pass the separate mesh in `deform_objects`; only the main body
supplies contact samples. Source is preserved. Context work/byte/topology budgets
apply before allocation; seed changes intentionally create a different surface.
Each bent fiber is admitted against conservative capsules covering both of its
segments, including its embedded root cap. A spatial grid limits nearby
comparisons. Generation tries at most64 candidates per requested fiber and
refuses with an explicit packing-budget error if the requested count cannot
fit; it never silently returns fewer fibers. This keeps distinct fibers apart
in the authored rest mesh. The root still overlaps the source surface by
design, and subsequent deformation does not simulate strand self-collision.
