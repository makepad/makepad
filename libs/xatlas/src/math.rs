//! Math types and helpers from `vendor/xatlas.cpp` (lines ~436–1904).

use std::f32;

// Call C libm so we match xatlas.cpp compiled with clang -O2 on this platform.
// Rust's f32::{sqrt,sin,...} can differ by 1 ULP from `sqrtf`/`sinf`.
extern "C" {
    fn sqrtf(x: f32) -> f32;
    fn sinf(x: f32) -> f32;
    fn cosf(x: f32) -> f32;
    fn acosf(x: f32) -> f32;
    fn atan2f(y: f32, x: f32) -> f32;
    fn ceilf(x: f32) -> f32;
    fn floorf(x: f32) -> f32;
    fn fabsf(x: f32) -> f32;
}

#[inline]
pub fn c_sqrt(x: f32) -> f32 {
    unsafe { sqrtf(x) }
}
#[inline]
pub fn c_sin(x: f32) -> f32 {
    unsafe { sinf(x) }
}
#[inline]
pub fn c_cos(x: f32) -> f32 {
    unsafe { cosf(x) }
}
#[inline]
pub fn c_acos(x: f32) -> f32 {
    unsafe { acosf(x) }
}
#[inline]
pub fn c_atan2(y: f32, x: f32) -> f32 {
    unsafe { atan2f(y, x) }
}
#[inline]
pub fn c_ceil(x: f32) -> f32 {
    unsafe { ceilf(x) }
}
#[inline]
pub fn c_floor(x: f32) -> f32 {
    unsafe { floorf(x) }
}
#[inline]
pub fn c_fabs(x: f32) -> f32 {
    unsafe { fabsf(x) }
}

pub const PI: f32 = 3.14159265358979323846;
pub const PI2: f32 = 6.28318530717958647692;
pub const EPSILON: f32 = 0.0001;
pub const AREA_EPSILON: f32 = f32::EPSILON;
pub const NORMAL_EPSILON: f32 = 0.001;
pub const MERGE_CHARTS_MIN_NORMAL_DEVIATION: f32 = 0.5;

#[inline]
pub fn align_i(x: i32, a: i32) -> i32 {
    (x + a - 1) & !(a - 1)
}

#[inline]
pub fn clamp<T: PartialOrd>(x: T, a: T, b: T) -> T {
    if x < a {
        a
    } else if x > b {
        b
    } else {
        x
    }
}

// xatlas.cpp:494 — not IEEE; matches the vendor bit check exactly.
#[inline]
pub fn is_finite_f(f: f32) -> bool {
    let u = f.to_bits();
    u != 0x7F80_0000 && u != 0x7F80_0001
}

#[inline]
pub fn is_nan_f(f: f32) -> bool {
    f != f
}

// xatlas.cpp:507
#[inline]
pub fn equal_f(f0: f32, f1: f32, epsilon: f32) -> bool {
    (f0 - f1).abs() <= epsilon * f0.abs().max(f1.abs()).max(1.0)
}

#[inline]
pub fn ftoi_ceil(val: f32) -> i32 {
    c_ceil(val) as i32
}

#[inline]
pub fn is_zero(f: f32, epsilon: f32) -> bool {
    f.abs() <= epsilon
}

#[inline]
pub fn square(f: f32) -> f32 {
    f * f
}

// xatlas.cpp:535 — behaviour for 0 is undefined in C++.
#[inline]
pub fn next_power_of_two(mut x: u32) -> u32 {
    debug_assert!(x != 0);
    x -= 1;
    x |= x >> 1;
    x |= x >> 2;
    x |= x >> 4;
    x |= x >> 8;
    x |= x >> 16;
    x + 1
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[inline]
    pub const fn splat(f: f32) -> Self {
        Self { x: f, y: f }
    }

    #[inline]
    pub fn as_bytes(self) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0..4].copy_from_slice(&self.x.to_le_bytes());
        b[4..8].copy_from_slice(&self.y.to_le_bytes());
        b
    }
}

impl PartialEq for Vec2 {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

impl std::ops::Neg for Vec2 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl std::ops::AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, v: Self) {
        self.x += v.x;
        self.y += v.y;
    }
}

impl std::ops::SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, v: Self) {
        self.x -= v.x;
        self.y -= v.y;
    }
}

impl std::ops::MulAssign<f32> for Vec2 {
    #[inline]
    fn mul_assign(&mut self, s: f32) {
        self.x *= s;
        self.y *= s;
    }
}

impl std::ops::MulAssign for Vec2 {
    #[inline]
    fn mul_assign(&mut self, v: Self) {
        self.x *= v.x;
        self.y *= v.y;
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;
    #[inline]
    fn sub(self, b: Self) -> Self {
        Self::new(self.x - b.x, self.y - b.y)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;
    #[inline]
    fn add(self, b: Self) -> Self {
        Self::new(self.x + b.x, self.y + b.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;
    #[inline]
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

#[inline]
pub fn dot2(a: Vec2, b: Vec2) -> f32 {
    // clang -O2 -ffp-contract=on: fma(ay,by, ax*bx)
    a.y.mul_add(b.y, a.x * b.x)
}

#[inline]
pub fn length_squared2(v: Vec2) -> f32 {
    v.y.mul_add(v.y, v.x * v.x)
}

#[inline]
pub fn length2(v: Vec2) -> f32 {
    c_sqrt(length_squared2(v))
}

#[inline]
pub fn normalize2(v: Vec2) -> Vec2 {
    let l = length2(v);
    debug_assert!(l > 0.0);
    v * (1.0 / l)
}

#[inline]
pub fn normalize_safe2(v: Vec2, fallback: Vec2) -> Vec2 {
    let l = length2(v);
    if l > 0.0 {
        v * (1.0 / l)
    } else {
        fallback
    }
}

#[inline]
pub fn equal2(v1: Vec2, v2: Vec2, epsilon: f32) -> bool {
    equal_f(v1.x, v2.x, epsilon) && equal_f(v1.y, v2.y, epsilon)
}

#[inline]
pub fn min2(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x.min(b.x), a.y.min(b.y))
}

#[inline]
pub fn max2(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x.max(b.x), a.y.max(b.y))
}

#[inline]
pub fn is_finite2(v: Vec2) -> bool {
    is_finite_f(v.x) && is_finite_f(v.y)
}

// xatlas.cpp:670
#[inline]
pub fn triangle_area2(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    let v0 = a - c;
    let v1 = b - c;
    // clang -O2: `a*b - c*d` => fma(a,b, -(c*d))
    v0.x.mul_add(v1.y, -(v0.y * v1.x)) * 0.5
}

// xatlas.cpp:685
pub fn lines_intersect(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2, epsilon: f32) -> bool {
    let v0 = a2 - a1;
    let v1 = b2 - b1;
    let denom = -v1.x * v0.y + v0.x * v1.y;
    if equal_f(denom, 0.0, epsilon) {
        return false;
    }
    let s = (-v0.y * (a1.x - b1.x) + v0.x * (a1.y - b1.y)) / denom;
    if s > epsilon && s < 1.0 - epsilon {
        let t = (v1.x * (a1.y - b1.y) - v1.y * (a1.x - b1.x)) / denom;
        return t > epsilon && t < 1.0 - epsilon;
    }
    false
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec2i {
    pub x: i32,
    pub y: i32,
}

impl Vec2i {
    #[inline]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub const fn splat(f: f32) -> Self {
        Self { x: f, y: f, z: f }
    }

    #[inline]
    pub fn xy(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    #[inline]
    pub fn as_bytes(self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0..4].copy_from_slice(&self.x.to_le_bytes());
        b[4..8].copy_from_slice(&self.y.to_le_bytes());
        b[8..12].copy_from_slice(&self.z.to_le_bytes());
        b
    }

    #[inline]
    pub fn axis(self, i: u32) -> f32 {
        match i {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }
}

impl PartialEq for Vec3 {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y && self.z == other.z
    }
}

impl std::ops::Neg for Vec3 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl std::ops::AddAssign for Vec3 {
    #[inline]
    fn add_assign(&mut self, v: Self) {
        self.x += v.x;
        self.y += v.y;
        self.z += v.z;
    }
}

impl std::ops::SubAssign for Vec3 {
    #[inline]
    fn sub_assign(&mut self, v: Self) {
        self.x -= v.x;
        self.y -= v.y;
        self.z -= v.z;
    }
}

impl std::ops::MulAssign<f32> for Vec3 {
    #[inline]
    fn mul_assign(&mut self, s: f32) {
        self.x *= s;
        self.y *= s;
        self.z *= s;
    }
}

impl std::ops::DivAssign<f32> for Vec3 {
    #[inline]
    fn div_assign(&mut self, s: f32) {
        let is = 1.0 / s;
        self.x *= is;
        self.y *= is;
        self.z *= is;
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    #[inline]
    fn add(self, b: Self) -> Self {
        Self::new(self.x + b.x, self.y + b.y, self.z + b.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    #[inline]
    fn sub(self, b: Self) -> Self {
        Self::new(self.x - b.x, self.y - b.y, self.z - b.z)
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    #[inline]
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

impl std::ops::Div<f32> for Vec3 {
    type Output = Self;
    #[inline]
    fn div(self, s: f32) -> Self {
        self * (1.0 / s)
    }
}

#[inline]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    // clang -O2: `p*q - r*s` => fma(p,q, -(r*s))
    Vec3::new(
        a.y.mul_add(b.z, -(a.z * b.y)),
        a.z.mul_add(b.x, -(a.x * b.z)),
        a.x.mul_add(b.y, -(a.y * b.x)),
    )
}

#[inline]
pub fn dot3(a: Vec3, b: Vec3) -> f32 {
    a.z.mul_add(b.z, a.y.mul_add(b.y, a.x * b.x))
}

#[inline]
pub fn length_squared3(v: Vec3) -> f32 {
    v.z.mul_add(v.z, v.x.mul_add(v.x, v.y * v.y))
}

#[inline]
pub fn length3(v: Vec3) -> f32 {
    c_sqrt(length_squared3(v))
}

#[inline]
pub fn is_normalized3(v: Vec3, epsilon: f32) -> bool {
    equal_f(length3(v), 1.0, epsilon)
}

#[inline]
pub fn normalize3(v: Vec3) -> Vec3 {
    let l = length3(v);
    debug_assert!(l > 0.0);
    v * (1.0 / l)
}

#[inline]
pub fn normalize_safe3(v: Vec3, fallback: Vec3) -> Vec3 {
    let l = length3(v);
    if l > 0.0 {
        v * (1.0 / l)
    } else {
        fallback
    }
}

// xatlas.cpp:839 — axis-wise abs compare, not the robust equal_f.
#[inline]
pub fn equal3(v0: Vec3, v1: Vec3, epsilon: f32) -> bool {
    (v0.x - v1.x).abs() <= epsilon
        && (v0.y - v1.y).abs() <= epsilon
        && (v0.z - v1.z).abs() <= epsilon
}

#[inline]
pub fn min3v(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z))
}

#[inline]
pub fn max3v(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z))
}

#[derive(Clone, Copy, Debug)]
pub struct Extents2 {
    pub min: Vec2,
    pub max: Vec2,
}

impl Extents2 {
    pub fn from_points(p1: Vec2, p2: Vec2) -> Self {
        Self {
            min: min2(p1, p2),
            max: max2(p1, p2),
        }
    }

    pub fn reset(&mut self) {
        self.min = Vec2::splat(f32::MAX);
        self.max = Vec2::splat(-f32::MAX);
    }

    pub fn add(&mut self, p: Vec2) {
        self.min = min2(self.min, p);
        self.max = max2(self.max, p);
    }

    pub fn midpoint(self) -> Vec2 {
        Vec2::new(
            self.min.x + (self.max.x - self.min.x) * 0.5,
            self.min.y + (self.max.y - self.min.y) * 0.5,
        )
    }

    pub fn intersect(e1: Self, e2: Self) -> bool {
        e1.min.x <= e2.max.x
            && e1.max.x >= e2.min.x
            && e1.min.y <= e2.max.y
            && e1.max.y >= e2.min.y
    }
}

impl Default for Extents2 {
    fn default() -> Self {
        Self {
            min: Vec2::splat(f32::MAX),
            max: Vec2::splat(-f32::MAX),
        }
    }
}

// xatlas.cpp:897
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Aabb {
    fn default() -> Self {
        Self {
            min: Vec3::splat(f32::MAX),
            max: Vec3::splat(-f32::MAX),
        }
    }
}

impl Aabb {
    pub fn from_point_radius(p: Vec3, radius: f32) -> Self {
        let mut a = Self { min: p, max: p };
        if radius > 0.0 {
            a.expand(radius);
        }
        a
    }

    pub fn intersect(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    pub fn expand_to_include_point(&mut self, p: Vec3) {
        self.min = min3v(self.min, p);
        self.max = max3v(self.max, p);
    }

    pub fn expand_to_include_aabb(&mut self, aabb: Aabb) {
        self.min = min3v(self.min, aabb.min);
        self.max = max3v(self.max, aabb.max);
    }

    pub fn expand(&mut self, amount: f32) {
        *self = Self {
            min: self.min - Vec3::splat(amount),
            max: self.max + Vec3::splat(amount),
        };
    }

    pub fn centroid(self) -> Vec3 {
        self.min + (self.max - self.min) * 0.5
    }

    pub fn max_dimension(self) -> u32 {
        let extent = self.max - self.min;
        let mut result = 0u32;
        if extent.y > extent.x {
            result = 1;
            if extent.z > extent.y {
                result = 2;
            }
        } else if extent.z > extent.x {
            result = 2;
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Basis {
    pub tangent: Vec3,
    pub bitangent: Vec3,
    pub normal: Vec3,
}

impl Basis {
    // xatlas.cpp:1261
    pub fn compute_tangent(normal: Vec3) -> Vec3 {
        debug_assert!(is_normalized3(normal, NORMAL_EPSILON));
        let mut tangent = if normal.x.abs() < normal.y.abs() && normal.x.abs() < normal.z.abs() {
            Vec3::new(1.0, 0.0, 0.0)
        } else if normal.y.abs() < normal.z.abs() {
            Vec3::new(0.0, 1.0, 0.0)
        } else {
            Vec3::new(0.0, 0.0, 1.0)
        };
        tangent -= normal * dot3(normal, tangent);
        normalize3(tangent)
    }

    pub fn compute_bitangent(normal: Vec3, tangent: Vec3) -> Vec3 {
        cross(normal, tangent)
    }
}

pub struct Fit;

/// Covariance of `points` about `centroid`. clang -O2 contracts
/// `xx += r.x * r.x` to FMA; match that (xatlas.cpp:1633).
fn cov_clang_o2(points: &[Vec3], centroid: Vec3) -> (f32, f32, f32, f32, f32, f32) {
    let mut xx = 0.0;
    let mut xy = 0.0;
    let mut xz = 0.0;
    let mut yy = 0.0;
    let mut yz = 0.0;
    let mut zz = 0.0;
    for p in points {
        let r = *p - centroid;
        xx = r.x.mul_add(r.x, xx);
        xy = r.x.mul_add(r.y, xy);
        xz = r.x.mul_add(r.z, xz);
        yy = r.y.mul_add(r.y, yy);
        yz = r.y.mul_add(r.z, yz);
        zz = r.z.mul_add(r.z, zz);
    }
    (xx, xy, xz, yy, yz, zz)
}

impl Fit {
    pub fn compute_basis(points: &[Vec3], basis: &mut Basis) -> bool {
        if Self::compute_least_squares_normal(points, &mut basis.normal) {
            basis.tangent = Basis::compute_tangent(basis.normal);
            basis.bitangent = Basis::compute_bitangent(basis.normal, basis.tangent);
            return true;
        }
        Self::compute_eigen(points, basis)
    }

    fn compute_least_squares_normal(points: &[Vec3], normal: &mut Vec3) -> bool {
        debug_assert!(points.len() >= 3);
        if points.len() == 3 {
            *normal = normalize3(cross(points[2] - points[0], points[1] - points[0]));
            return true;
        }
        let inv_n = 1.0 / points.len() as f32;
        let mut centroid = Vec3::splat(0.0);
        for p in points {
            centroid += *p;
        }
        centroid *= inv_n;
        let (xx, xy, xz, yy, yz, zz) = cov_clang_o2(points, centroid);
        let det_x = yy.mul_add(zz, -(yz * yz));
        let det_y = xx.mul_add(zz, -(xz * xz));
        let det_z = xx.mul_add(yy, -(xy * xy));
        let det_max = det_x.max(det_y.max(det_z));
        if det_max <= 0.0 {
            return false;
        }
        let dir = if det_max == det_x {
            Vec3::new(
                det_x,
                xz.mul_add(yz, -(xy * zz)),
                xy.mul_add(yz, -(xz * yy)),
            )
        } else if det_max == det_y {
            Vec3::new(
                xz.mul_add(yz, -(xy * zz)),
                det_y,
                xy.mul_add(xz, -(yz * xx)),
            )
        } else {
            Vec3::new(
                xy.mul_add(yz, -(xz * yy)),
                xy.mul_add(xz, -(yz * xx)),
                det_z,
            )
        };
        let len = length3(dir);
        if is_zero(len, EPSILON) {
            return false;
        }
        *normal = dir * (1.0 / len);
        is_normalized3(*normal, NORMAL_EPSILON)
    }

    fn compute_eigen(points: &[Vec3], basis: &mut Basis) -> bool {
        let mut matrix = [0.0f32; 6];
        Self::compute_covariance(points, &mut matrix);
        if matrix[0] == 0.0 && matrix[3] == 0.0 && matrix[5] == 0.0 {
            return false;
        }
        let mut eigen_values = [0.0f32; 3];
        let mut eigen_vectors = [Vec3::splat(0.0); 3];
        if !Self::eigen_solve_symmetric3(&matrix, &mut eigen_values, &mut eigen_vectors) {
            return false;
        }
        basis.normal = normalize3(eigen_vectors[2]);
        basis.tangent = normalize3(eigen_vectors[0]);
        basis.bitangent = normalize3(eigen_vectors[1]);
        true
    }

    fn compute_centroid(points: &[Vec3]) -> Vec3 {
        let mut centroid = Vec3::splat(0.0);
        for p in points {
            centroid += *p;
        }
        centroid / points.len() as f32
    }

    fn compute_covariance(points: &[Vec3], covariance: &mut [f32; 6]) -> Vec3 {
        let centroid = Self::compute_centroid(points);
        *covariance = [0.0; 6];
        for p in points {
            let v = *p - centroid;
            covariance[0] = v.x.mul_add(v.x, covariance[0]);
            covariance[1] = v.x.mul_add(v.y, covariance[1]);
            covariance[2] = v.x.mul_add(v.z, covariance[2]);
            covariance[3] = v.y.mul_add(v.y, covariance[3]);
            covariance[4] = v.y.mul_add(v.z, covariance[4]);
            covariance[5] = v.z.mul_add(v.z, covariance[5]);
        }
        centroid
    }

    fn eigen_solve_symmetric3(
        matrix: &[f32; 6],
        eigen_values: &mut [f32; 3],
        eigen_vectors: &mut [Vec3; 3],
    ) -> bool {
        let mut subd = [0.0f32; 3];
        let mut diag = [0.0f32; 3];
        let mut work = [[0.0f32; 3]; 3];
        work[0][0] = matrix[0];
        work[0][1] = matrix[1];
        work[1][0] = matrix[1];
        work[0][2] = matrix[2];
        work[2][0] = matrix[2];
        work[1][1] = matrix[3];
        work[1][2] = matrix[4];
        work[2][1] = matrix[4];
        work[2][2] = matrix[5];
        Self::tridiagonal(&mut work, &mut diag, &mut subd);
        if !Self::ql_algorithm(&mut work, &mut diag, &mut subd) {
            for i in 0..3 {
                eigen_values[i] = 0.0;
                eigen_vectors[i] = Vec3::splat(0.0);
            }
            return false;
        }
        for i in 0..3 {
            eigen_values[i] = diag[i];
        }
        for i in 0..3 {
            for j in 0..3 {
                match i {
                    0 => eigen_vectors[j].x = work[i][j],
                    1 => eigen_vectors[j].y = work[i][j],
                    _ => eigen_vectors[j].z = work[i][j],
                }
            }
        }
        if eigen_values[2] > eigen_values[0] && eigen_values[2] > eigen_values[1] {
            eigen_values.swap(0, 2);
            eigen_vectors.swap(0, 2);
        }
        if eigen_values[1] > eigen_values[0] {
            eigen_values.swap(0, 1);
            eigen_vectors.swap(0, 1);
        }
        if eigen_values[2] > eigen_values[1] {
            eigen_values.swap(1, 2);
            eigen_vectors.swap(1, 2);
        }
        true
    }

    fn tridiagonal(mat: &mut [[f32; 3]; 3], diag: &mut [f32; 3], subd: &mut [f32; 3]) {
        let epsilon = 1e-08f32;
        let a = mat[0][0];
        let mut b = mat[0][1];
        let mut c = mat[0][2];
        let d = mat[1][1];
        let e = mat[1][2];
        let f = mat[2][2];
        diag[0] = a;
        subd[2] = 0.0;
        if c.abs() >= epsilon {
            let ell = c_sqrt(b * b + c * c);
            b /= ell;
            c /= ell;
            let q = 2.0 * b * e + c * (f - d);
            diag[1] = d + c * q;
            diag[2] = f - c * q;
            subd[0] = ell;
            subd[1] = e - b * q;
            mat[0][0] = 1.0;
            mat[0][1] = 0.0;
            mat[0][2] = 0.0;
            mat[1][0] = 0.0;
            mat[1][1] = b;
            mat[1][2] = c;
            mat[2][0] = 0.0;
            mat[2][1] = c;
            mat[2][2] = -b;
        } else {
            diag[1] = d;
            diag[2] = f;
            subd[0] = b;
            subd[1] = e;
            mat[0][0] = 1.0;
            mat[0][1] = 0.0;
            mat[0][2] = 0.0;
            mat[1][0] = 0.0;
            mat[1][1] = 1.0;
            mat[1][2] = 0.0;
            mat[2][0] = 0.0;
            mat[2][1] = 0.0;
            mat[2][2] = 1.0;
        }
    }

    fn ql_algorithm(mat: &mut [[f32; 3]; 3], diag: &mut [f32; 3], subd: &mut [f32; 3]) -> bool {
        const MAXITER: i32 = 32;
        for ell in 0..3 {
            let mut iter = 0;
            while iter < MAXITER {
                let mut m = ell;
                while m <= 1 {
                    let dd = diag[m].abs() + diag[m + 1].abs();
                    if subd[m].abs() + dd == dd {
                        break;
                    }
                    m += 1;
                }
                if m == ell {
                    break;
                }
                let mut g = (diag[ell + 1] - diag[ell]) / (2.0 * subd[ell]);
                let mut r = c_sqrt(g * g + 1.0);
                if g < 0.0 {
                    g = diag[m] - diag[ell] + subd[ell] / (g - r);
                } else {
                    g = diag[m] - diag[ell] + subd[ell] / (g + r);
                }
                let mut s = 1.0;
                let mut c = 1.0;
                let mut p = 0.0;
                for i in (ell..m).rev() {
                    let f = s * subd[i];
                    let b = c * subd[i];
                    if f.abs() >= g.abs() {
                        c = g / f;
                        r = c_sqrt(c * c + 1.0);
                        subd[i + 1] = f * r;
                        s = 1.0 / r;
                        c *= s;
                    } else {
                        s = f / g;
                        r = c_sqrt(s * s + 1.0);
                        subd[i + 1] = g * r;
                        c = 1.0 / r;
                        s *= c;
                    }
                    g = diag[i + 1] - p;
                    r = (diag[i] - g) * s + 2.0 * b * c;
                    p = s * r;
                    diag[i + 1] = g + p;
                    g = c * r - b;
                    for k in 0..3 {
                        let f = mat[k][i + 1];
                        mat[k][i + 1] = s * mat[k][i] + c * f;
                        mat[k][i] = c * mat[k][i] - s * f;
                    }
                }
                diag[ell] -= p;
                subd[ell] = g;
                subd[m] = 0.0;
                iter += 1;
            }
            if iter == MAXITER {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod basis_probe {
    use super::*;

    #[test]
    fn irregular_face0_basis_bits() {
        let points = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.2, 0.05, 0.02),
            Vec3::new(1.15, 0.9, -0.04),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.15, 0.9, -0.04),
            Vec3::new(0.08, 1.05, 0.03),
        ];
        let mut basis = Basis::default();
        assert!(Fit::compute_basis(&points, &mut basis));
        eprintln!(
            "normal {:08x} {:08x} {:08x}",
            basis.normal.x.to_bits(),
            basis.normal.y.to_bits(),
            basis.normal.z.to_bits()
        );
        eprintln!(
            "tangent {:08x} {:08x} {:08x}",
            basis.tangent.x.to_bits(),
            basis.tangent.y.to_bits(),
            basis.tangent.z.to_bits()
        );
        eprintln!(
            "bitangent {:08x} {:08x} {:08x}",
            basis.bitangent.x.to_bits(),
            basis.bitangent.y.to_bits(),
            basis.bitangent.z.to_bits()
        );
        for (i, p) in points.iter().enumerate() {
            let u = dot3(basis.tangent, *p);
            let v = dot3(basis.bitangent, *p);
            eprintln!("uv{i} {:08x} {:08x}", u.to_bits(), v.to_bits());
        }
    }
}
