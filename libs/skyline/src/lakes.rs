//! Lakes (plan hdri-6 §4): ellipses in bearing and distance over open land, at least LAKE_MIN_M x Size from the eye, a
//! few a turn (as many over Size: each is Size times as large). The water shader mirrors them as it mirrors the sea;
//! `cover` is its `lake_at` on the CPU (`band::the_shaders_lakes_are_the_cpus` holds the shader's formula to it).
use crate::params::{Params, P};
use crate::rng::{fract, mix, wd, TAU};
use crate::zones::Zones;

pub const LAKES_MAX: usize = 12;
/// The nearest a lake's shore comes to the eye, and the farthest its near shore lies at, size units (m at Size 1).
pub const LAKE_MIN_M: f64 = 200.0;
pub const LAKE_FAR_M: f64 = 2000.0;
/// The owner's R2: every lake at 2/3 of plan 6's distance and size (its half-depth, its width across), so it keeps its
/// bearing and its width in bearing and is 1.5x as deep in angle (LAKE_MIN_M and LAKE_FAR_M were 300 and 3000).
const NEARER: f64 = 2.0 / 3.0;

/// A lake on the ground plane: its centre's bearing (u, world azimuth) and distance (m), its half-width (a share of the
/// turn) and its half-depth (m, along the view).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lake { pub u: f64, pub d: f64, pub half_u: f64, pub half_d: f64 }

/// The cells of the turn a lake may stand in (10 degrees each), and the places a cell tries for its lake.
const CELLS: usize = 36;
const TRIES: usize = 3;

/// The lakes of `p` around `z` (`Zones::new` keeps them): a share of the 36 cells of the turn (the slider's, as many over
/// Size) each tries up to three places for a lake and keeps the first that fits (`place_all`). Every draw is a hash of
/// its cell and stream (layer 1950), so the zones' other layers never move, and a lake moves only when an earlier one
/// takes its place. Dense enough to be seen: at Lakes 0.6 and the default land the median of 40 seeds has three to four
/// lakes at least half a texel deep (`depth`) in a strip 1024 texels round the turn.
/// The water shader takes LAKES_MAX of them. Where more were placed (n), it keeps LAKES_MAX spread round the whole turn,
/// the (2 i + 1) n / (2 LAKES_MAX)-th in bearing order for i = 0 .. LAKES_MAX, so no stretch of the turn loses its lakes
/// to the cap (plan 6's final review, M5); and it says so in the log, once for each seed and count it binds at.
pub fn place(z: &Zones, p: &Params) -> Vec<Lake> {
    let all = place_all(z, p);
    let n = all.len();
    if n <= LAKES_MAX { return all; }
    log_cap(p.seed, n);
    (0..LAKES_MAX).map(|i| all[(2 * i + 1) * n / (2 * LAKES_MAX)]).collect()
}

/// Says in the log that the lake cap binds: `n` lakes placed for seed `seed`, LAKES_MAX kept. Once for each (seed, n) in
/// a row, so a held drag that keeps them does not repeat it.
fn log_cap(seed: u32, n: usize) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(u64::MAX);
    let key = (seed as u64) << 32 | n as u64;
    if LAST.swap(key, Ordering::Relaxed) != key {
        makepad_widgets::log!("skyline: seed {seed} places {n} lakes; the water shader takes {LAKES_MAX}, spread round the turn");
    }
}

/// Every lake `place` chooses from, in bearing order (cell by cell round the turn), each clear of the ones before it.
pub fn place_all(z: &Zones, p: &Params) -> Vec<Lake> {
    let (amount, k) = (p[P::PrLake], p[P::Size]);
    let mut out: Vec<Lake> = Vec::new();
    if amount <= 0.0 { return out; }
    for j in 0..CELLS {
        let jf = j as f64;
        if z.rnd(jf, 1.0, 1950.0) >= amount / k { continue; }
        let fits = |t: usize| {
            let [r2, r3, r4, r5] = [2.0, 3.0, 4.0, 5.0].map(|s| z.rnd(jf, s + 10.0 * t as f64, 1950.0));
            let u = fract((jf + 0.2 + 0.6 * r2) / CELLS as f64);
            // up to two thirds of a kilometre deep along the view, nearer ones more often, 133-400 m across
            let half_d = mix(100.0, 500.0, r3) * NEARER * k;
            let d = LAKE_MIN_M * k + half_d + (LAKE_FAR_M - LAKE_MIN_M) * k * r5 * r5 * r5;
            let half_u = mix(100.0, 300.0, r4) * NEARER * k / (TAU * d);
            // the whole of it on open land (outside the city core, the town, the industry, the fair and the bridge)
            let on_land = (0..5).all(|i| z.open_n(fract(u + (i as f64 / 2.0 - 1.0) * half_u)) > 0.6);
            // clear of the power lines (straight, nearest q[1] m out at bearing q[0]: the lake's centre lies d cos(the
            // bearing between) along the line's normal) by its own reach and 50 m
            let reach = half_d.max(half_u * TAU * d) + 50.0 * k;
            let off_lines = z.pyl.iter().all(|q| q[3] <= 0.0 || (d * (wd(u, q[0]) * TAU).cos() - q[1]).abs() > reach);
            // clear of the other lakes: apart in bearing, or (one behind the other) in distance (their gap scaled with them)
            let clear = out.iter().all(|o| wd(o.u, u).abs() >= o.half_u + half_u + 0.01 || (o.d - d).abs() >= o.half_d + half_d + 50.0 * NEARER * k);
            (on_land && off_lines && clear).then_some(Lake { u, d, half_u, half_d })
        };
        if let Some(l) = (0..TRIES).find_map(fits) { out.push(l); }
    }
    out
}

/// Whether the ground point al rad down at world azimuth u (eye `eye` m up) is in lake `l`.
pub fn inside(l: &Lake, u: f64, al: f64, eye: f64) -> bool {
    if al <= 0.0 { return false; }
    let (x, y) = (wd(u, l.u) / l.half_u, (eye / al.tan() - l.d) / l.half_d);
    x * x + y * y < 1.0
}

/// The lakes' share of a pixel px rad a side, al rad down at u: the shader's `lake_at`, on the CPU.
/// The lake's depth at the pixel's place across it is a range of depressions (its far and near shore there); the pixel
/// covers al - px / 2 .. al + px / 2 of them. Boxed in depression, not on the ground: most lakes are thinner than a texel,
/// and near the horizon a texel's ground depth is far from dist^2 / eye x px. The lakes never overlap (`place`), so their
/// shares add, also where one lies behind another in the same pixel; a lake whose sides the pixel is past adds none.
pub fn cover(lakes: &[Lake], u: f64, al: f64, px: f64, eye: f64) -> f64 {
    if al <= 0.0 { return 0.0; }
    lakes.iter().fold(0.0f64, |m, l| {
        let x = wd(u, l.u) / l.half_u;
        if x.abs() >= 1.0 { return m; }
        let hy = (1.0 - x * x).sqrt();
        // the far and the near shore at x (rad down), the pixel's share between them, times its share across
        let (a0, a1) = ((eye / (l.d + hy * l.half_d)).atan(), (eye / (l.d - hy * l.half_d)).atan());
        let cy = ((al + 0.5 * px).clamp(a0, a1) - (al - 0.5 * px).clamp(a0, a1)) / px;
        let cx = ((1.0 - x.abs()) / (px / TAU / l.half_u) + 0.5).clamp(0.0, 1.0);
        m + cx * cy
    }).min(1.0)
}

/// How deep lake `l` looks across its middle from an eye `eye` m up: its near shore's depression less its far shore's (rad).
pub fn depth(l: &Lake, eye: f64) -> f64 { (eye / (l.d - l.half_d)).atan() - (eye / (l.d + l.half_d)).atan() }

/// The lake texture's texels: LAKES_MAX x 1 RGBA (u, d, half_u, half_d), zeros past the last lake.
pub fn texels(lakes: &[Lake]) -> Vec<f32> {
    let mut t = vec![0f32; LAKES_MAX * 4];
    for (o, l) in t.chunks_mut(4).zip(lakes) { o.copy_from_slice(&[l.u as f32, l.d as f32, l.half_u as f32, l.half_d as f32]); }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    fn with(seed: u32, amount: f64, size: f64) -> (Params, Zones) {
        let mut p = Params::default();
        p.set_seed(seed);
        p.set(P::PrLake as usize, amount);
        p.set(P::Size as usize, size);
        let z = Zones::new(&p);
        (p, z)
    }
    /// Plan hdri-6 §4 and the owner's R2 (1.5x as deep: 2/3 of the distance): a lake's near shore lies 200 to 2000 m x
    /// Size out, and the lakes stand on open land, at any seed and Size. Over 40 seeds the nearest shores come within 250 m
    /// x Size and the farthest past 1500 m x Size (the range is used).
    #[test]
    fn lakes_keep_their_distance_and_their_land() {
        let (mut seen, mut nearest, mut farthest) = (0, f64::MAX, 0.0f64);
        for seed in 0..40 {
            for k in [0.5, 1.0, 2.0] {
                let (_, z) = with(seed, 1.0, k);
                assert!(z.lakes.len() <= LAKES_MAX);
                for l in &z.lakes {
                    let shore = l.d - l.half_d;
                    assert!(shore >= 200.0 * k - 1e-9 && shore <= 2000.0 * k + 1e-9, "seed {seed}, Size {k}: a shore {shore} m out");
                    for i in 0..5 { assert!(z.open_n(fract(l.u + (i as f64 / 2.0 - 1.0) * l.half_u)) > 0.6, "seed {seed}: a lake off the land"); }
                    (nearest, farthest) = (nearest.min(shore / k), farthest.max(shore / k));
                    seen += 1;
                }
            }
        }
        assert!(seen > 100, "the premise: lakes were placed ({seen})");
        assert!(nearest < 250.0 && farthest > 1500.0, "the shores span {nearest} .. {farthest} m x Size");
    }
    /// The final review's M5 (decision D5): where more than LAKES_MAX lakes are placed, the ones kept are spread round
    /// the whole turn, not the first LAKES_MAX in cell order, which left the end of the turn without lakes. Over 40 seeds
    /// at Lakes 1 and Size 0.5 (every cell tries) and 1: on the seeds the cap binds at, the kept lakes are LAKES_MAX of the
    /// placed ones in bearing order, the first and the last within n / LAKES_MAX of the turn's two ends (n placed) and
    /// no two that many placed lakes or more apart; below the cap every placed lake is kept, as before.
    #[test]
    fn the_lakes_kept_are_spread_round_the_turn() {
        let (mut bound, mut under, mut most) = (0, 0, 0);
        for seed in 0..40 {
            for k in [0.5, 1.0] {
                let (p, z) = with(seed, 1.0, k);
                let all = place_all(&z, &p);
                let n = all.len();
                if n <= LAKES_MAX {
                    assert_eq!(z.lakes, all, "seed {seed}, Size {k}: below the cap every lake is kept");
                    under += 1;
                    continue;
                }
                (bound, most) = (bound + 1, most.max(n));
                let at: Vec<usize> = z.lakes.iter().map(|l| all.iter().position(|a| a == l).expect("a kept lake is a placed one")).collect();
                let step = n.div_ceil(LAKES_MAX);
                assert_eq!(at.len(), LAKES_MAX, "seed {seed}, Size {k}");
                assert!(at[0] < step && at[LAKES_MAX - 1] >= n - step, "seed {seed}, Size {k}: of {n} placed the {at:?}-th kept, not to the turn's ends");
                assert!(at.windows(2).all(|w| w[1] > w[0] && w[1] - w[0] <= step), "seed {seed}, Size {k}: of {n} placed the {at:?}-th kept, not spread");
            }
        }
        println!("the cap binds at {bound} of 80 (seed, Size), at most {most} lakes placed; below it at {under}");
        assert!(bound > 10 && under > 10, "the premise: the cap binds at some and not at others ({bound}, {under})");
    }
    /// More with the slider, none at 0; Size times as large, about as many over Size.
    #[test]
    fn more_lakes_with_the_slider_fewer_with_size() {
        let count = |amount: f64, k: f64| (0..40).map(|s| with(s, amount, k).1.lakes.len()).sum::<usize>();
        assert_eq!(count(0.0, 1.0), 0);
        let (few, many, big) = (count(0.3, 1.0), count(1.0, 1.0), count(1.0, 2.0));
        println!("lakes over 40 seeds: 0.3 -> {few}, 1 -> {many}, 1 at Size 2 -> {big}");
        assert!(few > 0 && many > few && big < many);
    }
    /// A lake's cover is its area share of the texel (no single-texel shimmer): at the strip's 1024 the per-texel cover
    /// along a lake's centre column is the mean of 16 x 256 hard looks within 0.15, and the column's sum within 10 %. (The
    /// looks are 256 deep: most lakes are thinner than 1/16 of a texel at 1024, so 16 rows of looks quantise them.)
    #[test]
    fn a_lakes_cover_is_its_area_share_of_the_texel() {
        let (p, z) = with(7, 1.0, 1.0);
        let (px, eye) = (TAU / 1024.0, crate::props::kit::EYE * p[P::Size]);
        assert!(!z.lakes.is_empty());
        for l in &z.lakes {
            let (mut sum_c, mut sum_h) = (0.0, 0.0);
            for j in 0..40 {
                let al = (j as f64 + 0.5) * px;
                let c = cover(&z.lakes, l.u, al, px, eye);
                let mut h = 0.0;
                for sx in 0..16 { for sy in 0..256 {
                    let (u, a) = (l.u + (sx as f64 - 7.5) / 16.0 * px / TAU, al + (sy as f64 - 127.5) / 256.0 * px);
                    if z.lakes.iter().any(|m| inside(m, u, a, eye)) { h += 1.0 / 4096.0; }
                } }
                assert!((c - h).abs() <= 0.15, "lake at u {:.3}: texel {j} covers {c:.3}, its looks {h:.3}", l.u);
                (sum_c, sum_h) = (sum_c + c, sum_h + h);
            }
            assert!((sum_c - sum_h).abs() <= 0.1 * sum_h.max(0.05), "lake at u {:.3}: {sum_c} against {sum_h}", l.u);
        }
    }
}
