//! The CPU mesh every geometry builder produces and the renderer uploads.
//!
//! Plain owned data, no `Cx`: positions, normals, uvs and optional vertex
//! colours and tangents, indexed triangles (counter-clockwise front faces,
//! right-handed, y up). The GPU vertex is Makepad's `geom.PbrVertex`
//! (16 floats, see [`Mesh::pack_pbr`]), so no pod type is registered here.

use makepad_math::*;

/// An indexed triangle mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// xyz tangent, w handedness; empty = derived when packing.
    pub tangents: Vec<[f32; 4]>,
    /// Linear RGBA per vertex; empty = white.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// Floats per packed GPU vertex (`geom.PbrVertex`).
pub const PBR_FLOATS: usize = 16;

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Push one vertex, returning its index.
    #[inline]
    pub fn vertex(&mut self, p: [f32; 3], n: [f32; 3], uv: [f32; 2]) -> u32 {
        let index = self.positions.len() as u32;
        self.positions.push(p);
        self.normals.push(n);
        self.uvs.push(uv);
        index
    }

    #[inline]
    pub fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    /// Two triangles a-b-c, a-c-d (counter-clockwise quad).
    #[inline]
    pub fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.indices.extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// Append `other`, keeping its colours and tangents aligned (missing
    /// ones filled with white / derived later).
    pub fn append(&mut self, other: &Mesh) {
        let base = self.positions.len() as u32;
        let had_colors = !self.colors.is_empty() || !other.colors.is_empty();
        if had_colors {
            self.colors.resize(self.positions.len(), [1.0; 4]);
        }
        let had_tangents = !self.tangents.is_empty() && !other.tangents.is_empty();
        if !had_tangents {
            self.tangents.clear();
        }
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.uvs.extend_from_slice(&other.uvs);
        if had_tangents {
            self.tangents.extend_from_slice(&other.tangents);
        }
        if had_colors {
            if other.colors.is_empty() {
                self.colors.resize(self.positions.len(), [1.0; 4]);
            } else {
                self.colors.extend_from_slice(&other.colors);
            }
        }
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    /// Axis-aligned bounds, `None` when empty.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let first = *self.positions.first()?;
        let mut min = first;
        let mut max = first;
        for p in &self.positions {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        Some((min, max))
    }

    /// Transform positions by `m` and normals by its inverse transpose.
    pub fn transform(&mut self, m: &Mat4f) {
        let normal_m = m.invert().transpose();
        for p in &mut self.positions {
            let v = m.transform_vec4(vec4(p[0], p[1], p[2], 1.0));
            *p = [v.x, v.y, v.z];
        }
        for n in &mut self.normals {
            let v = normal_m.transform_vec4(vec4(n[0], n[1], n[2], 0.0));
            let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1e-12);
            *n = [v.x / l, v.y / l, v.z / l];
        }
        for t in &mut self.tangents {
            let v = m.transform_vec4(vec4(t[0], t[1], t[2], 0.0));
            let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1e-12);
            *t = [v.x / l, v.y / l, v.z / l, t[3]];
        }
    }

    /// Smooth vertex normals from the triangles (area weighted), replacing
    /// whatever was there. Vertices shared by index are smoothed; split
    /// vertices keep their hard edge.
    pub fn compute_normals(&mut self) {
        let mut acc = vec![[0.0f32; 3]; self.positions.len()];
        for t in self.indices.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let n = tri_normal_unnormalized(self.positions[a], self.positions[b], self.positions[c]);
            for &i in &[a, b, c] {
                for k in 0..3 {
                    acc[i][k] += n[k];
                }
            }
        }
        self.normals = acc.into_iter().map(normalize_or_up).collect();
    }

    /// Per-vertex tangents from uvs (Lengyel), for normal maps and
    /// anisotropic looks. Degenerate uvs get an arbitrary perpendicular.
    pub fn compute_tangents(&mut self) {
        let n = self.positions.len();
        let mut tan = vec![[0.0f32; 3]; n];
        let mut bit = vec![[0.0f32; 3]; n];
        if self.uvs.len() == n {
            for t in self.indices.chunks_exact(3) {
                let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                let (p0, p1, p2) = (self.positions[a], self.positions[b], self.positions[c]);
                let (w0, w1, w2) = (self.uvs[a], self.uvs[b], self.uvs[c]);
                let e1 = sub3(p1, p0);
                let e2 = sub3(p2, p0);
                let (s1, t1) = (w1[0] - w0[0], w1[1] - w0[1]);
                let (s2, t2) = (w2[0] - w0[0], w2[1] - w0[1]);
                let det = s1 * t2 - s2 * t1;
                if det.abs() < 1e-12 {
                    continue;
                }
                let r = 1.0 / det;
                let sd = [(t2 * e1[0] - t1 * e2[0]) * r, (t2 * e1[1] - t1 * e2[1]) * r, (t2 * e1[2] - t1 * e2[2]) * r];
                let td = [(s1 * e2[0] - s2 * e1[0]) * r, (s1 * e2[1] - s2 * e1[1]) * r, (s1 * e2[2] - s2 * e1[2]) * r];
                for &i in &[a, b, c] {
                    for k in 0..3 {
                        tan[i][k] += sd[k];
                        bit[i][k] += td[k];
                    }
                }
            }
        }
        self.tangents = (0..n)
            .map(|i| {
                let nn = self.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                let t = tan[i];
                // Gram-Schmidt against the normal.
                let d = dot3(nn, t);
                let mut o = [t[0] - nn[0] * d, t[1] - nn[1] * d, t[2] - nn[2] * d];
                if dot3(o, o) < 1e-16 {
                    o = any_perpendicular(nn);
                }
                let o = normalize_or_up(o);
                let w = if dot3(cross3(nn, o), bit[i]) < 0.0 { -1.0 } else { 1.0 };
                [o[0], o[1], o[2], w]
            })
            .collect();
    }

    /// Split every triangle into its own three vertices and give it the
    /// face normal: the faceted look (`flat_shading: true`).
    pub fn flat_shaded(&self) -> Mesh {
        let mut out = Mesh::new();
        let colors = !self.colors.is_empty();
        for t in self.indices.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let n = normalize_or_up(tri_normal_unnormalized(self.positions[a], self.positions[b], self.positions[c]));
            for &i in &[a, b, c] {
                let uv = self.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                out.vertex(self.positions[i], n, uv);
                if colors {
                    out.colors.push(self.colors[i]);
                }
            }
            let base = out.positions.len() as u32 - 3;
            out.tri(base, base + 1, base + 2);
        }
        out
    }

    /// Barycentric coordinates in the vertex colour's rgb of an unindexed
    /// copy, for the wireframe material (edges where one channel is ~0).
    pub fn with_barycentrics(&self) -> Mesh {
        let mut out = Mesh::new();
        let keep_tangents = self.tangents.len() == self.positions.len();
        let bary = [[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]];
        for t in self.indices.chunks_exact(3) {
            for (k, &i) in t.iter().enumerate() {
                let i = i as usize;
                let n = self.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                let uv = self.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                out.vertex(self.positions[i], n, uv);
                out.colors.push(bary[k]);
                // Keep tangents (an exploding mesh carries its triangle centres there).
                if keep_tangents {
                    out.tangents.push(self.tangents[i]);
                }
            }
            let base = out.positions.len() as u32 - 3;
            out.tri(base, base + 1, base + 2);
        }
        out
    }

    /// Every attribute present for every vertex and every index in range.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.positions.len();
        if self.normals.len() != n {
            return Err(format!("{} normals for {n} vertices", self.normals.len()));
        }
        if self.uvs.len() != n {
            return Err(format!("{} uvs for {n} vertices", self.uvs.len()));
        }
        if !self.colors.is_empty() && self.colors.len() != n {
            return Err(format!("{} colours for {n} vertices", self.colors.len()));
        }
        if !self.tangents.is_empty() && self.tangents.len() != n {
            return Err(format!("{} tangents for {n} vertices", self.tangents.len()));
        }
        if self.indices.len() % 3 != 0 {
            return Err(format!("{} indices is not whole triangles", self.indices.len()));
        }
        if let Some(bad) = self.indices.iter().find(|&&i| i as usize >= n) {
            return Err(format!("index {bad} out of range ({n} vertices)"));
        }
        for p in &self.positions {
            if !p.iter().all(|v| v.is_finite()) {
                return Err(format!("non-finite position {p:?}"));
            }
        }
        for v in &self.normals {
            if !v.iter().all(|v| v.is_finite()) {
                return Err(format!("non-finite normal {v:?}"));
            }
        }
        Ok(())
    }

    /// The GPU vertex stream: `geom.PbrVertex` = (pos.xyz, n.x), (n.y, n.z,
    /// uv), (colour rgba), (tangent xyzw).
    pub fn pack_pbr(&self, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.positions.len() * PBR_FLOATS);
        for i in 0..self.positions.len() {
            let p = self.positions[i];
            let n = self.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let uv = self.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
            let t = self.tangents.get(i).copied().unwrap_or_else(|| {
                let o = any_perpendicular(n);
                [o[0], o[1], o[2], 1.0]
            });
            let c = self.colors.get(i).copied().unwrap_or([1.0; 4]);
            out.extend_from_slice(&[p[0], p[1], p[2], n[0], n[1], n[2], uv[0], uv[1], c[0], c[1], c[2], c[3], t[0], t[1], t[2], t[3]]);
        }
    }

    /// A content hash (positions, indices and attributes), so the renderer
    /// uploads a geometry once however many frames or layers use it.
    pub fn content_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bits: u32| {
            h ^= bits as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        };
        eat(self.positions.len() as u32);
        for p in &self.positions {
            p.iter().for_each(|v| eat(v.to_bits()));
        }
        for p in &self.normals {
            p.iter().for_each(|v| eat(v.to_bits()));
        }
        for p in &self.uvs {
            p.iter().for_each(|v| eat(v.to_bits()));
        }
        for p in &self.colors {
            p.iter().for_each(|v| eat(v.to_bits()));
        }
        for p in &self.tangents {
            p.iter().for_each(|v| eat(v.to_bits()));
        }
        self.indices.iter().for_each(|&i| eat(i));
        h
    }
}

#[inline]
pub fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[inline]
pub fn len3(a: [f32; 3]) -> f32 {
    dot3(a, a).sqrt()
}

/// Unit vector, or +y for a zero vector.
#[inline]
pub fn normalize_or_up(a: [f32; 3]) -> [f32; 3] {
    let l = len3(a);
    if l > 1e-20 && l.is_finite() {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

pub fn tri_normal_unnormalized(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    cross3(sub3(b, a), sub3(c, a))
}

/// Some unit vector perpendicular to `n`.
pub fn any_perpendicular(n: [f32; 3]) -> [f32; 3] {
    let helper = if n[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    normalize_or_up(cross3(helper, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> Mesh {
        let mut m = Mesh::new();
        let a = m.vertex([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0]);
        let b = m.vertex([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0]);
        let c = m.vertex([1.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0]);
        let d = m.vertex([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0]);
        m.quad(a, b, c, d);
        m
    }

    #[test]
    fn normals_face_the_ccw_side() {
        let mut m = quad();
        m.normals.iter_mut().for_each(|n| *n = [0.0, 0.0, 0.0]);
        m.compute_normals();
        assert!(m.normals.iter().all(|n| (n[2] - 1.0).abs() < 1e-6));
        m.validate().unwrap();
    }

    #[test]
    fn tangents_follow_u() {
        let mut m = quad();
        m.compute_tangents();
        for t in &m.tangents {
            assert!((t[0] - 1.0).abs() < 1e-5 && t[3] == 1.0, "{t:?}");
        }
    }

    #[test]
    fn append_offsets_indices_and_fills_colours() {
        let mut a = quad();
        let mut b = quad();
        b.colors = vec![[1.0, 0.0, 0.0, 1.0]; 4];
        a.append(&b);
        assert_eq!(a.vertex_count(), 8);
        assert_eq!(a.indices[6..], [4, 5, 6, 4, 6, 7]);
        assert_eq!(a.colors.len(), 8);
        assert_eq!(a.colors[0], [1.0; 4]);
        a.validate().unwrap();
    }

    #[test]
    fn transform_moves_and_turns() {
        let mut m = quad();
        m.transform(&Mat4f::mul(&crate::math::translation([0.0, 0.0, 5.0]), &Mat4f::rotation(vec3(0.0, std::f32::consts::FRAC_PI_2, 0.0))));
        let (min, _max) = m.bounds().unwrap();
        assert!((min[2] - 4.0).abs() < 1e-4 || (min[2] - 5.0).abs() < 1e-4);
        assert!(m.normals[0][0].abs() > 0.99, "{:?}", m.normals[0]);
    }

    #[test]
    fn packing_is_sixteen_floats() {
        let m = quad();
        let mut out = Vec::new();
        m.pack_pbr(&mut out);
        assert_eq!(out.len(), 4 * PBR_FLOATS);
        assert_eq!(&out[8..12], &[1.0; 4]);
    }

    #[test]
    fn flat_and_bary_split_vertices() {
        let m = quad();
        assert_eq!(m.flat_shaded().vertex_count(), 6);
        let w = m.with_barycentrics();
        assert_eq!(w.colors[1], [0.0, 1.0, 0.0, 1.0]);
    }
}

/// An inverted hull for ink outlines: the same surface with its winding
/// reversed and each vertex's normal averaged over every vertex at the same
/// place, so pushing out along it leaves no cracks at hard edges.
pub fn ink_hull(m: &Mesh) -> Mesh {
    use std::collections::HashMap;
    let key = |p: &[f32; 3]| ((p[0] * 1e4).round() as i64, (p[1] * 1e4).round() as i64, (p[2] * 1e4).round() as i64);
    let mut sum: HashMap<(i64, i64, i64), [f32; 3]> = HashMap::new();
    for (p, n) in m.positions.iter().zip(m.normals.iter()) {
        let e = sum.entry(key(p)).or_insert([0.0; 3]);
        e[0] += n[0];
        e[1] += n[1];
        e[2] += n[2];
    }
    let mut out = m.clone();
    for (i, p) in m.positions.iter().enumerate() {
        if let Some(s) = sum.get(&key(p)) {
            out.normals[i] = normalize_or_up(*s);
        }
    }
    for t in out.indices.chunks_exact_mut(3) {
        t.swap(1, 2);
    }
    out
}
