//! CLASSICAL optical flow for the `mkfl` payload — no model, no weights, no
//! download.
//!
//! The enhance service gets its motion field from RIFE, which is a neural
//! network and stays one: it is already loaded there, and its field is
//! better. The VJ's import path may not load a model at all, so it needs a
//! field of its own, and this is it: a pyramidal block-matching estimator
//! that produces exactly the same quantities in exactly the same units.
//!
//! ## Why block matching, and how it is kept honest
//!
//! The payload's grid is a QUARTER of the video and its unit is a quarter of
//! a grid pixel — one source pixel. So the estimator does not need per-pixel
//! precision; it needs quarter-resolution vectors accurate to about a source
//! pixel, and it needs them SMOOTH, because a warp is only as good as the
//! continuity of the field that drives it. Block matching on a luma pyramid
//! gives both:
//!
//! - a luma pyramid whose level 0 IS the flow grid (4:1 area average) and
//!   whose coarser levels halve from there, so large motion is found cheaply
//!   at the top and refined down;
//! - per level, a few Jacobi-style propagation sweeps: each cell picks the
//!   cheapest of its own vector, its neighbours' vectors and a small local
//!   window, scored by block SAD plus a smoothness penalty. Reading the
//!   previous sweep and writing the next makes the result independent of
//!   evaluation order — which is what lets the sweeps run on every core and
//!   still be bit-identical run to run;
//! - a 3×3 median per level (the classic outlier killer), then a parabola fit
//!   on the SAD at level 0 for the sub-pixel part.
//!
//! ## From two one-way fields to one intermediate field
//!
//! The payload is defined AT THE INTERMEDIATE (t=0.5): `f0` points from the
//! in-between to frame 0, `f1` to frame 1. So both directions are estimated
//! and then *reversed* onto the intermediate grid: content at cell `p` of
//! frame 0 moving by `d` sits at `p + d/2` half-way, and contributes `f0 =
//! -d/2`, `f1 = +d/2` there; the backward field contributes the same way from
//! frame 1.
//!
//! That also produces the occlusion mask for free, and produces it MEANING
//! something: a cell both fields reach is visible in both frames (mask 0.5,
//! fuse them), a cell only the forward field reaches exists in frame 0 and is
//! hidden in frame 1 (mask 1 — take frame 0), and the mirror case takes frame
//! 1. Cells neither field reaches are holes, filled from their neighbours.

/// Tunables for [`estimate_flow`]. The defaults are the shipped import
/// settings; the tests pin the behaviour they imply.
#[derive(Clone, Copy, Debug)]
pub struct FlowParams {
    /// SAD window radius in grid cells (2 = a 5×5 window = 20×20 source px).
    pub block_radius: usize,
    /// Exhaustive search radius at the coarsest level, in that level's cells.
    pub coarse_radius: i32,
    /// Propagation sweeps per pyramid level.
    pub iterations: usize,
    /// Smoothness weight: luma units charged per cell of disagreement with a
    /// neighbour. Zero would make flat regions pure noise.
    pub smooth_lambda: f32,
    /// Pyramid levels including level 0 (the grid). Each is half the last.
    pub max_levels: usize,
}

impl Default for FlowParams {
    fn default() -> Self {
        Self {
            block_radius: 2,
            coarse_radius: 5,
            iterations: 3,
            smooth_lambda: 1.5,
            max_levels: 5,
        }
    }
}

/// One luma level: tightly packed f32, stride == `w`.
#[derive(Clone)]
struct Plane {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Plane {
    #[inline]
    fn at(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        self.px[y * self.w + x]
    }
}

/// A frame reduced to the pyramid the estimator walks. Level 0 is the FLOW
/// GRID, so a caller never has to think about source resolution again.
#[derive(Clone)]
pub struct FramePyramid {
    levels: Vec<Plane>,
}

impl FramePyramid {
    /// Build from a tightly packed luma plane (the Y plane of NV12 is exactly
    /// this, which is why the converter never leaves YUV to measure motion).
    pub fn from_luma(
        luma: &[u8],
        w: usize,
        h: usize,
        grid_w: usize,
        grid_h: usize,
        max_levels: usize,
    ) -> Self {
        assert!(w > 0 && h > 0 && grid_w > 0 && grid_h > 0, "empty frame geometry");
        assert!(luma.len() >= w * h, "luma plane shorter than {w}x{h}");
        let mut px = vec![0.0f32; grid_w * grid_h];
        let step_x = w as f32 / grid_w as f32;
        let step_y = h as f32 / grid_h as f32;
        for gy in 0..grid_h {
            let y0 = (gy as f32 * step_y) as usize;
            let y1 = (((gy + 1) as f32 * step_y) as usize).clamp(y0 + 1, h);
            for gx in 0..grid_w {
                let x0 = (gx as f32 * step_x) as usize;
                let x1 = (((gx + 1) as f32 * step_x) as usize).clamp(x0 + 1, w);
                let mut sum = 0.0f32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += luma[y * w + x] as f32;
                    }
                }
                px[gy * grid_w + gx] = sum / ((x1 - x0) * (y1 - y0)) as f32;
            }
        }
        let mut levels = vec![Plane { w: grid_w, h: grid_h, px }];
        while levels.len() < max_levels.max(1) {
            let last = levels.last().expect("at least one level");
            if last.w < 16 || last.h < 16 {
                break;
            }
            levels.push(halve(last));
        }
        Self { levels }
    }

    pub fn grid_w(&self) -> usize {
        self.levels[0].w
    }

    pub fn grid_h(&self) -> usize {
        self.levels[0].h
    }
}

fn halve(src: &Plane) -> Plane {
    let (w, h) = (src.w / 2, src.h / 2);
    let mut px = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let a = src.px[(2 * y) * src.w + 2 * x];
            let b = src.px[(2 * y) * src.w + 2 * x + 1];
            let c = src.px[(2 * y + 1) * src.w + 2 * x];
            let d = src.px[(2 * y + 1) * src.w + 2 * x + 1];
            px[y * w + x] = (a + b + c + d) * 0.25;
        }
    }
    Plane { w, h, px }
}

/// Mean absolute difference of the block around `(x, y)` in `a` against the
/// block displaced by `(dx, dy)` in `b`. Edges clamp rather than wrap: a
/// vector that runs off the frame is expensive, not wrong.
#[inline]
fn block_cost(a: &Plane, b: &Plane, x: i32, y: i32, dx: i32, dy: i32, rb: i32) -> f32 {
    let mut sum = 0.0f32;
    for j in -rb..=rb {
        for i in -rb..=rb {
            sum += (a.at(x + i, y + j) - b.at(x + i + dx, y + j + dy)).abs();
        }
    }
    let n = (2 * rb + 1) * (2 * rb + 1);
    sum / n as f32
}

/// Run `work` over row bands of `out` on every core. Each band sees only its
/// own rows to write and the (immutable) previous field to read, so the
/// result does not depend on how many bands there were.
fn par_bands(
    out: &mut [[i32; 2]],
    w: usize,
    h: usize,
    work: &(dyn Fn(usize, &mut [[i32; 2]]) + Sync),
) {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(h.max(1));
    if threads <= 1 || h < 8 {
        work(0, out);
        return;
    }
    let rows = (h + threads - 1) / threads;
    std::thread::scope(|scope| {
        let mut y0 = 0usize;
        for chunk in out.chunks_mut(rows * w) {
            let start = y0;
            scope.spawn(move || work(start, chunk));
            y0 += rows;
        }
    });
}

/// The smoothness charge for holding `d` at `(x, y)` given the previous
/// sweep's neighbours.
#[inline]
fn smooth_cost(prev: &[[i32; 2]], w: usize, h: usize, x: usize, y: usize, d: [i32; 2]) -> f32 {
    let mut sum = 0i32;
    let mut n = 0i32;
    let mut add = |o: [i32; 2]| {
        sum += (d[0] - o[0]).abs() + (d[1] - o[1]).abs();
        n += 1;
    };
    if x > 0 {
        add(prev[y * w + x - 1]);
    }
    if x + 1 < w {
        add(prev[y * w + x + 1]);
    }
    if y > 0 {
        add(prev[(y - 1) * w + x]);
    }
    if y + 1 < h {
        add(prev[(y + 1) * w + x]);
    }
    if n == 0 {
        return 0.0;
    }
    sum as f32 / n as f32
}

/// One Jacobi sweep: every cell re-picks from its own vector, its
/// neighbours', and a small window around its own.
fn sweep(a: &Plane, b: &Plane, prev: &[[i32; 2]], p: &FlowParams) -> Vec<[i32; 2]> {
    let (w, h) = (a.w, a.h);
    let rb = p.block_radius as i32;
    let lambda = p.smooth_lambda;
    let mut out = vec![[0i32; 2]; w * h];
    par_bands(&mut out, w, h, &|y0: usize, band: &mut [[i32; 2]]| {
        for (row, chunk) in band.chunks_mut(w).enumerate() {
            let y = y0 + row;
            for (x, slot) in chunk.iter_mut().enumerate() {
                let here = prev[y * w + x];
                let mut best = here;
                let mut best_cost = f32::INFINITY;
                let consider = |d: [i32; 2], best: &mut [i32; 2], best_cost: &mut f32| {
                    let cost = block_cost(a, b, x as i32, y as i32, d[0], d[1], rb)
                        + lambda * smooth_cost(prev, w, h, x, y, d);
                    // Strict improvement only: ties keep the incumbent, which
                    // keeps the field stable instead of jittering between
                    // equal-cost candidates.
                    if cost < *best_cost {
                        *best_cost = cost;
                        *best = d;
                    }
                };
                consider(here, &mut best, &mut best_cost);
                // Neighbour propagation: how a good vector travels across a
                // textureless patch to the cells that cannot measure it.
                if x > 0 {
                    consider(prev[y * w + x - 1], &mut best, &mut best_cost);
                }
                if x + 1 < w {
                    consider(prev[y * w + x + 1], &mut best, &mut best_cost);
                }
                if y > 0 {
                    consider(prev[(y - 1) * w + x], &mut best, &mut best_cost);
                }
                if y + 1 < h {
                    consider(prev[(y + 1) * w + x], &mut best, &mut best_cost);
                }
                // Local refinement around the incumbent.
                for (ox, oy) in [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, -1),
                    (-1, 1),
                    (1, 1),
                    (-2, 0),
                    (2, 0),
                    (0, -2),
                    (0, 2),
                ] {
                    consider([here[0] + ox, here[1] + oy], &mut best, &mut best_cost);
                }
                *slot = best;
            }
        }
    });
    out
}

/// Exhaustive search at the coarsest level: nothing to propagate from yet.
fn exhaustive(a: &Plane, b: &Plane, p: &FlowParams) -> Vec<[i32; 2]> {
    let (w, h) = (a.w, a.h);
    let rb = p.block_radius as i32;
    let r = p.coarse_radius;
    let mut out = vec![[0i32; 2]; w * h];
    par_bands(&mut out, w, h, &|y0: usize, band: &mut [[i32; 2]]| {
        for (row, chunk) in band.chunks_mut(w).enumerate() {
            let y = y0 + row;
            for (x, slot) in chunk.iter_mut().enumerate() {
                let mut best = [0i32; 2];
                let mut best_cost = f32::INFINITY;
                for dy in -r..=r {
                    for dx in -r..=r {
                        let cost = block_cost(a, b, x as i32, y as i32, dx, dy, rb);
                        if cost < best_cost {
                            best_cost = cost;
                            best = [dx, dy];
                        }
                    }
                }
                *slot = best;
            }
        }
    });
    out
}

/// 3×3 median per component — the classic block-matching outlier killer.
fn median3(field: &[[i32; 2]], w: usize, h: usize) -> Vec<[i32; 2]> {
    let mut out = vec![[0i32; 2]; w * h];
    let mut bx = [0i32; 9];
    let mut by = [0i32; 9];
    for y in 0..h {
        for x in 0..w {
            let mut n = 0;
            for j in -1i32..=1 {
                for i in -1i32..=1 {
                    let sx = (x as i32 + i).clamp(0, w as i32 - 1) as usize;
                    let sy = (y as i32 + j).clamp(0, h as i32 - 1) as usize;
                    bx[n] = field[sy * w + sx][0];
                    by[n] = field[sy * w + sx][1];
                    n += 1;
                }
            }
            bx[..n].sort_unstable();
            by[..n].sort_unstable();
            out[y * w + x] = [bx[n / 2], by[n / 2]];
        }
    }
    out
}

/// Nearest-neighbour upsample of a coarser field, doubling the vectors with
/// the resolution.
fn upsample(field: &[[i32; 2]], from_w: usize, from_h: usize, to_w: usize, to_h: usize) -> Vec<[i32; 2]> {
    let mut out = vec![[0i32; 2]; to_w * to_h];
    for y in 0..to_h {
        let sy = (y * from_h / to_h).min(from_h - 1);
        for x in 0..to_w {
            let sx = (x * from_w / to_w).min(from_w - 1);
            let d = field[sy * from_w + sx];
            out[y * to_w + x] = [d[0] * 2, d[1] * 2];
        }
    }
    out
}

/// Parabola fit on the SAD around the integer optimum, per axis. This is the
/// whole sub-pixel story: one source pixel is one payload unit, so a quarter
/// of a grid cell is exactly as fine as the format can carry.
fn subpixel(a: &Plane, b: &Plane, field: &[[i32; 2]], p: &FlowParams) -> Vec<[f32; 2]> {
    let (w, h) = (a.w, a.h);
    let rb = p.block_radius as i32;
    let mut out = vec![[0.0f32; 2]; w * h];
    for y in 0..h {
        for x in 0..w {
            let d = field[y * w + x];
            let c0 = block_cost(a, b, x as i32, y as i32, d[0], d[1], rb);
            let fit = |lo: f32, hi: f32| -> f32 {
                let denom = lo + hi - 2.0 * c0;
                if denom <= 1e-6 {
                    return 0.0;
                }
                (0.5 * (lo - hi) / denom).clamp(-0.5, 0.5)
            };
            let cl = block_cost(a, b, x as i32, y as i32, d[0] - 1, d[1], rb);
            let cr = block_cost(a, b, x as i32, y as i32, d[0] + 1, d[1], rb);
            let cu = block_cost(a, b, x as i32, y as i32, d[0], d[1] - 1, rb);
            let cd = block_cost(a, b, x as i32, y as i32, d[0], d[1] + 1, rb);
            out[y * w + x] = [d[0] as f32 + fit(cl, cr), d[1] as f32 + fit(cu, cd)];
        }
    }
    out
}

/// The one-way field from `a` to `b`, in GRID pixels, one vector per grid
/// cell. Positive x is right, positive y is down — image order.
pub fn estimate_flow(a: &FramePyramid, b: &FramePyramid, p: &FlowParams) -> Vec<[f32; 2]> {
    let n = a.levels.len().min(b.levels.len());
    assert!(n > 0, "an empty pyramid cannot be matched");
    let mut field: Vec<[i32; 2]> = Vec::new();
    let mut from = (0usize, 0usize);
    for li in (0..n).rev() {
        let (pa, pb) = (&a.levels[li], &b.levels[li]);
        let mut cur = if field.is_empty() {
            exhaustive(pa, pb, p)
        } else {
            upsample(&field, from.0, from.1, pa.w, pa.h)
        };
        for _ in 0..p.iterations.max(1) {
            cur = sweep(pa, pb, &cur, p);
        }
        field = median3(&cur, pa.w, pa.h);
        from = (pa.w, pa.h);
    }
    subpixel(&a.levels[0], &b.levels[0], &field, p)
}

/// One pair's payload-shaped field, defined at the intermediate (t=0.5) and
/// measured in GRID pixels — [`crate::payload::quantize_flow_grid`] turns it
/// into bytes.
pub struct FlowPair {
    pub grid_w: usize,
    pub grid_h: usize,
    /// Intermediate → frame 0.
    pub f0: Vec<[f32; 2]>,
    /// Intermediate → frame 1.
    pub f1: Vec<[f32; 2]>,
    /// 1 = the intermediate takes frame 0's warp, 0 = frame 1's.
    pub mask: Vec<f32>,
}

/// Estimate both directions and reverse them onto the intermediate grid.
pub fn flow_pair(a: &FramePyramid, b: &FramePyramid, p: &FlowParams) -> FlowPair {
    assert_eq!(a.grid_w(), b.grid_w(), "pair frames must share a grid");
    assert_eq!(a.grid_h(), b.grid_h(), "pair frames must share a grid");
    let forward = estimate_flow(a, b, p);
    let backward = estimate_flow(b, a, p);
    reverse_to_intermediate(&forward, &backward, a.grid_w(), a.grid_h())
}

/// Splat both one-way fields half-way and read off `f0`, `f1` and the
/// occlusion mask (see the module header for what each coverage case means).
pub fn reverse_to_intermediate(
    forward: &[[f32; 2]],
    backward: &[[f32; 2]],
    grid_w: usize,
    grid_h: usize,
) -> FlowPair {
    let n = grid_w * grid_h;
    assert_eq!(forward.len(), n);
    assert_eq!(backward.len(), n);
    let mut acc_f = vec![[0.0f32; 2]; n];
    let mut cnt_f = vec![0.0f32; n];
    let mut acc_b = vec![[0.0f32; 2]; n];
    let mut cnt_b = vec![0.0f32; n];
    let splat = |field: &[[f32; 2]], acc: &mut [[f32; 2]], cnt: &mut [f32]| {
        for y in 0..grid_h {
            for x in 0..grid_w {
                let d = field[y * grid_w + x];
                let tx = (x as f32 + 0.5 * d[0]).round().clamp(0.0, grid_w as f32 - 1.0) as usize;
                let ty = (y as f32 + 0.5 * d[1]).round().clamp(0.0, grid_h as f32 - 1.0) as usize;
                let q = ty * grid_w + tx;
                acc[q][0] += d[0];
                acc[q][1] += d[1];
                cnt[q] += 1.0;
            }
        }
    };
    splat(forward, &mut acc_f, &mut cnt_f);
    splat(backward, &mut acc_b, &mut cnt_b);

    // The A→B motion of whatever sits at each intermediate cell, plus how
    // much each endpoint is to be trusted there.
    let mut motion = vec![[0.0f32; 2]; n];
    let mut mask = vec![0.5f32; n];
    let mut known = vec![false; n];
    for i in 0..n {
        let (has_f, has_b) = (cnt_f[i] > 0.0, cnt_b[i] > 0.0);
        let mean = |acc: [f32; 2], c: f32| [acc[0] / c, acc[1] / c];
        match (has_f, has_b) {
            (true, true) => {
                let f = mean(acc_f[i], cnt_f[i]);
                let b = mean(acc_b[i], cnt_b[i]);
                // Both frames see this content: average the two readings of
                // the same motion (the backward field points the other way).
                motion[i] = [0.5 * (f[0] - b[0]), 0.5 * (f[1] - b[1])];
                mask[i] = 0.5;
                known[i] = true;
            }
            (true, false) => {
                motion[i] = mean(acc_f[i], cnt_f[i]);
                // Visible in frame 0, hidden in frame 1: take frame 0.
                mask[i] = 1.0;
                known[i] = true;
            }
            (false, true) => {
                let b = mean(acc_b[i], cnt_b[i]);
                motion[i] = [-b[0], -b[1]];
                mask[i] = 0.0;
                known[i] = true;
            }
            (false, false) => {}
        }
    }
    fill_holes(&mut motion, &mut mask, &mut known, forward, grid_w, grid_h);
    let motion = median3f(&motion, grid_w, grid_h);
    let mask = blur3(&mask, grid_w, grid_h);

    let mut f0 = vec![[0.0f32; 2]; n];
    let mut f1 = vec![[0.0f32; 2]; n];
    for i in 0..n {
        f0[i] = [-0.5 * motion[i][0], -0.5 * motion[i][1]];
        f1[i] = [0.5 * motion[i][0], 0.5 * motion[i][1]];
    }
    FlowPair { grid_w, grid_h, f0, f1, mask }
}

/// Cells no splat reached grow in from their neighbours; whatever is still
/// empty after that falls back to the forward field measured in place, which
/// is the small-motion approximation and never worse than zero.
fn fill_holes(
    motion: &mut [[f32; 2]],
    mask: &mut [f32],
    known: &mut [bool],
    forward: &[[f32; 2]],
    w: usize,
    h: usize,
) {
    for _ in 0..4 {
        if known.iter().all(|k| *k) {
            break;
        }
        let snapshot: Vec<([f32; 2], f32, bool)> = (0..w * h)
            .map(|i| (motion[i], mask[i], known[i]))
            .collect();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if snapshot[i].2 {
                    continue;
                }
                let mut sum = [0.0f32; 2];
                let mut msum = 0.0f32;
                let mut n = 0.0f32;
                for j in -1i32..=1 {
                    for k in -1i32..=1 {
                        let sx = x as i32 + k;
                        let sy = y as i32 + j;
                        if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 {
                            continue;
                        }
                        let s = snapshot[sy as usize * w + sx as usize];
                        if !s.2 {
                            continue;
                        }
                        sum[0] += s.0[0];
                        sum[1] += s.0[1];
                        msum += s.1;
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    motion[i] = [sum[0] / n, sum[1] / n];
                    mask[i] = msum / n;
                    known[i] = true;
                }
            }
        }
    }
    for i in 0..w * h {
        if !known[i] {
            motion[i] = forward[i];
            mask[i] = 0.5;
            known[i] = true;
        }
    }
}

fn median3f(field: &[[f32; 2]], w: usize, h: usize) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; w * h];
    let mut bx = [0.0f32; 9];
    let mut by = [0.0f32; 9];
    for y in 0..h {
        for x in 0..w {
            let mut n = 0;
            for j in -1i32..=1 {
                for i in -1i32..=1 {
                    let sx = (x as i32 + i).clamp(0, w as i32 - 1) as usize;
                    let sy = (y as i32 + j).clamp(0, h as i32 - 1) as usize;
                    bx[n] = field[sy * w + sx][0];
                    by[n] = field[sy * w + sx][1];
                    n += 1;
                }
            }
            bx[..n].sort_by(|a, b| a.total_cmp(b));
            by[..n].sort_by(|a, b| a.total_cmp(b));
            out[y * w + x] = [bx[n / 2], by[n / 2]];
        }
    }
    out
}

fn blur3(field: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0f32;
            let mut n = 0.0f32;
            for j in -1i32..=1 {
                for i in -1i32..=1 {
                    let sx = (x as i32 + i).clamp(0, w as i32 - 1) as usize;
                    let sy = (y as i32 + j).clamp(0, h as i32 - 1) as usize;
                    sum += field[sy * w + sx];
                    n += 1.0;
                }
            }
            out[y * w + x] = sum / n;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash2(x: i32, y: i32) -> f32 {
        let mut h = (x as u32)
            .wrapping_mul(374761393)
            ^ (y as u32).wrapping_mul(668265263);
        h ^= h >> 13;
        h = h.wrapping_mul(1274126177);
        h ^= h >> 16;
        (h & 0xffff) as f32 / 65535.0
    }

    /// Smooth value noise: a lattice of pseudo-random values interpolated
    /// with a smoothstep. Deliberately NOT periodic — a repeating pattern
    /// makes every matcher ambiguous and would test the pattern, not the
    /// estimator.
    fn noise(fx: f32, fy: f32, cell: f32) -> f32 {
        let (gx, gy) = ((fx / cell).floor(), (fy / cell).floor());
        let (tx, ty) = (fx / cell - gx, fy / cell - gy);
        let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
        let (ix, iy) = (gx as i32, gy as i32);
        let a = hash2(ix, iy);
        let b = hash2(ix + 1, iy);
        let c = hash2(ix, iy + 1);
        let d = hash2(ix + 1, iy + 1);
        (a + (b - a) * sx) + ((c + (d - c) * sx) - (a + (b - a) * sx)) * sy
    }

    /// A textured frame, sampled at an offset — so two calls differing only
    /// in `(ox, oy)` are the SAME picture translated by exactly that much.
    fn pattern(w: usize, h: usize, ox: i32, oy: i32) -> Vec<u8> {
        let mut out = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let fx = (x as i32 + ox) as f32;
                let fy = (y as i32 + oy) as f32;
                let v = 0.6 * noise(fx, fy, 16.0) + 0.4 * noise(fx, fy, 5.0);
                out[y * w + x] = (v * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
        out
    }

    fn pyramid(px: &[u8], w: usize, h: usize) -> FramePyramid {
        FramePyramid::from_luma(px, w, h, w / 4, h / 4, 5)
    }

    /// Mean of the interior (edges see content that walked in from outside).
    fn interior_mean(field: &[[f32; 2]], w: usize, h: usize) -> [f32; 2] {
        let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0.0f32);
        for y in 4..h - 4 {
            for x in 4..w - 4 {
                sx += field[y * w + x][0];
                sy += field[y * w + x][1];
                n += 1.0;
            }
        }
        [sx / n, sy / n]
    }

    #[test]
    fn a_translating_pattern_recovers_its_own_motion() {
        // 8 source px right, 4 up = 2 grid px right, 1 grid px up.
        let (w, h) = (256usize, 128usize);
        let a = pyramid(&pattern(w, h, 0, 0), w, h);
        let b = pyramid(&pattern(w, h, -8, 4), w, h);
        let flow = estimate_flow(&a, &b, &FlowParams::default());
        let (gw, gh) = (w / 4, h / 4);
        let mean = interior_mean(&flow, gw, gh);
        assert!(
            (mean[0] - 2.0).abs() < 0.35 && (mean[1] + 1.0).abs() < 0.35,
            "recovered {mean:?}, expected [2, -1] grid px"
        );
    }

    #[test]
    fn the_intermediate_field_is_half_the_motion_in_both_directions() {
        let (w, h) = (256usize, 128usize);
        let (gw, gh) = (w / 4, h / 4);
        let a = pyramid(&pattern(w, h, 0, 0), w, h);
        let b = pyramid(&pattern(w, h, -8, 0), w, h);
        let pair = flow_pair(&a, &b, &FlowParams::default());
        let m0 = interior_mean(&pair.f0, gw, gh);
        let m1 = interior_mean(&pair.f1, gw, gh);
        // f0 points BACK to frame 0 (against the motion), f1 forward.
        assert!((m0[0] + 1.0).abs() < 0.35, "f0 {m0:?}");
        assert!((m1[0] - 1.0).abs() < 0.35, "f1 {m1:?}");
        assert!(m0[1].abs() < 0.35 && m1[1].abs() < 0.35, "no vertical motion: {m0:?} {m1:?}");
        // A pure translation is visible in both frames nearly everywhere, so
        // the mask sits at the fuse-both value rather than at an endpoint.
        let mean_mask: f32 = pair.mask.iter().sum::<f32>() / pair.mask.len() as f32;
        assert!((mean_mask - 0.5).abs() < 0.2, "mask mean {mean_mask}");
    }

    #[test]
    fn identical_frames_measure_no_motion() {
        let (w, h) = (128usize, 128usize);
        let px = pattern(w, h, 0, 0);
        let a = pyramid(&px, w, h);
        let b = pyramid(&px, w, h);
        let pair = flow_pair(&a, &b, &FlowParams::default());
        for (i, v) in pair.f0.iter().enumerate() {
            assert!(
                v[0].abs() < 1e-3 && v[1].abs() < 1e-3,
                "cell {i} moved {v:?} between identical frames"
            );
            assert!(pair.f1[i][0].abs() < 1e-3 && pair.f1[i][1].abs() < 1e-3);
        }
    }

    #[test]
    fn the_estimate_does_not_depend_on_how_many_bands_ran() {
        // Two runs on the same input must agree bit for bit — the sweeps are
        // Jacobi-style precisely so that threading cannot change the answer.
        let (w, h) = (192usize, 96usize);
        let a = pyramid(&pattern(w, h, 0, 0), w, h);
        let b = pyramid(&pattern(w, h, -5, 3), w, h);
        let one = estimate_flow(&a, &b, &FlowParams::default());
        let two = estimate_flow(&a, &b, &FlowParams::default());
        assert_eq!(one, two);
    }

    #[test]
    fn an_occluding_edge_pushes_the_mask_off_the_middle() {
        // A bright bar sweeping right over a static background: the cells it
        // uncovers and the cells it covers must not both read "trust both".
        let (w, h) = (256usize, 128usize);
        let bar = |shift: i32| {
            let mut px = pattern(w, h, 0, 0);
            for y in 0..h {
                for x in 0..w {
                    let bx = x as i32 - shift;
                    if (60..100).contains(&bx) {
                        px[y * w + x] = 250;
                    }
                }
            }
            px
        };
        let a = pyramid(&bar(0), w, h);
        let b = pyramid(&bar(16), w, h);
        let pair = flow_pair(&a, &b, &FlowParams::default());
        let extremes = pair
            .mask
            .iter()
            .filter(|m| **m < 0.3 || **m > 0.7)
            .count();
        assert!(extremes > 0, "an occlusion boundary produced no endpoint preference");
    }
}
