// Port of box3d/src/math_internal.h (inline functions and internal math types).
// Functions declared in math_internal.h but implemented in other .c files live
// in the module of the matching .c file.

use crate::b3_validate;
use crate::math_functions::*;

pub const TWO_PI: f32 = 6.283185307;
pub const PI_OVER_TWO: f32 = 1.570796327;
pub const PI_OVER_FOUR: f32 = 0.785398163;
pub const SQRT3: f32 = 1.732050808;

// todo eliminate this
pub const BOUNDS3_EMPTY: AABB = AABB {
    lower_bound: vec3(f32::MAX, f32::MAX, f32::MAX),
    upper_bound: vec3(-f32::MAX, -f32::MAX, -f32::MAX),
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Matrix2 {
    pub cx: Vec2,
    pub cy: Vec2,
}

#[derive(Clone, Copy, Debug)]
pub struct Triangle {
    pub vertices: [Vec3; 3],
    pub i1: i32,
    pub i2: i32,
    pub i3: i32,
    pub flags: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct TrianglePoint {
    pub point: Vec3,
    pub feature: crate::types::TriangleFeature,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ShapeExtent {
    pub min_extent: f32,
    pub max_extent: Vec3,
}

#[inline]
pub fn align_up_8(x: usize) -> usize {
    (x + 7) & !7usize
}

// https://en.wikipedia.org/wiki/Floor_and_ceiling_functions
#[inline]
pub fn ceiling_int(numerator: i32, denominator: i32) -> i32 {
    b3_validate!(denominator > 0);
    (numerator + denominator - 1) / denominator
}

// Assumes denominator == 2^exponent
#[inline]
pub fn ceiling_pow2(numerator: i32, denominator: i32, exponent: i32) -> i32 {
    b3_validate!(exponent > 0 && (denominator == 1 << exponent));
    (numerator + denominator - 1) >> exponent
}

#[inline(always)]
pub fn dot2(v1: Vec2, v2: Vec2) -> f32 {
    v1.x * v2.x + v1.y * v2.y
}

#[inline(always)]
pub fn length2(v: Vec2) -> f32 {
    dot2(v, v).sqrt()
}

#[inline(always)]
pub fn length_squared2(v: Vec2) -> f32 {
    dot2(v, v)
}

#[inline(always)]
pub fn min_vec2(v1: Vec2, v2: Vec2) -> Vec2 {
    Vec2 { x: min_float(v1.x, v2.x), y: min_float(v1.y, v2.y) }
}

#[inline(always)]
pub fn max_vec2(v1: Vec2, v2: Vec2) -> Vec2 {
    Vec2 { x: max_float(v1.x, v2.x), y: max_float(v1.y, v2.y) }
}

#[inline(always)]
pub fn store(dst: &mut [f32], src: Vec3) {
    dst[0] = src.x;
    dst[1] = src.y;
    dst[2] = src.z;
}

#[inline]
pub fn clamp_length(v: Vec3, max_length: f32) -> Vec3 {
    let length_sq = length_squared(v);
    if length_sq <= max_length * max_length {
        return v;
    }

    let length = length_sq.sqrt();
    mul_sv(max_length / length, v)
}

// Assume v is a unit vector
pub fn arbitrary_perp(v: Vec3) -> Vec3 {
    // At least one component of a unit vector must be greater or equal to 0.57735.
    let p;
    if v.x < -0.5 || 0.5 < v.x {
        // x is non-zero and it should not go into the x component
        let a = 0.67;
        let b = -0.42;
        p = vec3(a * v.y + b * v.z, -a * v.x, -b * v.x);
    } else if v.y < -0.5 || 0.5 < v.y {
        // y is non-zero and it should not go into the y component
        let a = 0.67;
        let c = -0.42;
        p = vec3(a * v.y, -a * v.x + c * v.z, -c * v.y);
    } else {
        // This would trip if the input is not a unit vector
        b3_validate!(v.z < -0.5 || 0.5 < v.z);

        // z is non-zero and it should not go into the z component
        let a = 0.67;
        let b = -0.42;
        p = vec3(a * v.z, b * v.z, -a * v.x - b * v.y);
    }

    b3_validate!(length_squared(p) > 0.1);
    b3_validate!(abs_float(dot(p, v)) < 100.0 * f32::EPSILON);

    normalize(p)
}

pub fn quat_from_exponential_map(v: Vec3) -> Quat {
    // Exponential map (Grassia)
    let threshold = 0.018581361;

    let angle = length(v);
    if angle < threshold {
        // Taylor expansion
        return Quat {
            v: mul_sv(0.5 + angle * angle / 48.0, v),
            s: cos(0.5 * angle),
        };
    }

    make_quat_from_axis_angle(mul_sv(1.0 / angle, v), angle)
}

/// Integrate rotation from angular velocity
/// q2 = q1 + 0.5 * omega * q1
#[inline]
pub fn integrate_rotation(q1: Quat, delta_rotation: Vec3) -> Quat {
    // https://fgiesen.wordpress.com/2012/08/24/quaternion-differentiation/
    let mut qd = Quat { v: mul_sv(0.5, delta_rotation), s: 0.0 };
    qd = mul_quat(qd, q1);
    let q2 = Quat { v: add(q1.v, qd.v), s: qd.s + q1.s };
    normalize_quat(q2)
}

// Pseudo angular velocity from a quaternion target
// w = 2 * (target - q) * conj(q)
#[inline]
pub fn delta_quat_to_rotation(q: Quat, target: Quat) -> Vec3 {
    let mut s = q;
    if dot_quat(q, target) < 0.0 {
        // Correct polarity
        s = negate_quat(q);
    }

    let diff = Quat { v: sub(target.v, s.v), s: target.s - s.s };
    let product = mul_quat(diff, conjugate(s));
    mul_sv(2.0, product.v)
}

#[inline(always)]
pub fn scalar_triple_product(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let d = Vec3 {
        x: b.y * c.z - b.z * c.y,
        y: b.z * c.x - b.x * c.z,
        z: b.x * c.y - b.y * c.x,
    };
    a.x * d.x + a.y * d.y + a.z * d.z
}

// Get a value by index.
#[inline(always)]
pub fn get_by_index(v: Vec3, index: i32) -> f32 {
    b3_validate!(0 <= index && index < 3);
    let temp = [v.x, v.y, v.z];
    temp[index as usize]
}

#[inline(always)]
pub fn major_axis(v: Vec3) -> i32 {
    if v.x < v.y {
        if v.y < v.z {
            2
        } else {
            1
        }
    } else if v.x < v.z {
        2
    } else {
        0
    }
}

#[inline(always)]
pub fn min_element(v: Vec3) -> f32 {
    min_float(v.x, min_float(v.y, v.z))
}

#[inline(always)]
pub fn max_element(v: Vec3) -> f32 {
    max_float(v.x, max_float(v.y, v.z))
}

#[inline(always)]
pub fn max_element_index(v: Vec3) -> i32 {
    if v.x < v.y {
        if v.y < v.z {
            2
        } else {
            1
        }
    } else if v.x < v.z {
        2
    } else {
        0
    }
}

#[inline(always)]
pub fn add2(a: Vec2, b: Vec2) -> Vec2 {
    Vec2 { x: a.x + b.x, y: a.y + b.y }
}

#[inline(always)]
pub fn sub2(a: Vec2, b: Vec2) -> Vec2 {
    Vec2 { x: a.x - b.x, y: a.y - b.y }
}

#[inline(always)]
pub fn neg2(v: Vec2) -> Vec2 {
    Vec2 { x: -v.x, y: -v.y }
}

#[inline(always)]
pub fn mul_sv2(s: f32, v: Vec2) -> Vec2 {
    Vec2 { x: s * v.x, y: s * v.y }
}

// a + s * b
#[inline(always)]
pub fn mul_add2(a: Vec2, s: f32, b: Vec2) -> Vec2 {
    Vec2 { x: a.x + s * b.x, y: a.y + s * b.y }
}

// a - s * b
#[inline(always)]
pub fn mul_sub2(a: Vec2, s: f32, b: Vec2) -> Vec2 {
    Vec2 { x: a.x - s * b.x, y: a.y - s * b.y }
}

#[inline(always)]
pub fn cross2(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

#[inline(always)]
pub fn distance_squared2(a: Vec2, b: Vec2) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    dx * dx + dy * dy
}

#[inline(always)]
pub fn mul_mv2(m: Matrix2, a: Vec2) -> Vec2 {
    Vec2 {
        x: m.cx.x * a.x + m.cy.x * a.y,
        y: m.cx.y * a.x + m.cy.y * a.y,
    }
}

#[inline]
pub fn mul_mm2(m1: Matrix2, m2: Matrix2) -> Matrix2 {
    Matrix2 { cx: mul_mv2(m1, m2.cx), cy: mul_mv2(m1, m2.cy) }
}

#[inline(always)]
pub fn det2(m: Matrix2) -> f32 {
    m.cx.x * m.cy.y - m.cx.y * m.cy.x
}

#[inline]
pub fn invert2(m: Matrix2) -> Matrix2 {
    let det = det2(m);
    if abs_float(det) > 1000.0 * f32::MIN_POSITIVE {
        let inv_det = 1.0 / det;
        return Matrix2 {
            cx: vec2(inv_det * m.cy.y, -inv_det * m.cx.y),
            cy: vec2(-inv_det * m.cy.x, inv_det * m.cx.x),
        };
    }

    Matrix2 { cx: vec2(0.0, 0.0), cy: vec2(0.0, 0.0) }
}

// Assumes positive semi-definite
#[inline]
pub fn solve2(m: Matrix2, b: Vec2) -> Vec2 {
    let det = det2(m);
    if det > 1000.0 * f32::MIN_POSITIVE {
        let inv_det = 1.0 / det;
        return Vec2 {
            x: inv_det * m.cy.y * b.x - inv_det * m.cy.x * b.y,
            y: -inv_det * m.cx.y * b.x + inv_det * m.cx.x * b.y,
        };
    }

    vec2(0.0, 0.0)
}

// Convenience function: s * a + t * b + u * c
#[inline(always)]
pub fn blend3(s: f32, a: Vec3, t: f32, b: Vec3, u: f32, c: Vec3) -> Vec3 {
    Vec3 {
        x: s * a.x + t * b.x + u * c.x,
        y: s * a.y + t * b.y + u * c.y,
        z: s * a.z + t * b.z + u * c.z,
    }
}

#[inline(always)]
pub fn modified_cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3 {
        x: a.y * b.z + a.z * b.y,
        y: a.z * b.x + a.x * b.z,
        z: a.x * b.y + a.y * b.x,
    }
}

#[inline(always)]
pub fn make_diagonal_matrix(a: f32, b: f32, c: f32) -> Matrix3 {
    Matrix3 {
        cx: vec3(a, 0.0, 0.0),
        cy: vec3(0.0, b, 0.0),
        cz: vec3(0.0, 0.0, c),
    }
}

#[inline(always)]
pub fn skew(v: Vec3) -> Matrix3 {
    Matrix3 {
        cx: vec3(0.0, v.z, -v.y),
        cy: vec3(-v.z, 0.0, v.x),
        cz: vec3(v.y, -v.x, 0.0),
    }
}

#[inline]
pub fn normalize_plane(plane: Plane) -> Plane {
    let inv_length = 1.0 / length(plane.normal);
    Plane { normal: mul_sv(inv_length, plane.normal), offset: inv_length * plane.offset }
}

#[inline(always)]
pub fn make_plane_from_normal_and_point(normal: Vec3, point: Vec3) -> Plane {
    Plane { normal, offset: dot(normal, point) }
}

#[inline]
pub fn make_plane_from_points(point1: Vec3, point2: Vec3, point3: Vec3) -> Plane {
    let mut plane = Plane::default();
    plane.normal = cross(sub(point2, point1), sub(point3, point1));
    plane.normal = normalize(plane.normal);
    plane.offset = dot(plane.normal, point1);
    plane
}

#[inline]
pub fn make_normal_from_points(point1: Vec3, point2: Vec3, point3: Vec3) -> Vec3 {
    let normal = cross(sub(point2, point1), sub(point3, point1));
    normalize(normal)
}

// normal2 = q * normal1
// offset2 = dot(normal2, p) + offset1
#[inline]
pub fn transform_plane(transform: Transform, plane: Plane) -> Plane {
    let normal = rotate_vector(transform.q, plane.normal);
    Plane { normal, offset: plane.offset + dot(normal, transform.p) }
}

/// Signed separation of a point from a plane
#[inline(always)]
pub fn plane_separation(plane: Plane, point: Vec3) -> f32 {
    dot(plane.normal, point) - plane.offset
}

// Negative if p is below the triangle v1-v2-v3
#[inline]
pub fn signed_volume(v1: Vec3, v2: Vec3, v3: Vec3, p: Vec3) -> f32 {
    let e1 = sub(v2, v1);
    let e2 = sub(v3, v1);
    let n = cross(e1, e2);
    dot(n, sub(p, v1))
}

// todo eliminate this
#[inline]
pub fn is_within_segments(result: &SegmentDistanceResult) -> bool {
    (0.0 <= result.fraction1 && result.fraction1 <= 1.0)
        && (0.0 <= result.fraction2 && result.fraction2 <= 1.0)
}

#[inline]
pub fn rotate_inertia(q: Quat, central_inertia: Matrix3) -> Matrix3 {
    let rotation_matrix = make_matrix_from_quat(q);
    mul_mm(rotation_matrix, mul_mm(central_inertia, transpose(rotation_matrix)))
}

#[inline]
pub fn transform_inertia(transform: Transform, central_inertia: Matrix3, mass: f32) -> Matrix3 {
    let inertia = rotate_inertia(transform.q, central_inertia);
    add_mm(inertia, steiner(mass, transform.p))
}

// Add a point to an AABB.
#[inline]
pub fn aabb_add_point(a: AABB, point: Vec3) -> AABB {
    AABB {
        lower_bound: min(a.lower_bound, point),
        upper_bound: max(a.upper_bound, point),
    }
}
