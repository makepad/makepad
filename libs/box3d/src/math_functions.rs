// Port of box3d/include/box3d/math_functions.h + src/math_functions.c
// Vector math types and functions. Order of float operations is preserved
// from the C source for cross-platform determinism.

use crate::b3_assert;
use crate::b3_validate;
use crate::constants::huge;
use crate::math_internal::{make_diagonal_matrix, TrianglePoint};
use crate::types::{RayCastInput, TriangleFeature};

/// https://en.wikipedia.org/wiki/Pi
pub const PI: f32 = 3.14159265359;

/// Convenience constant to convert from degrees to radians.
pub const DEG_TO_RAD: f32 = 0.01745329251;

/// Convenience constant to convert from radians to degrees.
pub const RAD_TO_DEG: f32 = 57.2957795131;

/// Minimum scale used for scaling collision meshes, etc.
pub const MIN_SCALE: f32 = 0.01;

/// A 2D vector.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// A 3D vector.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Cosine and sine pair.
/// This uses a custom implementation designed for cross-platform determinism.
#[derive(Clone, Copy, Debug, Default)]
pub struct CosSin {
    /// cosine and sine
    pub cosine: f32,
    pub sine: f32,
}

/// A quaternion.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Quat {
    pub v: Vec3,
    pub s: f32,
}

/// A rigid transform.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Transform {
    pub p: Vec3,
    pub q: Quat,
}

/// In single precision mode these types are the same.
#[cfg(not(feature = "double-precision"))]
pub type Pos = Vec3;

/// In single precision mode these types are the same.
#[cfg(not(feature = "double-precision"))]
pub type WorldTransform = Transform;

/// A world position. Double precision in large world mode so coordinates stay
/// accurate far from the origin.
#[cfg(feature = "double-precision")]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A world transform with double precision translation and float quaternion rotation.
/// Rotation is frame local and never needs the extra range, the same split as Jolt's
/// DMat44.
#[cfg(feature = "double-precision")]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WorldTransform {
    pub p: Pos,
    pub q: Quat,
}

/// (b3Pos){x, y, z} literal helper. C struct literals compile in either
/// precision mode; this is the port's equivalent.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub const fn pos(x: f32, y: f32, z: f32) -> Pos {
    Pos { x, y, z }
}

/// (b3Pos){x, y, z} literal helper (double precision mode).
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn pos(x: f32, y: f32, z: f32) -> Pos {
    Pos { x: x as f64, y: y as f64, z: z as f64 }
}

/// A 3x3 matrix (columns).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Matrix3 {
    pub cx: Vec3,
    pub cy: Vec3,
    pub cz: Vec3,
}

/// Axis aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct AABB {
    pub lower_bound: Vec3,
    pub upper_bound: Vec3,
}

/// A plane.
/// separation = dot(normal, point) - offset
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Plane {
    pub normal: Vec3,
    pub offset: f32,
}

/// (b3Vec2){x, y} literal helper
#[inline(always)]
pub const fn vec2(x: f32, y: f32) -> Vec2 {
    Vec2 { x, y }
}

/// (b3Vec3){x, y, z} literal helper
#[inline(always)]
pub const fn vec3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

/// (b3Quat){v, s} literal helper
#[inline(always)]
pub const fn quat(v: Vec3, s: f32) -> Quat {
    Quat { v, s }
}

impl Vec3 {
    pub const ZERO: Vec3 = vec3(0.0, 0.0, 0.0);
    pub const ONE: Vec3 = vec3(1.0, 1.0, 1.0);
    pub const AXIS_X: Vec3 = vec3(1.0, 0.0, 0.0);
    pub const AXIS_Y: Vec3 = vec3(0.0, 1.0, 0.0);
    pub const AXIS_Z: Vec3 = vec3(0.0, 0.0, 1.0);
}

impl Quat {
    pub const IDENTITY: Quat = Quat { v: Vec3::ZERO, s: 1.0 };
}

impl Transform {
    pub const IDENTITY: Transform = Transform { p: Vec3::ZERO, q: Quat::IDENTITY };
}

impl Matrix3 {
    pub const ZERO: Matrix3 = Matrix3 { cx: Vec3::ZERO, cy: Vec3::ZERO, cz: Vec3::ZERO };
    pub const IDENTITY: Matrix3 = Matrix3 { cx: Vec3::AXIS_X, cy: Vec3::AXIS_Y, cz: Vec3::AXIS_Z };
}

// Valid in both modes.
#[cfg(not(feature = "double-precision"))]
pub const POS_ZERO: Pos = Vec3::ZERO;
#[cfg(not(feature = "double-precision"))]
pub const WORLD_TRANSFORM_IDENTITY: WorldTransform = Transform::IDENTITY;

#[cfg(feature = "double-precision")]
pub const POS_ZERO: Pos = Pos { x: 0.0, y: 0.0, z: 0.0 };
#[cfg(feature = "double-precision")]
pub const WORLD_TRANSFORM_IDENTITY: WorldTransform =
    WorldTransform { p: POS_ZERO, q: Quat::IDENTITY };

/// @return the minimum of two integers
#[inline(always)]
pub fn min_int(a: i32, b: i32) -> i32 {
    if a < b {
        a
    } else {
        b
    }
}

/// @return the maximum of two integers
#[inline(always)]
pub fn max_int(a: i32, b: i32) -> i32 {
    if a > b {
        a
    } else {
        b
    }
}

/// @return an integer clamped between a lower and upper bound
#[inline(always)]
pub fn clamp_int(a: i32, lower: i32, upper: i32) -> i32 {
    if a < lower {
        lower
    } else if upper < a {
        upper
    } else {
        a
    }
}

/// @return is this float valid (finite and not NaN)
pub fn is_valid_float(a: f32) -> bool {
    if a.is_nan() {
        return false;
    }
    if a.is_infinite() {
        return false;
    }
    true
}

/// @return the absolute value of a float
#[inline(always)]
pub fn abs_float(a: f32) -> f32 {
    if a < 0.0 {
        -a
    } else {
        a
    }
}

/// @return the minimum of two floats
#[inline(always)]
pub fn min_float(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

/// @return the maximum of two floats
#[inline(always)]
pub fn max_float(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

/// @return a float clamped between a lower and upper bound
#[inline(always)]
pub fn clamp_float(a: f32, lower: f32, upper: f32) -> f32 {
    if a < lower {
        lower
    } else if upper < a {
        upper
    } else {
        a
    }
}

/// Interpolate a scalar.
#[inline(always)]
pub fn lerp_float(a: f32, b: f32, alpha: f32) -> f32 {
    // FP contraction: matches C built with -ffp-contract=on (clang default).
    (1.0 - alpha).mul_add(a, alpha * b)
}

/// Compute an approximate arctangent in the range [-pi, pi]
/// This is hand coded for cross-platform determinism.
/// Accurate to around 0.0023 degrees.
pub fn atan2(y: f32, x: f32) -> f32 {
    // Added check for (0,0) to match atan2f and avoid NaN
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }

    let ax = abs_float(x);
    let ay = abs_float(y);
    let mx = max_float(ay, ax);
    let mn = min_float(ay, ax);
    let a = mn / mx;

    // Minimax polynomial approximation to atan(a) on [0,1]
    let s = a * a;
    let c = s * a;
    let q = s * s;
    let mut r = 0.024840285 * q + 0.18681418;
    let t = -0.094097948 * q - 0.33213072;
    r = r * s + t;
    r = r * c + a;

    // Map to full circle
    if ay > ax {
        r = 1.57079637 - r;
    }

    if x < 0.0 {
        r = 3.14159274 - r;
    }

    if y < 0.0 {
        r = -r;
    }

    r
}

/// Compute the cosine and sine of an angle in radians. Implemented
/// for cross-platform determinism.
/// https://en.wikipedia.org/wiki/Bh%C4%81skara_I%27s_sine_approximation_formula
pub fn compute_cos_sin(radians: f32) -> CosSin {
    let x = unwind_angle(radians);
    let pi2 = PI * PI;

    // cosine needs angle in [-pi/2, pi/2]
    let c;
    if x < -0.5 * PI {
        let y = x + PI;
        let y2 = y * y;
        c = -(pi2 - 4.0 * y2) / (pi2 + y2);
    } else if x > 0.5 * PI {
        let y = x - PI;
        let y2 = y * y;
        c = -(pi2 - 4.0 * y2) / (pi2 + y2);
    } else {
        let y2 = x * x;
        c = (pi2 - 4.0 * y2) / (pi2 + y2);
    }

    // sine needs angle in [0, pi]
    let s;
    if x < 0.0 {
        let y = x + PI;
        s = -16.0 * y * (PI - y) / (5.0 * pi2 - 4.0 * y * (PI - y));
    } else {
        s = 16.0 * x * (PI - x) / (5.0 * pi2 - 4.0 * x * (PI - x));
    }

    let mag = (s * s + c * c).sqrt();
    let inv_mag = if mag > 0.0 { 1.0 / mag } else { 0.0 };
    CosSin { cosine: c * inv_mag, sine: s * inv_mag }
}

/// @deprecated
#[inline]
pub fn sin(radians: f32) -> f32 {
    compute_cos_sin(radians).sine
}

/// @deprecated
#[inline]
pub fn cos(radians: f32) -> f32 {
    compute_cos_sin(radians).cosine
}

/// IEEE 754 remainder (like C remainderf). Rust has no stable f32 equivalent,
/// so compute in f64: exact for the angle magnitudes the engine uses.
#[inline]
fn remainder_f32(x: f32, y: f32) -> f32 {
    let xd = x as f64;
    let yd = y as f64;
    let q = xd / yd;
    // round half to even
    let n = if (q - q.floor()) == 0.5 {
        let f = q.floor();
        if (f as i64) % 2 == 0 {
            f
        } else {
            f + 1.0
        }
    } else {
        q.round()
    };
    (xd - n * yd) as f32
}

/// Convert any angle into the range [-pi, pi].
#[inline]
pub fn unwind_angle(radians: f32) -> f32 {
    // Assuming this is deterministic
    remainder_f32(radians, 2.0 * PI)
}

/// Vector addition.
#[inline(always)]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    vec3(a.x + b.x, a.y + b.y, a.z + b.z)
}

/// Vector subtraction.
#[inline(always)]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    vec3(a.x - b.x, a.y - b.y, a.z - b.z)
}

/// Vector component-wise multiplication.
#[inline(always)]
pub fn mul(a: Vec3, b: Vec3) -> Vec3 {
    vec3(a.x * b.x, a.y * b.y, a.z * b.z)
}

/// Vector negation.
#[inline(always)]
pub fn neg(a: Vec3) -> Vec3 {
    vec3(-a.x, -a.y, -a.z)
}

/// Vector dot product.
#[inline(always)]
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    // FP contraction: left-to-right sum, later terms fused (C -ffp-contract=on).
    a.z.mul_add(b.z, a.y.mul_add(b.y, a.x * b.x))
}

/// Vector length.
#[inline(always)]
pub fn length(v: Vec3) -> f32 {
    dot(v, v).sqrt()
}

/// Vector length squared.
#[inline(always)]
pub fn length_squared(a: Vec3) -> f32 {
    a.z.mul_add(a.z, a.y.mul_add(a.y, a.x * a.x))
}

/// Distance between two points.
#[inline(always)]
pub fn distance(a: Vec3, b: Vec3) -> f32 {
    let dv = vec3(b.x - a.x, b.y - a.y, b.z - a.z);
    length(dv)
}

/// Squared distance between two points.
#[inline(always)]
pub fn distance_squared(a: Vec3, b: Vec3) -> f32 {
    let dv = vec3(b.x - a.x, b.y - a.y, b.z - a.z);
    dv.z.mul_add(dv.z, dv.y.mul_add(dv.y, dv.x * dv.x))
}

/// Normalize a vector. Returns a zero vector if the input vector is very small.
#[inline]
pub fn normalize(a: Vec3) -> Vec3 {
    let length_squared = a.z.mul_add(a.z, a.y.mul_add(a.y, a.x * a.x));

    if length_squared > 1000.0 * f32::MIN_POSITIVE {
        let s = 1.0 / length_squared.sqrt();
        return vec3(s * a.x, s * a.y, s * a.z);
    }

    vec3(0.0, 0.0, 0.0)
}

/// Normalize a vector and return the length. Returns a zero vector
/// if the input is very small.
#[inline]
pub fn get_length_and_normalize(length_out: &mut f32, a: Vec3) -> Vec3 {
    *length_out = length(a);
    if *length_out < f32::EPSILON {
        return Vec3::ZERO;
    }

    let inv_length = 1.0 / *length_out;
    vec3(inv_length * a.x, inv_length * a.y, inv_length * a.z)
}

/// Get a unit vector that is perpendicular to the supplied vector.
#[inline]
pub fn perp(a: Vec3) -> Vec3 {
    // At least one component of a unit vector must be greater or equal to 0.57735.
    let p = if a.x < -0.5 || 0.5 < a.x {
        vec3(a.y, -a.x, 0.0)
    } else {
        vec3(0.0, a.z, -a.y)
    };

    normalize(p)
}

/// Is a vector normalized? In other words, does it have unit length?
#[inline]
pub fn is_normalized(a: Vec3) -> bool {
    let aa = dot(a, a);
    abs_float(1.0 - aa) < 100.0 * f32::EPSILON
}

/// a + s * b
#[inline(always)]
pub fn mul_add(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    vec3(s.mul_add(b.x, a.x), s.mul_add(b.y, a.y), s.mul_add(b.z, a.z))
}

/// a - s * b
#[inline(always)]
pub fn mul_sub(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    vec3((-s).mul_add(b.x, a.x), (-s).mul_add(b.y, a.y), (-s).mul_add(b.z, a.z))
}

/// s * a
#[inline(always)]
pub fn mul_sv(s: f32, a: Vec3) -> Vec3 {
    vec3(s * a.x, s * a.y, s * a.z)
}

/// https://en.wikipedia.org/wiki/Cross_product
#[inline(always)]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3 {
        x: a.y.mul_add(b.z, -(a.z * b.y)),
        y: a.z.mul_add(b.x, -(a.x * b.z)),
        z: a.x.mul_add(b.y, -(a.y * b.x)),
    }
}

/// Linearly interpolate between two vectors.
#[inline]
pub fn lerp(a: Vec3, b: Vec3, alpha: f32) -> Vec3 {
    b3_assert!(0.0 <= alpha && alpha <= 1.0);

    Vec3 {
        x: (1.0 - alpha).mul_add(a.x, alpha * b.x),
        y: (1.0 - alpha).mul_add(a.y, alpha * b.y),
        z: (1.0 - alpha).mul_add(a.z, alpha * b.z),
    }
}

/// Blend two vectors: s * a + t * b
#[inline(always)]
pub fn blend2(s: f32, a: Vec3, t: f32, b: Vec3) -> Vec3 {
    Vec3 {
        x: s.mul_add(a.x, t * b.x),
        y: s.mul_add(a.y, t * b.y),
        z: s.mul_add(a.z, t * b.z),
    }
}

/// Component-wise absolute value.
#[inline(always)]
pub fn abs(a: Vec3) -> Vec3 {
    vec3(abs_float(a.x), abs_float(a.y), abs_float(a.z))
}

/// Component-wise -1 or 1 (1 if zero).
#[inline(always)]
pub fn sign(a: Vec3) -> Vec3 {
    vec3(
        if a.x >= 0.0 { 1.0 } else { -1.0 },
        if a.y >= 0.0 { 1.0 } else { -1.0 },
        if a.z >= 0.0 { 1.0 } else { -1.0 },
    )
}

/// Component-wise minimum value.
#[inline(always)]
pub fn min(a: Vec3, b: Vec3) -> Vec3 {
    vec3(min_float(a.x, b.x), min_float(a.y, b.y), min_float(a.z, b.z))
}

/// Component-wise maximum value.
#[inline(always)]
pub fn max(a: Vec3, b: Vec3) -> Vec3 {
    vec3(max_float(a.x, b.x), max_float(a.y, b.y), max_float(a.z, b.z))
}

/// Component-wise clamped value.
#[inline(always)]
pub fn clamp(a: Vec3, lower: Vec3, upper: Vec3) -> Vec3 {
    Vec3 {
        x: clamp_float(a.x, lower.x, upper.x),
        y: clamp_float(a.y, lower.y, upper.y),
        z: clamp_float(a.z, lower.z, upper.z),
    }
}

/// Create a safe scaling value for scaling collision. This allows
/// negative scale, but keeps scale sufficiently far from zero.
#[inline]
pub fn safe_scale(a: Vec3) -> Vec3 {
    let abs_scale = abs(a);
    let min_scale = vec3(MIN_SCALE, MIN_SCALE, MIN_SCALE);
    mul(sign(a), max(abs_scale, min_scale))
}

/// Does the supplied quaternion have unit length?
#[inline]
pub fn is_normalized_quat(q: Quat) -> bool {
    let qq = q.v.x * q.v.x + q.v.y * q.v.y + q.v.z * q.v.z + q.s * q.s;
    1.0 - 20.0 * f32::EPSILON < qq && qq < 1.0 + 20.0 * f32::EPSILON
}

/// Rotate a vector.
#[inline(always)]
pub fn rotate_vector(q: Quat, v: Vec3) -> Vec3 {
    // v + 2 * cross(q.v, cross(q.v, v) + q.s * v)
    let t1 = cross(q.v, v);
    let t2 = mul_add(t1, q.s, v);
    let t3 = cross(q.v, t2);
    mul_add(v, 2.0, t3)
}

/// Inverse rotate a vector.
#[inline(always)]
pub fn inv_rotate_vector(q: Quat, v: Vec3) -> Vec3 {
    // v + 2 * cross(q.v, cross(q.v, v) - q.s * v)
    let t1 = cross(q.v, v);
    let t2 = mul_sub(t1, q.s, v);
    let t3 = cross(q.v, t2);
    mul_add(v, 2.0, t3)
}

/// Compute dot product of two quaternions. Useful for polarity tests.
#[inline(always)]
pub fn dot_quat(a: Quat, b: Quat) -> f32 {
    a.s.mul_add(b.s, a.v.z.mul_add(b.v.z, a.v.y.mul_add(b.v.y, a.v.x * b.v.x)))
}

/// Multiply two quaternions.
#[inline(always)]
pub fn mul_quat(q1: Quat, q2: Quat) -> Quat {
    let t1 = cross(q1.v, q2.v);
    let t2 = mul_add(t1, q1.s, q2.v);
    let t3 = mul_add(t2, q2.s, q1.v);
    Quat { v: t3, s: q1.s.mul_add(q2.s, -dot(q1.v, q2.v)) }
}

/// Compute a relative quaternion: inv(q1) * q2
#[inline(always)]
pub fn inv_mul_quat(q1: Quat, q2: Quat) -> Quat {
    let t1 = cross(q2.v, q1.v);
    let t2 = mul_add(t1, q1.s, q2.v);
    let t3 = mul_sub(t2, q2.s, q1.v);
    Quat { v: t3, s: q1.s.mul_add(q2.s, dot(q1.v, q2.v)) }
}

/// Quaternion conjugate (cheap inverse).
#[inline(always)]
pub fn conjugate(q: Quat) -> Quat {
    Quat { v: vec3(-q.v.x, -q.v.y, -q.v.z), s: q.s }
}

/// Component-wise quaternion negation.
#[inline(always)]
pub fn negate_quat(q: Quat) -> Quat {
    Quat { v: vec3(-q.v.x, -q.v.y, -q.v.z), s: -q.s }
}

/// Normalize a quaternion.
#[inline]
pub fn normalize_quat(q: Quat) -> Quat {
    let length_sq = dot_quat(q, q);
    if length_sq > 1000.0 * f32::MIN_POSITIVE {
        let s = 1.0 / length_sq.sqrt();
        return Quat { v: vec3(s * q.v.x, s * q.v.y, s * q.v.z), s: s * q.s };
    }

    Quat::IDENTITY
}

/// Make a quaternion that is equivalent to rotating around an axis by a specified angle.
#[inline]
pub fn make_quat_from_axis_angle(axis: Vec3, radians: f32) -> Quat {
    b3_assert!(is_normalized(axis));
    let cs = compute_cos_sin(0.5 * radians);
    Quat { v: vec3(cs.sine * axis.x, cs.sine * axis.y, cs.sine * axis.z), s: cs.cosine }
}

/// Get the axis and angle from a quaternion. Assumes the quaternion is normalized.
#[inline]
pub fn get_axis_angle(radians: &mut f32, q: Quat) -> Vec3 {
    let length = (q.v.x * q.v.x + q.v.y * q.v.y + q.v.z * q.v.z).sqrt();
    *radians = 2.0 * atan2(length, q.s);
    if length > 0.0 {
        let inv_length = 1.0 / length;
        return vec3(inv_length * q.v.x, inv_length * q.v.y, inv_length * q.v.z);
    }

    Vec3::ZERO
}

/// Get the angle for a quaternion in radians
#[inline]
pub fn get_quat_angle(q: Quat) -> f32 {
    let length = (q.v.x * q.v.x + q.v.y * q.v.y + q.v.z * q.v.z).sqrt();
    2.0 * atan2(length, q.s)
}

/// Extract a quaternion from a rotation matrix.
pub fn make_quat_from_matrix(m: &Matrix3) -> Quat {
    let c1 = m.cx;
    let c2 = m.cy;
    let c3 = m.cz;

    let mut q = Quat::default();

    let trace = m.cx.x + m.cy.y + m.cz.z;
    if trace >= 0.0 {
        q.v.x = c2.z - c3.y;
        q.v.y = c3.x - c1.z;
        q.v.z = c1.y - c2.x;
        q.s = trace + 1.0;
    } else if c1.x > c2.y && c1.x > c3.z {
        q.v.x = c1.x - c2.y - c3.z + 1.0;
        q.v.y = c2.x + c1.y;
        q.v.z = c3.x + c1.z;
        q.s = c2.z - c3.y;
    } else if c2.y > c3.z {
        q.v.x = c1.y + c2.x;
        q.v.y = c2.y - c3.z - c1.x + 1.0;
        q.v.z = c3.y + c2.z;
        q.s = c3.x - c1.z;
    } else {
        q.v.x = c1.z + c3.x;
        q.v.y = c2.z + c3.y;
        q.v.z = c3.z - c1.x - c2.y + 1.0;
        q.s = c1.y - c2.x;
    }

    // The algorithm is simplified and made more accurate by normalizing at the end
    normalize_quat(q)
}

/// Find a quaternion that rotates one vector to another.
pub fn compute_quat_between_unit_vectors(v1: Vec3, v2: Vec3) -> Quat {
    b3_assert!(is_normalized(v1));
    b3_assert!(is_normalized(v2));

    let mut out = Quat::default();

    let m = lerp(v1, v2, 0.5);
    let tolerance = 100.0 * f32::EPSILON;
    if length_squared(m) > tolerance * tolerance {
        out.v = cross(v1, m);
        out.s = dot(v1, m);
    } else {
        // Anti-parallel: Use a perpendicular vector
        if abs_float(v1.x) > 0.5 {
            out.v.x = v1.y;
            out.v.y = -v1.x;
            out.v.z = 0.0;
        } else {
            out.v.x = 0.0;
            out.v.y = v1.z;
            out.v.z = -v1.y;
        }

        out.s = 0.0;
    }

    // The algorithm is simplified and made more accurate by normalizing at the end
    normalize_quat(out)
}

/// Twist angle around the z-axis, used for twist limit and revolute angle limit
#[inline]
pub fn get_twist_angle(q: Quat) -> f32 {
    // Account for polarity to keep the twist angle in range.
    let mut twist = if q.s < 0.0 { atan2(-q.v.z, -q.s) } else { atan2(q.v.z, q.s) };
    twist *= 2.0;
    b3_assert!(-PI <= twist && twist <= PI);
    twist
}

/// Swing angle used for cone limit
#[inline]
pub fn get_swing_angle(q: Quat) -> f32 {
    // Polarity should not matter because all terms are squared.
    let x = (q.v.z * q.v.z + q.s * q.s).sqrt();
    let y = (q.v.x * q.v.x + q.v.y * q.v.y).sqrt();
    let swing = 2.0 * atan2(y, x);
    b3_assert!(0.0 <= swing && swing <= PI);
    swing
}

/// Linearly interpolate and normalize between two quaternions
#[inline]
pub fn nlerp(q1: Quat, q2: Quat, alpha: f32) -> Quat {
    b3_validate!(0.0 <= alpha && alpha <= 1.0);
    let mut q1 = q1;
    if dot_quat(q1, q2) < 0.0 {
        q1 = Quat { v: vec3(-q1.v.x, -q1.v.y, -q1.v.z), s: -q1.s };
    }

    let q = Quat {
        v: lerp(q1.v, q2.v, alpha),
        s: (1.0 - alpha) * q1.s + alpha * q2.s,
    };

    normalize_quat(q)
}

/// Multiply two transforms.
#[inline]
pub fn mul_transforms(a: Transform, b: Transform) -> Transform {
    Transform {
        p: add(rotate_vector(a.q, b.p), a.p),
        q: mul_quat(a.q, b.q),
    }
}

/// Creates a transform that converts a local point in frame B to a local point in frame A.
#[inline(always)]
pub fn inv_mul_transforms(a: Transform, b: Transform) -> Transform {
    Transform {
        p: inv_rotate_vector(a.q, sub(b.p, a.p)),
        q: inv_mul_quat(a.q, b.q),
    }
}

/// Get the inverse of a transform.
#[inline]
pub fn invert_transform(t: Transform) -> Transform {
    Transform {
        p: inv_rotate_vector(t.q, neg(t.p)),
        q: conjugate(t.q),
    }
}

/// Transform a point.
#[inline(always)]
pub fn transform_point(t: Transform, v: Vec3) -> Vec3 {
    let rv = rotate_vector(t.q, v);
    add(rv, t.p)
}

/// Inverse transform a point.
#[inline(always)]
pub fn inv_transform_point(t: Transform, v: Vec3) -> Vec3 {
    inv_rotate_vector(t.q, sub(v, t.p))
}

// World position boundary. These cross between the double precision world space at the
// public boundary and the float interior. In single precision mode these are mostly no-ops.

/// Convert a vector to a world position.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn to_pos(v: Vec3) -> Pos {
    v
}

/// Convert a vector to a world position.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn to_pos(v: Vec3) -> Pos {
    Pos { x: v.x as f64, y: v.y as f64, z: v.z as f64 }
}

/// Lossy conversion of a world position to a float vector.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn to_vec3(p: Pos) -> Vec3 {
    p
}

/// Lossy conversion of a world position to a float vector.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn to_vec3(p: Pos) -> Vec3 {
    vec3(p.x as f32, p.y as f32, p.z as f32)
}

/// Narrow a world coordinate to float, rounding toward negative infinity.
/// With large world mode off this is a plain conversion.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn round_down_float(x: f32) -> f32 {
    x
}

/// Narrow a world coordinate to float, rounding toward negative infinity. Use with
/// round_up_float to build a conservative float box that always contains the double bounds,
/// where plain rounding far from the origin could clip. next_down is an exact IEEE operation
/// (C: nextafterf), so this is cross-platform deterministic.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn round_down_float(x: f64) -> f32 {
    let f = x as f32;
    if (f as f64) > x {
        f.next_down()
    } else {
        f
    }
}

/// Narrow a world coordinate to float, rounding toward positive infinity.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn round_up_float(x: f32) -> f32 {
    x
}

/// Narrow a world coordinate to float, rounding toward positive infinity.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn round_up_float(x: f64) -> f32 {
    let f = x as f32;
    if (f as f64) < x {
        f.next_up()
    } else {
        f
    }
}

/// a - b, demoted to float. The primary precision boundary operation.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn sub_pos(a: Pos, b: Pos) -> Vec3 {
    vec3(a.x - b.x, a.y - b.y, a.z - b.z)
}

/// a - b, demoted to float. The primary precision boundary operation.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn sub_pos(a: Pos, b: Pos) -> Vec3 {
    vec3((a.x - b.x) as f32, (a.y - b.y) as f32, (a.z - b.z) as f32)
}

/// p + d
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn offset_pos(p: Pos, d: Vec3) -> Pos {
    vec3(p.x + d.x, p.y + d.y, p.z + d.z)
}

/// p + d
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn offset_pos(p: Pos, d: Vec3) -> Pos {
    Pos { x: p.x + d.x as f64, y: p.y + d.y as f64, z: p.z + d.z as f64 }
}

/// World position interpolation for sweeps and sampling.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn lerp_position(a: Pos, b: Pos, t: f32) -> Pos {
    Vec3 {
        x: (1.0 - t).mul_add(a.x, t * b.x),
        y: (1.0 - t).mul_add(a.y, t * b.y),
        z: (1.0 - t).mul_add(a.z, t * b.z),
    }
}

/// World position interpolation for sweeps and sampling.
/// C: the float blend factors promote to double per component.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn lerp_position(a: Pos, b: Pos, t: f32) -> Pos {
    Pos {
        x: (((1.0 - t) as f64)).mul_add(a.x, (t as f64) * b.x),
        y: (((1.0 - t) as f64)).mul_add(a.y, (t as f64) * b.y),
        z: (((1.0 - t) as f64)).mul_add(a.z, (t as f64) * b.z),
    }
}

/// Transform a local point to a world position.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn transform_world_point(t: WorldTransform, p: Vec3) -> Pos {
    let r = rotate_vector(t.q, p);
    vec3(t.p.x + r.x, t.p.y + r.y, t.p.z + r.z)
}

/// Transform a local point to a world position. Rotation in float, translation in double.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn transform_world_point(t: WorldTransform, p: Vec3) -> Pos {
    let r = rotate_vector(t.q, p);
    Pos { x: t.p.x + r.x as f64, y: t.p.y + r.y as f64, z: t.p.z + r.z as f64 }
}

/// Transform a world position to a local point.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn inv_transform_world_point(t: WorldTransform, p: Pos) -> Vec3 {
    let d = vec3(p.x - t.p.x, p.y - t.p.y, p.z - t.p.z);
    inv_rotate_vector(t.q, d)
}

/// Transform a world position to a local point. One double subtraction, then float.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn inv_transform_world_point(t: WorldTransform, p: Pos) -> Vec3 {
    let d = vec3((p.x - t.p.x) as f32, (p.y - t.p.y) as f32, (p.z - t.p.z) as f32);
    inv_rotate_vector(t.q, d)
}

/// Relative transform of frame B in frame A. The narrow phase boundary.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn inv_mul_world_transforms(a: WorldTransform, b: WorldTransform) -> Transform {
    let q = inv_mul_quat(a.q, b.q);
    let d = vec3(b.p.x - a.p.x, b.p.y - a.p.y, b.p.z - a.p.z);
    Transform { p: inv_rotate_vector(a.q, d), q }
}

/// Relative transform of frame B in frame A. The narrow phase boundary.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn inv_mul_world_transforms(a: WorldTransform, b: WorldTransform) -> Transform {
    let q = inv_mul_quat(a.q, b.q);
    let d = vec3((b.p.x - a.p.x) as f32, (b.p.y - a.p.y) as f32, (b.p.z - a.p.z) as f32);
    Transform { p: inv_rotate_vector(a.q, d), q }
}

/// Compose a world transform with a local transform.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn mul_world_transforms(a: WorldTransform, b: Transform) -> WorldTransform {
    let q = mul_quat(a.q, b.q);
    let r = rotate_vector(a.q, b.p);
    Transform { p: vec3(a.p.x + r.x, a.p.y + r.y, a.p.z + r.z), q }
}

/// Compose a world transform with a local transform.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn mul_world_transforms(a: WorldTransform, b: Transform) -> WorldTransform {
    let q = mul_quat(a.q, b.q);
    let r = rotate_vector(a.q, b.p);
    WorldTransform {
        p: Pos { x: a.p.x + r.x as f64, y: a.p.y + r.y as f64, z: a.p.z + r.z as f64 },
        q,
    }
}

/// Shift a world transform into the frame of a base position.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn to_relative_transform(t: WorldTransform, base: Pos) -> Transform {
    Transform {
        p: vec3(t.p.x - base.x, t.p.y - base.y, t.p.z - base.z),
        q: t.q,
    }
}

/// Shift a world transform into the frame of a base position.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn to_relative_transform(t: WorldTransform, base: Pos) -> Transform {
    Transform {
        p: vec3(
            (t.p.x - base.x) as f32,
            (t.p.y - base.y) as f32,
            (t.p.z - base.z) as f32,
        ),
        q: t.q,
    }
}

/// Promote a float transform to a world transform. Lossless.
#[cfg(not(feature = "double-precision"))]
#[inline(always)]
pub fn make_world_transform(t: Transform) -> WorldTransform {
    t
}

/// Promote a float transform to a world transform. Lossless.
#[cfg(feature = "double-precision")]
#[inline(always)]
pub fn make_world_transform(t: Transform) -> WorldTransform {
    WorldTransform { p: to_pos(t.p), q: t.q }
}

/// Translate a local AABB by a world origin.
#[cfg(not(feature = "double-precision"))]
#[inline]
pub fn offset_aabb(local_box: AABB, origin: Pos) -> AABB {
    AABB {
        lower_bound: Vec3 {
            x: origin.x + local_box.lower_bound.x,
            y: origin.y + local_box.lower_bound.y,
            z: origin.z + local_box.lower_bound.z,
        },
        upper_bound: Vec3 {
            x: origin.x + local_box.upper_bound.x,
            y: origin.y + local_box.upper_bound.y,
            z: origin.z + local_box.upper_bound.z,
        },
    }
}

/// Translate a local AABB by a world origin, rounding outward so the float box always
/// contains the double box. Far from the origin a plain conversion could clip a shape
/// out of its own box.
#[cfg(feature = "double-precision")]
#[inline]
pub fn offset_aabb(local_box: AABB, origin: Pos) -> AABB {
    AABB {
        lower_bound: Vec3 {
            x: round_down_float(origin.x + local_box.lower_bound.x as f64),
            y: round_down_float(origin.y + local_box.lower_bound.y as f64),
            z: round_down_float(origin.z + local_box.lower_bound.z as f64),
        },
        upper_bound: Vec3 {
            x: round_up_float(origin.x + local_box.upper_bound.x as f64),
            y: round_up_float(origin.y + local_box.upper_bound.y as f64),
            z: round_up_float(origin.z + local_box.upper_bound.z as f64),
        },
    }
}

/// Compute the determinant of a 3-by-3 matrix.
#[inline(always)]
pub fn det(m: Matrix3) -> f32 {
    dot(m.cx, cross(m.cy, m.cz))
}

/// Multiply a matrix times a column vector.
#[inline(always)]
pub fn mul_mv(m: Matrix3, a: Vec3) -> Vec3 {
    Vec3 {
        x: m.cz.x.mul_add(a.z, m.cy.x.mul_add(a.y, m.cx.x * a.x)),
        y: m.cz.y.mul_add(a.z, m.cy.y.mul_add(a.y, m.cx.y * a.x)),
        z: m.cz.z.mul_add(a.z, m.cy.z.mul_add(a.y, m.cx.z * a.x)),
    }
}

/// Negate a matrix.
#[inline(always)]
pub fn negate_mat3(a: Matrix3) -> Matrix3 {
    Matrix3 {
        cx: vec3(-a.cx.x, -a.cx.y, -a.cx.z),
        cy: vec3(-a.cy.x, -a.cy.y, -a.cy.z),
        cz: vec3(-a.cz.x, -a.cz.y, -a.cz.z),
    }
}

/// Matrix addition: a + b
#[inline(always)]
pub fn add_mm(a: Matrix3, b: Matrix3) -> Matrix3 {
    Matrix3 {
        cx: vec3(a.cx.x + b.cx.x, a.cx.y + b.cx.y, a.cx.z + b.cx.z),
        cy: vec3(a.cy.x + b.cy.x, a.cy.y + b.cy.y, a.cy.z + b.cy.z),
        cz: vec3(a.cz.x + b.cz.x, a.cz.y + b.cz.y, a.cz.z + b.cz.z),
    }
}

/// Matrix subtraction: a - b
#[inline(always)]
pub fn sub_mm(a: Matrix3, b: Matrix3) -> Matrix3 {
    Matrix3 {
        cx: vec3(a.cx.x - b.cx.x, a.cx.y - b.cx.y, a.cx.z - b.cx.z),
        cy: vec3(a.cy.x - b.cy.x, a.cy.y - b.cy.y, a.cy.z - b.cy.z),
        cz: vec3(a.cz.x - b.cz.x, a.cz.y - b.cz.y, a.cz.z - b.cz.z),
    }
}

/// Multiply a matrix by a scalar, component-wise.
#[inline(always)]
pub fn mul_sm(s: f32, a: Matrix3) -> Matrix3 {
    Matrix3 {
        cx: vec3(s * a.cx.x, s * a.cx.y, s * a.cx.z),
        cy: vec3(s * a.cy.x, s * a.cy.y, s * a.cy.z),
        cz: vec3(s * a.cz.x, s * a.cz.y, s * a.cz.z),
    }
}

/// Matrix multiplication: a * b
#[inline]
pub fn mul_mm(a: Matrix3, b: Matrix3) -> Matrix3 {
    Matrix3 {
        cx: mul_mv(a, b.cx),
        cy: mul_mv(a, b.cy),
        cz: mul_mv(a, b.cz),
    }
}

/// Matrix transpose.
#[inline]
pub fn transpose(m: Matrix3) -> Matrix3 {
    Matrix3 {
        cx: vec3(m.cx.x, m.cy.x, m.cz.x),
        cy: vec3(m.cx.y, m.cy.y, m.cz.y),
        cz: vec3(m.cx.z, m.cy.z, m.cz.z),
    }
}

/// General matrix inverse.
#[inline]
pub fn invert_matrix(m: Matrix3) -> Matrix3 {
    let d = det(m);
    if abs_float(d) > 1000.0 * f32::MIN_POSITIVE {
        let inv_det = 1.0 / d;
        let out = Matrix3 {
            cx: mul_sv(inv_det, cross(m.cy, m.cz)),
            cy: mul_sv(inv_det, cross(m.cz, m.cx)),
            cz: mul_sv(inv_det, cross(m.cx, m.cy)),
        };

        return transpose(out);
    }

    Matrix3::ZERO
}

/// Solve a matrix equation: inv(m) * a
#[inline]
pub fn solve3(m: Matrix3, a: Vec3) -> Vec3 {
    let d = det(m);
    if abs_float(d) > 1000.0 * f32::MIN_POSITIVE {
        let inv_det = 1.0 / d;
        let s = Matrix3 {
            cx: cross(m.cy, m.cz),
            cy: cross(m.cz, m.cx),
            cz: cross(m.cx, m.cy),
        };

        return Vec3 {
            x: inv_det * dot(s.cx, a),
            y: inv_det * dot(s.cy, a),
            z: inv_det * dot(s.cz, a),
        };
    }

    Vec3::ZERO
}

/// Invert a matrix (transpose of the inverse).
#[inline]
pub fn invert_t(m: Matrix3) -> Matrix3 {
    let d = det(m);
    if abs_float(d) > 1000.0 * f32::MIN_POSITIVE {
        let inv_det = 1.0 / d;
        return Matrix3 {
            cx: mul_sv(inv_det, cross(m.cy, m.cz)),
            cy: mul_sv(inv_det, cross(m.cz, m.cx)),
            cz: mul_sv(inv_det, cross(m.cx, m.cy)),
        };
    }

    Matrix3::ZERO
}

/// Get the component-wise absolute value of a matrix.
#[inline]
pub fn abs_matrix3(m: Matrix3) -> Matrix3 {
    Matrix3 { cx: abs(m.cx), cy: abs(m.cy), cz: abs(m.cz) }
}

/// Make a matrix from a quaternion. This is useful if you need to
/// rotate many vectors.
#[inline(always)]
pub fn make_matrix_from_quat(q: Quat) -> Matrix3 {
    let xx = q.v.x * q.v.x;
    let yy = q.v.y * q.v.y;
    let zz = q.v.z * q.v.z;
    let xy = q.v.x * q.v.y;
    let xz = q.v.x * q.v.z;
    let xw = q.v.x * q.s;
    let yz = q.v.y * q.v.z;
    let yw = q.v.y * q.s;
    let zw = q.v.z * q.s;

    Matrix3 {
        cx: vec3((-2.0f32).mul_add(yy + zz, 1.0), 2.0 * (xy + zw), 2.0 * (xz - yw)),
        cy: vec3(2.0 * (xy - zw), (-2.0f32).mul_add(xx + zz, 1.0), 2.0 * (yz + xw)),
        cz: vec3(2.0 * (xz + yw), 2.0 * (yz - xw), (-2.0f32).mul_add(xx + yy, 1.0)),
    }
}

/// Get the inertia tensor of an offset point.
/// https://en.wikipedia.org/wiki/Parallel_axis_theorem
pub fn steiner(mass: f32, origin: Vec3) -> Matrix3 {
    // Usage: Io = Ic + Is and Ic = Io - Is
    let ixx = mass * (origin.y * origin.y + origin.z * origin.z);
    let iyy = mass * (origin.x * origin.x + origin.z * origin.z);
    let izz = mass * (origin.x * origin.x + origin.y * origin.y);
    let ixy = -mass * origin.x * origin.y;
    let ixz = -mass * origin.x * origin.z;
    let iyz = -mass * origin.y * origin.z;

    Matrix3 {
        cx: vec3(ixx, ixy, ixz),
        cy: vec3(ixy, iyy, iyz),
        cz: vec3(ixz, iyz, izz),
    }
}

/// Get the AABB of a point cloud.
#[inline]
pub fn make_aabb(points: &[Vec3], radius: f32) -> AABB {
    b3_assert!(!points.is_empty());
    let mut a = AABB { lower_bound: points[0], upper_bound: points[0] };
    for p in &points[1..] {
        a.lower_bound = min(a.lower_bound, *p);
        a.upper_bound = max(a.upper_bound, *p);
    }

    let r = vec3(radius, radius, radius);
    a.lower_bound = sub(a.lower_bound, r);
    a.upper_bound = add(a.upper_bound, r);

    a
}

/// Does a fully contain b?
#[inline]
pub fn aabb_contains(a: AABB, b: AABB) -> bool {
    if a.lower_bound.x > b.lower_bound.x || b.upper_bound.x > a.upper_bound.x {
        return false;
    }
    if a.lower_bound.y > b.lower_bound.y || b.upper_bound.y > a.upper_bound.y {
        return false;
    }
    if a.lower_bound.z > b.lower_bound.z || b.upper_bound.z > a.upper_bound.z {
        return false;
    }

    true
}

/// Get the surface area of an axis-aligned bounding box.
#[inline]
pub fn aabb_area(a: AABB) -> f32 {
    let delta = sub(a.upper_bound, a.lower_bound);
    2.0 * (delta.x * delta.y + delta.y * delta.z + delta.z * delta.x)
}

/// Get the center of an axis-aligned bounding box.
#[inline]
pub fn aabb_center(a: AABB) -> Vec3 {
    mul_sv(0.5, add(a.upper_bound, a.lower_bound))
}

/// Get the extents (half-widths) of an axis-aligned bounding box.
#[inline]
pub fn aabb_extents(a: AABB) -> Vec3 {
    mul_sv(0.5, sub(a.upper_bound, a.lower_bound))
}

/// Get the union of two axis-aligned bounding boxes.
#[inline]
pub fn aabb_union(a: AABB, b: AABB) -> AABB {
    AABB {
        lower_bound: min(a.lower_bound, b.lower_bound),
        upper_bound: max(a.upper_bound, b.upper_bound),
    }
}

/// Add uniform padding to an axis-aligned bounding box.
#[inline]
pub fn aabb_inflate(a: AABB, extension: f32) -> AABB {
    let radius = vec3(extension, extension, extension);
    AABB {
        lower_bound: sub(a.lower_bound, radius),
        upper_bound: add(a.upper_bound, radius),
    }
}

/// Do two axis-aligned boxes overlap?
#[inline]
pub fn aabb_overlaps(a: AABB, b: AABB) -> bool {
    // No intersection if separated along one axis
    if a.upper_bound.x < b.lower_bound.x || a.lower_bound.x > b.upper_bound.x {
        return false;
    }
    if a.upper_bound.y < b.lower_bound.y || a.lower_bound.y > b.upper_bound.y {
        return false;
    }
    if a.upper_bound.z < b.lower_bound.z || a.lower_bound.z > b.upper_bound.z {
        return false;
    }

    // Overlapping on all axis means bounds are intersecting
    true
}

/// Transform an axis-aligned bounding box. This can create a larger box
/// than if you recomputed the AABB of the original shape with the transform applied.
#[inline]
pub fn aabb_transform(transform: Transform, a: AABB) -> AABB {
    let center = transform_point(transform, aabb_center(a));
    let m = make_matrix_from_quat(transform.q);
    let extent = mul_mv(abs_matrix3(m), aabb_extents(a));
    AABB { lower_bound: sub(center, extent), upper_bound: add(center, extent) }
}

/// Get the closest point on an axis-aligned bounding box.
#[inline]
pub fn closest_point_to_aabb(point: Vec3, a: AABB) -> Vec3 {
    clamp(point, a.lower_bound, a.upper_bound)
}

/// The closest points between two segments or infinite lines.
#[derive(Clone, Copy, Debug, Default)]
pub struct SegmentDistanceResult {
    pub point1: Vec3,
    pub fraction1: f32,
    pub point2: Vec3,
    pub fraction2: f32,
}

/// Compute the closest points on two infinite lines.
pub fn line_distance(p1: Vec3, d1: Vec3, p2: Vec3, d2: Vec3) -> SegmentDistanceResult {
    let mut result = SegmentDistanceResult::default();

    // Solve A*x = b
    let a11 = dot(d1, d1);
    let a12 = -dot(d1, d2);
    let a21 = dot(d2, d1);
    let a22 = -dot(d2, d2);

    let w = sub(p1, p2);
    let b1 = -dot(d1, w);
    let b2 = -dot(d2, w);

    let d = a11 * a22 - a12 * a21;
    if d * d < 1000.0 * f32::MIN_POSITIVE {
        // Lines are parallel - project p2 onto line L1: x1 = p1 + s1 * d1
        let s1 = dot(sub(p2, p1), d1) / dot(d1, d1);
        let s2 = 0.0;

        result.point1 = mul_add(p1, s1, d1);
        result.fraction1 = s1;
        result.point2 = mul_add(p2, s2, d2);
        result.fraction2 = s2;

        return result;
    }

    let s1 = (a22 * b1 - a12 * b2) / d;
    let s2 = (a11 * b2 - a21 * b1) / d;

    result.point1 = mul_add(p1, s1, d1);
    result.fraction1 = s1;
    result.point2 = mul_add(p2, s2, d2);
    result.fraction2 = s2;
    result
}

/// Compute the closest points on two line segments.
pub fn segment_distance(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> SegmentDistanceResult {
    let mut result = SegmentDistanceResult::default();

    let d1 = sub(q1, p1);
    let d2 = sub(q2, p2);
    let r = sub(p1, p2);

    let a = dot(d1, d1);
    let b = dot(d1, d2);
    let c = dot(d1, r);
    let e = dot(d2, d2);
    let f = dot(d2, r);

    // Check if one of the segments degenerates into a point
    if a < 100.0 * f32::EPSILON && e < 100.0 * f32::EPSILON {
        // Both segments degenerate into points
        result.point1 = p1;
        result.fraction1 = 0.0;
        result.point2 = p2;
        result.fraction2 = 0.0;

        return result;
    }

    if a < 100.0 * f32::EPSILON {
        // First segment degenerates into a point
        let s2 = clamp_float(f / e, 0.0, 1.0);

        result.point1 = p1;
        result.fraction1 = 0.0;
        result.point2 = mul_add(p2, s2, d2);
        result.fraction2 = s2;

        return result;
    }

    if e < 100.0 * f32::EPSILON {
        // Second segment degenerates into a point
        let s1 = clamp_float(-c / a, 0.0, 1.0);

        result.point1 = mul_add(p1, s1, d1);
        result.fraction1 = s1;
        result.point2 = p2;
        result.fraction2 = 0.0;

        return result;
    }

    // Non-degenerate case
    let denom = a * e - b * b;
    let mut s1 = if denom > 1000.0 * f32::MIN_POSITIVE {
        clamp_float((b * f - c * e) / denom, 0.0, 1.0)
    } else {
        0.0
    };
    let mut s2 = (b * s1 + f) / e;

    // Clamp lambda2 and recompute lambda1 if necessary
    if s2 < 0.0 {
        s1 = clamp_float(-c / a, 0.0, 1.0);
        s2 = 0.0;
    } else if s2 > 1.0 {
        s1 = clamp_float((b - c) / a, 0.0, 1.0);
        s2 = 1.0;
    }

    result.point1 = mul_add(p1, s1, d1);
    result.fraction1 = s1;
    result.point2 = mul_add(p2, s2, d2);
    result.fraction2 = s2;

    result
}

/// Compute the closest point on the segment a-b to the target q.
pub fn point_to_segment_distance(a: Vec3, b: Vec3, q: Vec3) -> Vec3 {
    let ab = sub(b, a);
    let aq = sub(q, a);

    let mut alpha = dot(ab, aq);

    if alpha <= 0.0 {
        // q projects outside interval [a, b] on the side of a
        a
    } else {
        let denominator = dot(ab, ab);
        if alpha > denominator {
            // q projects outside interval [a, b] on the side of b
            b
        } else {
            // q projects inside interval [a, b]
            alpha /= denominator;
            mul_add(a, alpha, ab)
        }
    }
}

/// Closest point on a triangle with feature information.
pub fn closest_point_on_triangle(a: Vec3, b: Vec3, c: Vec3, q: Vec3) -> TrianglePoint {
    // Check if P lies in vertex region of A
    let ab = sub(b, a);
    let ac = sub(c, a);
    let aq = sub(q, a);

    let d1 = dot(ab, aq);
    let d2 = dot(ac, aq);
    if d1 <= 0.0 && d2 <= 0.0 {
        return TrianglePoint { point: a, feature: TriangleFeature::Vertex1 };
    }

    // Check if P lies in vertex region of B
    let bq = sub(q, b);

    let d3 = dot(ab, bq);
    let d4 = dot(ac, bq);
    if d3 > 0.0 && d4 <= d3 {
        return TrianglePoint { point: b, feature: TriangleFeature::Vertex2 };
    }

    // Check if P lies in edge region AB
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let t = d1 / (d1 - d3);
        return TrianglePoint { point: mul_add(a, t, ab), feature: TriangleFeature::Edge1 };
    }

    // Check if P lies in vertex region of C
    let cq = sub(q, c);

    let d5 = dot(ab, cq);
    let d6 = dot(ac, cq);
    if d6 >= 0.0 && d5 <= d6 {
        return TrianglePoint { point: c, feature: TriangleFeature::Vertex3 };
    }

    // Check if P lies in edge region AC
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let t = d2 / (d2 - d6);
        return TrianglePoint { point: mul_add(a, t, ac), feature: TriangleFeature::Edge3 };
    }

    // Check if P lies in edge region of BC
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 >= d3 && d5 >= d6 {
        let bc = sub(c, b);

        let t = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return TrianglePoint { point: mul_add(b, t, bc), feature: TriangleFeature::Edge2 };
    }

    // P inside face region ABC
    let t1 = vb / (va + vb + vc);
    let t2 = vc / (va + vb + vc);

    let mut p = mul_add(a, t1, ab);
    p = mul_add(p, t2, ac);
    TrianglePoint { point: p, feature: TriangleFeature::TriangleFace }
}

pub fn sphere_inertia(mass: f32, radius: f32) -> Matrix3 {
    let i = 0.4 * mass * radius * radius;
    make_diagonal_matrix(i, i, i)
}

pub fn cylinder_inertia(mass: f32, radius: f32, height: f32) -> Matrix3 {
    let ixx = mass * (3.0 * radius * radius + height * height) / 12.0;
    let iyy = 0.5 * mass * radius * radius;
    make_diagonal_matrix(ixx, iyy, ixx)
}

pub fn box_inertia(mass: f32, min: Vec3, max: Vec3) -> Matrix3 {
    let delta = sub(max, min);
    let ixx = mass * (delta.y * delta.y + delta.z * delta.z) / 12.0;
    let iyy = mass * (delta.x * delta.x + delta.z * delta.z) / 12.0;
    let izz = mass * (delta.x * delta.x + delta.y * delta.y) / 12.0;

    make_diagonal_matrix(ixx, iyy, izz)
}

/// Is this a valid vector? Not NaN or infinity.
pub fn is_valid_vec3(a: Vec3) -> bool {
    if a.x.is_nan() || a.y.is_nan() || a.z.is_nan() {
        return false;
    }

    if a.x.is_infinite() || a.y.is_infinite() || a.z.is_infinite() {
        return false;
    }

    true
}

/// Is this a valid quaternion? Not NaN or infinity. Is normalized.
pub fn is_valid_quat(a: Quat) -> bool {
    if a.v.x.is_nan() || a.v.y.is_nan() || a.v.z.is_nan() || a.s.is_nan() {
        return false;
    }

    if a.v.x.is_infinite() || a.v.y.is_infinite() || a.v.z.is_infinite() || a.s.is_infinite() {
        return false;
    }

    is_normalized_quat(a)
}

/// Is this a valid transform? Not NaN or infinity. Is normalized.
pub fn is_valid_transform(a: Transform) -> bool {
    is_valid_vec3(a.p) && is_valid_quat(a.q)
}

/// Is this a valid matrix? Not NaN or infinity.
pub fn is_valid_matrix3(a: Matrix3) -> bool {
    is_valid_vec3(a.cx) && is_valid_vec3(a.cy) && is_valid_vec3(a.cz)
}

/// Is this a valid bounding box? Not Nan or infinity. Upper bound greater than or equal to lower bound.
pub fn is_valid_aabb(a: AABB) -> bool {
    if !is_valid_vec3(a.lower_bound) {
        return false;
    }

    if !is_valid_vec3(a.upper_bound) {
        return false;
    }

    if a.lower_bound.x > a.upper_bound.x {
        return false;
    }

    if a.lower_bound.y > a.upper_bound.y {
        return false;
    }

    if a.lower_bound.z > a.upper_bound.z {
        return false;
    }

    true
}

/// Is this AABB reasonably close to the origin? See B3_HUGE.
pub fn is_bounded_aabb(a: AABB) -> bool {
    let h = huge();
    if a.lower_bound.x < -h || a.lower_bound.y < -h || a.lower_bound.z < -h {
        return false;
    }

    if a.upper_bound.x > h || a.upper_bound.y > h || a.upper_bound.z > h {
        return false;
    }

    true
}

/// Is this AABB valid and reasonable?
pub fn is_sane_aabb(a: AABB) -> bool {
    if !is_valid_aabb(a) {
        return false;
    }

    is_bounded_aabb(a)
}

/// Is this a valid plane? Normal is a unit vector. Not Nan or infinity.
pub fn is_valid_plane(a: Plane) -> bool {
    if !is_valid_vec3(a.normal) {
        return false;
    }

    if !is_normalized(a.normal) {
        return false;
    }

    is_valid_float(a.offset)
}

/// Is this a valid world position? Not NaN or infinity.
#[cfg(not(feature = "double-precision"))]
pub fn is_valid_position(p: Pos) -> bool {
    is_valid_vec3(p)
}

/// Is this a valid world position? Not NaN or infinity.
#[cfg(feature = "double-precision")]
pub fn is_valid_position(p: Pos) -> bool {
    if p.x.is_nan() || p.y.is_nan() || p.z.is_nan() {
        return false;
    }

    if p.x.is_infinite() || p.y.is_infinite() || p.z.is_infinite() {
        return false;
    }

    true
}

/// Is this a valid world transform? Not NaN or infinity. Rotation is normalized.
pub fn is_valid_world_transform(t: WorldTransform) -> bool {
    is_valid_position(t.p) && is_valid_quat(t.q)
}

/// Is this a valid ray?
pub fn is_valid_ray(input: &RayCastInput) -> bool {
    is_valid_vec3(input.origin)
        && is_valid_vec3(input.translation)
        && is_valid_float(input.max_fraction)
        && 0.0 <= input.max_fraction
        && input.max_fraction < huge()
}
