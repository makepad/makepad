//! Splines and the ribbon meshes built along them: race tracks, paths,
//! fences, walls.
//!
//! The racing fixture hand-builds an oval from a waypoint list, which is a lot
//! of script for something the engine can do better: given control points, a
//! Catmull-Rom spline gives a smooth centreline, and sweeping a cross-section
//! along it gives road, curbs, guardrails and a collision surface from one
//! description.

use crate::mesh::{GenVertex, MeshBuilder};
use makepad_game_math as gm;
use makepad_math::*;

/// A closed or open Catmull-Rom spline through the given control points.
#[derive(Clone, Debug)]
pub struct Spline {
    pub points: Vec<Vec3f>,
    pub closed: bool,
}

/// A frame on the spline: position plus an orthonormal basis. `right` is the
/// road's lateral axis, `up` its normal, `forward` the direction of travel.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub pos: Vec3f,
    pub forward: Vec3f,
    pub right: Vec3f,
    pub up: Vec3f,
    /// Signed curvature: positive turning left. Drives banking.
    pub curvature: f32,
    /// Distance along the centreline, for uv and lap progress.
    pub distance: f32,
}

fn norm(v: Vec3f) -> Vec3f {
    let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if l > 1.0e-8 {
        vec3f(v.x / l, v.y / l, v.z / l)
    } else {
        vec3f(0.0, 0.0, 1.0)
    }
}

fn cross(a: Vec3f, b: Vec3f) -> Vec3f {
    vec3f(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

fn add(a: Vec3f, b: Vec3f) -> Vec3f {
    vec3f(a.x + b.x, a.y + b.y, a.z + b.z)
}

fn scale(a: Vec3f, s: f32) -> Vec3f {
    vec3f(a.x * s, a.y * s, a.z * s)
}

impl Spline {
    pub fn new(points: Vec<Vec3f>, closed: bool) -> Self {
        Self { points, closed }
    }

    fn control(&self, i: isize) -> Vec3f {
        let n = self.points.len() as isize;
        if n == 0 {
            return Vec3f::default();
        }
        let idx = if self.closed {
            ((i % n) + n) % n
        } else {
            i.clamp(0, n - 1)
        };
        self.points[idx as usize]
    }

    /// Catmull-Rom position at segment `seg`, parameter `t` in [0, 1].
    pub fn point(&self, seg: isize, t: f32) -> Vec3f {
        let (p0, p1, p2, p3) = (
            self.control(seg - 1),
            self.control(seg),
            self.control(seg + 1),
            self.control(seg + 2),
        );
        let (t2, t3) = (t * t, t * t * t);
        let f = |a: f32, b: f32, c: f32, d: f32| {
            0.5 * ((2.0 * b)
                + (0.0 - a + c) * t
                + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
                + (0.0 - a + 3.0 * b - 3.0 * c + d) * t3)
        };
        vec3f(
            f(p0.x, p1.x, p2.x, p3.x),
            f(p0.y, p1.y, p2.y, p3.y),
            f(p0.z, p1.z, p2.z, p3.z),
        )
    }

    fn segment_count(&self) -> isize {
        let n = self.points.len() as isize;
        if n < 2 {
            0
        } else if self.closed {
            n
        } else {
            n - 1
        }
    }

    /// Sample evenly-spaced frames along the spline.
    ///
    /// `per_segment` samples per control segment. Frames use a parallel
    /// transport of the up vector rather than a fixed world-up, so a track can
    /// climb and bank without the basis flipping at vertical tangents.
    pub fn frames(&self, per_segment: usize) -> Vec<Frame> {
        let segs = self.segment_count();
        if segs == 0 || per_segment == 0 {
            return Vec::new();
        }
        let mut out: Vec<Frame> = Vec::with_capacity(segs as usize * per_segment);
        let mut up = vec3f(0.0, 1.0, 0.0);
        let mut distance = 0.0;
        let mut prev: Option<Vec3f> = None;

        for s in 0..segs {
            for i in 0..per_segment {
                let t = i as f32 / per_segment as f32;
                let pos = self.point(s, t);
                // Central difference for the tangent: cheaper and steadier
                // than the analytic derivative near the segment joins.
                let d = 1.0 / per_segment as f32 * 0.5;
                let ahead = self.point(s, t + d);
                let behind = self.point(s, t - d);
                let forward = norm(vec3f(
                    ahead.x - behind.x,
                    ahead.y - behind.y,
                    ahead.z - behind.z,
                ));
                // Parallel transport: keep the previous up, re-orthogonalise.
                let right = norm(cross(up, forward));
                up = norm(cross(forward, right));
                if let Some(p) = prev {
                    let step = vec3f(pos.x - p.x, pos.y - p.y, pos.z - p.z);
                    distance += (step.x * step.x + step.y * step.y + step.z * step.z).sqrt();
                }
                prev = Some(pos);
                out.push(Frame {
                    pos,
                    forward,
                    right,
                    up,
                    curvature: 0.0,
                    distance,
                });
            }
        }
        // Curvature from the turn between consecutive tangents, signed by
        // which way the road bends. Filled in a second pass because it needs
        // the neighbours.
        let n = out.len();
        for i in 0..n {
            let prev_i = if i == 0 {
                if self.closed {
                    n - 1
                } else {
                    0
                }
            } else {
                i - 1
            };
            let next_i = if i + 1 >= n {
                if self.closed {
                    0
                } else {
                    n - 1
                }
            } else {
                i + 1
            };
            let a = out[prev_i].forward;
            let b = out[next_i].forward;
            let turn = cross(a, b);
            let sign = turn.x * out[i].up.x + turn.y * out[i].up.y + turn.z * out[i].up.z;
            out[i].curvature = sign;
        }
        out
    }

    /// Total centreline length, at the given sampling density.
    pub fn length(&self, per_segment: usize) -> f32 {
        let f = self.frames(per_segment);
        match (f.first(), f.last()) {
            (Some(_), Some(l)) => l.distance,
            _ => 0.0,
        }
    }
}

/// Cross-section knobs for a track or path.
#[derive(Clone, Copy, Debug)]
pub struct TrackParams {
    pub width: f32,
    /// Radians of bank per unit curvature; 0 for a flat path.
    pub bank: f32,
    /// Curb width at each edge, 0 for none.
    pub curb: f32,
    pub curb_height: f32,
    /// Guardrail height, 0 for none.
    pub rail_height: f32,
    /// Samples per control segment. More = smoother, more triangles.
    pub resolution: usize,
    pub surface: [f32; 3],
    pub curb_color: [f32; 3],
    pub rail_color: [f32; 3],
}

impl Default for TrackParams {
    fn default() -> Self {
        Self {
            width: 8.0,
            bank: 0.35,
            curb: 0.8,
            curb_height: 0.12,
            rail_height: 0.0,
            resolution: 8,
            surface: [0.24, 0.24, 0.26],
            curb_color: [0.78, 0.22, 0.20],
            rail_color: [0.72, 0.72, 0.75],
        }
    }
}

/// A generated track: the drawable mesh plus the data a game needs to use it.
#[derive(Clone, Debug)]
pub struct Track {
    pub mesh: crate::mesh::GenMesh,
    /// Centreline frames — spawn points, checkpoints and AI racing lines all
    /// come from these rather than from a second hand-written list.
    pub frames: Vec<Frame>,
    pub length: f32,
}

/// Sweep a road cross-section along a spline.
pub fn track(spline: &Spline, p: TrackParams) -> Track {
    let frames = spline.frames(p.resolution.max(2));
    let mut b = MeshBuilder::new();
    if frames.len() < 2 {
        return Track {
            mesh: b.finish(),
            frames,
            length: 0.0,
        };
    }

    let half = p.width * 0.5;
    let ring = |f: &Frame| -> (Vec3f, Vec3f, Vec3f, Vec3f) {
        // Bank rotates the lateral axis about the direction of travel, so the
        // road leans into a corner instead of staying flat.
        let angle = (f.curvature * p.bank).clamp(-0.6, 0.6);
        let (s, c) = gm::sincos(angle);
        let right = norm(add(scale(f.right, c), scale(f.up, s)));
        let up = norm(cross(f.forward, right));
        let inner_l = add(f.pos, scale(right, -half));
        let inner_r = add(f.pos, scale(right, half));
        let outer_l = add(add(inner_l, scale(right, -p.curb)), scale(up, p.curb_height));
        let outer_r = add(add(inner_r, scale(right, p.curb)), scale(up, p.curb_height));
        (inner_l, inner_r, outer_l, outer_r)
    };

    let first_idx;
    {
        // Emit the surface + curb ring for every frame, stitching to the last.
        let emit_ring = |b: &mut MeshBuilder, f: &Frame| -> (u32, u32, u32, u32) {
            let (il, ir, ol, or) = ring(f);
            let v = f.distance * 0.08;
            let up = f.up;
            let mk = |b: &mut MeshBuilder, pos: Vec3f, u: f32, color: [f32; 3]| {
                b.vertex(GenVertex {
                    pos,
                    normal: up,
                    uv: [u, v],
                    color,
                    growth: 1.0,
                    flex: 0.0,
                })
            };
            (
                mk(b, ol, 0.0, p.curb_color),
                mk(b, il, 0.15, p.surface),
                mk(b, ir, 0.85, p.surface),
                mk(b, or, 1.0, p.curb_color),
            )
        };
        let f0 = emit_ring(&mut b, &frames[0]);
        first_idx = f0;
        let mut prev = Some(f0);
        for f in &frames[1..] {
            let cur = emit_ring(&mut b, f);
            if let Some(pv) = prev {
                // Curb, road, curb — three quads per step.
                b.quad(pv.0, cur.0, cur.1, pv.1);
                b.quad(pv.1, cur.1, cur.2, pv.2);
                b.quad(pv.2, cur.2, cur.3, pv.3);
            }
            prev = Some(cur);
        }
        if spline.closed {
            if let Some(pv) = prev {
                b.quad(pv.0, first_idx.0, first_idx.1, pv.1);
                b.quad(pv.1, first_idx.1, first_idx.2, pv.2);
                b.quad(pv.2, first_idx.2, first_idx.3, pv.3);
            }
        }
    }

    if p.rail_height > 0.0 {
        emit_rails(&mut b, &frames, &p, spline.closed);
    }

    let length = frames.last().map(|f| f.distance).unwrap_or(0.0);
    b.bake_ambient(0.25, 0.5);
    Track {
        mesh: b.finish(),
        frames,
        length,
    }
}

fn emit_rails(b: &mut MeshBuilder, frames: &[Frame], p: &TrackParams, closed: bool) {
    let half = p.width * 0.5 + p.curb;
    for side in [-1.0f32, 1.0] {
        let mut prev: Option<(u32, u32)> = None;
        let mut first: Option<(u32, u32)> = None;
        for f in frames {
            let base = add(f.pos, scale(f.right, half * side));
            let top = add(base, scale(f.up, p.rail_height));
            let n = norm(scale(f.right, side));
            let lo = b.vertex(GenVertex {
                pos: base,
                normal: n,
                uv: [f.distance * 0.1, 0.0],
                color: p.rail_color,
                growth: 1.0,
                flex: 0.0,
            });
            let hi = b.vertex(GenVertex {
                pos: top,
                normal: n,
                uv: [f.distance * 0.1, 1.0],
                color: p.rail_color,
                growth: 1.0,
                flex: 0.0,
            });
            if let Some((plo, phi)) = prev {
                if side < 0.0 {
                    b.quad(plo, lo, hi, phi);
                } else {
                    b.quad(phi, hi, lo, plo);
                }
            }
            if first.is_none() {
                first = Some((lo, hi));
            }
            prev = Some((lo, hi));
        }
        if closed {
            if let (Some((plo, phi)), Some((flo, fhi))) = (prev, first) {
                if side < 0.0 {
                    b.quad(plo, flo, fhi, phi);
                } else {
                    b.quad(phi, fhi, flo, plo);
                }
            }
        }
    }
}

/// An oval, the shape a racing prompt asks for most often. Returns a closed
/// spline a caller can hand straight to [`track`].
pub fn oval(width: f32, depth: f32, corner_points: usize) -> Spline {
    let n = corner_points.max(3);
    let mut pts = Vec::with_capacity(n);
    for i in 0..n {
        let a = (i as f32 / n as f32) * 6.283_185_3;
        let (s, c) = gm::sincos(a);
        pts.push(vec3f(c * width * 0.5, 0.0, s * depth * 0.5));
    }
    Spline::new(pts, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spline_passes_through_its_control_points() {
        let pts = vec![
            vec3f(0.0, 0.0, 0.0),
            vec3f(10.0, 0.0, 0.0),
            vec3f(10.0, 0.0, 10.0),
            vec3f(0.0, 0.0, 10.0),
        ];
        let s = Spline::new(pts.clone(), true);
        for (i, p) in pts.iter().enumerate() {
            let got = s.point(i as isize, 0.0);
            assert!((got.x - p.x).abs() < 1.0e-4 && (got.z - p.z).abs() < 1.0e-4);
        }
    }

    #[test]
    fn a_closed_oval_has_a_sane_length() {
        let s = oval(100.0, 60.0, 12);
        let len = s.length(8);
        // Ramanujan's ellipse perimeter for a=50, b=30 is ~254.
        assert!(len > 200.0 && len < 300.0, "oval length {len}");
    }

    #[test]
    fn track_is_watertight_enough_and_indices_are_in_range() {
        let s = oval(80.0, 50.0, 10);
        let t = track(&s, TrackParams::default());
        assert!(t.mesh.triangle_count() > 100);
        for i in &t.mesh.indices {
            assert!((*i as usize) < t.mesh.vertex_count());
        }
        for f in &t.mesh.vertices {
            assert!(f.is_finite());
        }
    }

    #[test]
    fn resolution_raises_triangle_count() {
        let s = oval(80.0, 50.0, 8);
        let lo = track(
            &s,
            TrackParams {
                resolution: 4,
                ..Default::default()
            },
        )
        .mesh
        .triangle_count();
        let hi = track(
            &s,
            TrackParams {
                resolution: 16,
                ..Default::default()
            },
        )
        .mesh
        .triangle_count();
        assert!(hi > lo, "resolution ignored: {lo} vs {hi}");
    }

    #[test]
    fn frames_are_orthonormal_all_the_way_round() {
        let s = oval(60.0, 60.0, 8);
        for f in s.frames(8) {
            let d = |a: Vec3f, b: Vec3f| a.x * b.x + a.y * b.y + a.z * b.z;
            assert!(d(f.forward, f.right).abs() < 1.0e-3, "not orthogonal");
            assert!(d(f.forward, f.up).abs() < 1.0e-3);
            assert!((d(f.forward, f.forward) - 1.0).abs() < 1.0e-3, "not unit");
        }
    }

    #[test]
    fn curvature_is_signed_and_zero_on_a_straight() {
        let straight = Spline::new(
            vec![
                vec3f(0.0, 0.0, 0.0),
                vec3f(0.0, 0.0, 10.0),
                vec3f(0.0, 0.0, 20.0),
                vec3f(0.0, 0.0, 30.0),
            ],
            false,
        );
        for f in straight.frames(4) {
            assert!(f.curvature.abs() < 1.0e-3, "straight bent: {}", f.curvature);
        }
        // A closed ring turns the same way the whole way round, so the sign
        // must be consistent rather than flipping frame to frame.
        let ring = oval(50.0, 50.0, 10);
        let fr = ring.frames(8);
        let positive = fr.iter().filter(|f| f.curvature > 0.0).count();
        assert!(
            positive == 0 || positive == fr.len(),
            "curvature sign flipped mid-corner: {positive}/{}",
            fr.len()
        );
    }

    #[test]
    fn rails_add_geometry_only_when_asked() {
        let s = oval(60.0, 40.0, 8);
        let bare = track(&s, TrackParams::default()).mesh.triangle_count();
        let railed = track(
            &s,
            TrackParams {
                rail_height: 1.0,
                ..Default::default()
            },
        )
        .mesh
        .triangle_count();
        assert!(railed > bare);
    }

    #[test]
    fn frames_carry_lap_distance_monotonically() {
        let s = oval(80.0, 50.0, 10);
        let fr = s.frames(8);
        for w in fr.windows(2) {
            assert!(w[1].distance >= w[0].distance);
        }
        assert!(fr.last().unwrap().distance > 0.0);
    }
}
