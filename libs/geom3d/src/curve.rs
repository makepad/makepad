//! 3D curves (paths for tubes, cameras and motion), arc-length sampling,
//! parallel-transport frames, and tubes that can draw themselves on.

use crate::primitives::torus_knot_point;
use crate::math::DEG;
use crate::mesh::*;
use std::f32::consts::TAU;

/// A 3D curve, parameterised 0..1.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve3 {
    /// Straight segments through the points.
    Polyline(Vec<[f32; 3]>),
    /// A smooth curve through the points (cardinal spline; `tension` 0.5 =
    /// Catmull-Rom, 0 = straight, 1 = loose). `closed` joins the ends.
    CatmullRom { points: Vec<[f32; 3]>, closed: bool, tension: f32 },
    /// Chained cubic Béziers: p0, c1, c2, p1, c1, c2, p2, ... (3n + 1 points).
    Bezier(Vec<[f32; 3]>),
    /// A circle in the xz plane (horizontal), starting at +x, counter-clockwise seen from +y.
    Circle { radius: f32 },
    /// A helix around y from -height/2 to +height/2.
    Helix { radius: f32, height: f32, turns: f32 },
    /// (a sin(fx 2πs + phase), b sin(fy 2πs), c sin(fz 2πs)); `phase` in degrees.
    Lissajous { a: f32, b: f32, c: f32, freq: [f32; 3], phase: f32 },
    /// The three.js (p, q) torus-knot path; `tube_offset` 1 = three.js's swing.
    TorusKnot { p: u32, q: u32, radius: f32, tube_offset: f32 },
}

impl Default for Curve3 {
    fn default() -> Self {
        Curve3::Circle { radius: 1.0 }
    }
}

/// A point on a curve with its rotation-minimising frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub position: [f32; 3],
    pub tangent: [f32; 3],
    pub normal: [f32; 3],
    pub binormal: [f32; 3],
}

impl Curve3 {
    /// Whether the curve ends where it starts.
    pub fn is_closed(&self) -> bool {
        match self {
            Curve3::CatmullRom { closed, .. } => *closed,
            Curve3::Circle { .. } | Curve3::TorusKnot { .. } => true,
            Curve3::Lissajous { freq, .. } => freq.iter().all(|f| (f - f.round()).abs() < 1e-6),
            _ => false,
        }
    }

    /// The point at raw parameter `t` 0..1 (not arc length; see [`Curve3::arc`]).
    pub fn raw_point(&self, t: f32) -> [f32; 3] {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        match self {
            Curve3::Polyline(p) => polyline_point(p, t),
            Curve3::CatmullRom { points, closed, tension } => catmull_point(points, *closed, *tension, t),
            Curve3::Bezier(p) => bezier_point(p, t),
            Curve3::Circle { radius } => {
                let a = t * TAU;
                [radius * a.cos(), 0.0, -radius * a.sin()]
            }
            Curve3::Helix { radius, height, turns } => {
                let a = t * TAU * turns;
                [radius * a.cos(), height * (t - 0.5), -radius * a.sin()]
            }
            Curve3::Lissajous { a, b, c, freq, phase } => {
                let s = t * TAU;
                [a * (freq[0] * s + phase * DEG).sin(), b * (freq[1] * s).sin(), c * (freq[2] * s).sin()]
            }
            Curve3::TorusKnot { p, q, radius, tube_offset } => {
                let (p, q) = ((*p).max(1) as f32, (*q).max(1) as f32);
                let u = t * TAU * p;
                if (*tube_offset - 1.0).abs() < 1e-6 {
                    torus_knot_point(u, p, q, *radius)
                } else {
                    let qu = q / p * u;
                    let ring = radius * (1.0 + tube_offset * 0.5 * qu.cos());
                    [ring * u.cos(), ring * u.sin(), radius * tube_offset * 0.5 * qu.sin()]
                }
            }
        }
    }

    /// The arc-length table: evenly spaced points by distance travelled.
    pub fn arc(&self) -> ArcTable {
        let n = match self {
            Curve3::Polyline(p) | Curve3::Bezier(p) | Curve3::CatmullRom { points: p, .. } => (p.len() * 32).clamp(256, 16384),
            Curve3::TorusKnot { p, q, .. } => (((p + q) as usize) * 128).clamp(512, 16384),
            Curve3::Helix { turns, .. } => ((turns.abs() * 128.0) as usize).clamp(256, 16384),
            _ => 512,
        };
        let mut ts = Vec::with_capacity(n + 1);
        let mut lengths = Vec::with_capacity(n + 1);
        let mut prev = self.raw_point(0.0);
        let mut acc = 0.0;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let p = self.raw_point(t);
            acc += len3(sub3(p, prev));
            prev = p;
            ts.push(t);
            lengths.push(acc);
        }
        ArcTable { curve: self.clone(), ts, lengths, closed: self.is_closed() }
    }

    /// The point at arc-length fraction `u` 0..1 (builds a table; keep an
    /// [`ArcTable`] when sampling many points).
    pub fn point_at(&self, u: f32) -> [f32; 3] {
        self.arc().point_at(u)
    }

    pub fn tangent_at(&self, u: f32) -> [f32; 3] {
        self.arc().tangent_at(u)
    }

    pub fn length(&self) -> f32 {
        self.arc().length()
    }
}

fn polyline_point(p: &[[f32; 3]], t: f32) -> [f32; 3] {
    match p.len() {
        0 => [0.0; 3],
        1 => p[0],
        n => {
            let f = t * (n - 1) as f32;
            let i = (f.floor() as usize).min(n - 2);
            lerp3(p[i], p[i + 1], f - i as f32)
        }
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], f: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
}

fn catmull_point(p: &[[f32; 3]], closed: bool, tension: f32, t: f32) -> [f32; 3] {
    let n = p.len();
    if n < 2 {
        return p.first().copied().unwrap_or([0.0; 3]);
    }
    let segs = if closed { n } else { n - 1 };
    let f = t * segs as f32;
    let i = (f.floor() as usize).min(segs - 1);
    let w = f - i as f32;
    let at = |k: isize| -> [f32; 3] {
        if closed {
            p[k.rem_euclid(n as isize) as usize]
        } else if k < 0 {
            sub3(scale3(p[0], 2.0), p[1])
        } else if k as usize >= n {
            sub3(scale3(p[n - 1], 2.0), p[n - 2])
        } else {
            p[k as usize]
        }
    };
    let i = i as isize;
    let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let mut out = [0.0; 3];
    for k in 0..3 {
        let t0 = tension * (p2[k] - p0[k]);
        let t1 = tension * (p3[k] - p1[k]);
        let (w2, w3) = (w * w, w * w * w);
        out[k] = (2.0 * w3 - 3.0 * w2 + 1.0) * p1[k] + (w3 - 2.0 * w2 + w) * t0 + (-2.0 * w3 + 3.0 * w2) * p2[k] + (w3 - w2) * t1;
    }
    out
}

fn bezier_point(p: &[[f32; 3]], t: f32) -> [f32; 3] {
    if p.len() < 4 {
        return polyline_point(p, t);
    }
    let segs = (p.len() - 1) / 3;
    let f = t * segs as f32;
    let i = (f.floor() as usize).min(segs - 1);
    let w = f - i as f32;
    let (a, b, c, d) = (p[3 * i], p[3 * i + 1], p[3 * i + 2], p[3 * i + 3]);
    let u = 1.0 - w;
    let mut out = [0.0; 3];
    for k in 0..3 {
        out[k] = u * u * u * a[k] + 3.0 * u * u * w * b[k] + 3.0 * u * w * w * c[k] + w * w * w * d[k];
    }
    out
}

/// A curve sampled by arc length.
#[derive(Clone, Debug)]
pub struct ArcTable {
    curve: Curve3,
    ts: Vec<f32>,
    lengths: Vec<f32>,
    closed: bool,
}

impl ArcTable {
    pub fn length(&self) -> f32 {
        *self.lengths.last().unwrap_or(&0.0)
    }

    /// The raw parameter at arc-length fraction `u`.
    pub fn t_at(&self, u: f32) -> f32 {
        let total = self.length();
        if total <= 0.0 || !u.is_finite() {
            return 0.0;
        }
        let d = u.clamp(0.0, 1.0) * total;
        let i = self.lengths.partition_point(|&l| l < d).clamp(1, self.lengths.len() - 1);
        let (l0, l1) = (self.lengths[i - 1], self.lengths[i]);
        let f = if l1 > l0 { (d - l0) / (l1 - l0) } else { 0.0 };
        self.ts[i - 1] + (self.ts[i] - self.ts[i - 1]) * f
    }

    pub fn point_at(&self, u: f32) -> [f32; 3] {
        self.curve.raw_point(self.t_at(u))
    }

    /// Unit tangent at arc-length fraction `u` (central difference,
    /// wrapping across the seam of a closed curve).
    pub fn tangent_at(&self, u: f32) -> [f32; 3] {
        let h = 1e-3;
        let wrap = |x: f32| if self.closed { x.rem_euclid(1.0) } else { x.clamp(0.0, 1.0) };
        normalize_or_up(sub3(self.point_at(wrap(u + h)), self.point_at(wrap(u - h))))
    }

    /// `n + 1` rotation-minimising frames at u = i / n (parallel transport;
    /// a closed curve spreads its leftover twist so the seam matches).
    pub fn frames(&self, n: usize) -> Vec<Frame> {
        let n = n.max(1);
        let us: Vec<f32> = (0..=n).map(|i| i as f32 / n as f32).collect();
        let mut frames = self.transport(&us);
        if self.closed && frames.len() > 1 {
            let first = frames[0].normal;
            let last = frames[n].normal;
            let t = frames[n].tangent;
            let mut theta = dot3(first, last).clamp(-1.0, 1.0).acos();
            if dot3(t, cross3(last, first)) < 0.0 {
                theta = -theta;
            }
            for (i, f) in frames.iter_mut().enumerate() {
                let a = theta * i as f32 / n as f32;
                f.normal = rotate_about(f.normal, f.tangent, a);
                f.binormal = cross3(f.tangent, f.normal);
            }
        }
        frames
    }

    /// Frames at the given increasing arc-length fractions, transported
    /// from u = 0 on a fixed fine table, so a sub-range (a tube drawing
    /// itself on) twists exactly like the whole.
    pub fn frames_at(&self, us: &[f32]) -> Vec<Frame> {
        self.transport(us)
    }

    fn transport(&self, us: &[f32]) -> Vec<Frame> {
        // Fixed fine steps from 0, then the requested points in between.
        const FINE: usize = 1024;
        let t0 = self.tangent_at(0.0);
        let mut normal = initial_normal(t0);
        let mut tangent = t0;
        let mut out = Vec::with_capacity(us.len());
        let mut step = 0usize;
        for &u in us {
            let u = u.clamp(0.0, 1.0);
            while ((step + 1) as f32 / FINE as f32) <= u {
                step += 1;
                let nt = self.tangent_at(step as f32 / FINE as f32);
                normal = transport_normal(normal, tangent, nt);
                tangent = nt;
            }
            let t = self.tangent_at(u);
            let n = transport_normal(normal, tangent, t);
            out.push(Frame { position: self.point_at(u), tangent: t, normal: n, binormal: normalize_or_up(cross3(t, n)) });
        }
        out
    }
}

/// three.js's start normal: perpendicular to the tangent, toward the
/// tangent's smallest axis.
fn initial_normal(t: [f32; 3]) -> [f32; 3] {
    let a = [t[0].abs(), t[1].abs(), t[2].abs()];
    let axis = if a[0] <= a[1] && a[0] <= a[2] {
        [1.0, 0.0, 0.0]
    } else if a[1] <= a[2] {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let v = normalize_or_up(cross3(t, axis));
    normalize_or_up(cross3(t, v))
}

/// Carry `n` from tangent `a` to tangent `b` by the smallest rotation.
fn transport_normal(n: [f32; 3], a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    let axis = cross3(a, b);
    let s = len3(axis);
    let out = if s > 1e-7 {
        let angle = s.atan2(dot3(a, b));
        rotate_about(n, scale3(axis, 1.0 / s), angle)
    } else {
        n
    };
    // Re-orthogonalise against b.
    normalize_or_up(sub3(out, scale3(b, dot3(out, b))))
}

/// Rodrigues rotation of `v` about unit `axis` by `angle` radians.
fn rotate_about(v: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    let k = axis;
    add3(add3(scale3(v, c), scale3(cross3(k, v), s)), scale3(k, dot3(k, v) * (1.0 - c)))
}

/// A tube swept along a curve (three.js TubeGeometry), optionally tapered.
#[derive(Clone, Debug, PartialEq)]
pub struct TubeGeo {
    /// The path.
    pub curve: Curve3,
    /// Rings along the path. Default 64.
    pub tubular_segments: u32,
    /// Radius at the start. Default 0.1.
    pub radius: f32,
    /// Segments around. Default 8.
    pub radial_segments: u32,
    /// Join the last ring to the first (for closed curves). Default: the curve's own closedness.
    pub closed: Option<bool>,
    /// Radius at the end; 0 = same as `radius` (default). A taper.
    pub radius_end: f32,
    /// Flat discs on open ends. Default false.
    pub caps: bool,
}

impl Default for TubeGeo {
    fn default() -> Self {
        Self { curve: Curve3::default(), tubular_segments: 64, radius: 0.1, radial_segments: 8, closed: None, radius_end: 0.0, caps: false }
    }
}

impl TubeGeo {
    pub fn build(&self) -> Mesh {
        self.build_range(0.0, 1.0)
    }

    /// The tube over arc-length fractions `start..end` only: "draw-on" by
    /// animating `end`. The twist and taper are those of the whole tube.
    pub fn build_range(&self, start: f32, end: f32) -> Mesh {
        let (start, end) = (start.clamp(0.0, 1.0), end.clamp(0.0, 1.0));
        let mut m = Mesh::new();
        if end - start <= 1e-6 {
            return m;
        }
        let full = (start <= 0.0 && end >= 1.0) && self.closed.unwrap_or(self.curve.is_closed());
        let ts = self.tubular_segments.clamp(1, 16384);
        let rings = ((ts as f32 * (end - start)).ceil() as u32).max(1);
        let rs = self.radial_segments.clamp(3, 1024);
        let table = self.curve.arc();
        let us: Vec<f32> = (0..=rings).map(|i| start + (end - start) * i as f32 / rings as f32).collect();
        let mut frames = if start <= 0.0 && end >= 1.0 { table.frames(rings as usize) } else { table.frames_at(&us) };
        if full {
            let f0 = frames[0];
            *frames.last_mut().unwrap() = f0;
        }
        let r1 = if self.radius_end > 0.0 { self.radius_end } else { self.radius };
        let radius = |u: f32| self.radius + (r1 - self.radius) * u;
        for (i, f) in frames.iter().enumerate() {
            let u = us[i];
            for j in 0..=rs {
                let v = j as f32 / rs as f32 * TAU;
                let n = normalize_or_up(add3(scale3(f.normal, -v.cos()), scale3(f.binormal, v.sin())));
                m.vertex(add3(f.position, scale3(n, radius(u))), n, [i as f32 / rings as f32, j as f32 / rs as f32]);
            }
        }
        let w = rs + 1;
        for j in 1..=rings {
            for i in 1..=rs {
                let a = w * (j - 1) + i - 1;
                let b = w * j + i - 1;
                let c = w * j + i;
                let d = w * (j - 1) + i;
                m.tri(a, b, d);
                m.tri(b, c, d);
            }
        }
        if self.caps && !full {
            for (k, sign) in [(0usize, -1.0f32), (frames.len() - 1, 1.0)] {
                let f = frames[k];
                let r = radius(us[k]);
                let n = scale3(f.tangent, sign);
                let centre = m.vertex(f.position, n, [0.5, 0.5]);
                for j in 0..=rs {
                    let v = j as f32 / rs as f32 * TAU;
                    let d = add3(scale3(f.normal, -v.cos()), scale3(f.binormal, v.sin()));
                    m.vertex(add3(f.position, scale3(d, r)), n, [0.5 - v.cos() * 0.5, 0.5 + v.sin() * 0.5]);
                }
                for j in 0..rs {
                    let (a, b) = (centre + 1 + j, centre + 2 + j);
                    // The ring runs -N, B, N, -B: clockwise about +T.
                    if sign > 0.0 {
                        m.tri(centre, b, a);
                    } else {
                        m.tri(centre, a, b);
                    }
                }
            }
        }
        m
    }
}

/// A tube over `start..end` of its length (see [`TubeGeo::build_range`]).
pub fn tube_trimmed(tube: &TubeGeo, start: f32, end: f32) -> Mesh {
    tube.build_range(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_length_is_even() {
        let c = Curve3::Bezier(vec![[0.0, 0.0, 0.0], [0.0, 3.0, 0.0], [1.0, 3.0, 0.0], [4.0, 0.0, 0.0]]);
        let a = c.arc();
        let pts: Vec<[f32; 3]> = (0..=20).map(|i| a.point_at(i as f32 / 20.0)).collect();
        let d: Vec<f32> = pts.windows(2).map(|w| len3(sub3(w[1], w[0]))).collect();
        let (mn, mx) = d.iter().fold((f32::MAX, 0.0f32), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(mx / mn < 1.05, "{mn} {mx}");
        assert_eq!(a.point_at(1.0), [4.0, 0.0, 0.0]);
    }

    #[test]
    fn circle_length_and_catmull_passes_points() {
        assert!((Curve3::Circle { radius: 2.0 }.length() - 4.0 * std::f32::consts::PI).abs() < 1e-2);
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 1.0], [3.0, 1.0, 1.0]];
        let c = Curve3::CatmullRom { points: pts.clone(), closed: false, tension: 0.5 };
        for (i, p) in pts.iter().enumerate() {
            let q = c.raw_point(i as f32 / 3.0);
            assert!(len3(sub3(q, *p)) < 1e-5, "{q:?} {p:?}");
        }
    }

    #[test]
    fn frames_are_orthonormal_and_closed_seam_matches() {
        for c in [Curve3::TorusKnot { p: 2, q: 3, radius: 1.0, tube_offset: 1.0 }, Curve3::Helix { radius: 1.0, height: 3.0, turns: 3.0 }] {
            let f = c.arc().frames(200);
            for fr in &f {
                assert!(dot3(fr.tangent, fr.normal).abs() < 1e-3);
                assert!((len3(fr.normal) - 1.0).abs() < 1e-3);
                assert!(dot3(fr.binormal, fr.normal).abs() < 1e-3);
            }
            if c.is_closed() {
                assert!(dot3(f[0].normal, f[200].normal) > 0.99);
            }
        }
    }

    #[test]
    fn tube_counts_and_outward() {
        let t = TubeGeo { curve: Curve3::Helix { radius: 1.0, height: 2.0, turns: 1.5 }, tubular_segments: 40, radial_segments: 6, ..Default::default() };
        let m = t.build();
        m.validate().unwrap();
        assert_eq!(m.vertex_count(), 41 * 7);
        assert_eq!(m.triangle_count(), 40 * 6 * 2);
        for tri in m.indices.chunks_exact(3) {
            let n = tri_normal_unnormalized(m.positions[tri[0] as usize], m.positions[tri[1] as usize], m.positions[tri[2] as usize]);
            assert!(dot3(n, m.normals[tri[0] as usize]) > 0.0);
        }
    }

    #[test]
    fn trimmed_tube_follows_the_whole() {
        let t = TubeGeo { curve: Curve3::Circle { radius: 1.0 }, tubular_segments: 64, caps: true, ..Default::default() };
        let whole = t.build();
        let part = tube_trimmed(&t, 0.0, 0.5);
        part.validate().unwrap();
        for tri in part.indices.chunks_exact(3) {
            let n = tri_normal_unnormalized(part.positions[tri[0] as usize], part.positions[tri[1] as usize], part.positions[tri[2] as usize]);
            if len3(n) > 1e-9 {
                assert!(dot3(n, part.normals[tri[0] as usize]) > 0.0);
            }
        }
        // The first ring is identical: same frames from u = 0.
        for j in 0..9 {
            assert!(len3(sub3(whole.positions[j], part.positions[j])) < 1e-3);
        }
        let (min, max) = part.bounds().unwrap();
        assert!(max[2] < 0.2 && min[2] < -1.0, "{min:?} {max:?}");
        assert!(tube_trimmed(&t, 0.3, 0.3).is_empty());
        // Tapered: the end ring is thinner.
        let taper = TubeGeo { curve: Curve3::Polyline(vec![[0.0; 3], [0.0, 0.0, -2.0]]), radius: 0.2, radius_end: 0.05, ..Default::default() }.build();
        let far: Vec<_> = taper.positions.iter().filter(|p| p[2] < -1.99).collect();
        assert!(far.iter().all(|p| (p[0] * p[0] + p[1] * p[1]).sqrt() < 0.051));
    }
}

/// A paperclip's wire in the xy plane: three nested U-turns (a Gem clip),
/// `length` tall and `width` wide, sampled densely for a smooth spline.
pub fn paperclip_points(length: f32, width: f32) -> Vec<[f32; 3]> {
    let (l, w) = (length.max(0.05), width.max(0.02));
    let mut pts: Vec<[f32; 3]> = Vec::new();
    let line = |pts: &mut Vec<[f32; 3]>, a: [f32; 2], b: [f32; 2]| {
        for k in 0..6 {
            let u = k as f32 / 6.0;
            pts.push([a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u, 0.0]);
        }
    };
    // A half turn about (cx, cy) of radius r from angle a0 to a0 + pi.
    let turn = |pts: &mut Vec<[f32; 3]>, cx: f32, cy: f32, r: f32, a0: f32| {
        for k in 0..12 {
            let a = a0 + std::f32::consts::PI * k as f32 / 12.0;
            pts.push([cx + r * a.cos(), cy + r * a.sin(), 0.0]);
        }
    };
    let (r1, r2, r3) = (w * 0.5, w * 0.36, w * 0.24);
    // Outer right leg up, top turn, left leg down, bottom turn, inner leg up, top turn, innermost down.
    line(&mut pts, [r1, -0.18 * l], [r1, 0.5 * l - r1]);
    turn(&mut pts, 0.0, 0.5 * l - r1, r1, 0.0);
    line(&mut pts, [-r1, 0.5 * l - r1], [-r1, -0.5 * l + r2]);
    turn(&mut pts, -r1 + r2, -0.5 * l + r2, r2, std::f32::consts::PI);
    line(&mut pts, [-r1 + 2.0 * r2, -0.5 * l + r2], [-r1 + 2.0 * r2, 0.36 * l - r3]);
    turn(&mut pts, -r1 + 2.0 * r2 - r3, 0.36 * l - r3, r3, 0.0);
    line(&mut pts, [-r1 + 2.0 * r2 - 2.0 * r3, 0.36 * l - r3], [-r1 + 2.0 * r2 - 2.0 * r3, -0.2 * l]);
    pts.push([-r1 + 2.0 * r2 - 2.0 * r3, -0.2 * l, 0.0]);
    pts
}

/// Strand `k` of a flat `strands`-strand braid along y: each strand weaves
/// side to side (x) and over and under (z), a third of a cycle apart.
pub fn braid_points(k: u32, strands: u32, radius: f32, turns: f32, length: f32) -> Vec<[f32; 3]> {
    let n = 240;
    let phase = std::f32::consts::TAU * k as f32 / strands as f32;
    (0..=n)
        .map(|i| {
            let s = i as f32 / n as f32;
            let a = std::f32::consts::TAU * turns * s + phase;
            [radius * a.sin(), (s - 0.5) * length, radius * 0.5 * (2.0 * a).sin()]
        })
        .collect()
}
