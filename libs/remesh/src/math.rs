//! Kernel-exact f32 vector helpers.
//!
//! Every function mirrors the reference CUDA helpers (dot3/cross3/sub3/...)
//! at the same operation ORDER: Rust scalar f32 is IEEE-754 with no FMA
//! contraction, so results are bit-identical to the validated numpy oracle.

pub type V3 = [f32; 3];

#[inline(always)]
pub fn dot3(a: V3, b: V3) -> f32 {
    // left-to-right: (x + y) + z, matching the kernels
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline(always)]
pub fn cross3(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline(always)]
pub fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline(always)]
pub fn add3(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline(always)]
pub fn scale3(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline(always)]
pub fn min3(a: V3, b: V3) -> V3 {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])]
}

#[inline(always)]
pub fn max3(a: V3, b: V3) -> V3 {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])]
}

/// torch-style clamp: min(max(x, lo), hi)
#[inline(always)]
pub fn clamp3(x: V3, lo: V3, hi: V3) -> V3 {
    [
        x[0].max(lo[0]).min(hi[0]),
        x[1].max(lo[1]).min(hi[1]),
        x[2].max(lo[2]).min(hi[2]),
    ]
}

/// torch .norm(): sqrt((x*x + y*y) + z*z) in f32
#[inline(always)]
pub fn norm3(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
