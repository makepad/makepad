//! Procedural geometry: parametric surfaces, noise displacement,
//! heightfields, subdivision, and deformers (twist, bend, taper). All
//! deterministic: noise comes from explicit seeds.

use crate::math::{fbm3, DEG};
use crate::mesh::*;
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

/// A built-in parametric surface, (u, v) in 0..1 to a point about unit size.
#[derive(Clone, Debug, PartialEq)]
pub enum Parametric {
    /// A unit square in xz facing +y.
    Plane,
    /// A unit sphere.
    Sphere,
    /// The Klein bottle (three.js ParametricGeometries.klein, scaled to ~2 units).
    Klein,
    /// A Möbius strip of radius 1 and width `width`.
    Mobius { width: f32 },
    /// A unit square in xz with y = amp sin(2π freq u) cos(2π freq v).
    Wave { amp: f32, freq: f32 },
    /// A unit sphere pushed out by fbm noise: radius 1 + amount * noise(freq p).
    Blob { amount: f32, freq: f32, seed: u64 },
    /// A helicoid: two turns, radius 0.5, height 1.
    Helicoid,
}

impl Parametric {
    pub fn eval(&self, u: f32, v: f32) -> [f32; 3] {
        match self {
            Parametric::Plane => [u - 0.5, 0.0, 0.5 - v],
            Parametric::Sphere => sphere(u, v),
            Parametric::Klein => {
                let (u, v) = (u * TAU, v * TAU);
                let (x, z);
                if u < PI {
                    x = 3.0 * u.cos() * (1.0 + u.sin()) + 2.0 * (1.0 - u.cos() / 2.0) * u.cos() * v.cos();
                    z = -8.0 * u.sin() - 2.0 * (1.0 - u.cos() / 2.0) * u.sin() * v.cos();
                } else {
                    x = 3.0 * u.cos() * (1.0 + u.sin()) + 2.0 * (1.0 - u.cos() / 2.0) * (v + PI).cos();
                    z = -8.0 * u.sin();
                }
                let y = -2.0 * (1.0 - u.cos() / 2.0) * v.sin();
                [x * 0.125, y * 0.125, z * 0.125]
            }
            Parametric::Mobius { width } => {
                let s = (u - 0.5) * width;
                let a = v * TAU;
                [a.cos() * (1.0 + s * (a / 2.0).cos()), s * (a / 2.0).sin(), a.sin() * (1.0 + s * (a / 2.0).cos())]
            }
            Parametric::Wave { amp, freq } => [u - 0.5, amp * (TAU * freq * u).sin() * (TAU * freq * v).cos(), 0.5 - v],
            Parametric::Blob { amount, freq, seed } => {
                let p = sphere(u, v);
                let r = 1.0 + amount * fbm3(*seed, scale3(p, *freq), 4, 0.5);
                scale3(p, r)
            }
            Parametric::Helicoid => {
                let a = u * 2.0 * TAU;
                let r = (v - 0.5) * 1.0;
                [r * a.cos(), u - 0.5, -r * a.sin()]
            }
        }
    }
}

fn sphere(u: f32, v: f32) -> [f32; 3] {
    let (t, p) = (u * TAU, (1.0 - v) * PI);
    [-t.cos() * p.sin(), p.cos(), t.sin() * p.sin()]
}

/// A parametric surface tessellated into a (slices x stacks) grid.
#[derive(Clone, Debug, PartialEq)]
pub struct ParametricGeo {
    /// The surface. Default Klein.
    pub func: Parametric,
    /// Cells along u. Default 32.
    pub slices: u32,
    /// Cells along v. Default 32.
    pub stacks: u32,
}

impl Default for ParametricGeo {
    fn default() -> Self {
        Self { func: Parametric::Klein, slices: 32, stacks: 32 }
    }
}

impl ParametricGeo {
    pub fn build(&self) -> Mesh {
        let f = self.func.clone();
        parametric(&move |u, v| f.eval(u, v), self.slices, self.stacks)
    }
}

/// Tessellate any `f(u, v)` over 0..1 x 0..1 (three.js ParametricGeometry):
/// normals are cross(∂f/∂u, ∂f/∂v), so the front is where u x v points.
pub fn parametric(f: &dyn Fn(f32, f32) -> [f32; 3], slices: u32, stacks: u32) -> Mesh {
    let (sl, st) = (slices.clamp(1, 4096), stacks.clamp(1, 4096));
    let eps = 1e-4;
    let mut m = Mesh::new();
    for i in 0..=st {
        let v = i as f32 / st as f32;
        for j in 0..=sl {
            let u = j as f32 / sl as f32;
            let p = f(u, v);
            let normal_at = |u: f32, v: f32| {
                let p = f(u, v);
                let pu = if u - eps >= 0.0 { sub3(p, f(u - eps, v)) } else { sub3(f(u + eps, v), p) };
                let pv = if v - eps >= 0.0 { sub3(p, f(u, v - eps)) } else { sub3(f(u, v + eps), p) };
                cross3(pu, pv)
            };
            let mut n = normal_at(u, v);
            if len3(n) < 1e-12 {
                // A pole: take the normal just inside the grid.
                n = normal_at(u, if v < 0.5 { v + 1e-3 } else { v - 1e-3 });
            }
            let p = p.map(|c| if c.is_finite() { c } else { 0.0 });
            m.vertex(p, normalize_or_up(n), [u, v]);
        }
    }
    let w = sl + 1;
    for i in 0..st {
        for j in 0..sl {
            let (a, b, c, d) = (i * w + j, i * w + j + 1, (i + 1) * w + j + 1, (i + 1) * w + j);
            m.tri(a, b, d);
            m.tri(b, c, d);
        }
    }
    m
}

/// Quantised position key for welding seams.
fn key(p: [f32; 3], q: f32) -> (i64, i64, i64) {
    ((p[0] * q).round() as i64, (p[1] * q).round() as i64, (p[2] * q).round() as i64)
}

fn weld_scale(m: &Mesh) -> f32 {
    let (min, max) = m.bounds().unwrap_or(([0.0; 3], [1.0; 3]));
    1e5 / len3(sub3(max, min)).max(1e-6)
}

/// Smooth normals over vertices that share a position (seams and split
/// edges welded), from area-weighted face normals.
pub fn welded_normals(m: &mut Mesh) {
    let q = weld_scale(m);
    let mut acc: HashMap<(i64, i64, i64), [f32; 3]> = HashMap::new();
    for t in m.indices.chunks_exact(3) {
        let n = tri_normal_unnormalized(m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
        for &i in t {
            let e = acc.entry(key(m.positions[i as usize], q)).or_insert([0.0; 3]);
            *e = add3(*e, n);
        }
    }
    m.normals = m.positions.iter().map(|p| normalize_or_up(acc.get(&key(*p, q)).copied().unwrap_or([0.0, 1.0, 0.0]))).collect();
    m.tangents.clear();
}

/// Push every vertex along its (position-welded) normal by `amount` times
/// fbm noise of `position * frequency + offset`, then recompute welded
/// normals. Animate `offset` (or the seed) for boiling surfaces.
pub fn displace_noise(m: &mut Mesh, amount: f32, frequency: f32, octaves: u32, seed: u64, offset: [f32; 3]) {
    let q = weld_scale(m);
    let mut dir: HashMap<(i64, i64, i64), [f32; 3]> = HashMap::new();
    for (p, n) in m.positions.iter().zip(&m.normals) {
        let e = dir.entry(key(*p, q)).or_insert([0.0; 3]);
        *e = add3(*e, *n);
    }
    for p in m.positions.iter_mut() {
        let n = normalize_or_up(dir[&key(*p, q)]);
        let s = fbm3(seed, add3(scale3(*p, frequency), offset), octaves, 0.5) * amount;
        *p = add3(*p, scale3(n, s));
    }
    welded_normals(m);
}

/// Apply a smooth deformation `f` to positions, carrying normals by the
/// inverse transpose of its Jacobian (finite differences) and tangents by
/// the Jacobian, so hard edges stay hard.
pub fn deform(m: &mut Mesh, f: &dyn Fn([f32; 3]) -> [f32; 3]) {
    let h = m.bounds().map(|(a, b)| len3(sub3(b, a))).unwrap_or(1.0).max(1e-6) * 1e-4;
    let mut jac = Vec::with_capacity(m.positions.len());
    for p in &m.positions {
        let col = |k: usize| {
            let (mut a, mut b) = (*p, *p);
            a[k] -= h;
            b[k] += h;
            scale3(sub3(f(b), f(a)), 0.5 / h)
        };
        jac.push([col(0), col(1), col(2)]);
    }
    for (i, p) in m.positions.iter_mut().enumerate() {
        *p = f(*p);
        let [c0, c1, c2] = jac[i];
        if let Some(n) = m.normals.get_mut(i) {
            let det = dot3(c0, cross3(c1, c2));
            let r = add3(add3(scale3(cross3(c1, c2), n[0]), scale3(cross3(c2, c0), n[1])), scale3(cross3(c0, c1), n[2]));
            *n = normalize_or_up(if det < 0.0 { scale3(r, -1.0) } else { r });
        }
        if let Some(t) = m.tangents.get_mut(i) {
            let d = add3(add3(scale3(c0, t[0]), scale3(c1, t[1])), scale3(c2, t[2]));
            let d = normalize_or_up(d);
            *t = [d[0], d[1], d[2], t[3]];
        }
    }
}

/// Twist around `axis` (0 x, 1 y, 2 z): each point turns by its coordinate
/// along the axis times `degrees_per_unit`.
pub fn twist(m: &mut Mesh, axis: usize, degrees_per_unit: f32) {
    let axis = axis.min(2);
    let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
    let k = degrees_per_unit * DEG;
    deform(m, &|p| {
        let (s, c) = (p[axis] * k).sin_cos();
        let mut o = p;
        o[a] = p[a] * c - p[b] * s;
        o[b] = p[a] * s + p[b] * c;
        o
    });
}

/// Bend the mesh's extent along `axis` into an arc of `degrees`, curling
/// toward the next axis (x bends toward +y, y toward +z, z toward +x). The
/// middle of the extent stays put.
pub fn bend(m: &mut Mesh, axis: usize, degrees: f32) {
    let axis = axis.min(2);
    let up = (axis + 1) % 3;
    let Some((min, max)) = m.bounds() else { return };
    let len = (max[axis] - min[axis]).max(1e-6);
    let theta = degrees * DEG;
    if theta.abs() < 1e-6 {
        return;
    }
    let mid = (min[axis] + max[axis]) * 0.5;
    let r = len / theta;
    deform(m, &|p| {
        let a = (p[axis] - mid) / r;
        let dist = r - p[up];
        let mut o = p;
        o[axis] = mid + dist * a.sin();
        o[up] = r - dist * a.cos();
        o
    });
}

/// Scale the two other axes along `axis`: 1 at the low end of the bounds,
/// 1 + `amount` at the high end (-1 tapers to a point).
pub fn taper(m: &mut Mesh, axis: usize, amount: f32) {
    let axis = axis.min(2);
    let Some((min, max)) = m.bounds() else { return };
    let len = (max[axis] - min[axis]).max(1e-6);
    let lo = min[axis];
    // Keep the far end a hair above zero so normals stay defined.
    deform(m, &|p| {
        let s = (1.0 + amount * (p[axis] - lo) / len).max(1e-4);
        let mut o = scale3(p, s);
        o[axis] = p[axis];
        o
    });
}

/// Split every triangle into four at its edge midpoints (shared midpoints,
/// attributes interpolated). Flat: combine with displacement for detail.
pub fn subdivide(m: &Mesh) -> Mesh {
    let mut out = m.clone();
    out.indices.clear();
    out.tangents.clear();
    let colors = !m.colors.is_empty();
    let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
    let mut mid = |out: &mut Mesh, a: u32, b: u32| -> u32 {
        *mids.entry((a.min(b), a.max(b))).or_insert_with(|| {
            let (a, b) = (a as usize, b as usize);
            let p = scale3(add3(out.positions[a], out.positions[b]), 0.5);
            let n = normalize_or_up(add3(out.normals[a], out.normals[b]));
            let uv = [(out.uvs[a][0] + out.uvs[b][0]) * 0.5, (out.uvs[a][1] + out.uvs[b][1]) * 0.5];
            let c = colors.then(|| [0, 1, 2, 3].map(|k| (out.colors[a][k] + out.colors[b][k]) * 0.5));
            let i = out.vertex(p, n, uv);
            if let Some(c) = c {
                out.colors.push(c);
            }
            i
        })
    };
    for t in m.indices.chunks_exact(3) {
        let (a, b, c) = (t[0], t[1], t[2]);
        let (ab, bc, ca) = (mid(&mut out, a, b), mid(&mut out, b, c), mid(&mut out, c, a));
        out.tri(a, ab, ca);
        out.tri(ab, b, bc);
        out.tri(ca, bc, c);
        out.tri(ab, bc, ca);
    }
    out
}

/// A noise terrain in xz facing +y, centred on the origin.
#[derive(Clone, Debug, PartialEq)]
pub struct HeightfieldGeo {
    /// Size along x. Default 10.
    pub width: f32,
    /// Size along z. Default 10.
    pub depth: f32,
    /// Cells per side. Default 64.
    pub segments: u32,
    /// Peak height (noise amplitude). Default 1.
    pub height: f32,
    /// Noise frequency per world unit. Default 0.3.
    pub frequency: f32,
    /// fbm octaves. Default 4.
    pub octaves: u32,
    /// Noise seed. Default 1.
    pub seed: u64,
}

impl Default for HeightfieldGeo {
    fn default() -> Self {
        Self { width: 10.0, depth: 10.0, segments: 64, height: 1.0, frequency: 0.3, octaves: 4, seed: 1 }
    }
}

impl HeightfieldGeo {
    pub fn build(&self) -> Mesh {
        let n = self.segments.clamp(1, 2048);
        let mut m = Mesh::new();
        for iz in 0..=n {
            let v = iz as f32 / n as f32;
            let z = (v - 0.5) * self.depth;
            for ix in 0..=n {
                let u = ix as f32 / n as f32;
                let x = (u - 0.5) * self.width;
                let y = self.height * fbm3(self.seed, [x * self.frequency, 0.0, z * self.frequency], self.octaves, 0.5);
                m.vertex([x, y, z], [0.0, 1.0, 0.0], [u, 1.0 - v]);
            }
        }
        let w = n + 1;
        for iz in 0..n {
            for ix in 0..n {
                let a = iz * w + ix;
                let (b, c, d) = (a + 1, a + w + 1, a + w);
                m.tri(a, d, b);
                m.tri(b, d, c);
            }
        }
        m.compute_normals();
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{BoxGeo, CylinderGeo, SphereGeo};

    fn winding_agrees(m: &Mesh) -> f32 {
        let mut agree = 0;
        let mut total = 0;
        for t in m.indices.chunks_exact(3) {
            let n = tri_normal_unnormalized(m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
            if len3(n) < 1e-10 {
                continue;
            }
            total += 1;
            if dot3(n, m.normals[t[0] as usize]) > 0.0 {
                agree += 1;
            }
        }
        agree as f32 / total.max(1) as f32
    }

    #[test]
    fn parametric_surfaces() {
        for func in [
            Parametric::Plane,
            Parametric::Sphere,
            Parametric::Klein,
            Parametric::Mobius { width: 0.5 },
            Parametric::Wave { amp: 0.1, freq: 2.0 },
            Parametric::Blob { amount: 0.3, freq: 2.0, seed: 3 },
            Parametric::Helicoid,
        ] {
            let m = ParametricGeo { func: func.clone(), slices: 16, stacks: 12 }.build();
            m.validate().unwrap();
            assert_eq!((m.vertex_count(), m.triangle_count()), (17 * 13, 16 * 12 * 2), "{func:?}");
            assert!(winding_agrees(&m) > 0.95, "{func:?}");
        }
        // The sphere faces out; the plane faces up.
        let s = ParametricGeo { func: Parametric::Sphere, slices: 16, stacks: 12 }.build();
        assert!(s.positions.iter().zip(&s.normals).all(|(p, n)| len3(*p) < 0.5 || dot3(*p, *n) > 0.0));
        let p = ParametricGeo { func: Parametric::Plane, slices: 2, stacks: 2 }.build();
        assert!(p.normals.iter().all(|n| n[1] > 0.99));
    }

    #[test]
    fn displacement_keeps_seams_closed() {
        let mut m = BoxGeo { segments: [8, 8, 8], ..Default::default() }.build();
        displace_noise(&mut m, 0.1, 3.0, 3, 7, [0.0; 3]);
        m.validate().unwrap();
        // Vertices that shared a position still do.
        let orig = BoxGeo { segments: [8, 8, 8], ..Default::default() }.build();
        let q = 1e4;
        let mut seen: HashMap<(i64, i64, i64), [f32; 3]> = HashMap::new();
        for (o, p) in orig.positions.iter().zip(&m.positions) {
            let e = seen.entry(key(*o, q)).or_insert(*p);
            assert!(len3(sub3(*e, *p)) < 1e-6);
        }
        let mut a = BoxGeo::default().build();
        let mut b = a.clone();
        displace_noise(&mut a, 0.2, 2.0, 2, 1, [0.5, 0.0, 0.0]);
        displace_noise(&mut b, 0.2, 2.0, 2, 1, [0.5, 0.0, 0.0]);
        assert_eq!(a, b);
    }

    #[test]
    fn twist_bend_taper() {
        let mut m = BoxGeo { size: [0.5, 4.0, 0.5], segments: [1, 16, 1], ..Default::default() }.build();
        twist(&mut m, 1, 45.0);
        m.validate().unwrap();
        assert!(winding_agrees(&m) > 0.99);
        // Top face turned by 90 degrees: its normal is unchanged (+y), a side's turned.
        let top = m.positions.iter().zip(&m.normals).find(|(p, n)| p[1] > 1.99 && n[1] > 0.9);
        assert!(top.is_some());

        let mut c = CylinderGeo { radius_top: 0.2, radius_bottom: 0.2, height: 4.0, height_segments: 32, ..Default::default() }.build();
        bend(&mut c, 1, 90.0);
        c.validate().unwrap();
        assert!(winding_agrees(&c) > 0.99);
        let (min, max) = c.bounds().unwrap();
        // A quarter circle of length 4: radius 8/π, the ends swing toward +z.
        assert!(max[2] > 0.5 && min[1] > -2.0 && max[1] < 2.0, "{min:?} {max:?}");

        let mut s = SphereGeo::default().build();
        taper(&mut s, 1, -0.5);
        s.validate().unwrap();
        let (_, max) = s.bounds().unwrap();
        assert!(max[0] < 1.0 && (max[1] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn heightfield_and_subdivide() {
        let h = HeightfieldGeo { segments: 16, ..Default::default() }.build();
        h.validate().unwrap();
        assert_eq!(h.vertex_count(), 17 * 17);
        assert!(h.normals.iter().all(|n| n[1] > 0.0));
        assert!(winding_agrees(&h) > 0.99);
        let b = BoxGeo::default().build();
        let s = subdivide(&b);
        s.validate().unwrap();
        assert_eq!(s.triangle_count(), b.triangle_count() * 4);
        // Shared midpoints: a unit quad face of 4 verts gains 5 (4 edges + diagonal).
        assert_eq!(s.vertex_count(), b.vertex_count() + 6 * 5);
    }
}

/// A grid mesh over `size` (x by z, centred) from `nx × nz` heights (row
/// major, x fastest), with normals from central differences and uv 0..1.
pub fn grid_mesh(h: &[f32], nx: usize, nz: usize, size: [f32; 2]) -> crate::mesh::Mesh {
    let mut m = crate::mesh::Mesh::new();
    let (dx, dz) = (size[0] / (nx - 1).max(1) as f32, size[1] / (nz - 1).max(1) as f32);
    let at = |i: usize, j: usize| h[j.min(nz - 1) * nx + i.min(nx - 1)];
    for j in 0..nz {
        for i in 0..nx {
            let x = (i as f32 / (nx - 1) as f32 - 0.5) * size[0];
            let z = (j as f32 / (nz - 1) as f32 - 0.5) * size[1];
            let (il, ir) = (i.saturating_sub(1), (i + 1).min(nx - 1));
            let (jd, ju) = (j.saturating_sub(1), (j + 1).min(nz - 1));
            let sx = (at(ir, j) - at(il, j)) / (dx * (ir - il).max(1) as f32);
            let sz = (at(i, ju) - at(i, jd)) / (dz * (ju - jd).max(1) as f32);
            let n = crate::mesh::normalize_or_up([-sx, 1.0, -sz]);
            m.vertex([x, at(i, j), z], n, [i as f32 / (nx - 1) as f32, j as f32 / (nz - 1) as f32]);
        }
    }
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let a = (j * nx + i) as u32;
            let b = a + 1;
            let c = a + nx as u32;
            let d = c + 1;
            m.tri(a, c, b);
            m.tri(b, c, d);
        }
    }
    m
}
