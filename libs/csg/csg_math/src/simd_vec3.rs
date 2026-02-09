// SIMD-accelerated batch operations on Vec3d arrays.
//
// Uses Rust's portable SIMD (nightly `std::simd`) when the `nightly` feature
// is enabled, otherwise falls back to scalar loops.
//
// The main acceleration target is batched ray-triangle intersection used in
// point_inside_mesh (the dominant cost in CSG boolean classification).

use crate::vec3::Vec3d;

/// Batch ray-triangle intersection: test one ray against multiple triangles.
///
/// Returns the number of triangles that the ray intersects (forward hits only).
/// This is the hot inner loop of `point_inside_mesh` which dominates CSG boolean time.
///
/// `origin`: ray origin
/// `dir`: ray direction
/// `triangles`: slice of (v0, v1, v2) triangle vertices
#[cfg(all(feature = "nightly", target_arch = "aarch64"))]
pub fn batch_ray_triangle_count(
    origin: Vec3d,
    dir: Vec3d,
    triangles: &[(Vec3d, Vec3d, Vec3d)],
) -> u32 {
    use std::simd::prelude::*;

    let ox = f64x4::splat(origin.x);
    let oy = f64x4::splat(origin.y);
    let oz = f64x4::splat(origin.z);
    let dx = f64x4::splat(dir.x);
    let dy = f64x4::splat(dir.y);
    let dz = f64x4::splat(dir.z);
    let zero = f64x4::splat(0.0);
    let one = f64x4::splat(1.0);
    let eps = f64x4::splat(1e-12);
    let t_eps = f64x4::splat(1e-10);

    let mut total_crossings = 0u32;
    let chunks = triangles.chunks_exact(4);
    let remainder = chunks.remainder();

    for chunk in chunks {
        // Load 4 triangles into SoA layout
        let v0x = f64x4::from_array([chunk[0].0.x, chunk[1].0.x, chunk[2].0.x, chunk[3].0.x]);
        let v0y = f64x4::from_array([chunk[0].0.y, chunk[1].0.y, chunk[2].0.y, chunk[3].0.y]);
        let v0z = f64x4::from_array([chunk[0].0.z, chunk[1].0.z, chunk[2].0.z, chunk[3].0.z]);

        let v1x = f64x4::from_array([chunk[0].1.x, chunk[1].1.x, chunk[2].1.x, chunk[3].1.x]);
        let v1y = f64x4::from_array([chunk[0].1.y, chunk[1].1.y, chunk[2].1.y, chunk[3].1.y]);
        let v1z = f64x4::from_array([chunk[0].1.z, chunk[1].1.z, chunk[2].1.z, chunk[3].1.z]);

        let v2x = f64x4::from_array([chunk[0].2.x, chunk[1].2.x, chunk[2].2.x, chunk[3].2.x]);
        let v2y = f64x4::from_array([chunk[0].2.y, chunk[1].2.y, chunk[2].2.y, chunk[3].2.y]);
        let v2z = f64x4::from_array([chunk[0].2.z, chunk[1].2.z, chunk[2].2.z, chunk[3].2.z]);

        // edge1 = v1 - v0
        let e1x = v1x - v0x;
        let e1y = v1y - v0y;
        let e1z = v1z - v0z;

        // edge2 = v2 - v0
        let e2x = v2x - v0x;
        let e2y = v2y - v0y;
        let e2z = v2z - v0z;

        // h = dir cross edge2
        let hx = dy * e2z - dz * e2y;
        let hy = dz * e2x - dx * e2z;
        let hz = dx * e2y - dy * e2x;

        // a = edge1 dot h
        let a = e1x * hx + e1y * hy + e1z * hz;

        // Skip if |a| < eps (ray parallel to triangle)
        let valid = a.abs().simd_gt(eps);
        if !valid.any() {
            continue;
        }

        let f = one / a;

        // s = origin - v0
        let sx = ox - v0x;
        let sy = oy - v0y;
        let sz = oz - v0z;

        // u = f * (s dot h)
        let u = f * (sx * hx + sy * hy + sz * hz);
        let valid = valid & u.simd_ge(zero) & u.simd_le(one);
        if !valid.any() {
            continue;
        }

        // q = s cross edge1
        let qx = sy * e1z - sz * e1y;
        let qy = sz * e1x - sx * e1z;
        let qz = sx * e1y - sy * e1x;

        // v = f * (dir dot q)
        let v = f * (dx * qx + dy * qy + dz * qz);
        let valid = valid & v.simd_ge(zero) & (u + v).simd_le(one);
        if !valid.any() {
            continue;
        }

        // t = f * (edge2 dot q)
        let t = f * (e2x * qx + e2y * qy + e2z * qz);
        let valid = valid & t.simd_gt(t_eps);

        total_crossings += valid.to_bitmask().count_ones();
    }

    // Handle remainder with scalar
    for &(v0, v1, v2) in remainder {
        if ray_intersects_triangle_scalar(origin, dir, v0, v1, v2) {
            total_crossings += 1;
        }
    }

    total_crossings
}

#[cfg(all(feature = "nightly", target_arch = "x86_64"))]
pub fn batch_ray_triangle_count(
    origin: Vec3d,
    dir: Vec3d,
    triangles: &[(Vec3d, Vec3d, Vec3d)],
) -> u32 {
    use std::simd::prelude::*;

    let ox = f64x4::splat(origin.x);
    let oy = f64x4::splat(origin.y);
    let oz = f64x4::splat(origin.z);
    let dx = f64x4::splat(dir.x);
    let dy = f64x4::splat(dir.y);
    let dz = f64x4::splat(dir.z);
    let zero = f64x4::splat(0.0);
    let one = f64x4::splat(1.0);
    let eps = f64x4::splat(1e-12);
    let t_eps = f64x4::splat(1e-10);

    let mut total_crossings = 0u32;
    let chunks = triangles.chunks_exact(4);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let v0x = f64x4::from_array([chunk[0].0.x, chunk[1].0.x, chunk[2].0.x, chunk[3].0.x]);
        let v0y = f64x4::from_array([chunk[0].0.y, chunk[1].0.y, chunk[2].0.y, chunk[3].0.y]);
        let v0z = f64x4::from_array([chunk[0].0.z, chunk[1].0.z, chunk[2].0.z, chunk[3].0.z]);
        let v1x = f64x4::from_array([chunk[0].1.x, chunk[1].1.x, chunk[2].1.x, chunk[3].1.x]);
        let v1y = f64x4::from_array([chunk[0].1.y, chunk[1].1.y, chunk[2].1.y, chunk[3].1.y]);
        let v1z = f64x4::from_array([chunk[0].1.z, chunk[1].1.z, chunk[2].1.z, chunk[3].1.z]);
        let v2x = f64x4::from_array([chunk[0].2.x, chunk[1].2.x, chunk[2].2.x, chunk[3].2.x]);
        let v2y = f64x4::from_array([chunk[0].2.y, chunk[1].2.y, chunk[2].2.y, chunk[3].2.y]);
        let v2z = f64x4::from_array([chunk[0].2.z, chunk[1].2.z, chunk[2].2.z, chunk[3].2.z]);

        let e1x = v1x - v0x;
        let e1y = v1y - v0y;
        let e1z = v1z - v0z;
        let e2x = v2x - v0x;
        let e2y = v2y - v0y;
        let e2z = v2z - v0z;
        let hx = dy * e2z - dz * e2y;
        let hy = dz * e2x - dx * e2z;
        let hz = dx * e2y - dy * e2x;
        let a = e1x * hx + e1y * hy + e1z * hz;
        let valid = a.abs().simd_gt(eps);
        if !valid.any() {
            continue;
        }
        let f = one / a;
        let sx = ox - v0x;
        let sy = oy - v0y;
        let sz = oz - v0z;
        let u = f * (sx * hx + sy * hy + sz * hz);
        let valid = valid & u.simd_ge(zero) & u.simd_le(one);
        if !valid.any() {
            continue;
        }
        let qx = sy * e1z - sz * e1y;
        let qy = sz * e1x - sx * e1z;
        let qz = sx * e1y - sy * e1x;
        let v = f * (dx * qx + dy * qy + dz * qz);
        let valid = valid & v.simd_ge(zero) & (u + v).simd_le(one);
        if !valid.any() {
            continue;
        }
        let t = f * (e2x * qx + e2y * qy + e2z * qz);
        let valid = valid & t.simd_gt(t_eps);
        total_crossings += valid.to_bitmask().count_ones();
    }

    for &(v0, v1, v2) in remainder {
        if ray_intersects_triangle_scalar(origin, dir, v0, v1, v2) {
            total_crossings += 1;
        }
    }

    total_crossings
}

/// Scalar fallback when nightly SIMD is not available.
#[cfg(not(feature = "nightly"))]
pub fn batch_ray_triangle_count(
    origin: Vec3d,
    dir: Vec3d,
    triangles: &[(Vec3d, Vec3d, Vec3d)],
) -> u32 {
    let mut crossings = 0u32;
    for &(v0, v1, v2) in triangles {
        if ray_intersects_triangle_scalar(origin, dir, v0, v1, v2) {
            crossings += 1;
        }
    }
    crossings
}

/// Moller-Trumbore ray-triangle intersection (scalar).
#[inline]
fn ray_intersects_triangle_scalar(
    origin: Vec3d,
    dir: Vec3d,
    v0: Vec3d,
    v1: Vec3d,
    v2: Vec3d,
) -> bool {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = dir.cross(edge2);
    let a = edge1.dot(h);

    if a.abs() < 1e-12 {
        return false;
    }

    let f = 1.0 / a;
    let s = origin - v0;
    let u = f * s.dot(h);
    if u < 0.0 || u > 1.0 {
        return false;
    }

    let q = s.cross(edge1);
    let v = f * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return false;
    }

    let t = f * edge2.dot(q);
    t > 1e-10
}

/// Batch transform: apply a Mat4d to a slice of Vec3d points in place.
/// This is the hot loop for TriMesh::transform.
#[cfg(feature = "nightly")]
pub fn batch_transform_points(points: &mut [Vec3d], mat: &crate::mat4::Mat4d) {
    use std::simd::prelude::*;

    let m = &mat.v;
    let m00 = f64x4::splat(m[0]);
    let m01 = f64x4::splat(m[1]);
    let m02 = f64x4::splat(m[2]);
    let m04 = f64x4::splat(m[4]);
    let m05 = f64x4::splat(m[5]);
    let m06 = f64x4::splat(m[6]);
    let m08 = f64x4::splat(m[8]);
    let m09 = f64x4::splat(m[9]);
    let m10 = f64x4::splat(m[10]);
    let m12 = f64x4::splat(m[12]);
    let m13 = f64x4::splat(m[13]);
    let m14 = f64x4::splat(m[14]);

    let chunks = points.chunks_exact_mut(4);
    let remainder_offset = chunks.len() * 4;

    for chunk in chunks {
        let px = f64x4::from_array([chunk[0].x, chunk[1].x, chunk[2].x, chunk[3].x]);
        let py = f64x4::from_array([chunk[0].y, chunk[1].y, chunk[2].y, chunk[3].y]);
        let pz = f64x4::from_array([chunk[0].z, chunk[1].z, chunk[2].z, chunk[3].z]);

        let rx = m00 * px + m04 * py + m08 * pz + m12;
        let ry = m01 * px + m05 * py + m09 * pz + m13;
        let rz = m02 * px + m06 * py + m10 * pz + m14;

        let rx_arr = rx.to_array();
        let ry_arr = ry.to_array();
        let rz_arr = rz.to_array();
        for i in 0..4 {
            chunk[i] = Vec3d::new(rx_arr[i], ry_arr[i], rz_arr[i]);
        }
    }

    // Scalar remainder
    for p in &mut points[remainder_offset..] {
        *p = mat.transform_point(*p);
    }
}

#[cfg(not(feature = "nightly"))]
pub fn batch_transform_points(points: &mut [Vec3d], mat: &crate::mat4::Mat4d) {
    for p in points.iter_mut() {
        *p = mat.transform_point(*p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec3::dvec3;

    #[test]
    fn test_batch_ray_triangle_count_basic() {
        let origin = dvec3(0.0, 0.0, 0.0);
        let dir = dvec3(0.0, 0.0, 1.0);

        let hit_tri = (
            dvec3(-1.0, -1.0, 1.0),
            dvec3(1.0, -1.0, 1.0),
            dvec3(0.0, 1.0, 1.0),
        );
        let miss_tri = (
            dvec3(10.0, 10.0, 1.0),
            dvec3(11.0, 10.0, 1.0),
            dvec3(10.0, 11.0, 1.0),
        );

        // Single hit
        assert_eq!(batch_ray_triangle_count(origin, dir, &[hit_tri]), 1);
        // Single miss
        assert_eq!(batch_ray_triangle_count(origin, dir, &[miss_tri]), 0);
        // Mixed batch
        let tris = vec![hit_tri, miss_tri, hit_tri, miss_tri, hit_tri];
        assert_eq!(batch_ray_triangle_count(origin, dir, &tris), 3);
    }

    #[test]
    fn test_batch_ray_triangle_count_matches_scalar() {
        let origin = dvec3(0.1, 0.2, -5.0);
        let dir = dvec3(0.0, 0.0, 1.0);

        let tris: Vec<(Vec3d, Vec3d, Vec3d)> = (0..17)
            .map(|i| {
                let x = (i as f64) * 0.3 - 2.0;
                (
                    dvec3(x - 0.5, -0.5, 0.0),
                    dvec3(x + 0.5, -0.5, 0.0),
                    dvec3(x, 0.5, 0.0),
                )
            })
            .collect();

        let batch_result = batch_ray_triangle_count(origin, dir, &tris);

        let mut scalar_result = 0u32;
        for &(v0, v1, v2) in &tris {
            if ray_intersects_triangle_scalar(origin, dir, v0, v1, v2) {
                scalar_result += 1;
            }
        }

        assert_eq!(batch_result, scalar_result);
    }

    #[test]
    fn test_batch_transform_points() {
        use crate::mat4::Mat4d;

        let mat = Mat4d::translation(dvec3(10.0, 20.0, 30.0));
        let mut points = vec![
            dvec3(1.0, 2.0, 3.0),
            dvec3(4.0, 5.0, 6.0),
            dvec3(7.0, 8.0, 9.0),
            dvec3(0.0, 0.0, 0.0),
            dvec3(-1.0, -2.0, -3.0),
        ];
        let expected: Vec<Vec3d> = points.iter().map(|&p| mat.transform_point(p)).collect();

        batch_transform_points(&mut points, &mat);

        for (p, e) in points.iter().zip(expected.iter()) {
            assert!((p.x - e.x).abs() < 1e-12);
            assert!((p.y - e.y).abs() < 1e-12);
            assert!((p.z - e.z).abs() < 1e-12);
        }
    }
}
