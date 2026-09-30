//! Primitive solids with three.js parameterisation and uv mapping. Angles
//! are in degrees; sizes in world units. Every builder returns a complete
//! [`Mesh`] (normals and uvs filled, counter-clockwise front faces).

use crate::math::DEG;
use crate::mesh::*;

/// A box centred on the origin, optionally with rounded edges.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxGeo {
    /// Edge lengths along x, y, z. Default 1, 1, 1.
    pub size: [f32; 3],
    /// Grid cells per face along x, y, z. Default 1, 1, 1.
    pub segments: [u32; 3],
    /// Edge rounding radius; 0 = sharp (default). Clamped to half the smallest edge.
    pub radius: f32,
    /// Cells across each rounded edge. Default 4.
    pub radius_segments: u32,
}

impl Default for BoxGeo {
    fn default() -> Self {
        Self { size: [1.0; 3], segments: [1; 3], radius: 0.0, radius_segments: 4 }
    }
}

impl BoxGeo {
    pub fn build(&self) -> Mesh {
        let size = self.size.map(|s| s.abs().max(1e-6));
        let seg = self.segments.map(|s| s.clamp(1, 512));
        let half = size.map(|s| s * 0.5);
        let radius = self.radius.max(0.0).min(half[0].min(half[1]).min(half[2]));
        let mut m = Mesh::new();
        // (u axis, v axis, w axis, u dir, v dir, w sign) per face, as three.js
        // BoxGeometry: px, nx, py, ny, pz, nz.
        let faces: [(usize, usize, usize, f32, f32, f32); 6] = [
            (2, 1, 0, -1.0, -1.0, 1.0),
            (2, 1, 0, 1.0, -1.0, -1.0),
            (0, 2, 1, 1.0, 1.0, 1.0),
            (0, 2, 1, 1.0, -1.0, -1.0),
            (0, 1, 2, 1.0, -1.0, 1.0),
            (0, 1, 2, -1.0, -1.0, -1.0),
        ];
        let rs = if radius > 0.0 { self.radius_segments.clamp(1, 64) } else { 0 };
        for (u, v, w, udir, vdir, wsign) in faces {
            // Coordinates along an axis: the rounded band, the flat middle, the band.
            let axis = |a: usize| -> Vec<f32> {
                let h = half[a];
                let flat = h - radius;
                let mut c = Vec::new();
                for i in 0..rs {
                    c.push(-h + radius * i as f32 / rs as f32);
                }
                for i in 0..=seg[a] {
                    c.push(-flat + 2.0 * flat * i as f32 / seg[a] as f32);
                }
                for i in 1..=rs {
                    c.push(flat + radius * i as f32 / rs as f32);
                }
                c
            };
            let (cu, cv) = (axis(u), axis(v));
            let base = m.positions.len() as u32;
            let nu = cu.len() as u32;
            for (iy, &y) in cv.iter().enumerate() {
                for (ix, &x) in cu.iter().enumerate() {
                    let mut p = [0.0f32; 3];
                    p[u] = x * udir;
                    p[v] = y * vdir;
                    p[w] = half[w] * wsign;
                    let mut n = [0.0f32; 3];
                    n[w] = wsign;
                    if radius > 0.0 {
                        let inner = [0, 1, 2].map(|k| p[k].clamp(-(half[k] - radius), half[k] - radius));
                        let d = normalize_or_up(sub3(p, inner));
                        n = d;
                        p = add3(inner, scale3(d, radius));
                    }
                    let uv = [ix as f32 / (nu - 1) as f32, 1.0 - iy as f32 / (cv.len() - 1) as f32];
                    m.vertex(p, n, uv);
                }
            }
            for iy in 0..cv.len() as u32 - 1 {
                for ix in 0..nu - 1 {
                    let a = base + ix + nu * iy;
                    let b = base + ix + nu * (iy + 1);
                    let c = base + ix + 1 + nu * (iy + 1);
                    let d = base + ix + 1 + nu * iy;
                    m.tri(a, b, d);
                    m.tri(b, c, d);
                }
            }
        }
        m
    }
}

/// A UV sphere (three.js SphereGeometry).
#[derive(Clone, Debug, PartialEq)]
pub struct SphereGeo {
    /// Radius. Default 1.
    pub radius: f32,
    /// Segments around (longitude). Default 32.
    pub width_segments: u32,
    /// Segments top to bottom (latitude). Default 16.
    pub height_segments: u32,
    /// Start of the longitude sweep, degrees. Default 0.
    pub phi_start: f32,
    /// Longitude sweep, degrees. Default 360.
    pub phi_length: f32,
    /// Start of the latitude sweep from the top pole, degrees. Default 0.
    pub theta_start: f32,
    /// Latitude sweep, degrees. Default 180.
    pub theta_length: f32,
}

impl Default for SphereGeo {
    fn default() -> Self {
        Self { radius: 1.0, width_segments: 32, height_segments: 16, phi_start: 0.0, phi_length: 360.0, theta_start: 0.0, theta_length: 180.0 }
    }
}

impl SphereGeo {
    pub fn build(&self) -> Mesh {
        let ws = self.width_segments.clamp(3, 1024);
        let hs = self.height_segments.clamp(2, 1024);
        let (ps, pl) = (self.phi_start * DEG, self.phi_length * DEG);
        let (ts, tl) = (self.theta_start * DEG, self.theta_length * DEG);
        let theta_end = (ts + tl).min(std::f32::consts::PI);
        let r = self.radius;
        let mut m = Mesh::new();
        for iy in 0..=hs {
            let v = iy as f32 / hs as f32;
            let u_offset = if iy == 0 && ts == 0.0 {
                0.5 / ws as f32
            } else if iy == hs && theta_end >= std::f32::consts::PI - 1e-6 {
                -0.5 / ws as f32
            } else {
                0.0
            };
            for ix in 0..=ws {
                let u = ix as f32 / ws as f32;
                let (sp, cp) = (ps + u * pl).sin_cos();
                let (st, ct) = (ts + v * tl).sin_cos();
                let p = [-r * cp * st, r * ct, r * sp * st];
                let n = normalize_or_up([-cp * st, ct, sp * st]);
                m.vertex(p, n, [u + u_offset, 1.0 - v]);
            }
        }
        let row = ws + 1;
        for iy in 0..hs {
            for ix in 0..ws {
                let a = iy * row + ix + 1;
                let b = iy * row + ix;
                let c = (iy + 1) * row + ix;
                let d = (iy + 1) * row + ix + 1;
                if iy != 0 || ts > 0.0 {
                    m.tri(a, b, d);
                }
                if iy != hs - 1 || theta_end < std::f32::consts::PI - 1e-6 {
                    m.tri(b, c, d);
                }
            }
        }
        m
    }
}

/// A torus in the xy plane around z (three.js TorusGeometry).
#[derive(Clone, Debug, PartialEq)]
pub struct TorusGeo {
    /// Ring radius, centre to tube centre. Default 1.
    pub radius: f32,
    /// Tube radius. Default 0.4.
    pub tube: f32,
    /// Segments around the tube. Default 16.
    pub radial_segments: u32,
    /// Segments along the ring. Default 48.
    pub tubular_segments: u32,
    /// Ring sweep, degrees. Default 360.
    pub arc: f32,
}

impl Default for TorusGeo {
    fn default() -> Self {
        Self { radius: 1.0, tube: 0.4, radial_segments: 16, tubular_segments: 48, arc: 360.0 }
    }
}

impl TorusGeo {
    pub fn build(&self) -> Mesh {
        let rs = self.radial_segments.clamp(3, 1024);
        let ts = self.tubular_segments.clamp(3, 4096);
        let arc = self.arc * DEG;
        let mut m = Mesh::new();
        for j in 0..=rs {
            for i in 0..=ts {
                let u = i as f32 / ts as f32 * arc;
                let v = j as f32 / rs as f32 * std::f32::consts::TAU;
                let p = [
                    (self.radius + self.tube * v.cos()) * u.cos(),
                    (self.radius + self.tube * v.cos()) * u.sin(),
                    self.tube * v.sin(),
                ];
                let c = [self.radius * u.cos(), self.radius * u.sin(), 0.0];
                m.vertex(p, normalize_or_up(sub3(p, c)), [i as f32 / ts as f32, j as f32 / rs as f32]);
            }
        }
        grid_indices(&mut m, rs, ts, false);
        m
    }
}

/// A (p, q) torus knot tube (three.js TorusKnotGeometry).
#[derive(Clone, Debug, PartialEq)]
pub struct TorusKnotGeo {
    /// Knot radius. Default 1.
    pub radius: f32,
    /// Tube radius. Default 0.4.
    pub tube: f32,
    /// Segments along the knot. Default 128.
    pub tubular_segments: u32,
    /// Segments around the tube. Default 16.
    pub radial_segments: u32,
    /// Winds around the axis of symmetry. Default 2.
    pub p: u32,
    /// Winds around the interior circle. Default 3.
    pub q: u32,
}

impl Default for TorusKnotGeo {
    fn default() -> Self {
        Self { radius: 1.0, tube: 0.4, tubular_segments: 128, radial_segments: 16, p: 2, q: 3 }
    }
}

/// A point on the three.js torus-knot curve.
pub fn torus_knot_point(u: f32, p: f32, q: f32, radius: f32) -> [f32; 3] {
    let qu = q / p * u;
    let cs = qu.cos();
    [radius * (2.0 + cs) * 0.5 * u.cos(), radius * (2.0 + cs) * 0.5 * u.sin(), radius * qu.sin() * 0.5]
}

impl TorusKnotGeo {
    pub fn build(&self) -> Mesh {
        let ts = self.tubular_segments.clamp(3, 8192);
        let rs = self.radial_segments.clamp(3, 1024);
        let (p, q) = (self.p.max(1) as f32, self.q.max(1) as f32);
        let mut m = Mesh::new();
        for i in 0..=ts {
            let u = i as f32 / ts as f32 * p * std::f32::consts::TAU;
            let p1 = torus_knot_point(u, p, q, self.radius);
            let p2 = torus_knot_point(u + 0.01, p, q, self.radius);
            let t = sub3(p2, p1);
            let n = add3(p2, p1);
            let b = normalize_or_up(cross3(t, n));
            let n = normalize_or_up(cross3(b, t));
            for j in 0..=rs {
                let v = j as f32 / rs as f32 * std::f32::consts::TAU;
                let cx = -self.tube * v.cos();
                let cy = self.tube * v.sin();
                let pos = add3(p1, add3(scale3(n, cx), scale3(b, cy)));
                m.vertex(pos, normalize_or_up(sub3(pos, p1)), [i as f32 / ts as f32, j as f32 / rs as f32]);
            }
        }
        grid_indices(&mut m, ts, rs, true);
        m
    }
}

/// Indices for a (rows + 1) x (cols + 1) vertex grid laid out row-major,
/// three.js torus winding (`flip` reverses it).
fn grid_indices(m: &mut Mesh, rows: u32, cols: u32, flip: bool) {
    let w = cols + 1;
    for j in 1..=rows {
        for i in 1..=cols {
            let a = w * j + i - 1;
            let b = w * (j - 1) + i - 1;
            let c = w * (j - 1) + i;
            let d = w * j + i;
            if flip {
                m.tri(a, d, b);
                m.tri(b, d, c);
            } else {
                m.tri(a, b, d);
                m.tri(b, c, d);
            }
        }
    }
}

/// A cylinder or cone along y, centred on the origin (three.js
/// CylinderGeometry; a cone is `radius_top: 0`).
#[derive(Clone, Debug, PartialEq)]
pub struct CylinderGeo {
    /// Radius at +y. Default 1.
    pub radius_top: f32,
    /// Radius at -y. Default 1.
    pub radius_bottom: f32,
    /// Height along y. Default 1.
    pub height: f32,
    /// Segments around. Default 32.
    pub radial_segments: u32,
    /// Rows along the height. Default 1.
    pub height_segments: u32,
    /// No caps. Default false.
    pub open_ended: bool,
    /// Start of the sweep, degrees. Default 0.
    pub theta_start: f32,
    /// Sweep, degrees. Default 360.
    pub theta_length: f32,
}

impl Default for CylinderGeo {
    fn default() -> Self {
        Self { radius_top: 1.0, radius_bottom: 1.0, height: 1.0, radial_segments: 32, height_segments: 1, open_ended: false, theta_start: 0.0, theta_length: 360.0 }
    }
}

impl CylinderGeo {
    pub fn build(&self) -> Mesh {
        let rs = self.radial_segments.clamp(3, 4096);
        let hs = self.height_segments.clamp(1, 4096);
        let (ts, tl) = (self.theta_start * DEG, self.theta_length * DEG);
        let half = self.height * 0.5;
        let (rt, rb) = (self.radius_top.max(0.0), self.radius_bottom.max(0.0));
        let slope = (rb - rt) / self.height.max(1e-6);
        let mut m = Mesh::new();
        for y in 0..=hs {
            let v = y as f32 / hs as f32;
            let radius = v * (rb - rt) + rt;
            for x in 0..=rs {
                let u = x as f32 / rs as f32;
                let (s, c) = (u * tl + ts).sin_cos();
                m.vertex([radius * s, -v * self.height + half, radius * c], normalize_or_up([s, slope, c]), [u, 1.0 - v]);
            }
        }
        let w = rs + 1;
        for x in 0..rs {
            for y in 0..hs {
                let a = y * w + x;
                let b = (y + 1) * w + x;
                let c = (y + 1) * w + x + 1;
                let d = y * w + x + 1;
                if !(y == 0 && rt == 0.0) {
                    m.tri(a, b, d);
                }
                if !(y == hs - 1 && rb == 0.0) {
                    m.tri(b, c, d);
                }
            }
        }
        if !self.open_ended {
            for (top, radius) in [(true, rt), (false, rb)] {
                if radius <= 0.0 {
                    continue;
                }
                let sign = if top { 1.0 } else { -1.0 };
                let centre = m.positions.len() as u32;
                for _ in 0..rs {
                    m.vertex([0.0, half * sign, 0.0], [0.0, sign, 0.0], [0.5, 0.5]);
                }
                let rim = m.positions.len() as u32;
                for x in 0..=rs {
                    let u = x as f32 / rs as f32;
                    let (s, c) = (u * tl + ts).sin_cos();
                    m.vertex([radius * s, half * sign, radius * c], [0.0, sign, 0.0], [c * 0.5 + 0.5, s * 0.5 * sign + 0.5]);
                }
                for x in 0..rs {
                    let (c, i) = (centre + x, rim + x);
                    if top {
                        m.tri(i, i + 1, c);
                    } else {
                        m.tri(i + 1, i, c);
                    }
                }
            }
        }
        m
    }
}

/// A plane in xy facing +z, centred on the origin (three.js PlaneGeometry).
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneGeo {
    /// Size along x. Default 1.
    pub width: f32,
    /// Size along y. Default 1.
    pub height: f32,
    /// Cells along x. Default 1.
    pub width_segments: u32,
    /// Cells along y. Default 1.
    pub height_segments: u32,
}

impl Default for PlaneGeo {
    fn default() -> Self {
        Self { width: 1.0, height: 1.0, width_segments: 1, height_segments: 1 }
    }
}

impl PlaneGeo {
    pub fn build(&self) -> Mesh {
        let gx = self.width_segments.clamp(1, 4096);
        let gy = self.height_segments.clamp(1, 4096);
        let mut m = Mesh::new();
        for iy in 0..=gy {
            let y = iy as f32 * self.height / gy as f32 - self.height * 0.5;
            for ix in 0..=gx {
                let x = ix as f32 * self.width / gx as f32 - self.width * 0.5;
                m.vertex([x, -y, 0.0], [0.0, 0.0, 1.0], [ix as f32 / gx as f32, 1.0 - iy as f32 / gy as f32]);
            }
        }
        let w = gx + 1;
        for iy in 0..gy {
            for ix in 0..gx {
                let a = ix + w * iy;
                let b = ix + w * (iy + 1);
                let c = ix + 1 + w * (iy + 1);
                let d = ix + 1 + w * iy;
                m.tri(a, b, d);
                m.tri(b, c, d);
            }
        }
        m
    }
}

/// A flat disc (or sector) in xy facing +z (three.js CircleGeometry).
#[derive(Clone, Debug, PartialEq)]
pub struct CircleGeo {
    /// Radius. Default 1.
    pub radius: f32,
    /// Segments around. Default 32.
    pub segments: u32,
    /// Start of the sweep, degrees. Default 0.
    pub theta_start: f32,
    /// Sweep, degrees. Default 360.
    pub theta_length: f32,
}

impl Default for CircleGeo {
    fn default() -> Self {
        Self { radius: 1.0, segments: 32, theta_start: 0.0, theta_length: 360.0 }
    }
}

impl CircleGeo {
    pub fn build(&self) -> Mesh {
        let seg = self.segments.clamp(3, 4096);
        let r = self.radius.max(1e-6);
        let mut m = Mesh::new();
        m.vertex([0.0; 3], [0.0, 0.0, 1.0], [0.5, 0.5]);
        for s in 0..=seg {
            let a = (self.theta_start + s as f32 / seg as f32 * self.theta_length) * DEG;
            let (x, y) = (r * a.cos(), r * a.sin());
            m.vertex([x, y, 0.0], [0.0, 0.0, 1.0], [(x / r + 1.0) * 0.5, (y / r + 1.0) * 0.5]);
        }
        for i in 1..=seg {
            m.tri(i, i + 1, 0);
        }
        m
    }
}

/// A flat annulus in xy facing +z (three.js RingGeometry).
#[derive(Clone, Debug, PartialEq)]
pub struct RingGeo {
    /// Inner radius. Default 0.5.
    pub inner: f32,
    /// Outer radius. Default 1.
    pub outer: f32,
    /// Segments around. Default 32.
    pub theta_segments: u32,
    /// Rings from inner to outer. Default 1.
    pub phi_segments: u32,
    /// Start of the sweep, degrees. Default 0.
    pub theta_start: f32,
    /// Sweep, degrees. Default 360.
    pub theta_length: f32,
}

impl Default for RingGeo {
    fn default() -> Self {
        Self { inner: 0.5, outer: 1.0, theta_segments: 32, phi_segments: 1, theta_start: 0.0, theta_length: 360.0 }
    }
}

impl RingGeo {
    pub fn build(&self) -> Mesh {
        let ts = self.theta_segments.clamp(3, 4096);
        let ps = self.phi_segments.clamp(1, 1024);
        let outer = self.outer.max(1e-6);
        let step = (self.outer - self.inner) / ps as f32;
        let mut m = Mesh::new();
        for j in 0..=ps {
            let r = self.inner + j as f32 * step;
            for i in 0..=ts {
                let a = (self.theta_start + i as f32 / ts as f32 * self.theta_length) * DEG;
                let (x, y) = (r * a.cos(), r * a.sin());
                m.vertex([x, y, 0.0], [0.0, 0.0, 1.0], [(x / outer + 1.0) * 0.5, (y / outer + 1.0) * 0.5]);
            }
        }
        for j in 0..ps {
            for i in 0..ts {
                let s = i + j * (ts + 1);
                let (a, b, c, d) = (s, s + ts + 1, s + ts + 2, s + 1);
                m.tri(a, b, d);
                m.tri(b, c, d);
            }
        }
        m
    }
}

/// A capsule along y: a cylinder of `length` with hemispherical ends
/// (three.js CapsuleGeometry).
#[derive(Clone, Debug, PartialEq)]
pub struct CapsuleGeo {
    /// Radius. Default 0.5.
    pub radius: f32,
    /// Length of the straight middle. Default 1.
    pub length: f32,
    /// Segments per hemisphere, pole to equator. Default 8.
    pub cap_segments: u32,
    /// Segments around. Default 24.
    pub radial_segments: u32,
}

impl Default for CapsuleGeo {
    fn default() -> Self {
        Self { radius: 0.5, length: 1.0, cap_segments: 8, radial_segments: 24 }
    }
}

impl CapsuleGeo {
    pub fn build(&self) -> Mesh {
        let cs = self.cap_segments.clamp(1, 256);
        let rs = self.radial_segments.clamp(3, 4096);
        let (r, h) = (self.radius.max(1e-6), self.length.max(0.0) * 0.5);
        // Profile rings bottom to top: (radius, y, normal radial, normal y).
        let mut rings: Vec<[f32; 4]> = Vec::new();
        for i in 0..=cs {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 / cs as f32 * std::f32::consts::FRAC_PI_2;
            rings.push([r * a.cos(), -h + r * a.sin(), a.cos(), a.sin()]);
        }
        for i in 0..=cs {
            let a = i as f32 / cs as f32 * std::f32::consts::FRAC_PI_2;
            rings.push([r * a.cos(), h + r * a.sin(), a.cos(), a.sin()]);
        }
        let total = 2.0 * h + std::f32::consts::PI * r;
        let mut acc = 0.0;
        let mut vs = vec![0.0f32];
        for w in rings.windows(2) {
            acc += ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
            vs.push(acc / total.max(1e-6));
        }
        revolve(&rings, &vs, rs, 0.0, std::f32::consts::TAU)
    }
}

/// Revolve profile rings `[radius, y, normal radial, normal y]` around y
/// (x = r sin φ, z = r cos φ, three.js LatheGeometry orientation). `vs`
/// are the rings' v coordinates.
pub(crate) fn revolve(rings: &[[f32; 4]], vs: &[f32], segments: u32, phi_start: f32, phi_length: f32) -> Mesh {
    let mut m = Mesh::new();
    let np = rings.len() as u32;
    for i in 0..=segments {
        let phi = phi_start + i as f32 / segments as f32 * phi_length;
        let (s, c) = phi.sin_cos();
        for (j, r) in rings.iter().enumerate() {
            m.vertex([r[0] * s, r[1], r[0] * c], normalize_or_up([r[2] * s, r[3], r[2] * c]), [i as f32 / segments as f32, vs[j]]);
        }
    }
    for i in 0..segments {
        for j in 0..np.saturating_sub(1) {
            let base = j + i * np;
            let (a, b, c, d) = (base, base + np, base + np + 1, base + 1);
            // Skip the zero-area triangle at a pole.
            if rings[j as usize][0] > 1e-9 {
                m.tri(a, b, d);
            }
            if rings[j as usize + 1][0] > 1e-9 {
                m.tri(c, d, b);
            }
        }
    }
    m
}

/// The platonic solid a [`PolyhedronGeo`] starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolyKind {
    Tetra,
    Octa,
    Icosa,
    Dodeca,
}

/// A platonic solid on a sphere, each face split `detail` times, with flat
/// normals (three.js Tetrahedron/Octahedron/Icosahedron/DodecahedronGeometry).
#[derive(Clone, Debug, PartialEq)]
pub struct PolyhedronGeo {
    /// Which solid. Default Icosa.
    pub kind: PolyKind,
    /// Circumradius. Default 1.
    pub radius: f32,
    /// Subdivisions per face edge; 0 = the plain solid (default).
    pub detail: u32,
}

impl Default for PolyhedronGeo {
    fn default() -> Self {
        Self { kind: PolyKind::Icosa, radius: 1.0, detail: 0 }
    }
}

impl PolyhedronGeo {
    pub fn build(&self) -> Mesh {
        let t = (1.0 + 5f32.sqrt()) / 2.0;
        let r = 1.0 / t;
        let (verts, idx): (Vec<f32>, Vec<u32>) = match self.kind {
            PolyKind::Tetra => (vec![1., 1., 1., -1., -1., 1., -1., 1., -1., 1., -1., -1.], vec![2, 1, 0, 0, 3, 2, 1, 3, 0, 2, 3, 1]),
            PolyKind::Octa => (
                vec![1., 0., 0., -1., 0., 0., 0., 1., 0., 0., -1., 0., 0., 0., 1., 0., 0., -1.],
                vec![0, 2, 4, 0, 4, 3, 0, 3, 5, 0, 5, 2, 1, 2, 5, 1, 5, 3, 1, 3, 4, 1, 4, 2],
            ),
            PolyKind::Icosa => (
                vec![-1., t, 0., 1., t, 0., -1., -t, 0., 1., -t, 0., 0., -1., t, 0., 1., t, 0., -1., -t, 0., 1., -t, t, 0., -1., t, 0., 1., -t, 0., -1., -t, 0., 1.],
                vec![0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8, 3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1],
            ),
            PolyKind::Dodeca => (
                vec![
                    -1., -1., -1., -1., -1., 1., -1., 1., -1., -1., 1., 1., 1., -1., -1., 1., -1., 1., 1., 1., -1., 1., 1., 1., 0., -r, -t, 0., -r, t, 0., r, -t, 0., r, t, -r, -t, 0., -r, t, 0., r, -t, 0., r, t, 0., -t, 0., -r, t, 0., -r, -t, 0., r, t, 0., r,
                ],
                vec![
                    3, 11, 7, 3, 7, 15, 3, 15, 13, 7, 19, 17, 7, 17, 6, 7, 6, 15, 17, 4, 8, 17, 8, 10, 17, 10, 6, 8, 0, 16, 8, 16, 2, 8, 2, 10, 0, 12, 1, 0, 1, 18, 0, 18, 16, 6, 10, 2, 6, 2, 13, 6, 13, 15, 2, 16, 18, 2, 18, 3, 2, 3, 13, 18, 1, 9, 18,
                    9, 11, 18, 11, 3, 4, 14, 12, 4, 12, 0, 4, 0, 8, 11, 9, 5, 11, 5, 19, 11, 19, 7, 19, 5, 14, 19, 14, 4, 19, 4, 17, 1, 12, 14, 1, 14, 5, 1, 5, 9,
                ],
            ),
        };
        let v = |i: u32| [verts[i as usize * 3], verts[i as usize * 3 + 1], verts[i as usize * 3 + 2]];
        let n = self.detail.min(64) + 1;
        let radius = self.radius;
        let mut m = Mesh::new();
        let mut emit = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
            let (a, b, c) = (scale3(normalize_or_up(a), radius), scale3(normalize_or_up(b), radius), scale3(normalize_or_up(c), radius));
            let mut fnrm = tri_normal_unnormalized(a, b, c);
            let centroid = scale3(add3(add3(a, b), c), 1.0 / 3.0);
            let (b, c) = if dot3(fnrm, centroid) < 0.0 {
                fnrm = scale3(fnrm, -1.0);
                (c, b)
            } else {
                (b, c)
            };
            let fnrm = normalize_or_up(fnrm);
            let base = m.positions.len() as u32;
            for p in [a, b, c] {
                m.vertex(p, fnrm, sphere_uv(p));
            }
            m.tri(base, base + 1, base + 2);
        };
        for f in idx.chunks_exact(3) {
            let (a, b, c) = (v(f[0]), v(f[1]), v(f[2]));
            // Barycentric grid of n rows.
            let at = |i: u32, j: u32| {
                let (fi, fj) = (i as f32 / n as f32, j as f32 / n as f32);
                add3(a, add3(scale3(sub3(b, a), fi), scale3(sub3(c, a), fj)))
            };
            for i in 0..n {
                for j in 0..n - i {
                    emit(at(i, j), at(i + 1, j), at(i, j + 1));
                    if j + i + 1 < n {
                        emit(at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                    }
                }
            }
        }
        m
    }
}

/// Equirectangular uv of a direction (azimuth, inclination).
fn sphere_uv(p: [f32; 3]) -> [f32; 2] {
    let n = normalize_or_up(p);
    let u = n[2].atan2(-n[0]) / std::f32::consts::TAU + 0.5;
    let v = n[1].clamp(-1.0, 1.0).asin() / std::f32::consts::PI + 0.5;
    [u, v]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every triangle faces away from `centre` (closed convex solids).
    fn outward(m: &Mesh, centre: [f32; 3]) {
        m.validate().unwrap();
        for t in m.indices.chunks_exact(3) {
            let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
            let n = tri_normal_unnormalized(a, b, c);
            if len3(n) < 1e-9 {
                continue;
            }
            let g = scale3(add3(add3(a, b), c), 1.0 / 3.0);
            assert!(dot3(n, sub3(g, centre)) > 0.0, "inward triangle at {g:?}");
        }
        for (p, n) in m.positions.iter().zip(&m.normals) {
            assert!(dot3(*n, sub3(*p, centre)) > -1e-4, "inward normal {n:?} at {p:?}");
        }
    }

    #[test]
    fn box_counts_and_outward() {
        let m = BoxGeo { segments: [2, 3, 4], ..Default::default() }.build();
        // three.js: per face (gx+1)(gy+1) vertices.
        let v = 2 * ((3 + 1) * (4 + 1) + (2 + 1) * (4 + 1) + (2 + 1) * (3 + 1));
        assert_eq!(m.vertex_count(), v);
        assert_eq!(m.triangle_count(), 2 * 2 * (3 * 4 + 2 * 4 + 2 * 3));
        outward(&m, [0.0; 3]);
        let (min, max) = m.bounds().unwrap();
        assert_eq!((min, max), ([-0.5; 3], [0.5; 3]));
    }

    #[test]
    fn rounded_box_stays_in_bounds() {
        let m = BoxGeo { size: [2.0, 1.0, 1.0], radius: 0.2, radius_segments: 3, ..Default::default() }.build();
        outward(&m, [0.0; 3]);
        let (min, max) = m.bounds().unwrap();
        assert!((max[0] - 1.0).abs() < 1e-5 && (min[1] + 0.5).abs() < 1e-5);
        // A corner vertex sits on the rounding sphere, not at the sharp corner.
        assert!(m.positions.iter().all(|p| !(p[0].abs() > 0.999 && p[1].abs() > 0.499 && p[2].abs() > 0.499)));
    }

    #[test]
    fn sphere_matches_three() {
        let m = SphereGeo { width_segments: 8, height_segments: 6, ..Default::default() }.build();
        assert_eq!(m.vertex_count(), 9 * 7);
        assert_eq!(m.triangle_count(), 8 * 6 * 2 - 2 * 8);
        outward(&m, [0.0; 3]);
    }

    #[test]
    fn torus_and_knot() {
        let t = TorusGeo { radial_segments: 6, tubular_segments: 10, ..Default::default() }.build();
        assert_eq!(t.vertex_count(), 7 * 11);
        assert_eq!(t.triangle_count(), 6 * 10 * 2);
        t.validate().unwrap();
        winding_agrees(&t);
        // Tube normals point away from the ring centre line.
        for (p, n) in t.positions.iter().zip(&t.normals) {
            let a = p[1].atan2(p[0]);
            let c = [a.cos(), a.sin(), 0.0];
            assert!(dot3(*n, sub3(*p, c)) > 0.0);
        }
        let k = TorusKnotGeo { tubular_segments: 64, radial_segments: 8, ..Default::default() }.build();
        assert_eq!(k.vertex_count(), 65 * 9);
        k.validate().unwrap();
        winding_agrees(&k);
    }

    /// Face winding agrees with the vertex normals.
    fn winding_agrees(m: &Mesh) {
        let mut agree = 0;
        for tri in m.indices.chunks_exact(3) {
            let (a, b, c) = (m.positions[tri[0] as usize], m.positions[tri[1] as usize], m.positions[tri[2] as usize]);
            if dot3(tri_normal_unnormalized(a, b, c), m.normals[tri[0] as usize]) > 0.0 {
                agree += 1;
            }
        }
        assert!(agree as f32 > 0.95 * m.triangle_count() as f32);
    }

    #[test]
    fn cylinder_and_cone() {
        let c = CylinderGeo { radial_segments: 8, height_segments: 2, ..Default::default() }.build();
        assert_eq!(c.vertex_count(), 9 * 3 + 2 * (8 + 9));
        outward(&c, [0.0; 3]);
        let k = CylinderGeo { radius_top: 0.0, radial_segments: 12, ..Default::default() }.build();
        outward(&k, [0.0, -0.25, 0.0]);
        let (_, max) = k.bounds().unwrap();
        assert!((max[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn flat_shapes_face_z() {
        for m in [
            PlaneGeo { width_segments: 3, height_segments: 2, ..Default::default() }.build(),
            CircleGeo { segments: 12, ..Default::default() }.build(),
            RingGeo { theta_segments: 10, phi_segments: 2, ..Default::default() }.build(),
        ] {
            m.validate().unwrap();
            for t in m.indices.chunks_exact(3) {
                let n = tri_normal_unnormalized(m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
                assert!(n[2] > 0.0);
            }
        }
        let p = PlaneGeo { width_segments: 3, height_segments: 2, ..Default::default() }.build();
        assert_eq!((p.vertex_count(), p.triangle_count()), (12, 12));
        let r = RingGeo { theta_segments: 10, phi_segments: 2, ..Default::default() }.build();
        assert_eq!((r.vertex_count(), r.triangle_count()), (33, 40));
    }

    #[test]
    fn capsule_outward() {
        let m = CapsuleGeo::default().build();
        outward(&m, [0.0; 3]);
        let (min, max) = m.bounds().unwrap();
        assert!((max[1] - 1.0).abs() < 1e-5 && (min[1] + 1.0).abs() < 1e-5);
    }

    #[test]
    fn polyhedra_flat_and_outward() {
        for kind in [PolyKind::Tetra, PolyKind::Octa, PolyKind::Icosa, PolyKind::Dodeca] {
            let faces = match kind {
                PolyKind::Tetra => 4,
                PolyKind::Octa => 8,
                PolyKind::Icosa => 20,
                PolyKind::Dodeca => 36,
            };
            for detail in [0, 2] {
                let m = PolyhedronGeo { kind, radius: 2.0, detail }.build();
                assert_eq!(m.triangle_count(), faces * (detail as usize + 1).pow(2), "{kind:?}");
                outward(&m, [0.0; 3]);
                for p in &m.positions {
                    assert!((len3(*p) - 2.0).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(TorusKnotGeo::default().build(), TorusKnotGeo::default().build());
    }
}
