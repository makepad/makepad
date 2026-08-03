//! Mesh building and packing into `geom.GameMeshVertex`.
//!
//! Generators build into a fat intermediate (positions, normals, uvs,
//! colours, plus the two animation weights), bake vertex shading into the
//! colours, then pack once at `finish()`. Nothing downstream sees the fat
//! form, so the conversion is a single pass at generation time rather than
//! per frame.

use makepad_math::*;

/// Floats per vertex in the packed `geom.GameMeshVertex` layout.
pub const MESH_VERTEX_FLOATS: usize = 6;

/// Octahedral normal encoding: a unit vector into two components in [-1, 1].
///
/// Canonical home for this: `libs/game/render/src/skin.rs` used to own a
/// private copy, and two subtly different encoders paired with one shader
/// decoder is a bug waiting to happen.
pub fn oct_encode(n: Vec3f) -> (f32, f32) {
    let l1 = n.x.abs() + n.y.abs() + n.z.abs();
    if l1 < 1.0e-8 {
        return (0.0, 0.0);
    }
    let (x, y, z) = (n.x / l1, n.y / l1, n.z / l1);
    if z >= 0.0 {
        (x, y)
    } else {
        let sx = if x >= 0.0 { 1.0 } else { -1.0 };
        let sy = if y >= 0.0 { 1.0 } else { -1.0 };
        ((1.0 - y.abs()) * sx, (1.0 - x.abs()) * sy)
    }
}

/// Pack the growth order and wind flex weight into ONE unorm8 lane as two
/// 4-bit fields.
///
/// Vertex bytes are the measured Quest bottleneck, so these two animation
/// weights share the colour's alpha byte rather than adding a seventh float
/// (which would cost 4 bytes/vertex, +17%). 16 levels is ample for both: the
/// growth reveal uses a smoothstep band wide enough to hide the quantisation,
/// and flex is a soft weight nobody can count steps in.
///
/// Layout: `high nibble = growth, low nibble = flex`.
pub fn pack_growth_flex(growth: f32, flex: f32) -> f32 {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 15.0) + 0.5) as u32;
    ((q(growth) << 4) | q(flex)) as f32 / 255.0
}

/// Inverse of [`pack_growth_flex`], for tests and CPU-side queries. The
/// shader does the same arithmetic on the unpacked alpha lane.
pub fn unpack_growth_flex(a: f32) -> (f32, f32) {
    let v = (a * 255.0 + 0.5) as u32;
    (((v >> 4) & 0xf) as f32 / 15.0, (v & 0xf) as f32 / 15.0)
}

/// A finished, packed mesh ready for `Geometry::update`.
#[derive(Clone, Debug, Default)]
pub struct GenMesh {
    /// Packed `GameMeshVertex` stream, [`MESH_VERTEX_FLOATS`] per vertex.
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub min: Vec3f,
    pub max: Vec3f,
}

impl GenMesh {
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / MESH_VERTEX_FLOATS
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Bytes this mesh occupies on the GPU (vertices + indices).
    pub fn gpu_bytes(&self) -> usize {
        self.vertices.len() * 4 + self.indices.len() * 4
    }

    pub fn size(&self) -> Vec3f {
        vec3f(
            self.max.x - self.min.x,
            self.max.y - self.min.y,
            self.max.z - self.min.z,
        )
    }
}

/// One vertex in the fat intermediate form.
#[derive(Clone, Copy, Debug)]
pub struct GenVertex {
    pub pos: Vec3f,
    pub normal: Vec3f,
    pub uv: [f32; 2],
    pub color: [f32; 3],
    /// Generation order along the skeleton, 0 at the root, 1 at the tips.
    /// Drives the growth reveal.
    pub growth: f32,
    /// Wind flex weight, 0 = anchored (trunk), 1 = floppiest (leaf tip).
    pub flex: f32,
}

impl Default for GenVertex {
    fn default() -> Self {
        Self {
            pos: Vec3f::default(),
            normal: vec3f(0.0, 1.0, 0.0),
            uv: [0.0, 0.0],
            color: [1.0, 1.0, 1.0],
            growth: 1.0,
            flex: 0.0,
        }
    }
}

/// Accumulates geometry, then bakes and packs it.
#[derive(Clone, Debug, Default)]
pub struct MeshBuilder {
    pub verts: Vec<GenVertex>,
    pub indices: Vec<u32>,
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn vertex(&mut self, v: GenVertex) -> u32 {
        self.verts.push(v);
        (self.verts.len() - 1) as u32
    }

    pub fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    pub fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Append another builder's geometry, offsetting its indices.
    pub fn append(&mut self, other: &MeshBuilder) {
        let base = self.verts.len() as u32;
        self.verts.extend_from_slice(&other.verts);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    fn bounds(&self) -> (Vec3f, Vec3f) {
        if self.verts.is_empty() {
            return (Vec3f::default(), Vec3f::default());
        }
        let mut min = self.verts[0].pos;
        let mut max = self.verts[0].pos;
        for v in &self.verts[1..] {
            min.x = min.x.min(v.pos.x);
            min.y = min.y.min(v.pos.y);
            min.z = min.z.min(v.pos.z);
            max.x = max.x.max(v.pos.x);
            max.y = max.y.max(v.pos.y);
            max.z = max.z.max(v.pos.z);
        }
        (min, max)
    }

    /// Bake cheap ambient shading into vertex colours.
    ///
    /// No ray casting: for stylised low-poly geometry two signals carry
    /// almost all of the readable shading, and both are O(vertices).
    ///
    /// * **Height** — lower is darker. Objects sit on ground, and contact
    ///   darkening is the single strongest grounding cue.
    /// * **Convexity** — a vertex whose normal points away from the mesh
    ///   centroid is exposed; one pointing back into the body is in a
    ///   crevice. `dot(normal, dir_to_centroid)` separates the two.
    ///
    /// The renderer already multiplies vertex colour into the lit result,
    /// so this costs nothing at runtime.
    pub fn bake_ambient(&mut self, strength: f32, height_bias: f32) {
        if self.verts.is_empty() || strength <= 0.0 {
            return;
        }
        let (min, max) = self.bounds();
        let height = (max.y - min.y).max(1.0e-6);
        let mut centroid = Vec3f::default();
        for v in &self.verts {
            centroid.x += v.pos.x;
            centroid.y += v.pos.y;
            centroid.z += v.pos.z;
        }
        let n = self.verts.len() as f32;
        centroid.x /= n;
        centroid.y /= n;
        centroid.z /= n;

        for v in &mut self.verts {
            let h = ((v.pos.y - min.y) / height).clamp(0.0, 1.0);
            let to_c = vec3f(
                centroid.x - v.pos.x,
                centroid.y - v.pos.y,
                centroid.z - v.pos.z,
            );
            let len = (to_c.x * to_c.x + to_c.y * to_c.y + to_c.z * to_c.z).sqrt();
            // Facing away from the centre = exposed = 1; facing into the
            // body = occluded = 0.
            let convex = if len > 1.0e-6 {
                let d = (v.normal.x * to_c.x + v.normal.y * to_c.y + v.normal.z * to_c.z) / len;
                (0.5 - 0.5 * d).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let lift = h * height_bias + (1.0 - height_bias);
            let occ = (convex * lift).clamp(0.0, 1.0);
            let shade = 1.0 - strength * (1.0 - occ);
            v.color[0] *= shade;
            v.color[1] *= shade;
            v.color[2] *= shade;
        }
    }

    /// Recompute normals by area-weighted face averaging. Generators that
    /// know their analytic normals should set them directly; this is for
    /// polygonised output.
    pub fn recompute_normals(&mut self) {
        for v in &mut self.verts {
            v.normal = Vec3f::default();
        }
        for t in self.indices.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let (pa, pb, pc) = (self.verts[a].pos, self.verts[b].pos, self.verts[c].pos);
            let u = vec3f(pb.x - pa.x, pb.y - pa.y, pb.z - pa.z);
            let w = vec3f(pc.x - pa.x, pc.y - pa.y, pc.z - pa.z);
            // Cross product magnitude is twice the area, so accumulating the
            // raw cross weights each face by its area for free.
            let f = vec3f(
                u.y * w.z - u.z * w.y,
                u.z * w.x - u.x * w.z,
                u.x * w.y - u.y * w.x,
            );
            for i in [a, b, c] {
                self.verts[i].normal.x += f.x;
                self.verts[i].normal.y += f.y;
                self.verts[i].normal.z += f.z;
            }
        }
        for v in &mut self.verts {
            let n = v.normal;
            let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
            v.normal = if len > 1.0e-8 {
                vec3f(n.x / len, n.y / len, n.z / len)
            } else {
                vec3f(0.0, 1.0, 0.0)
            };
        }
    }

    /// Pack into the GPU layout. Consumes the fat form.
    pub fn finish(self) -> GenMesh {
        let (min, max) = self.bounds();
        let mut vertices = Vec::with_capacity(self.verts.len() * MESH_VERTEX_FLOATS);
        for v in &self.verts {
            let (ox, oy) = oct_encode(v.normal);
            vertices.extend_from_slice(&[
                v.pos.x,
                v.pos.y,
                v.pos.z,
                makepad_draw::pack_pair_f16(ox, oy),
                makepad_draw::pack_pair_f16(v.uv[0], v.uv[1]),
                makepad_draw::pack_unorm8x4(
                    v.color[0],
                    v.color[1],
                    v.color[2],
                    pack_growth_flex(v.growth, v.flex),
                ),
            ]);
        }
        GenMesh {
            vertices,
            indices: self.indices,
            min,
            max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_flex_roundtrip_within_quantisation() {
        for gi in 0..16 {
            for fi in 0..16 {
                let (g, f) = (gi as f32 / 15.0, fi as f32 / 15.0);
                let (rg, rf) = unpack_growth_flex(pack_growth_flex(g, f));
                assert!((rg - g).abs() < 1.0e-4, "growth {g} -> {rg}");
                assert!((rf - f).abs() < 1.0e-4, "flex {f} -> {rf}");
            }
        }
        // The two fields must not bleed into each other.
        let (g, f) = unpack_growth_flex(pack_growth_flex(1.0, 0.0));
        assert_eq!((g, f), (1.0, 0.0));
        let (g, f) = unpack_growth_flex(pack_growth_flex(0.0, 1.0));
        assert_eq!((g, f), (0.0, 1.0));
    }

    #[test]
    fn oct_encode_survives_the_round_trip() {
        // Mirror of the shader decode, to prove encoder and decoder agree.
        fn oct_decode(ox: f32, oy: f32) -> Vec3f {
            let z = 1.0 - ox.abs() - oy.abs();
            let (mut x, mut y) = (ox, oy);
            if z < 0.0 {
                let sx = if x >= 0.0 { 1.0 } else { -1.0 };
                let sy = if y >= 0.0 { 1.0 } else { -1.0 };
                let (nx, ny) = ((1.0 - y.abs()) * sx, (1.0 - x.abs()) * sy);
                x = nx;
                y = ny;
            }
            let v = vec3f(x, y, z);
            let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
            vec3f(v.x / l, v.y / l, v.z / l)
        }
        let dirs = [
            vec3f(0.0, 1.0, 0.0),
            vec3f(0.0, -1.0, 0.0),
            vec3f(1.0, 0.0, 0.0),
            vec3f(-0.577, 0.577, -0.577),
            vec3f(0.267, 0.535, 0.802),
        ];
        for d in dirs {
            let l = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
            let n = vec3f(d.x / l, d.y / l, d.z / l);
            let (ox, oy) = oct_encode(n);
            let r = oct_decode(ox, oy);
            let dot = n.x * r.x + n.y * r.y + n.z * r.z;
            assert!(dot > 0.999, "normal {n:?} round-tripped to {r:?}");
        }
    }

    #[test]
    fn packed_stride_matches_the_gpu_layout() {
        let mut b = MeshBuilder::new();
        let a = b.vertex(GenVertex::default());
        let c = b.vertex(GenVertex {
            pos: vec3f(1.0, 0.0, 0.0),
            ..Default::default()
        });
        let d = b.vertex(GenVertex {
            pos: vec3f(0.0, 0.0, 1.0),
            ..Default::default()
        });
        b.tri(a, c, d);
        let m = b.finish();
        assert_eq!(m.vertices.len(), 3 * MESH_VERTEX_FLOATS);
        assert_eq!(m.vertex_count(), 3);
        assert_eq!(m.triangle_count(), 1);
        // 6 floats + 1 index each = the packed budget we advertise.
        assert_eq!(m.gpu_bytes(), 3 * 24 + 3 * 4);
    }

    #[test]
    fn ambient_bake_darkens_the_base_not_the_top() {
        let mut b = MeshBuilder::new();
        let low = b.vertex(GenVertex {
            pos: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, -1.0, 0.0),
            ..Default::default()
        });
        let high = b.vertex(GenVertex {
            pos: vec3f(0.0, 4.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            ..Default::default()
        });
        let side = b.vertex(GenVertex {
            pos: vec3f(1.0, 2.0, 0.0),
            normal: vec3f(1.0, 0.0, 0.0),
            ..Default::default()
        });
        b.tri(low, high, side);
        b.bake_ambient(0.6, 0.8);
        let (low, high) = (low as usize, high as usize);
        assert!(
            b.verts[low].color[0] < b.verts[high].color[0],
            "base {} should be darker than top {}",
            b.verts[low].color[0],
            b.verts[high].color[0]
        );
        for v in &b.verts {
            assert!((0.0..=1.0).contains(&v.color[0]));
        }
    }

    #[test]
    fn recomputed_normals_point_outward_and_are_unit() {
        // Two triangles of a ground plane; normals must come out +Y.
        let mut b = MeshBuilder::new();
        let a = b.vertex(GenVertex {
            pos: vec3f(0.0, 0.0, 0.0),
            ..Default::default()
        });
        let c = b.vertex(GenVertex {
            pos: vec3f(0.0, 0.0, 1.0),
            ..Default::default()
        });
        let d = b.vertex(GenVertex {
            pos: vec3f(1.0, 0.0, 0.0),
            ..Default::default()
        });
        b.tri(a, c, d);
        b.recompute_normals();
        for v in &b.verts {
            let n = v.normal;
            let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
            assert!((len - 1.0).abs() < 1.0e-5);
            assert!(n.y > 0.99, "expected +Y, got {n:?}");
        }
    }

    #[test]
    fn append_offsets_indices() {
        let mut a = MeshBuilder::new();
        let v0 = a.vertex(GenVertex::default());
        let v1 = a.vertex(GenVertex::default());
        let v2 = a.vertex(GenVertex::default());
        a.tri(v0, v1, v2);
        let b = a.clone();
        a.append(&b);
        assert_eq!(a.verts.len(), 6);
        assert_eq!(&a.indices[3..], &[3, 4, 5]);
    }
}
