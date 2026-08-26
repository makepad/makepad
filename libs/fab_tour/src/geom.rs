//! Small geometry helpers: AABB algebra, conservative triangle/box overlap
//! (Akenine-Möller SAT) and ray/triangle intersection. No allocation, no deps
//! beyond `makepad_math`.

use makepad_math::{vec3, Aabb, Vec3f};

pub fn aabb_empty() -> Aabb {
    Aabb {
        min: vec3(f32::INFINITY, f32::INFINITY, f32::INFINITY),
        max: vec3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
    }
}

pub fn aabb_is_empty(a: &Aabb) -> bool {
    a.min.x > a.max.x || a.min.y > a.max.y || a.min.z > a.max.z
}

pub fn aabb_union_point(a: &Aabb, p: Vec3f) -> Aabb {
    Aabb {
        min: Vec3f::min_componentwise(a.min, p),
        max: Vec3f::max_componentwise(a.max, p),
    }
}

pub fn aabb_union(a: &Aabb, b: &Aabb) -> Aabb {
    if aabb_is_empty(a) {
        return *b;
    }
    if aabb_is_empty(b) {
        return *a;
    }
    Aabb {
        min: Vec3f::min_componentwise(a.min, b.min),
        max: Vec3f::max_componentwise(a.max, b.max),
    }
}

pub fn aabb_center(a: &Aabb) -> Vec3f {
    (a.min + a.max) * 0.5
}

pub fn aabb_size(a: &Aabb) -> Vec3f {
    a.max - a.min
}

pub fn aabb_expand(a: &Aabb, by: f32) -> Aabb {
    let e = vec3(by, by, by);
    Aabb {
        min: a.min - e,
        max: a.max + e,
    }
}

pub fn aabb_contains(a: &Aabb, p: Vec3f) -> bool {
    p.x >= a.min.x
        && p.x <= a.max.x
        && p.y >= a.min.y
        && p.y <= a.max.y
        && p.z >= a.min.z
        && p.z <= a.max.z
}

/// Shortest distance from `p` to the box (0 inside).
pub fn aabb_distance(a: &Aabb, p: Vec3f) -> f32 {
    let d = Vec3f::max_componentwise(
        Vec3f::max_componentwise(a.min - p, p - a.max),
        vec3(0.0, 0.0, 0.0),
    );
    d.length()
}

/// Does triangle `(v0,v1,v2)` overlap the box centred at `c` with half-size
/// `h`? Exact separating-axis test (Akenine-Möller, "Fast 3D Triangle-Box
/// Overlap Testing"): 9 edge cross-products, 3 box normals, 1 triangle normal.
///
/// Conservative voxelisation depends on this being *exact* — an approximate
/// test leaks free space through walls, and free space that isn't free is how
/// a camera ends up inside a wall.
pub fn tri_box_overlap(c: Vec3f, h: Vec3f, v0: Vec3f, v1: Vec3f, v2: Vec3f) -> bool {
    let a = v0 - c;
    let b = v1 - c;
    let d = v2 - c;
    let e0 = b - a;
    let e1 = d - b;
    let e2 = a - d;

    // 9 axis tests: cross(box axis, triangle edge).
    macro_rules! axis_test {
        ($p0:expr, $p1:expr, $r:expr) => {{
            let (mn, mx) = if $p0 < $p1 { ($p0, $p1) } else { ($p1, $p0) };
            if mn > $r || mx < -$r {
                return false;
            }
        }};
    }
    // e0
    axis_test!(
        e0.z * a.y - e0.y * a.z,
        e0.z * d.y - e0.y * d.z,
        e0.z.abs() * h.y + e0.y.abs() * h.z
    );
    axis_test!(
        -e0.z * a.x + e0.x * a.z,
        -e0.z * d.x + e0.x * d.z,
        e0.z.abs() * h.x + e0.x.abs() * h.z
    );
    axis_test!(
        e0.y * b.x - e0.x * b.y,
        e0.y * d.x - e0.x * d.y,
        e0.y.abs() * h.x + e0.x.abs() * h.y
    );
    // e1
    axis_test!(
        e1.z * a.y - e1.y * a.z,
        e1.z * d.y - e1.y * d.z,
        e1.z.abs() * h.y + e1.y.abs() * h.z
    );
    axis_test!(
        -e1.z * a.x + e1.x * a.z,
        -e1.z * d.x + e1.x * d.z,
        e1.z.abs() * h.x + e1.x.abs() * h.z
    );
    axis_test!(
        e1.y * a.x - e1.x * a.y,
        e1.y * b.x - e1.x * b.y,
        e1.y.abs() * h.x + e1.x.abs() * h.y
    );
    // e2
    axis_test!(
        e2.z * a.y - e2.y * a.z,
        e2.z * b.y - e2.y * b.z,
        e2.z.abs() * h.y + e2.y.abs() * h.z
    );
    axis_test!(
        -e2.z * a.x + e2.x * a.z,
        -e2.z * b.x + e2.x * b.z,
        e2.z.abs() * h.x + e2.x.abs() * h.z
    );
    axis_test!(
        e2.y * b.x - e2.x * b.y,
        e2.y * d.x - e2.x * d.y,
        e2.y.abs() * h.x + e2.x.abs() * h.y
    );

    // 3 box face normals.
    if a.x.min(b.x).min(d.x) > h.x || a.x.max(b.x).max(d.x) < -h.x {
        return false;
    }
    if a.y.min(b.y).min(d.y) > h.y || a.y.max(b.y).max(d.y) < -h.y {
        return false;
    }
    if a.z.min(b.z).min(d.z) > h.z || a.z.max(b.z).max(d.z) < -h.z {
        return false;
    }

    // triangle plane vs box
    let n = Vec3f::cross(e0, e1);
    let dd = n.dot(a);
    let r = n.x.abs() * h.x + n.y.abs() * h.y + n.z.abs() * h.z;
    dd.abs() <= r
}

/// Möller-Trumbore. Returns `t` along `dir` (which need not be normalised)
/// for the front-or-back facing hit, or `None`.
pub fn ray_triangle(
    origin: Vec3f,
    dir: Vec3f,
    v0: Vec3f,
    v1: Vec3f,
    v2: Vec3f,
) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let p = Vec3f::cross(dir, e2);
    let det = e1.dot(p);
    if det.abs() < EPS {
        return None;
    }
    let inv = 1.0 / det;
    let t = origin - v0;
    let u = t.dot(p) * inv;
    if !(-1e-5..=1.000_01).contains(&u) {
        return None;
    }
    let q = Vec3f::cross(t, e1);
    let v = dir.dot(q) * inv;
    if v < -1e-5 || u + v > 1.000_01 {
        return None;
    }
    let d = e2.dot(q) * inv;
    if d > EPS {
        Some(d)
    } else {
        None
    }
}

/// Slab test. Returns `(t_near, t_far)` clipped to `[0, max]`, or `None`.
pub fn ray_aabb(origin: Vec3f, inv_dir: Vec3f, b: &Aabb, max: f32) -> Option<(f32, f32)> {
    let t0 = (b.min - origin) * inv_dir;
    let t1 = (b.max - origin) * inv_dir;
    let lo = Vec3f::min_componentwise(t0, t1);
    let hi = Vec3f::max_componentwise(t0, t1);
    let tn = lo.max_elem().max(0.0);
    let tf = hi.min_elem().min(max);
    if tn <= tf {
        Some((tn, tf))
    } else {
        None
    }
}

/// A yaw/pitch pair for a direction, Z up. Yaw is measured from +X toward +Y.
pub fn dir_to_yaw_pitch(d: Vec3f) -> (f32, f32) {
    let yaw = d.y.atan2(d.x);
    let pitch = d.z.atan2((d.x * d.x + d.y * d.y).sqrt());
    (yaw, pitch)
}

pub fn yaw_pitch_to_dir(yaw: f32, pitch: f32) -> Vec3f {
    let cp = pitch.cos();
    vec3(yaw.cos() * cp, yaw.sin() * cp, pitch.sin())
}

/// Shortest signed angle from `a` to `b`, in radians, wrapped to `[-pi, pi]`.
pub fn angle_delta(a: f32, b: f32) -> f32 {
    let mut d = (b - a) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

/// Angle between two directions in radians, numerically safe near 0 and pi.
pub fn angle_between(a: Vec3f, b: Vec3f) -> f32 {
    let a = a.normalize();
    let b = b.normalize();
    let d = a.dot(b).clamp(-1.0, 1.0);
    d.acos()
}

pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Quintic smootherstep: zero first *and* second derivative at both ends, so
/// an eased time warp adds no acceleration step at the joins.
pub fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tri_box_basics() {
        let c = vec3(0.0, 0.0, 0.0);
        let h = vec3(0.5, 0.5, 0.5);
        // triangle straight through the middle
        assert!(tri_box_overlap(
            c,
            h,
            vec3(-2.0, 0.0, 0.0),
            vec3(2.0, 0.0, 0.0),
            vec3(0.0, 2.0, 0.0)
        ));
        // far away
        assert!(!tri_box_overlap(
            c,
            h,
            vec3(5.0, 5.0, 5.0),
            vec3(6.0, 5.0, 5.0),
            vec3(5.0, 6.0, 5.0)
        ));
        // The case a cheap AABB-vs-AABB test gets wrong: the triangle's box
        // overlaps the voxel, but its *plane* clears the corner. Plane
        // x+y+z=3 is 1.73 from the origin; the box only reaches 0.87 that way.
        assert!(!tri_box_overlap(
            c,
            h,
            vec3(3.0, 0.0, 0.0),
            vec3(0.0, 3.0, 0.0),
            vec3(0.0, 0.0, 3.0)
        ));
        // ...and the same triangle pulled in until it does touch.
        assert!(tri_box_overlap(
            c,
            h,
            vec3(0.8, 0.0, 0.0),
            vec3(0.0, 0.8, 0.0),
            vec3(0.0, 0.0, 0.8)
        ));
    }

    #[test]
    fn ray_tri_hits_and_misses() {
        let (a, b, c) = (
            vec3(-1.0, -1.0, 0.0),
            vec3(1.0, -1.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        );
        let t = ray_triangle(vec3(0.0, 0.0, -3.0), vec3(0.0, 0.0, 1.0), a, b, c);
        assert!((t.unwrap() - 3.0).abs() < 1e-4);
        assert!(ray_triangle(vec3(5.0, 5.0, -3.0), vec3(0.0, 0.0, 1.0), a, b, c).is_none());
        // pointing away
        assert!(ray_triangle(vec3(0.0, 0.0, 3.0), vec3(0.0, 0.0, 1.0), a, b, c).is_none());
    }

    #[test]
    fn angles_wrap() {
        use std::f32::consts::PI;
        assert!((angle_delta(3.0, -3.0) - (PI * 2.0 - 6.0)).abs() < 1e-4);
        assert!(angle_delta(0.0, 0.1) > 0.0);
        assert!(angle_between(vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)) > 1.5);
    }
}
