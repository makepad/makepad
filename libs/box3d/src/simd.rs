// Port of box3d/src/simd.h + simd.c — b3V32 is a 3-lane float vector used by
// the mesh/height field BVH traversal and triangle tests.
//
// Path selection mirrors the C build (core.h):
// - x86_64: B3_SIMD_SSE2 (SSE2 is baseline for the target)
// - aarch64: the C intentionally uses the scalar path for b3V32
//   ("I don't expect the use case of b3V32 to benefit from Neon code")
// - feature "disable-simd" (C: BOX3D_DISABLE_SIMD) or other targets: scalar
//
// The V32 representation is opaque outside this module: all consumers go
// through load_vec3/load_v and the exported functions, so the type can be a
// raw __m128 on x86_64 without touching call sites.

// ---------------------------------------------------------------------------
// B3_SIMD_SSE2 path (x86_64)
// ---------------------------------------------------------------------------
#[cfg(all(target_arch = "x86_64", not(feature = "disable-simd")))]
mod imp {
    // SAFETY throughout this module: SSE2 is part of the x86_64 baseline, so
    // every intrinsic used here is unconditionally available on this target.
    use core::arch::x86_64::*;

    /// C b3V32 (SSE2): wide float holds 4 numbers; lane 3 is a zero passenger
    /// (b3LoadV loads exactly {x, y, z, 0}).
    #[derive(Clone, Copy, Debug)]
    pub struct V32(pub(crate) __m128);

    #[inline(always)]
    pub fn zero_v() -> V32 {
        unsafe { V32(_mm_setzero_ps()) }
    }

    #[inline(always)]
    pub fn half_v() -> V32 {
        splat_v(0.5)
    }

    #[inline(always)]
    pub fn one_v() -> V32 {
        splat_v(1.0)
    }

    #[inline(always)]
    pub fn add_v(a: V32, b: V32) -> V32 {
        unsafe { V32(_mm_add_ps(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn sub_v(a: V32, b: V32) -> V32 {
        unsafe { V32(_mm_sub_ps(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn mul_v(a: V32, b: V32) -> V32 {
        unsafe { V32(_mm_mul_ps(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn div_v(a: V32, b: V32) -> V32 {
        unsafe { V32(_mm_div_ps(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn neg_v(a: V32) -> V32 {
        unsafe { V32(_mm_sub_ps(_mm_setzero_ps(), a.0)) }
    }

    /// C b3LoadV loads exactly 12 bytes (movsd + movss + movelh) producing
    /// { src[0], src[1], src[2], 0.0 }. The materialization differs here
    /// (setr), the resulting vector value is identical.
    #[inline(always)]
    pub fn load_v(src: &[f32]) -> V32 {
        unsafe { V32(_mm_setr_ps(src[0], src[1], src[2], 0.0)) }
    }

    /// Load from a Vec3 (the common call site pattern `b3LoadV(&v.x)`).
    #[inline(always)]
    pub fn load_vec3(v: crate::math_functions::Vec3) -> V32 {
        unsafe { V32(_mm_setr_ps(v.x, v.y, v.z, 0.0)) }
    }

    #[inline(always)]
    pub fn get_x_v(a: V32) -> f32 {
        unsafe { _mm_cvtss_f32(a.0) }
    }

    #[inline(always)]
    pub fn get_y_v(a: V32) -> f32 {
        unsafe { _mm_cvtss_f32(_mm_shuffle_ps(a.0, a.0, 0b01_01_01_01)) }
    }

    #[inline(always)]
    pub fn get_z_v(a: V32) -> f32 {
        unsafe { _mm_cvtss_f32(_mm_shuffle_ps(a.0, a.0, 0b10_10_10_10)) }
    }

    /// C: union punning b3128.f[index].
    #[inline(always)]
    pub fn get_v(a: V32, index: i32) -> f32 {
        let mut f = [0.0f32; 4];
        unsafe { _mm_storeu_ps(f.as_mut_ptr(), a.0) };
        f[index as usize]
    }

    #[inline(always)]
    pub fn splat_v(x: f32) -> V32 {
        unsafe { V32(_mm_set1_ps(x)) }
    }

    #[inline(always)]
    pub fn abs_v(a: V32) -> V32 {
        // Abs( V ) = Max( -V, V )
        unsafe {
            let zero = _mm_setzero_ps();
            V32(_mm_max_ps(_mm_sub_ps(zero, a.0), a.0))
        }
    }

    #[inline(always)]
    pub fn min_v(a: V32, b: V32) -> V32 {
        unsafe { V32(_mm_min_ps(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn max_v(a: V32, b: V32) -> V32 {
        unsafe { V32(_mm_max_ps(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn cross_v(a: V32, b: V32) -> V32 {
        unsafe {
            // _MM_SHUFFLE(3, 0, 2, 1) and _MM_SHUFFLE(3, 1, 0, 2)
            let yz_x1 = _mm_shuffle_ps(a.0, a.0, 0b11_00_10_01);
            let zx_y1 = _mm_shuffle_ps(a.0, a.0, 0b11_01_00_10);
            let yz_x2 = _mm_shuffle_ps(b.0, b.0, 0b11_00_10_01);
            let zx_y2 = _mm_shuffle_ps(b.0, b.0, 0b11_01_00_10);

            V32(_mm_sub_ps(_mm_mul_ps(yz_x1, zx_y2), _mm_mul_ps(zx_y1, yz_x2)))
        }
    }

    #[inline(always)]
    pub fn modified_cross_v(a: V32, b: V32) -> V32 {
        unsafe {
            let yz_x1 = _mm_shuffle_ps(a.0, a.0, 0b11_00_10_01);
            let zx_y1 = _mm_shuffle_ps(a.0, a.0, 0b11_01_00_10);
            let yz_x2 = _mm_shuffle_ps(b.0, b.0, 0b11_00_10_01);
            let zx_y2 = _mm_shuffle_ps(b.0, b.0, 0b11_01_00_10);

            V32(_mm_add_ps(_mm_mul_ps(yz_x1, zx_y2), _mm_mul_ps(zx_y1, yz_x2)))
        }
    }

    #[inline(always)]
    pub fn any_less_3v(a: V32, b: V32) -> bool {
        unsafe {
            let v = _mm_cmplt_ps(a.0, b.0);
            (_mm_movemask_ps(v) & 0x07) != 0
        }
    }

    #[inline(always)]
    pub fn any_less_eq_3v(a: V32, b: V32) -> bool {
        unsafe {
            let v = _mm_cmple_ps(a.0, b.0);
            (_mm_movemask_ps(v) & 0x07) != 0
        }
    }

    #[inline(always)]
    pub fn any_greater_3v(a: V32, b: V32) -> bool {
        unsafe {
            let v = _mm_cmpgt_ps(a.0, b.0);
            (_mm_movemask_ps(v) & 0x07) != 0
        }
    }

    #[inline(always)]
    pub fn all_less_eq_3v(a: V32, b: V32) -> bool {
        unsafe {
            let v = _mm_cmple_ps(a.0, b.0);
            (_mm_movemask_ps(v) & 0x07) == 0x07
        }
    }

    // simd.c — SSE2 internals used by the shared triangle/ray tests

    #[inline(always)]
    pub(super) fn splat_x_v(v: V32) -> V32 {
        unsafe { V32(_mm_shuffle_ps(v.0, v.0, 0b00_00_00_00)) }
    }

    #[inline(always)]
    pub(super) fn splat_y_v(v: V32) -> V32 {
        unsafe { V32(_mm_shuffle_ps(v.0, v.0, 0b01_01_01_01)) }
    }

    #[inline(always)]
    pub(super) fn splat_z_v(v: V32) -> V32 {
        unsafe { V32(_mm_shuffle_ps(v.0, v.0, 0b10_10_10_10)) }
    }

    #[inline(always)]
    pub(super) fn any_greater_eq_3v(a: V32, b: V32) -> bool {
        unsafe {
            let v = _mm_cmpge_ps(a.0, b.0);
            (_mm_movemask_ps(v) & 0x07) != 0
        }
    }

    #[inline(always)]
    pub(super) fn dot_3v(a: V32, b: V32) -> V32 {
        unsafe {
            let m = _mm_mul_ps(a.0, b.0);
            let x = _mm_shuffle_ps(m, m, 0b00_00_00_00);
            let y = _mm_shuffle_ps(m, m, 0b01_01_01_01);
            let z = _mm_shuffle_ps(m, m, 0b10_10_10_10);

            V32(_mm_add_ps(_mm_add_ps(x, y), z))
        }
    }

    // B3_TRANSPOSE3 (SSE2 path)
    #[inline(always)]
    pub(super) fn transpose3(c1: &mut V32, c2: &mut V32, c3: &mut V32) {
        unsafe {
            let t1 = _mm_unpacklo_ps(c1.0, c2.0);
            let t2 = _mm_unpackhi_ps(c1.0, c2.0);
            // _MM_SHUFFLE(0, 0, 1, 0), _MM_SHUFFLE(1, 1, 3, 2), _MM_SHUFFLE(2, 2, 1, 0)
            let n1 = _mm_shuffle_ps(t1, c3.0, 0b00_00_01_00);
            let n2 = _mm_shuffle_ps(t1, c3.0, 0b01_01_11_10);
            let n3 = _mm_shuffle_ps(t2, c3.0, 0b10_10_01_00);
            c1.0 = n1;
            c2.0 = n2;
            c3.0 = n3;
        }
    }
}

// ---------------------------------------------------------------------------
// Scalar path (C: B3_SIMD_NONE, and the ARM b3V32 fallback)
// ---------------------------------------------------------------------------
#[cfg(not(all(target_arch = "x86_64", not(feature = "disable-simd"))))]
mod imp {
    /// scalar math (C b3V32)
    #[derive(Clone, Copy, Debug, Default)]
    pub struct V32 {
        pub x: f32,
        pub y: f32,
        pub z: f32,
    }

    pub const ZERO_V: V32 = V32 { x: 0.0, y: 0.0, z: 0.0 };
    pub const HALF_V: V32 = V32 { x: 0.5, y: 0.5, z: 0.5 };
    pub const ONE_V: V32 = V32 { x: 1.0, y: 1.0, z: 1.0 };

    #[inline(always)]
    pub fn zero_v() -> V32 {
        ZERO_V
    }

    #[inline(always)]
    pub fn half_v() -> V32 {
        HALF_V
    }

    #[inline(always)]
    pub fn one_v() -> V32 {
        ONE_V
    }

    #[inline(always)]
    pub fn add_v(a: V32, b: V32) -> V32 {
        V32 { x: a.x + b.x, y: a.y + b.y, z: a.z + b.z }
    }

    #[inline(always)]
    pub fn sub_v(a: V32, b: V32) -> V32 {
        V32 { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z }
    }

    #[inline(always)]
    pub fn mul_v(a: V32, b: V32) -> V32 {
        V32 { x: a.x * b.x, y: a.y * b.y, z: a.z * b.z }
    }

    #[inline(always)]
    pub fn div_v(a: V32, b: V32) -> V32 {
        V32 { x: a.x / b.x, y: a.y / b.y, z: a.z / b.z }
    }

    #[inline(always)]
    pub fn neg_v(a: V32) -> V32 {
        V32 { x: -a.x, y: -a.y, z: -a.z }
    }

    // Unaligned loads are much faster on recent hardware with little to no penalty
    #[inline(always)]
    pub fn load_v(src: &[f32]) -> V32 {
        V32 { x: src[0], y: src[1], z: src[2] }
    }

    /// Load from a Vec3 (the common call site pattern `b3LoadV(&v.x)`).
    #[inline(always)]
    pub fn load_vec3(v: crate::math_functions::Vec3) -> V32 {
        V32 { x: v.x, y: v.y, z: v.z }
    }

    #[inline(always)]
    pub fn get_x_v(a: V32) -> f32 {
        a.x
    }

    #[inline(always)]
    pub fn get_y_v(a: V32) -> f32 {
        a.y
    }

    #[inline(always)]
    pub fn get_z_v(a: V32) -> f32 {
        a.z
    }

    #[inline(always)]
    pub fn get_v(a: V32, index: i32) -> f32 {
        let f = [a.x, a.y, a.z];
        f[index as usize]
    }

    #[inline(always)]
    pub fn splat_v(x: f32) -> V32 {
        V32 { x, y: x, z: x }
    }

    #[inline(always)]
    pub fn abs_v(a: V32) -> V32 {
        V32 {
            x: if a.x < 0.0 { -a.x } else { a.x },
            y: if a.y < 0.0 { -a.y } else { a.y },
            z: if a.z < 0.0 { -a.z } else { a.z },
        }
    }

    #[inline(always)]
    pub fn min_v(a: V32, b: V32) -> V32 {
        V32 {
            x: if a.x < b.x { a.x } else { b.x },
            y: if a.y < b.y { a.y } else { b.y },
            z: if a.z < b.z { a.z } else { b.z },
        }
    }

    #[inline(always)]
    pub fn max_v(a: V32, b: V32) -> V32 {
        V32 {
            x: if a.x > b.x { a.x } else { b.x },
            y: if a.y > b.y { a.y } else { b.y },
            z: if a.z > b.z { a.z } else { b.z },
        }
    }

    #[inline(always)]
    pub fn cross_v(a: V32, b: V32) -> V32 {
        V32 {
            x: a.y * b.z - a.z * b.y,
            y: a.z * b.x - a.x * b.z,
            z: a.x * b.y - a.y * b.x,
        }
    }

    #[inline(always)]
    pub fn modified_cross_v(a: V32, b: V32) -> V32 {
        V32 {
            x: a.y * b.z + a.z * b.y,
            y: a.z * b.x + a.x * b.z,
            z: a.x * b.y + a.y * b.x,
        }
    }

    #[inline(always)]
    pub fn any_less_3v(a: V32, b: V32) -> bool {
        a.x < b.x || a.y < b.y || a.z < b.z
    }

    #[inline(always)]
    pub fn any_less_eq_3v(a: V32, b: V32) -> bool {
        a.x <= b.x || a.y <= b.y || a.z <= b.z
    }

    #[inline(always)]
    pub fn any_greater_3v(a: V32, b: V32) -> bool {
        a.x > b.x || a.y > b.y || a.z > b.z
    }

    #[inline(always)]
    pub fn all_less_eq_3v(a: V32, b: V32) -> bool {
        a.x <= b.x && a.y <= b.y && a.z <= b.z
    }

    // simd.c — scalar internals used by the shared triangle/ray tests

    #[inline(always)]
    pub(super) fn splat_x_v(a: V32) -> V32 {
        V32 { x: a.x, y: a.x, z: a.x }
    }

    #[inline(always)]
    pub(super) fn splat_y_v(a: V32) -> V32 {
        V32 { x: a.y, y: a.y, z: a.y }
    }

    #[inline(always)]
    pub(super) fn splat_z_v(a: V32) -> V32 {
        V32 { x: a.z, y: a.z, z: a.z }
    }

    #[inline(always)]
    pub(super) fn any_greater_eq_3v(a: V32, b: V32) -> bool {
        a.x >= b.x || a.y >= b.y || a.z >= b.z
    }

    #[inline(always)]
    pub(super) fn dot_3v(a: V32, b: V32) -> V32 {
        let d = a.x * b.x + a.y * b.y + a.z * b.z;
        V32 { x: d, y: d, z: d }
    }

    // B3_TRANSPOSE3 (scalar path)
    #[inline(always)]
    pub(super) fn transpose3(c1: &mut V32, c2: &mut V32, c3: &mut V32) {
        let temp1 = c1.y;
        let temp2 = c1.z;
        let temp3 = c2.z;

        c1.y = c2.x;
        c1.z = c3.x;
        c2.z = c3.y;

        c2.x = temp1;
        c3.x = temp2;
        c3.y = temp3;
    }
}

pub use imp::*;
use imp::{any_greater_eq_3v, dot_3v, splat_x_v, splat_y_v, splat_z_v, transpose3};

// ---------------------------------------------------------------------------
// Shared functions (identical in all C paths, written against the primitives)
// ---------------------------------------------------------------------------

#[inline(always)]
pub fn test_bounds_overlap(node_min1: V32, node_max1: V32, node_min2: V32, node_max2: V32) -> bool {
    let separation = max_v(sub_v(node_min2, node_max1), sub_v(node_min1, node_max2));
    all_less_eq_3v(separation, zero_v())
}

/// Test a ray for edge separation with an AABB (Gino, p80).
#[inline]
pub fn test_bounds_ray_overlap(node_min: V32, node_max: V32, ray_start: V32, ray_delta: V32) -> bool {
    // Setup node
    let node_center = mul_v(half_v(), add_v(node_min, node_max));
    let node_extent = sub_v(node_max, node_center);

    // Setup ray
    let ray_start = sub_v(ray_start, node_center);

    // SAT: Edge separation
    let edge_separation = sub_v(
        abs_v(cross_v(ray_delta, ray_start)),
        modified_cross_v(abs_v(ray_delta), node_extent),
    );
    all_less_eq_3v(edge_separation, zero_v())
}

pub fn test_bounds_triangle_overlap(node_center: V32, node_extent: V32, vertex1: V32, vertex2: V32, vertex3: V32) -> bool {
    let two = splat_v(2.0);

    // Setup triangle
    let vertex1 = sub_v(vertex1, node_center);
    let vertex2 = sub_v(vertex2, node_center);
    let vertex3 = sub_v(vertex3, node_center);

    // Face separation
    let triangle_min = min_v(vertex1, min_v(vertex2, vertex3));
    let triangle_max = max_v(vertex1, max_v(vertex2, vertex3));

    let separation1 = sub_v(triangle_min, node_extent);
    let separation2 = add_v(triangle_max, node_extent);

    let face_separation = max_v(separation1, neg_v(separation2));
    if any_greater_3v(face_separation, zero_v()) {
        return false;
    }

    // SAT: Face separation
    let edge1 = sub_v(vertex2, vertex1);
    let edge2 = sub_v(vertex3, vertex2);
    let edge3 = sub_v(vertex1, vertex3);

    let normal = cross_v(edge1, edge2);

    let triangle_separation = sub_v(abs_v(dot_3v(normal, vertex1)), dot_3v(abs_v(normal), node_extent));
    if any_greater_3v(triangle_separation, zero_v()) {
        return false;
    }

    // SAT: Edge separation
    let edge_separation1 = sub_v(
        sub_v(abs_v(cross_v(edge1, add_v(vertex1, vertex3))), abs_v(cross_v(edge1, edge3))),
        mul_v(two, modified_cross_v(abs_v(edge1), node_extent)),
    );
    if any_greater_3v(edge_separation1, zero_v()) {
        return false;
    }

    let edge_separation2 = sub_v(
        sub_v(abs_v(cross_v(edge2, add_v(vertex1, vertex2))), abs_v(cross_v(edge2, edge1))),
        mul_v(two, modified_cross_v(abs_v(edge2), node_extent)),
    );
    if any_greater_3v(edge_separation2, zero_v()) {
        return false;
    }

    let edge_separation3 = sub_v(
        sub_v(abs_v(cross_v(edge3, add_v(vertex2, vertex3))), abs_v(cross_v(edge3, edge2))),
        mul_v(two, modified_cross_v(abs_v(edge3), node_extent)),
    );
    if any_greater_3v(edge_separation3, zero_v()) {
        return false;
    }

    true
}

pub fn intersect_ray_triangle(ray_start: V32, ray_delta: V32, vertex1: V32, vertex2: V32, vertex3: V32) -> f32 {
    // Test if ray intersects this triangle sharing same calculations for each triangle
    {
        let edge1 = sub_v(vertex3, vertex2);
        let edge2 = sub_v(vertex1, vertex3);
        let edge3 = sub_v(vertex2, vertex1);

        let mid_point1 = mul_v(half_v(), add_v(vertex2, vertex3));
        let mid_point2 = mul_v(half_v(), add_v(vertex3, vertex1));
        let mid_point3 = mul_v(half_v(), add_v(vertex1, vertex2));

        let mut normal1 = cross_v(edge1, sub_v(mid_point1, ray_start));
        let mut normal2 = cross_v(edge2, sub_v(mid_point2, ray_start));
        let mut normal3 = cross_v(edge3, sub_v(mid_point3, ray_start));
        transpose3(&mut normal1, &mut normal2, &mut normal3);

        let ray_delta_x = splat_x_v(ray_delta);
        let ray_delta_y = splat_y_v(ray_delta);
        let ray_delta_z = splat_z_v(ray_delta);

        let volumes = add_v(add_v(mul_v(normal1, ray_delta_x), mul_v(normal2, ray_delta_y)), mul_v(normal3, ray_delta_z));
        if any_less_3v(volumes, zero_v()) {
            return 1.0;
        }
    }

    // Compute intersection with triangle plane
    let edge1 = sub_v(vertex2, vertex1);
    let edge2 = sub_v(vertex3, vertex1);
    let normal = cross_v(edge1, edge2);

    let denominator = dot_3v(normal, ray_delta);
    if any_greater_eq_3v(denominator, zero_v()) {
        return 1.0;
    }

    let mut lambda = div_v(dot_3v(normal, sub_v(vertex1, ray_start)), denominator);
    if any_less_eq_3v(lambda, zero_v()) {
        return 1.0;
    }

    lambda = min_v(lambda, one_v());
    get_x_v(lambda)
}
