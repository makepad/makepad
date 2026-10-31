//! Transforms, rotations, deterministic randomness and noise.
//!
//! Every animated value in a scene is a pure function of `t`, so the only
//! randomness is hashed from explicit seeds, and noise is lattice noise over
//! those hashes: the same document renders the same pixels on every run.

use makepad_math::*;

pub const DEG: f32 = std::f32::consts::PI / 180.0;

/// Column-major translation.
pub fn translation(t: [f32; 3]) -> Mat4f {
    let mut m = Mat4f::identity();
    m.v[12] = t[0];
    m.v[13] = t[1];
    m.v[14] = t[2];
    m
}

pub fn scaling(s: [f32; 3]) -> Mat4f {
    let mut m = Mat4f::identity();
    m.v[0] = s[0];
    m.v[5] = s[1];
    m.v[10] = s[2];
    m
}

/// `a * b` (b applied first).
#[inline]
pub fn mul(a: &Mat4f, b: &Mat4f) -> Mat4f {
    Mat4f::mul(a, b)
}

/// A unit quaternion `[x, y, z, w]` (glTF order).
pub type Quat4 = [f32; 4];

pub const QUAT_ID: Quat4 = [0.0, 0.0, 0.0, 1.0];

pub fn quat_axis_angle(axis: [f32; 3], angle: f32) -> Quat4 {
    let a = crate::mesh::normalize_or_up(axis);
    let (s, c) = (angle * 0.5).sin_cos();
    [a[0] * s, a[1] * s, a[2] * s, c]
}

/// Unit quaternion from Euler angles in DEGREES, applied x, then y, then z.
pub fn quat_from_euler_deg(e: [f32; 3]) -> Quat4 {
    let qx = quat_axis_angle([1.0, 0.0, 0.0], e[0] * DEG);
    let qy = quat_axis_angle([0.0, 1.0, 0.0], e[1] * DEG);
    let qz = quat_axis_angle([0.0, 0.0, 1.0], e[2] * DEG);
    // z * y * x: x applied first.
    quat_mul(quat_mul(qz, qy), qx)
}

/// Hamilton product `a * b` (b applied first).
pub fn quat_mul(a: Quat4, b: Quat4) -> Quat4 {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

pub fn quat_normalize(q: Quat4) -> Quat4 {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if l > 1e-20 && l.is_finite() {
        [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
    } else {
        QUAT_ID
    }
}

/// Normalised linear blend (for blending clip poses), sign-aligned.
pub fn quat_nlerp(a: Quat4, b: Quat4, f: f32) -> Quat4 {
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let s = if d < 0.0 { -1.0 } else { 1.0 };
    quat_normalize([
        a[0] + (b[0] * s - a[0]) * f,
        a[1] + (b[1] * s - a[1]) * f,
        a[2] + (b[2] * s - a[2]) * f,
        a[3] + (b[3] * s - a[3]) * f,
    ])
}

pub fn quat_slerp(a: Quat4, b: Quat4, f: f32) -> Quat4 {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let mut b = b;
    if d < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        d = -d;
    }
    if d > 0.9995 {
        return quat_nlerp(a, b, f);
    }
    let theta = d.clamp(-1.0, 1.0).acos();
    let s = theta.sin();
    let wa = ((1.0 - f) * theta).sin() / s;
    let wb = (f * theta).sin() / s;
    [a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb, a[2] * wa + b[2] * wb, a[3] * wa + b[3] * wb]
}

/// Rotation matrix of a unit quaternion.
pub fn quat_to_mat4(q: Quat4) -> Mat4f {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    Mat4f {
        v: [
            1.0 - 2.0 * (yy + zz),
            2.0 * (xy + wz),
            2.0 * (xz - wy),
            0.0,
            2.0 * (xy - wz),
            1.0 - 2.0 * (xx + zz),
            2.0 * (yz + wx),
            0.0,
            2.0 * (xz + wy),
            2.0 * (yz - wx),
            1.0 - 2.0 * (xx + yy),
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ],
    }
}

/// `T * R * S`.
pub fn compose(t: [f32; 3], r: Quat4, s: [f32; 3]) -> Mat4f {
    let mut m = quat_to_mat4(r);
    for k in 0..3 {
        m.v[k] *= s[0];
        m.v[4 + k] *= s[1];
        m.v[8 + k] *= s[2];
    }
    m.v[12] = t[0];
    m.v[13] = t[1];
    m.v[14] = t[2];
    m
}

/// Rotation that turns +z toward `forward`, keeping `up` up (a node's
/// `look_at`; cameras and lights look down -z and use the negated
/// direction).
pub fn look_rotation(forward: [f32; 3], up: [f32; 3]) -> Quat4 {
    let f = crate::mesh::normalize_or_up(forward);
    let mut r = crate::mesh::cross3(up, f);
    if crate::mesh::dot3(r, r) < 1e-10 {
        r = crate::mesh::any_perpendicular(f);
    }
    let r = crate::mesh::normalize_or_up(r);
    let u = crate::mesh::cross3(f, r);
    // Columns r, u, f are the rotated x, y, z axes.
    let (m00, m01, m02) = (r[0], u[0], f[0]);
    let (m10, m11, m12) = (r[1], u[1], f[1]);
    let (m20, m21, m22) = (r[2], u[2], f[2]);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
    };
    quat_normalize(q)
}

pub fn transform_point(m: &Mat4f, p: [f32; 3]) -> [f32; 3] {
    let v = m.transform_vec4(vec4(p[0], p[1], p[2], 1.0));
    [v.x, v.y, v.z]
}

pub fn transform_dir(m: &Mat4f, p: [f32; 3]) -> [f32; 3] {
    let v = m.transform_vec4(vec4(p[0], p[1], p[2], 0.0));
    [v.x, v.y, v.z]
}

pub fn mat_translation(m: &Mat4f) -> [f32; 3] {
    [m.v[12], m.v[13], m.v[14]]
}

// ---------------------------------------------------------------------------
// Deterministic randomness and noise.
// ---------------------------------------------------------------------------

/// A 64-bit mix (splitmix64 finaliser).
#[inline]
pub fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Uniform 0..1 from a seed and an index.
#[inline]
pub fn rand01(seed: u64, index: u64) -> f32 {
    ((hash64(seed ^ hash64(index)) >> 40) as f32) / ((1u64 << 24) as f32)
}

/// Uniform in -1..1.
#[inline]
pub fn rand11(seed: u64, index: u64) -> f32 {
    rand01(seed, index) * 2.0 - 1.0
}

/// A sequential deterministic generator.
#[derive(Clone, Debug)]
pub struct Rng {
    seed: u64,
    n: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { seed: hash64(seed), n: 0 }
    }
    pub fn f32(&mut self) -> f32 {
        self.n += 1;
        rand01(self.seed, self.n)
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
    /// A point uniformly inside the unit sphere.
    pub fn in_sphere(&mut self) -> [f32; 3] {
        loop {
            let p = [self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0)];
            if crate::mesh::dot3(p, p) <= 1.0 {
                return p;
            }
        }
    }
    /// A unit vector uniformly on the sphere.
    pub fn on_sphere(&mut self) -> [f32; 3] {
        let z = self.range(-1.0, 1.0);
        let a = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        [r * a.cos(), r * a.sin(), z]
    }
}

#[inline]
fn lattice(seed: u64, x: i32, y: i32, z: i32) -> f32 {
    let k = (x as u32 as u64) | ((y as u32 as u64) << 21) ^ ((z as u32 as u64) << 42);
    rand11(seed, k)
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Value noise in -1..1, smooth (quintic), period-free.
pub fn noise3(seed: u64, p: [f32; 3]) -> f32 {
    let (fx, fy, fz) = (p[0].floor(), p[1].floor(), p[2].floor());
    let (ix, iy, iz) = (fx as i32, fy as i32, fz as i32);
    let (tx, ty, tz) = (smooth(p[0] - fx), smooth(p[1] - fy), smooth(p[2] - fz));
    let mut v = [0.0f32; 8];
    for (k, slot) in v.iter_mut().enumerate() {
        *slot = lattice(seed, ix + (k & 1) as i32, iy + ((k >> 1) & 1) as i32, iz + ((k >> 2) & 1) as i32);
    }
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(v[0], v[1], tx);
    let x10 = lerp(v[2], v[3], tx);
    let x01 = lerp(v[4], v[5], tx);
    let x11 = lerp(v[6], v[7], tx);
    lerp(lerp(x00, x10, ty), lerp(x01, x11, ty), tz)
}

/// Fractal (fbm) noise: `octaves` of [`noise3`], each double the frequency
/// and `gain` the amplitude; normalised to about -1..1.
pub fn fbm3(seed: u64, p: [f32; 3], octaves: u32, gain: f32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut f = 1.0;
    for o in 0..octaves.clamp(1, 10) {
        sum += noise3(seed.wrapping_add(o as u64 * 7919), [p[0] * f, p[1] * f, p[2] * f]) * amp;
        norm += amp;
        amp *= gain;
        f *= 2.0;
    }
    sum / norm.max(1e-6)
}

/// Smooth 1D noise over time: a wiggle in -1..1.
pub fn noise1(seed: u64, t: f32) -> f32 {
    noise3(seed, [t, 0.37, 0.71])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
    }

    #[test]
    fn euler_order_is_x_first() {
        // x 90 then y 90: +y goes to +z (x), then +z goes to +x (y).
        let q = quat_from_euler_deg([90.0, 90.0, 0.0]);
        let m = quat_to_mat4(q);
        assert!(close(transform_dir(&m, [0.0, 1.0, 0.0]), [1.0, 0.0, 0.0]), "{:?}", transform_dir(&m, [0.0, 1.0, 0.0]));
    }

    #[test]
    fn compose_is_trs() {
        let m = compose([1.0, 2.0, 3.0], quat_from_euler_deg([0.0, 0.0, 90.0]), [2.0, 2.0, 2.0]);
        assert!(close(transform_point(&m, [1.0, 0.0, 0.0]), [1.0, 4.0, 3.0]));
    }

    #[test]
    fn look_rotation_turns_z_to_forward() {
        for f in [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.3, 0.5, -0.8], [0.0, 1.0, 0.0]] {
            let q = look_rotation(f, [0.0, 1.0, 0.0]);
            let got = transform_dir(&quat_to_mat4(q), [0.0, 0.0, 1.0]);
            assert!(close(got, crate::mesh::normalize_or_up(f)), "{f:?} -> {got:?}");
        }
    }

    #[test]
    fn slerp_ends() {
        let a = quat_from_euler_deg([0.0, 10.0, 0.0]);
        let b = quat_from_euler_deg([0.0, 170.0, 0.0]);
        let m = quat_slerp(a, b, 0.5);
        let d = transform_dir(&quat_to_mat4(m), [1.0, 0.0, 0.0]);
        assert!(close(d, [0.0, 0.0, -1.0]), "{d:?}");
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let mut max: f32 = 0.0;
        for i in 0..2000 {
            let p = [i as f32 * 0.173, i as f32 * 0.071, 0.5];
            let a = fbm3(9, p, 4, 0.5);
            assert_eq!(a, fbm3(9, p, 4, 0.5));
            max = max.max(a.abs());
        }
        assert!(max <= 1.0 && max > 0.2);
        assert_ne!(noise3(1, [0.5, 0.5, 0.5]), noise3(2, [0.5, 0.5, 0.5]));
    }

    #[test]
    fn rng_sphere() {
        let mut r = Rng::new(3);
        for _ in 0..100 {
            let p = r.on_sphere();
            assert!((crate::mesh::len3(p) - 1.0).abs() < 1e-4);
        }
    }
}
