//! Static glTF/GLB props — the Kenney stock catalogue.
//!
//! A static mesh is a skinned one minus joints and weights, so this reuses
//! [`crate::skin`]'s container, JSON and accessor code rather than growing a
//! second parser that could drift from it. What differs is the baking: a prop
//! never animates, so each node's world transform is folded into its vertices
//! once at load and the whole model becomes one buffer with one draw.
//!
//! Kenney GLBs are NOT self-contained — every material points at an external
//! `Textures/colormap.png` shared by the entire pack. That is the reason a
//! whole pack draws in a single batch: same texture, so no state change.

use crate::skin::{mat4_mul_dir, mat4_mul_point, oct_encode, trs_to_mat4, Accessors, JsonParser, NodeTrs, Val};
use makepad_draw::makepad_math::{Mat4f, Quat, Vec3f};
use std::collections::BTreeMap;

/// Floats per packed vertex — matches `geom.GameMeshVertex` and the skinned
/// stream, so both paths feed the same shader.
/// pos.xyz, packed normal, packed uv, packed colour+AO, packed AO-atlas uv.
///
/// Matches `geom.GameMeshVertexAo` — a separate POD from GameMeshVertex so
/// the shadow mesh, which shares that layout and has no AO, does not pay four
/// bytes a vertex for a lane it never reads.
pub const MODEL_VERTEX_FLOATS: usize = 7;

/// A loaded prop: packed vertices ready for upload, plus where its texture
/// lives relative to the GLB.
pub struct StaticModel {
    /// Packed `geom.GameMeshVertex` floats: pos.xyz, oct-normal, f16 uv, unorm8 colour.
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    /// glTF image URI, e.g. `Textures/colormap.png`. Relative to the GLB.
    pub texture_uri: Option<String>,
    /// Base-color texture EMBEDDED in the GLB (image with a bufferView, the
    /// self-contained convention generated meshes use). Encoded PNG bytes;
    /// takes over when no external atlas is supplied.
    pub texture_png: Option<Vec<u8>>,
    /// Model-space bounds, for placing a prop on the ground without guessing.
    pub min: Vec3f,
    pub max: Vec3f,
    /// Per-primitive model-space bounds — the prop's own decomposition.
    ///
    /// Kenney authors a house as walls, roof, door frame and chimney, and a
    /// tree as trunk and canopy, so each primitive's box is a ready-made
    /// low-res collider part. One AABB round the whole prop would make a
    /// doorway solid and a canopy a wall you bump into from ten feet away —
    /// both feel worse than no collision at all.
    pub parts: Vec<(Vec3f, Vec3f)>,
    /// The model's baked shadow on flat ground (single-model bakes only) —
    /// consumed by the shadow-mesh layer per instance, never by the shader.
    pub ground_ao: Option<crate::ao_atlas::GroundAo>,
    /// One GPU draw per distinct embedded base-color image. Empty means the
    /// merged `vertices` + `texture_png` / `texture_uri` is the only layer
    /// (Kenney atlas, aomesh sidecar, single-tile mesh). World maps ship one
    /// PNG per tile; the walk viewer and the GPU thumbnailer both draw these
    /// through Renderer so they stay in lockstep.
    pub draw_layers: Vec<StaticDrawLayer>,
    /// Single-layer Q3 / Unreal detail overlay (empty `draw_layers`).
    pub detail_png: Option<Vec<u8>>,
    pub detail_scale: [f32; 2],
    /// Vertex COLOR_0 is a baked lightmap (Q3 worlds). The walk shader
    /// must not multiply the analytic sun on top or inward vaults go black.
    pub prelit: bool,
}

/// One textured subset of a [`StaticModel`]. Positions live in the packed
/// vertex stream; `uvs` stay raw so a CPU spawn preview can sample the same
/// wrap the GPU shader uses.
#[derive(Clone)]
pub struct StaticDrawLayer {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub uvs: Vec<[f32; 2]>,
    pub texture_png: Option<Vec<u8>>,
    pub detail_png: Option<Vec<u8>>,
    pub detail_scale: [f32; 2],
}

/// Boxes below this fraction of the model's largest dimension are decoration
/// — door handles, window frames, chimney pots. Colliding with them is worse
/// than ignoring them: they add cost and snag a walker on nothing.
const PART_MIN_FRACTION: f32 = 0.10;
/// "Low-res" is the point. A handful of boxes captures a house; thirty
/// captures its trim and feels no different to walk into.
const PART_MAX: usize = 8;

/// Triangle-AABB overlap, Akenine-Moller separating axis test. The box is
/// (centre, half). Exact — a diagonal brace only fills the cells its plane
/// truly passes through, which is what keeps voxel colliders from smearing
/// slopes and cross-braces into solid walls.
fn vec3f(x: f32, y: f32, z: f32) -> Vec3f {
    Vec3f { x, y, z }
}

fn tri_box_overlap(c: Vec3f, h: Vec3f, a: Vec3f, b: Vec3f, v: Vec3f) -> bool {
    let v0 = a - c;
    let v1 = b - c;
    let v2 = v - c;
    let e0 = v1 - v0;
    let e1 = v2 - v1;
    let e2 = v0 - v2;
    let axis = |ax: f32, ay: f32, az: f32| -> bool {
        let p0 = ax * v0.x + ay * v0.y + az * v0.z;
        let p1 = ax * v1.x + ay * v1.y + az * v1.z;
        let p2 = ax * v2.x + ay * v2.y + az * v2.z;
        let r = h.x * ax.abs() + h.y * ay.abs() + h.z * az.abs();
        let lo = p0.min(p1).min(p2);
        let hi = p0.max(p1).max(p2);
        lo > r || hi < -r
    };
    // 9 cross-product axes.
    if axis(0.0, -e0.z, e0.y) || axis(0.0, -e1.z, e1.y) || axis(0.0, -e2.z, e2.y) {
        return false;
    }
    if axis(e0.z, 0.0, -e0.x) || axis(e1.z, 0.0, -e1.x) || axis(e2.z, 0.0, -e2.x) {
        return false;
    }
    if axis(-e0.y, e0.x, 0.0) || axis(-e1.y, e1.x, 0.0) || axis(-e2.y, e2.x, 0.0) {
        return false;
    }
    // Box face axes.
    if v0.x.min(v1.x).min(v2.x) > h.x || v0.x.max(v1.x).max(v2.x) < -h.x {
        return false;
    }
    if v0.y.min(v1.y).min(v2.y) > h.y || v0.y.max(v1.y).max(v2.y) < -h.y {
        return false;
    }
    if v0.z.min(v1.z).min(v2.z) > h.z || v0.z.max(v1.z).max(v2.z) < -h.z {
        return false;
    }
    // Triangle plane vs box.
    let n = Vec3f {
        x: e0.y * e2.z - e0.z * e2.y,
        y: e0.z * e2.x - e0.x * e2.z,
        z: e0.x * e2.y - e0.y * e2.x,
    };
    let r = h.x * n.x.abs() + h.y * n.y.abs() + h.z * n.z.abs();
    let d = n.x * v0.x + n.y * v0.y + n.z * v0.z;
    d.abs() <= r
}

impl StaticModel {
    /// A low-res multi-box collider derived from the prop's own primitives.
    ///
    /// Returned in model space, so the caller scales and offsets them exactly
    /// as it does the visual instance. Boxes are dropped if they are tiny
    /// relative to the model, merged when they nearly coincide, and capped —
    /// the aim is a collider that *feels* right, not one that is exact.
    /// Collider boxes derived from the TRIANGLES, not the primitive AABBs.
    ///
    /// Measured against the real kits (see `real_asset_tests`), per-primitive
    /// decomposition is a fiction: most Kenney tiles arrive as ONE merged
    /// primitive, so its AABB is the whole tile and its "parts" carry no
    /// structure. The player then runs through gallows legs and cannot jump
    /// onto a platform deck, in every game mode at once — colliders are the
    /// one physics world. This voxelizes the mesh (triangle-box SAT, not
    /// triangle-AABB, so diagonal braces do not smear into solid walls) and
    /// greedily merges filled cells into boxes: legs, decks, walls and
    /// openings all come out where the art put them.
    pub fn voxel_collider_boxes(&self) -> Vec<(Vec3f, Vec3f)> {
        let size = self.max - self.min;
        let span = size.x.max(size.y).max(size.z);
        if span <= 0.0 || self.indices.len() < 3 {
            return Vec::new();
        }
        // ~24 cells across the longest side resolves a stair step and keeps a
        // corridor wall one cell thin; the floor keeps degenerate axes sane.
        let cell = (span / 24.0).max(0.05);
        let dims = [
            ((size.x / cell).ceil() as usize).clamp(1, 32),
            ((size.y / cell).ceil() as usize).clamp(1, 32),
            ((size.z / cell).ceil() as usize).clamp(1, 32),
        ];
        let cs = [
            size.x / dims[0] as f32,
            size.y / dims[1] as f32,
            size.z / dims[2] as f32,
        ];
        // Per-cell TIGHT bounds, not booleans: a box snapped to cell edges
        // sits up to half a cell away from the art (a metre-wide cell on a
        // big room tile — the player visibly waded into a pole before the
        // wall answered). Cells vote on topology; the emitted box hugs the
        // triangles.
        let mut filled: Vec<Option<(Vec3f, Vec3f)>> =
            vec![None; dims[0] * dims[1] * dims[2]];
        let at = |x: usize, y: usize, z: usize| x + dims[0] * (y + dims[1] * z);
        let vp = |i: u32| {
            let o = i as usize * MODEL_VERTEX_FLOATS;
            Vec3f {
                x: self.vertices[o],
                y: self.vertices[o + 1],
                z: self.vertices[o + 2],
            }
        };
        for tri in self.indices.chunks_exact(3) {
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let tlo = vec3f(
                a.x.min(b.x).min(c.x),
                a.y.min(b.y).min(c.y),
                a.z.min(b.z).min(c.z),
            );
            let thi = vec3f(
                a.x.max(b.x).max(c.x),
                a.y.max(b.y).max(c.y),
                a.z.max(b.z).max(c.z),
            );
            let cell_range = |lo: f32, hi: f32, min: f32, cw: f32, n: usize| {
                let s = (((lo - min) / cw).floor() as isize).clamp(0, n as isize - 1) as usize;
                let e = (((hi - min) / cw).ceil() as isize).clamp(1, n as isize) as usize;
                (s, e)
            };
            let (x0, x1) = cell_range(tlo.x, thi.x, self.min.x, cs[0], dims[0]);
            let (y0, y1) = cell_range(tlo.y, thi.y, self.min.y, cs[1], dims[1]);
            let (z0, z1) = cell_range(tlo.z, thi.z, self.min.z, cs[2], dims[2]);
            for zi in z0..z1 {
                for yi in y0..y1 {
                    for xi in x0..x1 {
                        let centre = vec3f(
                            self.min.x + (xi as f32 + 0.5) * cs[0],
                            self.min.y + (yi as f32 + 0.5) * cs[1],
                            self.min.z + (zi as f32 + 0.5) * cs[2],
                        );
                        let half = vec3f(cs[0] * 0.5, cs[1] * 0.5, cs[2] * 0.5);
                        if !tri_box_overlap(centre, half, a, b, c) {
                            continue;
                        }
                        // Tight bounds: triangle AABB clamped to this cell.
                        let cell_lo = centre - half;
                        let cell_hi = centre + half;
                        let lo = vec3f(
                            tlo.x.max(cell_lo.x),
                            tlo.y.max(cell_lo.y),
                            tlo.z.max(cell_lo.z),
                        );
                        let hi = vec3f(
                            thi.x.min(cell_hi.x),
                            thi.y.min(cell_hi.y),
                            thi.z.min(cell_hi.z),
                        );
                        let e = &mut filled[at(xi, yi, zi)];
                        *e = Some(match *e {
                            None => (lo, hi),
                            Some((plo, phi)) => (
                                vec3f(
                                    plo.x.min(lo.x),
                                    plo.y.min(lo.y),
                                    plo.z.min(lo.z),
                                ),
                                vec3f(
                                    phi.x.max(hi.x),
                                    phi.y.max(hi.y),
                                    phi.z.max(hi.z),
                                ),
                            ),
                        });
                    }
                }
            }
        }
        // Greedy merge: grow a run along x, widen it in z, thicken in y.
        let mut consumed = vec![false; filled.len()];
        let mut out = Vec::new();
        let is_on = |f: &Vec<Option<(Vec3f, Vec3f)>>, c: &Vec<bool>, i: usize| {
            f[i].is_some() && !c[i]
        };
        for zi in 0..dims[2] {
            for yi in 0..dims[1] {
                for xi in 0..dims[0] {
                    if !is_on(&filled, &consumed, at(xi, yi, zi)) {
                        continue;
                    }
                    let mut x1 = xi + 1;
                    while x1 < dims[0] && is_on(&filled, &consumed, at(x1, yi, zi)) {
                        x1 += 1;
                    }
                    let mut z1 = zi + 1;
                    'z: while z1 < dims[2] {
                        for x in xi..x1 {
                            if !is_on(&filled, &consumed, at(x, yi, z1)) {
                                break 'z;
                            }
                        }
                        z1 += 1;
                    }
                    let mut y1 = yi + 1;
                    'y: while y1 < dims[1] {
                        for z in zi..z1 {
                            for x in xi..x1 {
                                if !is_on(&filled, &consumed, at(x, y1, z)) {
                                    break 'y;
                                }
                            }
                        }
                        y1 += 1;
                    }
                    // The emitted box is the UNION of the constituent cells'
                    // tight bounds — it ends where the triangles end, not
                    // where the grid line happens to fall.
                    let mut lo = vec3f(f32::MAX, f32::MAX, f32::MAX);
                    let mut hi = vec3f(f32::MIN, f32::MIN, f32::MIN);
                    for z in zi..z1 {
                        for y in yi..y1 {
                            for x in xi..x1 {
                                let i = at(x, y, z);
                                if let Some((clo, chi)) = filled[i] {
                                    lo.x = lo.x.min(clo.x);
                                    lo.y = lo.y.min(clo.y);
                                    lo.z = lo.z.min(clo.z);
                                    hi.x = hi.x.max(chi.x);
                                    hi.y = hi.y.max(chi.y);
                                    hi.z = hi.z.max(chi.z);
                                }
                                consumed[i] = true;
                            }
                        }
                    }
                    if hi.x > lo.x && hi.y > lo.y && hi.z > lo.z {
                        out.push((lo, hi));
                    }
                }
            }
        }
        // Budget: physics iterates these per placed instance. Biggest first;
        // the tail is single-cell crumbs (rivets, torch nubs) nobody misses.
        const VOXEL_BOX_MAX: usize = 120;
        if out.len() > VOXEL_BOX_MAX {
            out.sort_by(|p, q| {
                let vol = |b: &(Vec3f, Vec3f)| {
                    (b.1.x - b.0.x) * (b.1.y - b.0.y) * (b.1.z - b.0.z)
                };
                vol(q).partial_cmp(&vol(p)).unwrap_or(std::cmp::Ordering::Equal)
            });
            out.truncate(VOXEL_BOX_MAX);
        }
        out
    }

    pub fn collider_parts(&self) -> Vec<(Vec3f, Vec3f)> {
        let span = (self.max.x - self.min.x)
            .max(self.max.y - self.min.y)
            .max(self.max.z - self.min.z);
        if span <= 0.0 {
            return Vec::new();
        }
        let floor = span * PART_MIN_FRACTION;
        let mut kept: Vec<(Vec3f, Vec3f)> = Vec::new();
        for (a, b) in &self.parts {
            let (w, h, d) = (b.x - a.x, b.y - a.y, b.z - a.z);
            // A part must be substantial in at least two axes (a flat panel
            // is a wall and matters) — OR be a POST: tall in y and starting
            // low enough to walk into. "Thin rod = trim" threw away every
            // gate post, table leg and gallows upright, and a player ran
            // straight through the legs of anything built on poles. A rod
            // that hangs high (a lintel, a hand rail bracket) is still trim.
            let big = [w, h, d].iter().filter(|v| **v >= floor).count();
            let post = h >= span * 0.3
                && a.y - self.min.y <= span * 0.15
                && w >= span * 0.01
                && d >= span * 0.01;
            if big < 2 && !post {
                continue;
            }
            // Merge into an existing box when they nearly coincide, which is
            // what a wall split across several primitives looks like.
            let tol = span * 0.08;
            if let Some(e) = kept.iter_mut().find(|(ka, kb)| {
                (ka.x - a.x).abs() < tol
                    && (ka.z - a.z).abs() < tol
                    && (kb.x - b.x).abs() < tol
                    && (kb.z - b.z).abs() < tol
            }) {
                e.0.y = e.0.y.min(a.y);
                e.1.y = e.1.y.max(b.y);
                continue;
            }
            kept.push((*a, *b));
        }
        // Keep the biggest when over budget: the parts that carry the shape.
        if kept.len() > PART_MAX {
            // Ground-reachable parts outrank volume: it is worse to run
            // through a leg than to clip a high beam nobody can touch.
            kept.sort_by(|x, y| {
                let key = |p: &(Vec3f, Vec3f)| {
                    let low = p.0.y - self.min.y <= span * 0.15;
                    let vol = (p.1.x - p.0.x) * (p.1.y - p.0.y) * (p.1.z - p.0.z);
                    (if low { 1 } else { 0 }, vol)
                };
                let (kx, ky) = (key(x), key(y));
                ky.0.cmp(&kx.0).then(
                    ky.1.partial_cmp(&kx.1).unwrap_or(std::cmp::Ordering::Equal),
                )
            });
            kept.truncate(PART_MAX);
        }
        kept
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / MODEL_VERTEX_FLOATS
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Height of the model, so a caller can sit it on the ground.
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// Parse a GLB for RUNTIME use. Bakes nothing.
    ///
    /// AO is an offline product: `tools/ao_bake` produces it, the game loads
    /// it. Baking at load put a 10-rays-per-texel cost on every launch for an
    /// answer that never changes, and raising quality made it worse —
    /// 128 rays pushed startup past a minute. The player should pay nothing
    /// for a bake, so this path does not have one.
    pub fn parse_glb(bytes: &[u8]) -> Result<StaticModel, String> {
        Self::parse_glb_inner(bytes, None)
    }

    /// Parse a GLB and bake its self-occlusion into `ao_atlas`. For the
    /// OFFLINE tool only — never call this from the game.
    ///
    /// The atlas is passed IN rather than returned because it is shared by
    /// every model of a pack; a per-model atlas would be a per-model texture,
    /// and props batch by texture.
    pub fn parse_glb_baked(
        bytes: &[u8],
        ao_atlas: &mut crate::ao_atlas::AoAtlas,
    ) -> Result<StaticModel, String> {
        Self::parse_glb_inner(bytes, Some(ao_atlas))
    }

    fn parse_glb_inner(
        bytes: &[u8],
        ao_atlas: Option<&mut crate::ao_atlas::AoAtlas>,
    ) -> Result<StaticModel, String> {
        if bytes.len() < 12 || &bytes[0..4] != b"glTF" {
            return Err("not a GLB (magic mismatch)".into());
        }
        let mut json_chunk: Option<&[u8]> = None;
        let mut bin_chunk: &[u8] = &[];
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let data = bytes
                .get(at + 8..at + 8 + len)
                .ok_or("GLB chunk out of range")?;
            match kind {
                b"JSON" => json_chunk = Some(data),
                b"BIN\0" => bin_chunk = data,
                _ => {}
            }
            at += 8 + len + (4 - len % 4) % 4;
        }
        let json = JsonParser::parse(json_chunk.ok_or("GLB has no JSON chunk")?)?;
        let acc = Accessors {
            json: &json,
            bin: bin_chunk,
        };

        // Node rest transforms, then parents from the children lists.
        let node_vals = json.get("nodes").map(|n| n.arr()).unwrap_or(&[]);
        let mut rests: Vec<NodeTrs> = Vec::with_capacity(node_vals.len());
        let mut parents: Vec<Option<usize>> = vec![None; node_vals.len()];
        for n in node_vals {
            let mut rest = NodeTrs::default();
            if let Some(t) = n.get("translation") {
                rest.t = Vec3f {
                    x: t.idx(0).and_then(Val::f64).unwrap_or(0.0) as f32,
                    y: t.idx(1).and_then(Val::f64).unwrap_or(0.0) as f32,
                    z: t.idx(2).and_then(Val::f64).unwrap_or(0.0) as f32,
                };
            }
            if let Some(r) = n.get("rotation") {
                rest.r = Quat {
                    x: r.idx(0).and_then(Val::f64).unwrap_or(0.0) as f32,
                    y: r.idx(1).and_then(Val::f64).unwrap_or(0.0) as f32,
                    z: r.idx(2).and_then(Val::f64).unwrap_or(0.0) as f32,
                    w: r.idx(3).and_then(Val::f64).unwrap_or(1.0) as f32,
                };
            }
            if let Some(s) = n.get("scale") {
                rest.s = Vec3f {
                    x: s.idx(0).and_then(Val::f64).unwrap_or(1.0) as f32,
                    y: s.idx(1).and_then(Val::f64).unwrap_or(1.0) as f32,
                    z: s.idx(2).and_then(Val::f64).unwrap_or(1.0) as f32,
                };
            }
            rests.push(rest);
        }
        for (parent_index, n) in node_vals.iter().enumerate() {
            if let Some(children) = n.get("children") {
                for c in children.arr() {
                    if let Some(ci) = c.usize() {
                        if ci < parents.len() {
                            parents[ci] = Some(parent_index);
                        }
                    }
                }
            }
        }
        // World transform per node: walk to the root and multiply down. Depth
        // is a handful for a prop, so recomputing per mesh node is cheaper
        // than caching and far easier to read.
        let world_of = |mut node: usize| -> Mat4f {
            let mut chain = vec![node];
            while let Some(p) = parents[node] {
                chain.push(p);
                node = p;
            }
            let mut m = Mat4f::identity();
            for idx in chain.iter().rev() {
                m = Mat4f::mul(&m, &trs_to_mat4(&rests[*idx]));
            }
            m
        };

        let mut vertices: Vec<f32> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut min = Vec3f {
            x: f32::MAX,
            y: f32::MAX,
            z: f32::MAX,
        };
        let mut max = Vec3f {
            x: f32::MIN,
            y: f32::MIN,
            z: f32::MIN,
        };
        let mut vert_total = 0usize;
        let mut parts: Vec<(Vec3f, Vec3f)> = Vec::new();
        // Kept alongside the packed stream purely so AO can be baked once the
        // whole mesh is known — occlusion needs every triangle, and primitives
        // arrive one at a time. Dropped at the end of this function.
        let mut raw_pos: Vec<Vec3f> = Vec::new();
        let mut raw_nrm: Vec<Vec3f> = Vec::new();
        let mut raw_tint: Vec<[f32; 3]> = Vec::new();
        let mut raw_uv: Vec<[f32; 2]> = Vec::new();
        // (image, detail_image, detail_scale, v0, v1, i0, i1)
        let mut prim_spans: Vec<PrimSpan> = Vec::new();

        for (node_index, n) in node_vals.iter().enumerate() {
            let Some(mesh_index) = n.get("mesh").and_then(Val::usize) else {
                continue;
            };
            let world = world_of(node_index);
            let mesh = json
                .get("meshes")
                .and_then(|m| m.idx(mesh_index))
                .ok_or("bad mesh index")?;
            for prim in mesh.get("primitives").map(|p| p.arr()).unwrap_or(&[]) {
                let attrs = prim
                    .get("attributes")
                    .ok_or("primitive without attributes")?;
                // Kenney ships two conventions: most packs UV-map everything
                // into one `colormap.png`, but some (nature-kit) carry no
                // texture at all and colour each primitive with a material
                // baseColorFactor. Baking that factor into the vertex colour
                // lets both render through one shader — atlas models simply
                // carry white.
                let tint = prim
                    .get("material")
                    .and_then(Val::usize)
                    .and_then(|mi| json.get("materials").and_then(|m| m.idx(mi)))
                    .and_then(|m| m.get("pbrMetallicRoughness"))
                    .and_then(|p| p.get("baseColorFactor"))
                    .map(|f| {
                        [
                            f.idx(0).and_then(Val::f64).unwrap_or(1.0) as f32,
                            f.idx(1).and_then(Val::f64).unwrap_or(1.0) as f32,
                            f.idx(2).and_then(Val::f64).unwrap_or(1.0) as f32,
                            f.idx(3).and_then(Val::f64).unwrap_or(1.0) as f32,
                        ]
                    })
                    .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                let pos_acc = attrs
                    .get("POSITION")
                    .and_then(Val::usize)
                    .ok_or("primitive without POSITION")?;
                let (pos, _) = acc.read_f32(pos_acc)?;
                let normal = attrs
                    .get("NORMAL")
                    .and_then(Val::usize)
                    .map(|a| acc.read_f32(a))
                    .transpose()?
                    .map(|(v, _)| v);
                let uv = attrs
                    .get("TEXCOORD_0")
                    .and_then(Val::usize)
                    .map(|a| acc.read_f32(a))
                    .transpose()?
                    .map(|(v, _)| v);
                // COLOR_0 vertex colors (e.g. trellis-generated meshes carry
                // their PBR base color here): multiplied into the material
                // tint per vertex, exactly like the shader multiplies tint
                // with the texture. read_f32 already normalizes ubyte/ushort.
                let vcolor = attrs
                    .get("COLOR_0")
                    .and_then(Val::usize)
                    .map(|a| acc.read_f32(a))
                    .transpose()?;

                let base = vert_total as u32;
                let count = pos.len() / 3;
                let prim_image = gltf_prim_image_index(&json, prim);
                let (detail_image, detail_scale) = gltf_prim_detail(&json, prim);
                let prim_v0 = vert_total;
                let prim_i0 = indices.len();
                // Mirrored node (negative determinant): a plain direction
                // transform flips the authored normal INTO the solid, and
                // the winding flips too. Lighting half-hid it (dark kits);
                // the AO baker aimed hemispheres inside-out from it. Correct
                // both here so every consumer sees honest outward data.
                let mirrored = {
                    let m = &world.v;
                    let det = m[0] * (m[5] * m[10] - m[6] * m[9])
                        - m[4] * (m[1] * m[10] - m[2] * m[9])
                        + m[8] * (m[1] * m[6] - m[2] * m[5]);
                    det < 0.0
                };
                let mut pmin = Vec3f { x: f32::MAX, y: f32::MAX, z: f32::MAX };
                let mut pmax = Vec3f { x: f32::MIN, y: f32::MIN, z: f32::MIN };
                for i in 0..count {
                    let g = |src: &Option<Vec<f32>>, lanes: usize, lane: usize, dflt: f32| {
                        src.as_ref()
                            .and_then(|v| v.get(i * lanes + lane).copied())
                            .unwrap_or(dflt)
                    };
                    // Bake the node transform in: a prop never animates, so
                    // this is done once here instead of per frame on the GPU.
                    let p = mat4_mul_point(
                        &world,
                        Vec3f {
                            x: pos[i * 3],
                            y: pos[i * 3 + 1],
                            z: pos[i * 3 + 2],
                        },
                    );
                    let mut nrm = mat4_mul_dir(
                        &world,
                        Vec3f {
                            x: g(&normal, 3, 0, 0.0),
                            y: g(&normal, 3, 1, 1.0),
                            z: g(&normal, 3, 2, 0.0),
                        },
                    );
                    let len = (nrm.x * nrm.x + nrm.y * nrm.y + nrm.z * nrm.z).sqrt();
                    if len > 1.0e-8 {
                        nrm.x /= len;
                        nrm.y /= len;
                        nrm.z /= len;
                    }
                    if mirrored {
                        nrm = Vec3f { x: -nrm.x, y: -nrm.y, z: -nrm.z };
                    }
                    min.x = min.x.min(p.x);
                    min.y = min.y.min(p.y);
                    min.z = min.z.min(p.z);
                    max.x = max.x.max(p.x);
                    max.y = max.y.max(p.y);
                    max.z = max.z.max(p.z);
                    pmin.x = pmin.x.min(p.x);
                    pmin.y = pmin.y.min(p.y);
                    pmin.z = pmin.z.min(p.z);
                    pmax.x = pmax.x.max(p.x);
                    pmax.y = pmax.y.max(p.y);
                    pmax.z = pmax.z.max(p.z);
                    let (ox, oy) = oct_encode(nrm);
                    let vt = match &vcolor {
                        Some((values, lanes)) => [
                            tint[0] * values.get(i * lanes).copied().unwrap_or(1.0),
                            tint[1] * values.get(i * lanes + 1).copied().unwrap_or(1.0),
                            tint[2] * values.get(i * lanes + 2).copied().unwrap_or(1.0),
                        ],
                        None => [tint[0], tint[1], tint[2]],
                    };
                    raw_pos.push(p);
                    raw_nrm.push(nrm);
                    raw_tint.push(vt);
                    raw_uv.push([g(&uv, 2, 0, 0.0), g(&uv, 2, 1, 0.0)]);
                    vertices.extend_from_slice(&[
                        p.x,
                        p.y,
                        p.z,
                        makepad_draw::pack_pair_f16(ox, oy),
                        makepad_draw::pack_pair_f16(g(&uv, 2, 0, 0.0), g(&uv, 2, 1, 0.0)),
                        // Colour's alpha lane is a PLACEHOLDER here: baked AO
                        // is written into it below, once every primitive has
                        // been read. The material's own alpha was already
                        // discarded by the shader (it returns opaque), so the
                        // lane costs nothing to repurpose.
                        makepad_draw::pack_unorm8x4(vt[0], vt[1], vt[2], 1.0),
                    ]);
                }
                if let Some(idx_acc) = prim.get("indices").and_then(Val::usize) {
                    let (idx, _) = acc.read_f32(idx_acc)?;
                    if mirrored {
                        for tri in idx.chunks_exact(3) {
                            indices.extend_from_slice(&[
                                base + tri[0] as u32,
                                base + tri[2] as u32,
                                base + tri[1] as u32,
                            ]);
                        }
                    } else {
                        indices.extend(idx.iter().map(|v| base + *v as u32));
                    }
                } else if mirrored {
                    for t in (0..count as u32).step_by(3) {
                        indices.extend_from_slice(&[base + t, base + t + 2, base + t + 1]);
                    }
                } else {
                    indices.extend((0..count as u32).map(|i| base + i));
                }
                vert_total += count;
                if count > 0 {
                    parts.push((pmin, pmax));
                    prim_spans.push(PrimSpan {
                        image: prim_image,
                        detail_image,
                        detail_scale,
                        v0: prim_v0,
                        v1: vert_total,
                        i0: prim_i0,
                        i1: indices.len(),
                    });
                }
            }
        }
        if vertices.is_empty() {
            return Err("no mesh primitives found".into());
        }

        // Bake self-occlusion into the colour's alpha lane. This is the whole
        // reason a prop reads as a solid object rather than a flat-shaded
        // shell: the underside of an eave, the inside of an arch and the gap
        // between a bench's slats all darken, and none of it moves when the
        // sun does. Zero extra vertex bytes — the lane was already there and
        // already ignored.
        // Self-occlusion comes from the offline bake, or not at all.
        //
        // With an atlas: the mesh is un-indexed so every triangle can own a
        // patch, and each vertex carries its atlas coordinate. Without one:
        // the mesh is left as authored and the AO lanes are neutral, so props
        // light exactly as they did before AO existed. No runtime cost either
        // way, because there is no runtime bake.
        let baked_ao = ao_atlas.is_some();
        let (ao_uv, vertex_ao, ground_ao) = match ao_atlas {
            Some(atlas) => {
                // Normals are REBUILT before baking, not trusted from the
                // file. Kenney exports smoothed vertex normals across the
                // rounded corner bevels, so a flat wall inherits a normal
                // GRADIENT from its corners — and the hemisphere ambient then
                // paints soft grey washes across surfaces that should be flat
                // colour with AO in the creases (measured on the suburban
                // houses: vertex normals up to 45-180 degrees off their own
                // face). Faces meeting within ~35 degrees still smooth, so a
                // barrel stays round; anything sharper becomes a hard edge.
                // The AUTHORED normals, captured before the rebuild: the
                // orientation tie-break in the baker needs the artist's
                // intent, and the rebuilt corner normals are winding-derived
                // — they flip with a mirror baked into the vertex stream,
                // which is the exact failure the tie-break exists to fix.
                let mut raw_authored = raw_nrm.clone();
                resolve_corner_normals(
                    &mut raw_pos, &mut raw_nrm, &mut raw_authored, &mut raw_uv, &mut raw_tint,
                    &mut indices, min, max,
                );
                let baked = crate::ao_atlas::bake_into_authored(
                    atlas, &mut raw_pos, &mut raw_nrm, Some(&raw_authored), &mut indices,
                    min, max,
                );
                let mut uv = Vec::with_capacity(raw_pos.len());
                let mut tint = Vec::with_capacity(raw_pos.len());
                for src in &baked.source_vertex {
                    uv.push(raw_uv[*src as usize]);
                    tint.push(raw_tint[*src as usize]);
                }
                raw_uv = uv;
                raw_tint = tint;
                (baked.ao_uv, baked.vertex_ao, baked.ground)
            }
            None => (vec![[0.0, 0.0]; raw_pos.len()], vec![1.0; raw_pos.len()], None),
        };

        vertices.clear();
        vertices.reserve(raw_pos.len() * MODEL_VERTEX_FLOATS);
        for i in 0..raw_pos.len() {
            let p = raw_pos[i];
            let (ox, oy) = oct_encode(raw_nrm[i]);
            let t = raw_tint[i];
            vertices.extend_from_slice(&[
                p.x,
                p.y,
                p.z,
                makepad_draw::pack_pair_f16(ox, oy),
                makepad_draw::pack_pair_f16(raw_uv[i][0], raw_uv[i][1]),
                makepad_draw::pack_unorm8x4(t[0], t[1], t[2], vertex_ao[i]),
                pack_ao_uv(ao_uv[i][0], ao_uv[i][1]),
            ]);
        }

        // First image URI: Kenney packs use exactly one atlas per pack, and a
        // model referencing several would batch badly anyway.
        let texture_uri = json
            .get("images")
            .and_then(|i| i.idx(0))
            .and_then(|i| i.get("uri"))
            .and_then(Val::str)
            .map(str::to_string);
        // Embedded base color (image stored in the BIN chunk via bufferView):
        // the self-contained convention of generated/baked GLBs. Resolved
        // through the first material's baseColorTexture; falls back to
        // images[0] when materials carry no texture reference.
        let image_index = json
            .get("materials")
            .and_then(|m| m.idx(0))
            .and_then(|m| m.get("pbrMetallicRoughness"))
            .and_then(|p| p.get("baseColorTexture"))
            .and_then(|t| t.get("index"))
            .and_then(Val::usize)
            .and_then(|ti| {
                json.get("textures")
                    .and_then(|t| t.idx(ti))
                    .and_then(|t| t.get("source"))
                    .and_then(Val::usize)
            })
            .unwrap_or(0);
        let texture_png = gltf_embedded_png(&json, bin_chunk, image_index);

        // Split by embedded image so a world GLB (one PNG per tile) draws
        // every surface instead of smearing image 0. AO bake un-indexes the
        // whole mesh into one atlas, so that path stays a single layer.
        let draw_layers = if !baked_ao {
            split_draw_layers(&vertices, &indices, &raw_uv, &prim_spans, &json, bin_chunk)
        } else {
            Vec::new()
        };
        let (detail_png, detail_scale) = first_detail_layer(&draw_layers, &prim_spans, &json, bin_chunk);
        let prelit = materials_have_lightmap(&json);

        Ok(StaticModel {
            vertices,
            indices,
            texture_uri,
            texture_png,
            min,
            max,
            parts,
            ground_ao,
            draw_layers,
            detail_png,
            detail_scale,
            prelit,
        })
    }
}

#[derive(Clone)]
struct PrimSpan {
    image: usize,
    detail_image: Option<usize>,
    detail_scale: [f32; 2],
    v0: usize,
    v1: usize,
    i0: usize,
    i1: usize,
}

fn materials_have_lightmap(json: &Val) -> bool {
    let Some(mats) = json.get("materials") else {
        return false;
    };
    for m in mats.arr() {
        if m.get("extras").and_then(|e| e.get("lightmapTexture")).is_some() {
            return true;
        }
    }
    false
}

fn gltf_prim_detail(json: &Val, prim: &Val) -> (Option<usize>, [f32; 2]) {
    let mat = prim
        .get("material")
        .and_then(Val::usize)
        .and_then(|mi| json.get("materials").and_then(|m| m.idx(mi)));
    let extra = match mat.and_then(|m| m.get("extras")) {
        Some(e) => e,
        None => return (None, [0.0, 0.0]),
    };
    let det = match extra.get("detailTexture") {
        Some(d) => d,
        None => return (None, [0.0, 0.0]),
    };
    let ti = match det.get("index").and_then(Val::usize) {
        Some(i) => i,
        None => return (None, [0.0, 0.0]),
    };
    let src = json
        .get("textures")
        .and_then(|t| t.idx(ti))
        .and_then(|t| t.get("source"))
        .and_then(Val::usize);
    let sx = det
        .get("scale")
        .and_then(|s| s.idx(0))
        .and_then(Val::f64)
        .unwrap_or(1.0) as f32;
    let sy = det
        .get("scale")
        .and_then(|s| s.idx(1))
        .and_then(Val::f64)
        .unwrap_or(sx as f64) as f32;
    (src, [sx, sy])
}

fn first_detail_layer(
    layers: &[StaticDrawLayer],
    spans: &[PrimSpan],
    json: &Val,
    bin: &[u8],
) -> (Option<Vec<u8>>, [f32; 2]) {
    if let Some(layer) = layers.iter().find(|l| l.detail_png.is_some()) {
        return (layer.detail_png.clone(), layer.detail_scale);
    }
    if let Some(span) = spans.iter().find(|s| s.detail_image.is_some()) {
        let png = span
            .detail_image
            .and_then(|i| gltf_embedded_png(json, bin, i));
        return (png, span.detail_scale);
    }
    (None, [0.0, 0.0])
}

fn gltf_prim_image_index(json: &Val, prim: &Val) -> usize {
    prim.get("material")
        .and_then(Val::usize)
        .and_then(|mi| json.get("materials").and_then(|m| m.idx(mi)))
        .and_then(|m| m.get("pbrMetallicRoughness"))
        .and_then(|p| p.get("baseColorTexture"))
        .and_then(|t| t.get("index"))
        .and_then(Val::usize)
        .and_then(|ti| {
            json.get("textures")
                .and_then(|t| t.idx(ti))
                .and_then(|t| t.get("source"))
                .and_then(Val::usize)
        })
        .unwrap_or(0)
}

fn gltf_embedded_png(json: &Val, bin: &[u8], image_index: usize) -> Option<Vec<u8>> {
    let image = json.get("images").and_then(|i| i.idx(image_index))?;
    let bv = image.get("bufferView").and_then(Val::usize)?;
    let view = json.get("bufferViews").and_then(|v| v.idx(bv))?;
    let offset = view.get("byteOffset").and_then(Val::usize).unwrap_or(0);
    let length = view.get("byteLength").and_then(Val::usize)?;
    bin.get(offset..offset + length).map(<[u8]>::to_vec)
}

fn split_draw_layers(
    vertices: &[f32],
    indices: &[u32],
    raw_uv: &[[f32; 2]],
    prim_spans: &[PrimSpan],
    json: &Val,
    bin: &[u8],
) -> Vec<StaticDrawLayer> {
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct LayerKey {
        image: usize,
        detail: i32,
        sx: u32,
        sy: u32,
    }
    let key_of = |s: &PrimSpan| LayerKey {
        image: s.image,
        detail: s.detail_image.map(|i| i as i32).unwrap_or(-1),
        sx: s.detail_scale[0].to_bits(),
        sy: s.detail_scale[1].to_bits(),
    };
    let mut by_img: BTreeMap<LayerKey, (Vec<f32>, Vec<u32>, Vec<[f32; 2]>, PrimSpan)> =
        BTreeMap::new();
    for span in prim_spans {
        if span.v1 <= span.v0 || span.i1 <= span.i0 {
            continue;
        }
        let v_end = (span.v1 * MODEL_VERTEX_FLOATS).min(vertices.len());
        let v_start = (span.v0 * MODEL_VERTEX_FLOATS).min(v_end);
        if v_end <= v_start {
            continue;
        }
        let dest = by_img.entry(key_of(span)).or_insert_with(|| {
            (Vec::new(), Vec::new(), Vec::new(), span.clone())
        });
        let base = (dest.0.len() / MODEL_VERTEX_FLOATS) as u32;
        dest.0.extend_from_slice(&vertices[v_start..v_end]);
        if span.v1 <= raw_uv.len() {
            dest.2.extend_from_slice(&raw_uv[span.v0..span.v1]);
        }
        for &idx in &indices[span.i0.min(indices.len())..span.i1.min(indices.len())] {
            dest.1.push(idx - span.v0 as u32 + base);
        }
    }
    if by_img.len() <= 1 {
        return Vec::new();
    }
    let mut layers = Vec::with_capacity(by_img.len());
    for (_key, (verts, inds, uvs, span)) in by_img {
        if inds.len() < 3 || verts.len() < MODEL_VERTEX_FLOATS * 3 {
            continue;
        }
        layers.push(StaticDrawLayer {
            vertices: verts,
            indices: inds,
            uvs,
            texture_png: gltf_embedded_png(json, bin, span.image),
            detail_png: span
                .detail_image
                .and_then(|i| gltf_embedded_png(json, bin, i)),
            detail_scale: span.detail_scale,
        });
    }
    if layers.len() <= 1 {
        Vec::new()
    } else {
        layers
    }
}

/// How parallel two faces must be for a shared corner to smooth between
/// them. cos(35 degrees): a Kenney corner bevel meets its wall at 45+, so
/// bevel-to-wall goes hard; a cylinder's segments meet well inside it and
/// stay round.
const HARD_EDGE_DOT: f32 = 0.819;

/// Replace authored vertex normals with angle-thresholded corner normals,
/// un-indexing the mesh so every triangle corner can carry its own.
///
/// Per corner: the area-weighted average of the face normals of every face
/// sharing that POSITION (welded, so duplicated vertices count) that lies
/// within `HARD_EDGE_DOT` of this corner's own face. Area weighting keeps a
/// sliver from tipping a big wall's normal; the threshold is what turns the
/// authored corner smoothing into hard edges without flattening genuine
/// curves.
/// `authored` rides along untouched (the pre-rebuild normals, per vertex):
/// the un-indexing must keep it parallel to `pos` so the AO baker's
/// orientation tie-break can still read the artist's facing per corner.
#[allow(clippy::too_many_arguments)]
fn resolve_corner_normals(
    pos: &mut Vec<Vec3f>,
    nrm: &mut Vec<Vec3f>,
    authored: &mut Vec<Vec3f>,
    uv: &mut Vec<[f32; 2]>,
    tint: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
) {
    let tri_count = indices.len() / 3;
    if tri_count == 0 {
        return;
    }
    // Unnormalised cross products: length is 2x the face's area, which is
    // exactly the weight the average wants.
    let mut fnorm = Vec::with_capacity(tri_count);
    for t in 0..tri_count {
        let (a, b, c) = (
            pos[indices[t * 3] as usize],
            pos[indices[t * 3 + 1] as usize],
            pos[indices[t * 3 + 2] as usize],
        );
        let (e1, e2) = (
            Vec3f { x: b.x - a.x, y: b.y - a.y, z: b.z - a.z },
            Vec3f { x: c.x - a.x, y: c.y - a.y, z: c.z - a.z },
        );
        fnorm.push(Vec3f {
            x: e1.y * e2.z - e1.z * e2.y,
            y: e1.z * e2.x - e1.x * e2.z,
            z: e1.x * e2.y - e1.y * e2.x,
        });
    }
    let unit = |v: Vec3f| {
        let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        if l < 1.0e-12 {
            None
        } else {
            Some(Vec3f { x: v.x / l, y: v.y / l, z: v.z / l })
        }
    };

    let span = (max.x - min.x)
        .max(max.y - min.y)
        .max(max.z - min.z)
        .max(1.0e-5);
    let inv_eps = 1.0 / (span * 1.0e-5);
    let quant = |p: Vec3f| {
        (
            (p.x * inv_eps).round() as i64,
            (p.y * inv_eps).round() as i64,
            (p.z * inv_eps).round() as i64,
        )
    };
    let mut at_position: std::collections::HashMap<(i64, i64, i64), Vec<u32>> =
        std::collections::HashMap::with_capacity(pos.len());
    for t in 0..tri_count {
        for k in 0..3 {
            at_position
                .entry(quant(pos[indices[t * 3 + k] as usize]))
                .or_default()
                .push(t as u32);
        }
    }

    let mut out_pos = Vec::with_capacity(tri_count * 3);
    let mut out_nrm = Vec::with_capacity(tri_count * 3);
    let mut out_authored = Vec::with_capacity(tri_count * 3);
    let mut out_uv = Vec::with_capacity(tri_count * 3);
    let mut out_tint = Vec::with_capacity(tri_count * 3);
    for t in 0..tri_count {
        let face = unit(fnorm[t]);
        for k in 0..3 {
            let vi = indices[t * 3 + k] as usize;
            // Degenerate face: no orientation of its own, keep the authored
            // normal — it rasterises nothing either way.
            let n = match face {
                None => nrm[vi],
                Some(f) => {
                    let mut acc = Vec3f { x: 0.0, y: 0.0, z: 0.0 };
                    for &ot in &at_position[&quant(pos[vi])] {
                        if let Some(of) = unit(fnorm[ot as usize]) {
                            if of.x * f.x + of.y * f.y + of.z * f.z > HARD_EDGE_DOT {
                                let w = fnorm[ot as usize];
                                acc.x += w.x;
                                acc.y += w.y;
                                acc.z += w.z;
                            }
                        }
                    }
                    unit(acc).unwrap_or(f)
                }
            };
            out_pos.push(pos[vi]);
            out_nrm.push(n);
            out_authored.push(authored[vi]);
            out_uv.push(uv[vi]);
            out_tint.push(tint[vi]);
        }
    }
    *indices = (0..(tri_count * 3) as u32).collect();
    *pos = out_pos;
    *nrm = out_nrm;
    *authored = out_authored;
    *uv = out_uv;
    *tint = out_tint;
}

/// Atlas uv into one f32 vertex lane as unorm16x2.
///
/// NOT `pack_pair_f16`. An f16's spacing near 1.0 is 2^-10 — one whole texel
/// of a 1024 atlas — so packing uvs as f16 snapped every vertex's uv by up to
/// half a texel in an arbitrary direction. On charts a few texels across that
/// warped the sampled field visibly: occlusion drifted off its crease and
/// every facade wore a slightly different smear. A unorm16 is uniform 1/65535
/// across [0,1] — 1/64th of a texel — and the atlas uv is by construction in
/// [0,1], which is the case f16 is the wrong tool for.
///
/// The shader unpacks with `unpack4u8` (little-endian bytes b0..b3) as
/// `(b0 + 256*b1) / 257` per axis — exact, because 255 * 257 = 65535.
pub fn pack_ao_uv(u: f32, v: f32) -> f32 {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 65535.0 + 0.5) as u32;
    f32::from_bits(q(u) | (q(v) << 16))
}

/// Inverse of [`pack_ao_uv`]: recover the two halves the GPU will see.
///
/// Exists so a test can read back exactly what the shader fetches. Decoding
/// the SHIPPED bytes rather than keeping the pre-pack floats is the point — a
/// uv that survives in `f32` but lands wrong once packed is a real defect that
/// comparing against the unpacked value would hide.
pub fn unpack_ao_uv(packed: f32) -> [f32; 2] {
    let bits = packed.to_bits();
    [
        (bits & 0xFFFF) as f32 / 65535.0,
        (bits >> 16) as f32 / 65535.0,
    ]
}

/// Magic + version. Bumped whenever the vertex layout changes, so a stale
/// sidecar is IGNORED rather than silently sampled with the wrong stride.
const AOMESH_MAGIC: &[u8; 8] = b"AOMESH\x03\x00";

impl StaticModel {
    /// Serialise a baked model to its sidecar bytes.
    ///
    /// # Why a sidecar exists at all
    ///
    /// Baking un-indexes the mesh and assigns each triangle a patch in the
    /// pack's atlas, so the vertex list the tool produced and the UVs it wrote
    /// are two halves of one artefact. The runtime cannot re-derive either —
    /// it does not bake, and the atlas coordinates depend on the packing order
    /// of every OTHER model in the pack. Shipping the geometry next to the
    /// atlas is what makes the two agree by construction instead of by luck.
    pub fn to_aomesh(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(self.vertices.len() * 4 + self.indices.len() * 4 + 64);
        b.extend_from_slice(AOMESH_MAGIC);
        let uri = self.texture_uri.as_deref().unwrap_or("");
        for n in [
            self.vertices.len() as u32,
            self.indices.len() as u32,
            self.parts.len() as u32,
            uri.len() as u32,
        ] {
            b.extend_from_slice(&n.to_le_bytes());
        }
        let put3 = |v: Vec3f, b: &mut Vec<u8>| {
            for f in [v.x, v.y, v.z] {
                b.extend_from_slice(&f.to_le_bytes());
            }
        };
        put3(self.min, &mut b);
        put3(self.max, &mut b);
        for f in &self.vertices {
            b.extend_from_slice(&f.to_le_bytes());
        }
        for i in &self.indices {
            b.extend_from_slice(&i.to_le_bytes());
        }
        for (lo, hi) in &self.parts {
            put3(*lo, &mut b);
            put3(*hi, &mut b);
        }
        b.extend_from_slice(uri.as_bytes());
        // Ground AO trails the uri so every offset before it is unchanged:
        // presence flag, then rect + plane, grid dims, pixel rows.
        match &self.ground_ao {
            None => b.extend_from_slice(&0u32.to_le_bytes()),
            Some(g) => {
                b.extend_from_slice(&1u32.to_le_bytes());
                for f in [g.x0, g.z0, g.x1, g.z1, g.y] {
                    b.extend_from_slice(&f.to_le_bytes());
                }
                b.extend_from_slice(&(g.w as u32).to_le_bytes());
                b.extend_from_slice(&(g.h as u32).to_le_bytes());
                b.extend_from_slice(&g.pixels);
            }
        }
        b
    }

    /// Parse sidecar bytes. `None` for anything unrecognised, truncated or of
    /// a different version — the caller falls back to the plain `.glb`, which
    /// renders correctly just without AO.
    pub fn from_aomesh(bytes: &[u8]) -> Option<StaticModel> {
        if bytes.len() < 8 + 16 + 24 || &bytes[..8] != AOMESH_MAGIC {
            return None;
        }
        // Nested fns rather than closures: several of these read the same
        // cursor, and closures capturing it would borrow-conflict.
        fn rd_u32(b: &[u8], o: &mut usize) -> u32 {
            let v = u32::from_le_bytes(b[*o..*o + 4].try_into().unwrap());
            *o += 4;
            v
        }
        fn rd_f32(b: &[u8], o: &mut usize) -> f32 {
            let v = f32::from_le_bytes(b[*o..*o + 4].try_into().unwrap());
            *o += 4;
            v
        }
        fn rd_v3(b: &[u8], o: &mut usize) -> Vec3f {
            Vec3f {
                x: rd_f32(b, o),
                y: rd_f32(b, o),
                z: rd_f32(b, o),
            }
        }

        let mut o = 8;
        let (nv, ni, np, nu) = (
            rd_u32(bytes, &mut o) as usize,
            rd_u32(bytes, &mut o) as usize,
            rd_u32(bytes, &mut o) as usize,
            rd_u32(bytes, &mut o) as usize,
        );
        if nv % MODEL_VERTEX_FLOATS != 0 || ni % 3 != 0 {
            return None;
        }
        // Every subsequent read is bounds-checked ONCE, here, so the readers
        // above can index directly.
        if bytes.len() < 24 + 24 + nv * 4 + ni * 4 + np * 24 + nu {
            return None;
        }
        let min = rd_v3(bytes, &mut o);
        let max = rd_v3(bytes, &mut o);
        let mut vertices = Vec::with_capacity(nv);
        for _ in 0..nv {
            vertices.push(rd_f32(bytes, &mut o));
        }
        let mut indices = Vec::with_capacity(ni);
        for _ in 0..ni {
            indices.push(rd_u32(bytes, &mut o));
        }
        let mut parts = Vec::with_capacity(np);
        for _ in 0..np {
            let lo = rd_v3(bytes, &mut o);
            parts.push((lo, rd_v3(bytes, &mut o)));
        }
        let texture_uri = if nu == 0 {
            None
        } else {
            Some(String::from_utf8(bytes[o..o + nu].to_vec()).ok()?)
        };
        o += nu;
        // Ground AO block. Bounds-checked stepwise — unlike the header
        // section it was not covered by the single check above.
        let ground_ao = {
            if bytes.len() < o + 4 {
                return None;
            }
            if rd_u32(bytes, &mut o) == 0 {
                None
            } else {
                if bytes.len() < o + 5 * 4 + 2 * 4 {
                    return None;
                }
                let (x0, z0, x1, z1, y) = (
                    rd_f32(bytes, &mut o),
                    rd_f32(bytes, &mut o),
                    rd_f32(bytes, &mut o),
                    rd_f32(bytes, &mut o),
                    rd_f32(bytes, &mut o),
                );
                let w = rd_u32(bytes, &mut o) as usize;
                let h = rd_u32(bytes, &mut o) as usize;
                if w == 0 || h == 0 || w > 4096 || h > 4096 || bytes.len() < o + w * h {
                    return None;
                }
                let pixels = bytes[o..o + w * h].to_vec();
                Some(crate::ao_atlas::GroundAo { x0, z0, x1, z1, y, w, h, pixels })
            }
        };
        // A sidecar whose indices point outside its own vertex list would be a
        // GPU read out of bounds, so it is rejected here rather than trusted.
        let vert_count = nv / MODEL_VERTEX_FLOATS;
        if indices.iter().any(|&i| i as usize >= vert_count) {
            return None;
        }
        Some(StaticModel {
            vertices,
            indices,
            texture_uri,
            // Sidecars carry their pack atlas by URI; embedded textures are
            // a generated-GLB concern and are not serialized.
            texture_png: None,
            min,
            max,
            parts,
            ground_ao,
            draw_layers: Vec::new(),
            detail_png: None,
            detail_scale: [0.0, 0.0],
            prelit: false,
        })
    }
}

#[cfg(test)]
mod sidecar_tests {
    use super::*;

    fn sample() -> StaticModel {
        StaticModel {
            vertices: (0..MODEL_VERTEX_FLOATS as u32 * 3).map(|i| i as f32 * 0.25).collect(),
            indices: vec![0, 1, 2],
            texture_uri: Some("Textures/colormap.png".into()),
            texture_png: None,
            min: Vec3f { x: -1.0, y: 0.0, z: -2.0 },
            max: Vec3f { x: 1.0, y: 3.0, z: 2.0 },
            parts: vec![(
                Vec3f { x: -1.0, y: 0.0, z: -2.0 },
                Vec3f { x: 1.0, y: 3.0, z: 2.0 },
            )],
            draw_layers: Vec::new(),
            detail_png: None,
            detail_scale: [0.0, 0.0],
            prelit: false,
            ground_ao: Some(crate::ao_atlas::GroundAo {
                x0: -1.1,
                z0: -2.1,
                x1: 1.1,
                z1: 2.1,
                y: 0.0,
                w: 3,
                h: 2,
                pixels: vec![200, 150, 255, 255, 130, 255],
            }),
        }
    }

    /// The sidecar is the ONLY carrier of baked AO UVs, so anything it loses
    /// in the round trip is AO the game never sees.
    #[test]
    fn a_sidecar_round_trips_every_field() {
        let m = sample();
        let back = StaticModel::from_aomesh(&m.to_aomesh()).expect("round trip");
        assert_eq!(back.vertices, m.vertices, "vertex floats changed");
        assert_eq!(back.indices, m.indices);
        assert_eq!(back.texture_uri, m.texture_uri);
        assert_eq!(back.parts.len(), m.parts.len());
        assert_eq!(back.min.y, m.min.y);
        assert_eq!(back.max.z, m.max.z);
        let (g, gb) = (m.ground_ao.as_ref().unwrap(), back.ground_ao.as_ref().unwrap());
        assert_eq!(gb.pixels, g.pixels, "ground AO pixels changed");
        assert_eq!((gb.w, gb.h), (g.w, g.h));
        assert_eq!(gb.x0, g.x0);
        assert_eq!(gb.y, g.y);
    }

    /// A stale or corrupt sidecar must be REJECTED, not half-read. Loading one
    /// with the wrong stride would sample the atlas through nonsense UVs, and
    /// out-of-range indices would be a GPU read past the vertex buffer.
    #[test]
    fn junk_is_rejected_rather_than_half_read() {
        assert!(StaticModel::from_aomesh(b"").is_none(), "empty accepted");
        assert!(StaticModel::from_aomesh(b"not-a-mesh-at-all").is_none(), "bad magic accepted");

        let good = sample().to_aomesh();
        assert!(
            StaticModel::from_aomesh(&good[..good.len() - 8]).is_none(),
            "truncated sidecar accepted"
        );

        let mut wrong_version = good.clone();
        wrong_version[6] = 0xFF;
        assert!(StaticModel::from_aomesh(&wrong_version).is_none(), "old version accepted");

        let mut bad_index = sample();
        bad_index.indices = vec![0, 1, 99];
        assert!(
            StaticModel::from_aomesh(&bad_index.to_aomesh()).is_none(),
            "index past the end of the vertex list accepted"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single-triangle GLB built in code, so the parser is covered without
    /// requiring the downloaded catalogue.
    fn tiny_glb(with_node_translation: bool) -> Vec<u8> {
        let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut bin: Vec<u8> = Vec::new();
        for f in positions {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let node = if with_node_translation {
            r#"{"mesh":0,"translation":[10.0,0.0,0.0]}"#
        } else {
            r#"{"mesh":0}"#
        };
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},
            "nodes":[{node}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}}],
            "buffers":[{{"byteLength":{}}}],
            "images":[{{"uri":"Textures/colormap.png"}}]}}"#,
            bin.len(),
            bin.len()
        );
        let mut json_bytes = json.into_bytes();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    #[test]
    fn parses_a_static_mesh_without_a_skin() {
        let m = StaticModel::parse_glb(&tiny_glb(false)).unwrap();
        assert_eq!(m.vertex_count(), 3);
        assert_eq!(m.triangle_count(), 1);
        assert_eq!(m.texture_uri.as_deref(), Some("Textures/colormap.png"));
        assert_eq!(m.vertices.len(), 3 * MODEL_VERTEX_FLOATS);
        assert!(m.draw_layers.is_empty(), "one image is still a single draw");
    }

    #[test]
    fn parse_glb_keeps_one_draw_layer_per_embedded_image() {
        let positions_a: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let positions_b: [[f32; 3]; 3] = [[2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 0.0]];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let indices = [0u32, 1, 2];
        let glb = makepad_gltf::write_glb_mesh_textured_parts(
            &[
                makepad_gltf::GlbTexturedPart {
                    positions: &positions_a,
                    uvs: &uvs,
                    indices: &indices,
                    base_color_png: b"png-layer-a",
                    normals: None,
                    base_color_factor: None,
                },
                makepad_gltf::GlbTexturedPart {
                    positions: &positions_b,
                    uvs: &uvs,
                    indices: &indices,
                    base_color_png: b"png-layer-b",
                    normals: None,
                    base_color_factor: None,
                },
            ],
            true,
        );
        let m = StaticModel::parse_glb(&glb).expect("two-part glb");
        assert_eq!(m.draw_layers.len(), 2, "each embedded image is its own draw");
        let pngs: Vec<_> = m
            .draw_layers
            .iter()
            .filter_map(|l| l.texture_png.as_deref())
            .collect();
        assert!(pngs.contains(&b"png-layer-a".as_slice()));
        assert!(pngs.contains(&b"png-layer-b".as_slice()));
        assert_eq!(
            m.draw_layers.iter().map(|l| l.indices.len()).sum::<usize>(),
            6
        );
    }

    #[test]
    fn parse_local_duke_e1l1_keeps_every_tile_layer() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../local/ai_content_app/import/duke3d/work/source/worlds/e1l1.glb"
        );
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let m = StaticModel::parse_glb(&bytes).expect("e1l1 glb");
        assert!(
            m.draw_layers.len() > 8,
            "expected many tile layers, got {}",
            m.draw_layers.len()
        );
        assert!(
            m.draw_layers.iter().all(|l| l.texture_png.is_some()),
            "every layer should keep its embedded PNG"
        );
    }

    /// COLOR_0 vertex colors (float VEC3, the trellis/gltf-writer shape)
    /// multiply into the packed tint lane.
    #[test]
    fn color0_multiplies_into_vertex_tint() {
        let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let colors: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, 0.5, 1.0];
        let mut bin: Vec<u8> = Vec::new();
        for f in positions {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        for f in colors {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},
            "nodes":[{{"mesh":0}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"COLOR_0":1}}}}]}}],
            "accessors":[
              {{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
              {{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}}],
            "bufferViews":[
              {{"buffer":0,"byteOffset":0,"byteLength":36}},
              {{"buffer":0,"byteOffset":36,"byteLength":36}}],
            "buffers":[{{"byteLength":{}}}]}}"#,
            bin.len()
        );
        let mut json_bytes = json.into_bytes();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);

        let m = StaticModel::parse_glb(&glb).unwrap();
        assert_eq!(m.vertex_count(), 3);
        // No material -> tint 1.0; the packed colour lane is the vertex
        // colour itself. pack_unorm8x4 order: r | g<<8 | b<<16 | ao<<24.
        let unpack = |v: f32| {
            let bits = v.to_bits();
            [
                (bits & 255) as f32 / 255.0,
                ((bits >> 8) & 255) as f32 / 255.0,
                ((bits >> 16) & 255) as f32 / 255.0,
            ]
        };
        let c0 = unpack(m.vertices[5]);
        let c2 = unpack(m.vertices[2 * MODEL_VERTEX_FLOATS + 5]);
        assert!((c0[0] - 1.0).abs() < 0.01 && c0[1] < 0.01 && c0[2] < 0.01, "{c0:?}");
        assert!(
            (c2[0] - 0.5).abs() < 0.01 && (c2[1] - 0.5).abs() < 0.01 && (c2[2] - 1.0).abs() < 0.01,
            "{c2:?}"
        );
    }

    /// The node transform must be folded into the vertices, not dropped —
    /// dropping it is why a prop would silently render at the origin.
    #[test]
    fn node_transform_is_baked_into_vertices() {
        let m = StaticModel::parse_glb(&tiny_glb(true)).unwrap();
        assert!(
            (m.min.x - 10.0).abs() < 1.0e-5,
            "translation not baked: min.x = {}",
            m.min.x
        );
        assert!((m.max.x - 11.0).abs() < 1.0e-5);
    }

    #[test]
    fn rejects_non_glb_input() {
        assert!(StaticModel::parse_glb(b"not a gltf at all").is_err());
        assert!(StaticModel::parse_glb(&[]).is_err());
    }

    /// A house is walls + roof + door frame, so its collider must be several
    /// boxes with the doorway left as a GAP. One AABB would make the door
    /// solid, which is the difference between a building and a rock.
    #[test]
    fn collider_parts_keep_structure_and_drop_trim() {
        let model = StaticModel {
            vertices: Vec::new(),
            indices: Vec::new(),
            texture_uri: None,
            texture_png: None,
            min: Vec3f { x: -2.0, y: 0.0, z: -2.0 },
            max: Vec3f { x: 2.0, y: 3.0, z: 2.0 },
            parts: vec![
                // Two wall slabs either side of a doorway.
                (Vec3f { x: -2.0, y: 0.0, z: -2.0 }, Vec3f { x: -0.5, y: 3.0, z: 2.0 }),
                (Vec3f { x: 0.5, y: 0.0, z: -2.0 }, Vec3f { x: 2.0, y: 3.0, z: 2.0 }),
                // A door handle: substantial in no axis, must be dropped.
                (Vec3f { x: -0.4, y: 1.2, z: 1.9 }, Vec3f { x: -0.3, y: 1.3, z: 2.0 }),
            ],
            draw_layers: Vec::new(),
            detail_png: None,
            detail_scale: [0.0, 0.0],
            prelit: false,
            ground_ao: None,
        };
        let parts = model.collider_parts();
        assert_eq!(parts.len(), 2, "expected two walls, got {parts:?}");
        // And the doorway between them really is open.
        let gap = parts.iter().all(|(a, b)| !(a.x < 0.0 && b.x > 0.0));
        assert!(gap, "a collider spans the doorway: {parts:?}");
    }

    /// A gallows is four POSTS and some beams. Posts are thin in x and z —
    /// the old "substantial in two axes" rule filed them as trim, and the
    /// player ran clean through the legs of every pole-built prop. A post
    /// (tall, starts at the ground) must survive; a high hanging rod (a
    /// lintel bracket) is still trim.
    #[test]
    fn collider_parts_keep_posts_and_drop_hanging_trim() {
        let model = StaticModel {
            vertices: Vec::new(),
            indices: Vec::new(),
            texture_uri: None,
            texture_png: None,
            min: Vec3f { x: -2.0, y: 0.0, z: -2.0 },
            max: Vec3f { x: 2.0, y: 3.0, z: 2.0 },
            parts: vec![
                // A leg: 0.2 × 2.4 × 0.2, starting on the ground.
                (Vec3f { x: -1.9, y: 0.0, z: -1.9 }, Vec3f { x: -1.7, y: 2.4, z: -1.7 }),
                // A hanging rod up top, same girth: trim, must drop.
                (Vec3f { x: 0.0, y: 2.5, z: 0.0 }, Vec3f { x: 0.2, y: 2.9, z: 0.2 }),
            ],
            draw_layers: Vec::new(),
            detail_png: None,
            detail_scale: [0.0, 0.0],
            prelit: false,
            ground_ao: None,
        };
        let parts = model.collider_parts();
        assert_eq!(parts.len(), 1, "the leg survives, the rod does not: {parts:?}");
        assert!(parts[0].0.y < 0.1, "the survivor is the ground-standing leg");
    }

    /// Low-res by design: a prop with many primitives must not produce a
    /// collider per screw. The biggest boxes carry the shape.
    #[test]
    fn collider_parts_are_capped() {
        let mut parts = Vec::new();
        for i in 0..40 {
            let x = i as f32 * 0.5;
            parts.push((
                Vec3f { x, y: 0.0, z: 0.0 },
                Vec3f { x: x + 2.0, y: 2.0, z: 2.0 },
            ));
        }
        let model = StaticModel {
            vertices: Vec::new(),
            indices: Vec::new(),
            texture_uri: None,
            texture_png: None,
            min: Vec3f { x: 0.0, y: 0.0, z: 0.0 },
            max: Vec3f { x: 22.0, y: 2.0, z: 2.0 },
            parts,
            draw_layers: Vec::new(),
            detail_png: None,
            detail_scale: [0.0, 0.0],
            prelit: false,
            ground_ao: None,
        };
        assert!(model.collider_parts().len() <= 8);
    }
}

// Real-asset evaluation: the collider contract measured against the ACTUAL
// kit meshes the games place, not synthetic fixtures. A player must not pass
// through legs/posts, must be able to STAND on the walkable deck, and a
// gate's opening must stay open — in every game mode, this is the one
// physics world. Kept as a test so the contract re-verifies whenever the
// extraction rules change. Skips silently when the asset pack is absent.
#[cfg(test)]
mod real_asset_tests {
    use super::*;

    fn kit_model(name: &str) -> Option<StaticModel> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit")
            .join(name);
        let bytes = std::fs::read(path).ok()?;
        StaticModel::parse_glb(&bytes).ok()
    }

    fn dump(name: &str, m: &StaticModel) {
        eprintln!(
            "== {name}: bounds ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}), {} raw parts",
            m.min.x, m.min.y, m.min.z, m.max.x, m.max.y, m.max.z,
            m.parts.len()
        );
        for (i, (a, b)) in m.parts.iter().enumerate() {
            eprintln!(
                "   raw[{i}] ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) size {:.2}x{:.2}x{:.2}",
                a.x, a.y, a.z, b.x, b.y, b.z,
                b.x - a.x, b.y - a.y, b.z - a.z
            );
        }
        for (i, (a, b)) in m.collider_parts().iter().enumerate() {
            eprintln!(
                "   col[{i}] ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2})",
                a.x, a.y, a.z, b.x, b.y, b.z
            );
        }
    }

    /// The voxel extraction IS the collider contract now — assert it against
    /// the real meshes: a corridor must have a walkable floor and side walls
    /// with an OPEN middle; a barred gate must keep its frame; every model's
    /// interior air at head height must not be solid.
    /// For every kit mesh: slice at eye height and compare the triangles'
    /// silhouette against the voxel boxes at the same height. A collider
    /// tighter than the art is the "wade into the pole before it blocks"
    /// bug, measured — per model, worst side, in meters.
    #[test]
    fn eye_height_silhouette_gap_report() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("asset pack absent — skipped");
            return;
        };
        let mut worst_overall = 0.0f32;
        let mut worst_name = String::new();
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("glb") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(m) = StaticModel::parse_glb(&bytes) else { continue };
            let boxes = m.voxel_collider_boxes();
            // eye height in model space for a scale-2 placement
            let y = 0.8f32;
            // Triangle silhouette at the slice: xz-AABB of triangles
            // crossing y.
            let vp = |i: u32| {
                let o = i as usize * MODEL_VERTEX_FLOATS;
                (m.vertices[o], m.vertices[o + 1], m.vertices[o + 2])
            };
            let mut t_lo = (f32::MAX, f32::MAX);
            let mut t_hi = (f32::MIN, f32::MIN);
            for tri in m.indices.chunks_exact(3) {
                let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
                let ymin = a.1.min(b.1).min(c.1);
                let ymax = a.1.max(b.1).max(c.1);
                if ymin > y || ymax < y {
                    continue;
                }
                for v in [a, b, c] {
                    t_lo.0 = t_lo.0.min(v.0);
                    t_lo.1 = t_lo.1.min(v.2);
                    t_hi.0 = t_hi.0.max(v.0);
                    t_hi.1 = t_hi.1.max(v.2);
                }
            }
            if t_lo.0 > t_hi.0 {
                continue; // nothing at eye height
            }
            // Voxel silhouette at the slice.
            let mut v_lo = (f32::MAX, f32::MAX);
            let mut v_hi = (f32::MIN, f32::MIN);
            for (a, b) in &boxes {
                if a.y > y || b.y < y {
                    continue;
                }
                v_lo.0 = v_lo.0.min(a.x);
                v_lo.1 = v_lo.1.min(a.z);
                v_hi.0 = v_hi.0.max(b.x);
                v_hi.1 = v_hi.1.max(b.z);
            }
            let gap = if v_lo.0 > v_hi.0 {
                // art at eye height, NO collider at all
                f32::MAX
            } else {
                (v_lo.0 - t_lo.0)
                    .max(v_lo.1 - t_lo.1)
                    .max(t_hi.0 - v_hi.0)
                    .max(t_hi.1 - v_hi.1)
                    .max(0.0)
            };
            if gap == f32::MAX {
                eprintln!("{name}: ART at eye height but NO collider");
                worst_overall = worst_overall.max(99.0);
                worst_name = name.clone();
            } else if gap > 0.05 {
                eprintln!(
                    "{name}: collider tighter than art by {:.2}m (x{:.2} model)",
                    gap * 2.0, gap
                );
                if gap > worst_overall {
                    worst_overall = gap;
                    worst_name = name.clone();
                }
            }
        }
        eprintln!("worst: {worst_name} {worst_overall:.2}m model-space");
    }

    #[test]
    fn voxel_box_budget_covers_the_big_rooms() {
        for name in ["room-large.glb", "room-small.glb", "corridor.glb",
                     "gate-metal-bars.glb"] {
            let Some(m) = kit_model(name) else { continue };
            let boxes = m.voxel_collider_boxes();
            eprintln!("{name}: {} boxes", boxes.len());
        }
    }

    #[test]
    fn voxel_colliders_from_real_meshes() {
        let Some(m) = kit_model("corridor.glb") else {
            eprintln!("asset pack absent — skipped");
            return;
        };
        let boxes = m.voxel_collider_boxes();
        eprintln!("corridor: {} voxel boxes", boxes.len());
        for (a, b) in &boxes {
            eprintln!(
                "   ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2})",
                a.x, a.y, a.z, b.x, b.y, b.z
            );
        }
        assert!(!boxes.is_empty());
        // A floor to stand on near the model base…
        assert!(
            boxes.iter().any(|(a, b)| a.y <= 0.4 && (b.x - a.x) > 2.0 && (b.z - a.z) > 2.0),
            "no walkable floor slab found"
        );
        // …and free air mid-corridor at head height: nothing solid may cover
        // the model centre at y≈1.5.
        let c = vec3f(0.0, 1.5, 0.0);
        let blocked = boxes.iter().any(|(a, b)| {
            c.x > a.x && c.x < b.x && c.y > a.y && c.y < b.y && c.z > a.z && c.z < b.z
        });
        assert!(!blocked, "corridor interior is solid at head height");

        if let Some(g) = kit_model("gate-metal-bars.glb") {
            let gb = g.voxel_collider_boxes();
            eprintln!("gate-metal-bars: {} voxel boxes", gb.len());
            // The frame's jambs must be present: something solid near both
            // x extremes at walking height.
            let left = gb.iter().any(|(a, b)| a.x <= -1.6 && b.y > 1.0 && a.y < 1.0);
            let right = gb.iter().any(|(a, b)| b.x >= 1.6 && b.y > 1.0 && a.y < 1.0);
            assert!(left && right, "gate jambs missing: {gb:?}");
        }
    }

    #[test]
    fn dungeon_kit_colliders_match_the_meshes() {
        let names = [
            "corridor.glb",
            "corridor-corner.glb",
            "room-small.glb",
            "gate.glb",
            "gate-metal-bars.glb",
            "stairs.glb",
        ];
        let mut seen = 0;
        for name in names {
            let Some(m) = kit_model(name) else { continue };
            seen += 1;
            dump(name, &m);
        }
        if seen == 0 {
            eprintln!("asset pack absent — skipped");
        }
    }
}

// Focused AO-correctness cycle for the two broken-looking arena assets (the
// dungeon kit's column tile and timber structures). Physical invariant: an
// upward-facing surface with open sky above it must bake BRIGHT. The live
// symptom (dark column cap, dark timber faces with bright convex edges) is
// the signature of hemisphere origins on the wrong side of the surface.
#[cfg(test)]
mod ao_invariant_tests {
    use super::*;

    fn kit_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit")
            .join(name)
    }

    #[test]
    fn tops_open_to_the_sky_bake_bright() {
        let Ok(entries) = std::fs::read_dir(kit_path("")) else {
            eprintln!("asset pack absent — skipped");
            return;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".glb"))
            .collect();
        names.sort();
        // Fast iterate cycle: ONLY the two problem assets (the timber
        // structure tile and the big column tile) — never the whole pack;
        // the big rooms alone cost minutes per bake.
        names.retain(|n| n == "corridor.glb" || n == "template-corner.glb");
        // Fast iterate cycle: the two problem assets (timber structure and
        // the corner column) and one control — NOT the whole pack; the big
        // rooms alone cost minutes per bake.
        names.retain(|n| {
            n == "corridor.glb" || n == "template-corner.glb" || n == "gate.glb"
        });
        for name in names {
            let Ok(bytes) = std::fs::read(kit_path(&name)) else { continue };
            let mut atlas = crate::ao_atlas::AoAtlas::new(crate::ao_atlas::ATLAS_MAX);
            let Ok(m) = StaticModel::parse_glb_baked(&bytes, &mut atlas) else {
                continue;
            };
            if atlas.size == 0 {
                continue;
            }
            let vp = |i: u32| {
                let o = i as usize * MODEL_VERTEX_FLOATS;
                Vec3f {
                    x: m.vertices[o],
                    y: m.vertices[o + 1],
                    z: m.vertices[o + 2],
                }
            };
            // The highest upward-facing triangle: sky above it is open by
            // construction (nothing in the model is higher).
            let mut top: Option<(usize, f32)> = None;
            for (t, tri) in m.indices.chunks_exact(3).enumerate() {
                let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
                let e1 = b - a;
                let e2 = c - a;
                let n = Vec3f {
                    x: e1.y * e2.z - e1.z * e2.y,
                    y: e1.z * e2.x - e1.x * e2.z,
                    z: e1.x * e2.y - e1.y * e2.x,
                };
                let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
                if len < 1.0e-9 {
                    continue;
                }
                let cy = (a.y + b.y + c.y) / 3.0;
                if n.y.abs() / len > 0.9 {
                    if top.map_or(true, |(_, best)| cy > best) {
                        top = Some((t, cy));
                    }
                }
            }
            let Some((t, _)) = top else { continue };
            let tri = &m.indices[t * 3..t * 3 + 3];
            // Sample the baked atlas at the triangle's centroid UV.
            let uv = |i: u32| {
                crate::model::unpack_ao_uv(
                    m.vertices[i as usize * MODEL_VERTEX_FLOATS + 6],
                )
            };
            let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
            let u = (ua[0] + ub[0] + uc[0]) / 3.0;
            let v = (ua[1] + ub[1] + uc[1]) / 3.0;
            let x = ((u * atlas.size as f32) as usize).min(atlas.size - 1);
            let y = ((v * atlas.size as f32) as usize).min(atlas.size - 1);
            let bright = atlas.pixels[y * atlas.size + x] as f32 / 255.0;
            // Geometric-vs-winding orientation at the top: does the highest
            // face's cross-product normal point UP (correct winding) or DOWN?
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let e1 = b - a;
            let e2 = c - a;
            let ny = e1.z * e2.x - e1.x * e2.z;
            eprintln!(
                "{name}: top texel {:.2} (1=open sky), winding normal.y {}",
                bright,
                if ny > 0.0 { "UP ok" } else { "DOWN (flipped!)" }
            );
        }
    }
}

// One-model, 8-second bake check: did the authored-normal orientation fix
// actually flip the corridor's bake right-side out? Top face open to the sky
// must be bright; before the fix it baked dark (inside-out hemispheres).
#[cfg(test)]
mod ao_fix_check {
    use super::*;

    #[test]
    fn corridor_bakes_right_side_out() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit/corridor.glb");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let mut atlas = crate::ao_atlas::AoAtlas::new(crate::ao_atlas::ATLAS_MAX);
        let m = StaticModel::parse_glb_baked(&bytes, &mut atlas).unwrap();
        let vp = |i: u32| {
            let o = i as usize * MODEL_VERTEX_FLOATS;
            Vec3f { x: m.vertices[o], y: m.vertices[o + 1], z: m.vertices[o + 2] }
        };
        let mut top: Option<(usize, f32)> = None;
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let e1 = b - a;
            let e2 = c - a;
            let ny = e1.z * e2.x - e1.x * e2.z;
            let nlen = {
                let nx = e1.y * e2.z - e1.z * e2.y;
                let nz = e1.x * e2.y - e1.y * e2.x;
                (nx * nx + ny * ny + nz * nz).sqrt()
            };
            if nlen < 1.0e-9 { continue; }
            if ny.abs() / nlen > 0.9 {
                let cy = (a.y + b.y + c.y) / 3.0;
                if top.map_or(true, |(_, best)| cy > best) {
                    top = Some((t, cy));
                }
            }
        }
        let (t, _) = top.expect("no horizontal face");
        let tri = &m.indices[t * 3..t * 3 + 3];
        let uv = |i: u32| unpack_ao_uv(m.vertices[i as usize * MODEL_VERTEX_FLOATS + 6]);
        let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
        let u = (ua[0] + ub[0] + uc[0]) / 3.0;
        let v = (ua[1] + ub[1] + uc[1]) / 3.0;
        let x = ((u * atlas.size as f32) as usize).min(atlas.size - 1);
        let y = ((v * atlas.size as f32) as usize).min(atlas.size - 1);
        let bright = atlas.pixels[y * atlas.size + x] as f32 / 255.0;
        eprintln!("corridor top texel: {bright:.2} (1.0 = open sky)");
        assert!(bright > 0.6, "top face still bakes dark: {bright:.2} — inside-out");
    }
}

// Checks the SHIPPED sidecar pair on disk (what the app actually loads),
// not an in-process bake: top face texel must be bright.
#[cfg(test)]
mod sidecar_pair_check {
    use super::*;

    #[test]
    fn corridor_sidecar_top_is_bright() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Some(m) = std::fs::read(root.join("corridor.aomesh"))
            .ok()
            .and_then(|b| StaticModel::from_aomesh(&b))
        else {
            eprintln!("sidecar absent — skipped");
            return;
        };
        // Decode the greyscale png the same way the renderer does
        // (renderer::gray_png_texture): 8-bit grey, filter 0, zlib IDAT.
        let png = std::fs::read(root.join("corridor.ao.png")).unwrap();
        let (mut o, mut w, mut h, mut idat) = (8usize, 0usize, 0usize, Vec::new());
        while o + 8 <= png.len() {
            let len =
                u32::from_be_bytes(png[o..o + 4].try_into().unwrap()) as usize;
            let kind = &png[o + 4..o + 8];
            let body = &png[o + 8..o + 8 + len];
            match kind {
                b"IHDR" => {
                    w = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
                    h = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
                }
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            o += 8 + len + 4;
        }
        let inflated = makepad_fast_inflate::zlib_decompress_vec(&idat).unwrap();
        let mut raw = Vec::with_capacity(w * h);
        for row in inflated.chunks_exact(w + 1) {
            raw.extend_from_slice(&row[1..]);
        }
        let vp = |i: u32| {
            let o = i as usize * MODEL_VERTEX_FLOATS;
            Vec3f { x: m.vertices[o], y: m.vertices[o + 1], z: m.vertices[o + 2] }
        };
        let mut top: Option<(usize, f32)> = None;
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let e1 = b - a;
            let e2 = c - a;
            let nx = e1.y * e2.z - e1.z * e2.y;
            let ny = e1.z * e2.x - e1.x * e2.z;
            let nz = e1.x * e2.y - e1.y * e2.x;
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            if len < 1.0e-9 { continue; }
            if ny.abs() / len > 0.9 {
                let cy = (a.y + b.y + c.y) / 3.0;
                if top.map_or(true, |(_, best)| cy > best) {
                    top = Some((t, cy));
                }
            }
        }
        let (t, _) = top.expect("no horizontal face");
        let tri = &m.indices[t * 3..t * 3 + 3];
        let uv = |i: u32| unpack_ao_uv(m.vertices[i as usize * MODEL_VERTEX_FLOATS + 6]);
        let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
        let u = (ua[0] + ub[0] + uc[0]) / 3.0;
        let v = (ua[1] + ub[1] + uc[1]) / 3.0;
        let x = ((u * w as f32) as usize).min(w - 1);
        let y = ((v * h as f32) as usize).min(h - 1);
        let bright = raw[y * w + x] as f32 / 255.0;
        eprintln!("disk sidecar corridor top texel: {bright:.2}");
        assert!(bright > 0.6, "disk pair still dark on top: {bright:.2}");
    }
}

// Chart-border forensics: print the texel neighbourhood around a top-face
// CORNER uv. Distinguishes "edge texels baked dark" from "white edge, dark
// gutter bleeding through bilinear".
#[cfg(test)]
mod chart_border_forensics {
    use super::*;

    #[test]
    fn corner_texel_neighbourhood() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Some(m) = std::fs::read(root.join("corridor.aomesh"))
            .ok()
            .and_then(|b| StaticModel::from_aomesh(&b))
        else {
            eprintln!("sidecar absent — skipped");
            return;
        };
        let png = std::fs::read(root.join("corridor.ao.png")).unwrap();
        let (mut o, mut w, mut h, mut idat) = (8usize, 0usize, 0usize, Vec::new());
        while o + 8 <= png.len() {
            let len = u32::from_be_bytes(png[o..o + 4].try_into().unwrap()) as usize;
            let kind = &png[o + 4..o + 8];
            let body = &png[o + 8..o + 8 + len];
            match kind {
                b"IHDR" => {
                    w = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
                    h = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
                }
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            o += 8 + len + 4;
        }
        let inflated = makepad_fast_inflate::zlib_decompress_vec(&idat).unwrap();
        let mut raw = Vec::with_capacity(w * h);
        for row in inflated.chunks_exact(w + 1) {
            raw.extend_from_slice(&row[1..]);
        }
        let vp = |i: u32| {
            let o = i as usize * MODEL_VERTEX_FLOATS;
            Vec3f { x: m.vertices[o], y: m.vertices[o + 1], z: m.vertices[o + 2] }
        };
        // highest horizontal face again
        let mut top: Option<(usize, f32)> = None;
        for (t, tri) in m.indices.chunks_exact(3).enumerate() {
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let e1 = b - a;
            let e2 = c - a;
            let nx = e1.y * e2.z - e1.z * e2.y;
            let ny = e1.z * e2.x - e1.x * e2.z;
            let nz = e1.x * e2.y - e1.y * e2.x;
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            if len < 1.0e-9 { continue; }
            if ny.abs() / len > 0.9 {
                let cy = (a.y + b.y + c.y) / 3.0;
                if top.map_or(true, |(_, best)| cy > best) {
                    top = Some((t, cy));
                }
            }
        }
        let (t, _) = top.unwrap();
        let tri = &m.indices[t * 3..t * 3 + 3];
        let uv = |i: u32| unpack_ao_uv(m.vertices[i as usize * MODEL_VERTEX_FLOATS + 6]);
        for (k, &vi) in tri.iter().enumerate() {
            let [u, v] = uv(vi);
            let x = ((u * w as f32) as isize).clamp(0, w as isize - 1);
            let y = ((v * h as f32) as isize).clamp(0, h as isize - 1);
            eprintln!("corner {k}: uv ({u:.4},{v:.4}) -> texel ({x},{y})");
            for dy in -2i32..=2 {
                let mut row = String::new();
                for dx in -2i32..=2 {
                    let sx = (x + dx as isize).clamp(0, w as isize - 1) as usize;
                    let sy = (y + dy as isize).clamp(0, h as isize - 1) as usize;
                    row.push_str(&format!("{:4}", raw[sy * w + sx]));
                }
                eprintln!("   {row}");
            }
        }
    }
}

#[cfg(test)]
mod double_sided_probe {
    use super::*;

    /// Count coincident opposite-winding triangle pairs — double-sided
    /// authoring — in the pergola tile that keeps defeating AO orientation.
    #[test]
    fn pergola_double_sidedness() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit/template-floor-layer-raised.glb",
        );
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let m = StaticModel::parse_glb(&bytes).unwrap();
        let vp = |i: u32| {
            let o = i as usize * MODEL_VERTEX_FLOATS;
            (
                (m.vertices[o] * 2048.0).round() as i64,
                (m.vertices[o + 1] * 2048.0).round() as i64,
                (m.vertices[o + 2] * 2048.0).round() as i64,
            )
        };
        use std::collections::HashMap;
        let mut seen: HashMap<[(i64, i64, i64); 3], usize> = HashMap::new();
        let tris = m.indices.len() / 3;
        for t in 0..tris {
            let mut k = [
                vp(m.indices[t * 3]),
                vp(m.indices[t * 3 + 1]),
                vp(m.indices[t * 3 + 2]),
            ];
            k.sort();
            *seen.entry(k).or_insert(0) += 1;
        }
        let dup: usize = seen.values().filter(|&&c| c > 1).map(|&c| c).sum();
        let unique = seen.len();
        eprintln!(
            "{tris} triangles, {unique} unique position-keys, {dup} in coincident groups"
        );
    }
}

#[cfg(test)]
mod normal_authoring_probe {
    use super::*;

    /// Are the kit meshes flat-shaded (per-face normals, edge vertices
    /// duplicated per side) or smooth-shaded across edges (shared/averaged
    /// normals)? Every bake-normal decision hinges on this — measured, not
    /// assumed: for each position shared by faces of different planes,
    /// report whether its normals are per-face or averaged.
    #[test]
    fn pergola_edge_normals() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit/template-floor-layer-raised.glb",
        );
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let m = StaticModel::parse_glb(&bytes).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let count = m.vertices.len() / stride;
        use std::collections::HashMap;
        let mut by_pos: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
        for i in 0..count {
            let k = (
                (m.vertices[i * stride] * 4096.0).round() as i64,
                (m.vertices[i * stride + 1] * 4096.0).round() as i64,
                (m.vertices[i * stride + 2] * 4096.0).round() as i64,
            );
            by_pos.entry(k).or_default().push(i);
        }
        let dec = |i: usize| {
            let (ox, oy) = crate::skin::unpack2f16_pub(m.vertices[i * stride + 3]);
            crate::skin::oct_decode_pub(ox, oy)
        };
        let (mut flat_like, mut smooth_like, mut printed) = (0, 0, 0);
        for (_, ids) in by_pos.iter().filter(|(_, v)| v.len() >= 2) {
            // spread of normals among the copies at this position
            let mut min_dot = 1.0f32;
            for a in 0..ids.len() {
                for b in (a + 1)..ids.len() {
                    let na = dec(ids[a]);
                    let nb = dec(ids[b]);
                    let d = na.x * nb.x + na.y * nb.y + na.z * nb.z;
                    if d < min_dot {
                        min_dot = d;
                    }
                }
            }
            if min_dot > 0.985 {
                smooth_like += 1; // all copies share one direction
            } else {
                flat_like += 1; // distinct per-face directions
            }
            if printed < 6 && min_dot <= 0.985 {
                printed += 1;
                let n0 = dec(ids[0]);
                let n1 = dec(ids[ids.len() - 1]);
                eprintln!(
                    "corner pos with {} verts: n0 ({:.2},{:.2},{:.2}) nN ({:.2},{:.2},{:.2}) min_dot {:.2}",
                    ids.len(), n0.x, n0.y, n0.z, n1.x, n1.y, n1.z, min_dot
                );
            }
        }
        eprintln!(
            "shared-position vertices: {} flat-like (distinct normals), {} smooth-like (one direction)",
            flat_like, smooth_like
        );
    }
}
