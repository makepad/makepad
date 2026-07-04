// Port of box3d/shared/utils.h (the parts the unit tests use).
// Simple random number generator. Using this instead of rand() for cross
// platform determinism.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::math_functions::{Pos, Vec3, PI};

pub const RAND_LIMIT: i32 = 32767;
pub const RAND_SEED: u32 = 12345;

// Global seed for simple random number generator (C: g_randomSeed).
static RANDOM_SEED: AtomicU32 = AtomicU32::new(RAND_SEED);

pub fn set_random_seed(seed: u32) {
    RANDOM_SEED.store(seed, Ordering::Relaxed);
}

pub fn get_random_seed() -> u32 {
    RANDOM_SEED.load(Ordering::Relaxed)
}

/// XorShift32 algorithm.
pub fn random_int() -> i32 {
    let mut x = RANDOM_SEED.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    RANDOM_SEED.store(x, Ordering::Relaxed);

    // Map the 32-bit value to the range 0 to RAND_LIMIT
    (x % (RAND_LIMIT as u32 + 1)) as i32
}

/// Random integer in range [lo, hi].
pub fn random_int_range(lo: i32, hi: i32) -> i32 {
    lo + random_int() % (hi - lo + 1)
}

/// Random number in range [-1,1].
pub fn random_float() -> f32 {
    let mut r = (random_int() & RAND_LIMIT) as f32;
    r /= RAND_LIMIT as f32;
    r = 2.0 * r - 1.0;
    r
}

/// Random floating point number in range [lo, hi].
pub fn random_float_range(lo: f32, hi: f32) -> f32 {
    let mut r = (random_int() & RAND_LIMIT) as f32;
    r /= RAND_LIMIT as f32;
    r = (hi - lo) * r + lo;
    r
}

/// Random vector with coordinates in range [lo, hi].
pub fn random_vec3(lo: Vec3, hi: Vec3) -> Vec3 {
    Vec3 {
        x: random_float_range(lo.x, hi.x),
        y: random_float_range(lo.y, hi.y),
        z: random_float_range(lo.z, hi.z),
    }
}

/// Random world position with coordinates in range [lo, hi].
pub fn random_pos(lo: Vec3, hi: Vec3) -> Pos {
    random_vec3(lo, hi)
}

pub fn random_vec3_uniform(lo: f32, hi: f32) -> Vec3 {
    Vec3 {
        x: random_float_range(lo, hi),
        y: random_float_range(lo, hi),
        z: random_float_range(lo, hi),
    }
}

/// Generate uniformly distributed random unit vector using Shoemake's method.
pub fn random_unit_vector() -> Vec3 {
    let u1 = random_float_range(0.0, 1.0);
    let u2 = random_float_range(0.0, 2.0 * PI);
    let u3 = random_float_range(0.0, 2.0 * PI);

    let sqrt1_minus_u1 = (1.0 - u1).sqrt();
    let sqrt_u1 = u1.sqrt();

    Vec3 {
        x: sqrt1_minus_u1 * u2.sin(),
        y: sqrt1_minus_u1 * u2.cos(),
        z: sqrt_u1 * u3.sin(),
    }
}

/// Generate uniformly distributed random quaternion using Shoemake's method.
pub fn random_quat() -> crate::math_functions::Quat {
    let u1 = random_float_range(0.0, 1.0);
    let u2 = random_float_range(0.0, 2.0 * PI);
    let u3 = random_float_range(0.0, 2.0 * PI);

    let sqrt1_minus_u1 = (1.0 - u1).sqrt();
    let sqrt_u1 = u1.sqrt();

    crate::math_functions::Quat {
        v: Vec3 {
            x: sqrt1_minus_u1 * u2.sin(),
            y: sqrt1_minus_u1 * u2.cos(),
            z: sqrt_u1 * u3.sin(),
        },
        s: sqrt_u1 * u3.cos(),
    }
}
