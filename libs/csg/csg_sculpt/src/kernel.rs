//! The stroke bake as a GPU-shaped kernel: the exact per-sample body the
//! compute pass runs, and the stroke parameter block it reads.
//!
//! The engine's per-stroke dispatch is
//! `kernel(brick_list, src_bricks f32, dst_bricks f32, stroke params)`: for
//! each pass (1, or [`passes`] Jacobi passes for smooth), every sample of
//! every brick in the write list gets `dst = sample(params, g, src)`, then
//! src and dst swap. Reads of `src` cover the write list dilated by
//! `layer::reach_bricks`. [`crate::layer::Layer`] bakes through this same
//! function, so the CPU reference and the kernel cannot drift apart: port
//! [`sample`] line by line.
//!
//! Parameter block (f32 words; integers are exact in f32):
//!
//! | word | meaning |
//! |---|---|
//! | 0 | mode: 0 add, 1 carve, 2 smooth, 3 inflate, 4 flatten, 5 pinch, 6 grab, 7 paint |
//! | 1 2 3 | radius, blend, strength |
//! | 4..8 | flatten plane n.x n.y n.z d |
//! | 8 | material slot |
//! | 9 | point count n |
//! | 10 11 | cell, band |
//! | 12 | passes |
//! | 13 | chamfer 0/1 |
//! | 14 | bone (not read by the kernel; one layer per bone) |
//! | 15 | reserved (0) |
//! | 16 + 4i .. | point i: x y z pressure |

use crate::math::*;
use crate::stroke::{Mode, Stroke};

pub const HEADER_WORDS: usize = 16;

pub fn mode_code(m: Mode) -> u32 {
    match m {
        Mode::Add => 0,
        Mode::Carve => 1,
        Mode::Smooth => 2,
        Mode::Inflate => 3,
        Mode::Flatten => 4,
        Mode::Pinch => 5,
        Mode::Grab => 6,
        Mode::Paint => 7,
    }
}

pub fn mode_from_code(c: u32) -> Mode {
    match c {
        0 => Mode::Add,
        1 => Mode::Carve,
        2 => Mode::Smooth,
        3 => Mode::Inflate,
        4 => Mode::Flatten,
        5 => Mode::Pinch,
        6 => Mode::Grab,
        _ => Mode::Paint,
    }
}

/// The Jacobi passes this stroke's dispatch runs.
pub fn passes(s: &Stroke, cell: f32) -> u32 {
    if s.mode == Mode::Smooth {
        s.smooth_passes(cell)
    } else {
        1
    }
}

/// The stroke's parameter block.
pub fn pack(s: &Stroke, cell: f32, band: f32) -> Vec<f32> {
    let mut w = vec![0.0f32; HEADER_WORDS + 4 * s.path.len()];
    w[0] = mode_code(s.mode) as f32;
    w[1] = s.radius;
    w[2] = s.blend;
    w[3] = s.strength;
    w[4..8].copy_from_slice(&s.plane);
    w[8] = s.material as f32;
    w[9] = s.path.len() as f32;
    w[10] = cell;
    w[11] = band;
    w[12] = passes(s, cell) as f32;
    w[13] = if s.chamfer { 1.0 } else { 0.0 };
    w[14] = s.bone as f32;
    for (i, p) in s.path.iter().enumerate() {
        w[HEADER_WORDS + 4 * i..HEADER_WORDS + 4 * i + 4].copy_from_slice(p);
    }
    w
}

/// The stroke a parameter block stands for (with its cell and band).
pub fn unpack(w: &[f32]) -> (Stroke, f32, f32) {
    let n = w[9] as usize;
    let path = (0..n).map(|i| {
        let o = HEADER_WORDS + 4 * i;
        [w[o], w[o + 1], w[o + 2], w[o + 3]]
    });
    let s = Stroke {
        mode: mode_from_code(w[0] as u32),
        radius: w[1],
        blend: w[2],
        strength: w[3],
        plane: [w[4], w[5], w[6], w[7]],
        material: w[8] as u8,
        chamfer: w[13] != 0.0,
        bone: w[14] as u16,
        path: path.collect(),
    };
    (s, w[10], w[11])
}

/// Trilinear distance at `x` over the source grid.
pub fn trilinear(src: &dyn Fn([i32; 3]) -> (f32, u8), x: V3, cell: f32) -> f32 {
    let u = scale(x, 1.0 / cell);
    let f = [u[0].floor(), u[1].floor(), u[2].floor()];
    let g = [f[0] as i32, f[1] as i32, f[2] as i32];
    let t = [u[0] - f[0], u[1] - f[1], u[2] - f[2]];
    let s = |dx: i32, dy: i32, dz: i32| src([g[0] + dx, g[1] + dy, g[2] + dz]).0;
    let x00 = mix(s(0, 0, 0), s(1, 0, 0), t[0]);
    let x10 = mix(s(0, 1, 0), s(1, 1, 0), t[0]);
    let x01 = mix(s(0, 0, 1), s(1, 0, 1), t[0]);
    let x11 = mix(s(0, 1, 1), s(1, 1, 1), t[0]);
    mix(mix(x00, x10, t[1]), mix(x01, x11, t[1]), t[2])
}

/// The material of the sample nearest to `x`.
pub fn nearest_material(src: &dyn Fn([i32; 3]) -> (f32, u8), x: V3, cell: f32) -> u8 {
    src([nearest(x[0] / cell), nearest(x[1] / cell), nearest(x[2] / cell)]).1
}

/// One pass, one sample: the new (distance, material) at lattice point `g`
/// from the pass's source grid.
pub fn sample(s: &Stroke, cell: f32, band: f32, g: [i32; 3], src: &dyn Fn([i32; 3]) -> (f32, u8)) -> (f32, u8) {
    let q = [g[0] as f32 * cell, g[1] as f32 * cell, g[2] as f32 * cell];
    let (p, m) = src(g);
    match s.mode {
        Mode::Add | Mode::Carve | Mode::Inflate | Mode::Flatten | Mode::Paint => {
            let (v, nm) = s.apply_sample(q, p, m);
            (v.clamp(-band, band), nm)
        }
        Mode::Pinch | Mode::Grab => match s.warp_source(q) {
            Some(x) => (trilinear(src, x, cell).clamp(-band, band), nearest_material(src, x, cell)),
            None => (p, m),
        },
        Mode::Smooth => {
            let w = (s.weight(q).0 * 0.5).min(0.5);
            if w <= 0.0 {
                return (p, m);
            }
            let n = src([g[0] - 1, g[1], g[2]]).0
                + src([g[0] + 1, g[1], g[2]]).0
                + src([g[0], g[1] - 1, g[2]]).0
                + src([g[0], g[1] + 1, g[2]]).0
                + src([g[0], g[1], g[2] - 1]).0
                + src([g[0], g[1], g[2] + 1]).0;
            (mix(p, n * (1.0 / 6.0), w).clamp(-band, band), m)
        }
    }
}
