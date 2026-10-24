//! The aurora (plan hdri-7 §4, Task 4c), written from general techniques: thin field-aligned sheets on a curved Earth
//! whose footprints are meandering ribbons, their colour from the emission lines. No volume is marched: a view ray meets
//! each ribbon's sheet in closed form, once for every pass through it.
//!
//! - **Footprint.** Each ribbon's foot (its lower border, BORDER_KM up) is a polyline on the ground, in the map round
//!   the observer that keeps ground distances and azimuths: x along the arc, y toward the arc's middle at
//!   `direction_deg`. Unwarped it is the line y = D0, an arc in the sky that sinks to the horizon at both ends, its
//!   border at `height_deg` in the middle: D0 = R (acos(R cos e0 / (R + h0)) - e0), exact on a sphere. Folds warp it
//!   along its length (domain warping): a slow meander toward and away (S-bends) that the ribbons share, small bends of
//!   each ribbon's own, folds where it doubles back on itself (a Z on the ground, shared with jitter), and now and then
//!   a curl (a loop). The warps are pinned at the middle, so the border there stays at Height. Folds 0.1 to 0.7 bring
//!   in a second and a third ribbon behind the first, fainter.
//! - **Crossings.** The polyline's segments are indexed by azimuth; a view meets every segment its azimuth crosses, so
//!   along a fold it passes through the sheet three times. The sheet leans poleward with height (FIELD_TILT, a field
//!   dip of about 77 degrees): a crossing's distance grows by the lean / cos beta (beta the view's azimuth from the
//!   middle), exact where the ribbon runs along the arc.
//! - **Brightness.** A thin sheet's light is its column along the view: face-on 0.3 of the brightest (EDGE_MAX, the top
//!   of refs §5.1's 0.1 to 0.3), seen edge-on (a fold's turn, a curl, the arc's ends) up to all of it: 50 kR of green
//!   at Strength 1. Knots brighten and dim each ribbon along its length. The rays are field lines counted along the
//!   ribbon, soft, irregularly spaced and of varying length.
//! - **Colour.** The lines' heights above the border: nitrogen (427.8 nm and the first positive bands, pink together)
//!   in its bottom 12 km or so, the green 557.7 nm line from a few km up fading over 25 km, held off the bottom 7 km
//!   more where the nitrogen is strong (a storm's harder electrons), with the N2+ first negative bands (427.8 and
//!   470.9 nm) rising over it and fading slower, so the lime lower edge turns cyan-green, teal, then blue; the red
//!   630 nm line from about 150 km to 350 km with 427.8 nm over it, magenta-purple. Summed in XYZ per height and turned
//!   into Rec.709 once, luminance kept (`spectrum::xyz_to_rgb_keep_luminance`).
//! - **Band limits.** Every noise reads the lookup's texel (AURORA_BARE_TEXEL for the few looks round the horizon) as
//!   km along the ribbon, its octaves fading to their mean below two texels, and the rays fade out where a texel spans
//!   more than one ray (edge-on, or a bare lookup), so held and released bakes agree in their means.

use makepad_draw::*;
use super::noise::{fbm1_d, fbm1_frac_full, hash_u32, octaves_for};
use super::spectrum::{line_xyz_per_rayleigh, xyz_to_rgb_keep_luminance};
use super::{dir_from_az_el, AuroraParams};
use std::f32::consts::{PI, TAU};

/// The band limit of a bare lookup (the strip's rings, the dome, the night's quadrature): the rings' spacing.
pub(super) const AURORA_BARE_TEXEL: f32 = TAU / 64.0;
const R_KM: f32 = 6371.0;
/// The lower border's height, and how far its wave moves it (+-2 km over about 40 km along the ribbon).
const BORDER_KM: f32 = 100.0;
const WAVE_KM: f32 = 40.0;
const WAVE_DEPTH_KM: f32 = 4.0;
/// The second and third ribbons stand this much further poleward.
const ARC_STEP_KM: f32 = 60.0;
/// The sheet's poleward lean per km of height: cot of the field's dip (about 77 degrees).
const FIELD_TILT: f32 = 0.23;
/// Edge-on over face-on, at most: the face-on body is 0.3 of the brightest lines of sight (refs §5.1: 0.1 to 0.3).
const EDGE_MAX: f32 = 10.0 / 3.0;
/// Where a view grazes the sheet (a fold's turn seen edge-on, the cosine of its angle to the sheet's normal under
/// this) its column fades out, so a fold's outer edge is soft rather than a cut.
const TIP_SOFT: f32 = 0.35;
/// The footprint's shapes are sized for an arc SHAPE_KM away (its border 15 degrees up in the middle); a nearer arc,
/// higher in the sky, takes them smaller in proportion (down to SHAPE_MIN), so they keep their size in the sky and the
/// ribbon stays in front of the observer.
const SHAPE_KM: f32 = 340.0;
const SHAPE_MIN: f32 = 0.15;
/// The S-bends: the meander toward and away that the ribbons share, its wavelength along the arc and its depth at
/// Folds 1 (the depth goes with the square root of Folds).
const MEANDER_KM: f32 = 420.0;
const MEANDER_DEPTH_KM: f32 = 320.0;
/// Each ribbon's own small bends, both ways, at Folds 1.
const BEND_KM: f32 = 110.0;
const BEND_DEPTH_KM: f32 = 12.0;
/// The folds: in each FOLD_CELL_KM of the arc a fold with a chance of 0.5 + 0.45 Folds, a switchback where the ribbon
/// doubles back on itself: a U-turn of radius 6 to 20 km, a leg back along the arc of 40 to 140 km (x (0.7 + 0.3
/// Folds)), slightly bowed, and a U-turn the other way, so it goes on 2 (rho1 + rho2) further toward or away. The
/// ribbons share them, each a little moved and its leg a little longer or shorter.
const FOLD_CELL_KM: f32 = 340.0;
/// The curls: in each CURL_CELL_KM of a ribbon a loop of radius 12 to 30 km with a chance of 0.6 Folds, tighter on
/// one side (a spiral's start).
const CURL_CELL_KM: f32 = 520.0;
/// The polyline's step at the arc's distance, coarser further out in proportion to the distance; how far it reaches on
/// the ground (the top of the emission sets beyond about 2300 km); and its segments' azimuth bins.
const STEP_KM: f32 = 1.5;
const REACH_KM: f32 = 2400.0;
const BINS: usize = 1024;
/// The knots' wavelength along a ribbon.
const KNOT_KM: f32 = 150.0;
/// The rays' wavelength along the ribbon, the warp of their spacing (RAY_WARP of a wavelength over RAY_WARP_KM), and
/// the wavelength of their lengths (40 to 360 km over the border).
const RAY_KM: f32 = 9.0;
const RAY_WARP: f32 = 1.2;
const RAY_WARP_KM: f32 = 40.0;
const RAY_LENGTH_KM: f32 = 25.0;
/// The top of the emission, and the line table's span and size.
const TOP_KM: f32 = 400.0;
const TABLE_LOW_KM: f32 = 80.0;
const TABLE: usize = 128;
/// Rayleighs of a line at 1 of the mix on the brightest lines of sight (Strength 1).
const PEAK_RAYLEIGHS: f32 = 50_000.0;
/// The nitrogen unit: 427.8 nm at 20 kR and the first positive bands at 50 kR, in their two visible groups: 35 kR
/// round 650-690 nm (taken at 670 nm) and 15 kR round 580-610 nm (at 600 nm). The second group carries most of their
/// luminance, so with the violet the lower border reads pink, not a dim maroon seam.
const N2_VIOLET_SHARE: f32 = 0.4;
const N2_PINK_SHARE: f32 = 0.7;
const N2_PINK_NM: f32 = 670.0;
const N2_ORANGE_SHARE: f32 = 0.3;
const N2_ORANGE_NM: f32 = 600.0;
/// The nitrogen's height over the border and its spread (refs §5.1: from the border to about 15 km over it).
const N2_KM: f32 = 6.0;
const N2_SPREAD_KM: f32 = 5.0;
/// How far a storm (nitrogen 0.5 to 2 in the mix) holds the green off its bottom (refs §5.1: 5 to 10 km).
const GREEN_HOLD_KM: f32 = 7.0;
/// The N2+ first negative bands that come with the green (part of its unit): TEAL_SHARE rayleighs per rayleigh of the
/// green at its brightest, rising from TEAL_FROM_KM to TEAL_KM over the border and falling over TEAL_FALL_KM above it,
/// slower than the green (a sunlit curtain's tops scatter the bands' sunlight). In their two visible heads, 470.9 and
/// 427.8 nm, 3 to 1: the bands' own share is about 1 to 3, but with the violet head foremost the high curtain read
/// violet-purple where the photographs show blue. So the curtain runs from the lime green line at its lower edge
/// through cyan-green and teal to blue higher up, as the owner's photographs show it (2026-10-24).
const TEAL_SHARE: f32 = 0.25;
const TEAL_FROM_KM: f32 = 4.0;
const TEAL_KM: f32 = 30.0;
const TEAL_FALL_KM: f32 = 80.0;
const TEAL_VIOLET_SHARE: f32 = 0.25;
const TEAL_BLUE_SHARE: f32 = 0.75;
const TEAL_BLUE_NM: f32 = 470.9;
/// The 427.8 nm violet that comes with the red line, per rayleigh of 630 nm (part of its unit): the sunlit tops'
/// bands over the oxygen's red, so a high curtain reads magenta-purple, never deep red or maroon (owner, 2026-10-24).
const TOPS_VIOLET: f32 = 0.8;
/// The window's soft edge at the arc's ends, and the widest half-width.
const WINDOW_EDGE: f32 = 10.0 * PI / 180.0;
const MAX_HALF: f32 = 88.0 * PI / 180.0;
const SALT: u32 = 0x4155_524f;

/// A ribbon's footprint and its segments by azimuth.
#[derive(Clone, Debug)]
struct Footprint {
    /// From one end to the other: x, y (km) and the length along it to here (km).
    points: Vec<[f32; 3]>,
    /// The segments (the index of their first point) whose azimuths reach into bin b: `segments[start[b]..start[b + 1]]`.
    start: Vec<u32>,
    segments: Vec<u32>,
}

/// One ribbon (an arc of the aurora).
#[derive(Clone, Debug)]
struct Sheet {
    full: Footprint,
    /// What a bare lookup meets (the strip's rings, the dome, the night's quadrature: a few hundred looks, too few for
    /// the folds and the knots): the ribbon's meander alone, without its folds, curls and bends, carrying the folded,
    /// knotted ribbon's light (`bare_gain`). None at Folds 0, where the full footprint is its meander.
    bare: Option<Footprint>,
    bare_gain: f32,
    brightness: f32,
    /// The sine of the highest elevation any of its points reaches: above it a look skips the ribbon.
    top_sin: f32,
    ray_seed: u32,
    wave_seed: u32,
    knot_seed: u32,
}

/// Where a view ray meets a sheet: above the local border (km), the edge-on factor, the ray's and the knots'
/// modulation, and for the tests its height (km) and its field line's place along the ribbon (km).
#[derive(Clone, Copy, Debug)]
struct Hit {
    above: f32,
    edge: f32,
    ray: f32,
    knot: f32,
    #[cfg(test)]
    h: f32,
    #[cfg(test)]
    x: f32,
}

/// A fold or a curl of a footprint, `at` km along the arc.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// From `at` on, a switchback toward `side` (+-1): a U-turn of radius `rho[0]`, `leg` km back along the arc bowed
    /// `bow` km to the side at its middle, and a U-turn of radius `rho[1]` the other way; the arc's parameter runs as
    /// the length along it, and past it the ribbon goes on displaced.
    Fold { at: f32, rho: [f32; 2], leg: f32, bow: f32, side: f32 },
    /// Over `at` +- w a loop of radius `rho` (a prolate cycloid's under an envelope), to the side `side` (+-1).
    Curl { at: f32, w: f32, rho: f32, side: f32 },
}

impl Shape {
    /// What it adds to the footprint's (x, y) at `t` km along the arc.
    fn add(&self, t: f32, x: &mut f32, y: &mut f32) {
        match *self {
            Shape::Fold { at, rho, leg, bow, side } => {
                let s = t - at;
                if !(s > 0.0) {
                    return;
                }
                let (turned, back) = (PI * rho[0], PI * rho[0] + leg);
                let total = back + PI * rho[1];
                // the switchback's own shape from where it starts, s km along it
                let (fx, fy) = if s < turned {
                    let (sin_p, cos_p) = (s / rho[0]).sin_cos();
                    (rho[0] * sin_p, rho[0] * (1.0 - cos_p))
                } else if s < back {
                    let swell = (PI * (s - turned) / leg).sin();
                    (turned - s, 2.0 * rho[0] + bow * swell * swell)
                } else if s < total {
                    let (sin_p, cos_p) = ((s - back) / rho[1]).sin_cos();
                    (-leg - rho[1] * sin_p, 2.0 * rho[0] + rho[1] * (1.0 - cos_p))
                } else {
                    (s - total - leg, 2.0 * (rho[0] + rho[1]))
                };
                *x += fx - s;
                *y += side * fy;
            }
            Shape::Curl { at, w, rho, side } => {
                let q = (t - at) / w;
                if q.abs() < 1.0 {
                    let envelope = (1.0 - q * q) * (1.0 - q * q);
                    // rho omega 1.7 at the middle: the ribbon runs back there, a loop; tighter on one side
                    let r = rho * envelope * (1.0 + 0.35 * q);
                    let (sin_p, cos_p) = ((t - at) * 1.7 / rho).sin_cos();
                    *x -= r * sin_p;
                    *y += side * r * (1.0 - cos_p);
                }
            }
        }
    }
}

/// A hash's top 24 bits in [0, 1).
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// The folds and curls of ribbon `k` within `span` km of the middle (`sc` the shapes' scale): the folds from the
/// ribbons' `shared` seed, each ribbon's a little moved, the curls from its `own`.
fn shapes(folds: f32, sc: f32, span: f32, shared: u32, own: u32, k: u32) -> Vec<Shape> {
    let mut shapes = Vec::new();
    if !(folds > 0.0) {
        return shapes;
    }
    let cells = |cell: f32| {
        let n = (span / cell).ceil() as i32 + 1;
        -n..=n
    };
    // the folds cell by cell outward from the middle, each cell FOLD_CELL_KM along the arc plus the length its fold
    // takes of the parameter (the fold runs along itself there), so they stand about a cell apart along the arc
    let fold_cell = FOLD_CELL_KM * sc;
    for sign in [1.0f32, -1.0] {
        let (mut cursor, mut c) = (0.0f32, 0i32);
        while cursor < span {
            c += 1;
            let h = hash_u32(shared ^ ((sign as i32 * c) as u32).wrapping_mul(0x9e37_79b9));
            if !(unit(h) < 0.5 + 0.45 * folds) {
                cursor += fold_cell;
                continue;
            }
            let u = |salt: u32| unit(hash_u32(h ^ salt));
            let rho = [sc * (6.0 + 14.0 * u(1)), sc * (6.0 + 14.0 * u(2))];
            let mut leg = sc * (40.0 + 100.0 * u(3)) * (0.7 + 0.3 * folds);
            // bowed less than the nearer U-turn's radius, so the leg never meets the ribbon either side of it
            let bow = (2.0 * u(4) - 1.0) * 0.8 * rho[0].min(rho[1]);
            let side = if u(5) < 0.5 { -1.0 } else { 1.0 };
            let length = PI * (rho[0] + rho[1]) + leg;
            // where in its cell it starts, from the middle outward
            let from = cursor + 0.8 * u(6) * fold_cell;
            let mut at = if sign > 0.0 { from } else { -from - length };
            cursor += fold_cell + length;
            // each ribbon's own: a little moved, its leg a little longer or shorter
            if k > 0 {
                let mine = |salt: u32| unit(hash_u32(h ^ k.wrapping_mul(0x85eb_ca6b) ^ salt));
                at += (mine(7) - 0.5) * 0.3 * leg;
                leg *= 0.85 + 0.3 * mine(8);
            }
            shapes.push(Shape::Fold { at, rho, leg, bow, side });
        }
    }
    let curl_cell = CURL_CELL_KM * sc;
    for c in cells(curl_cell) {
        let h = hash_u32(own ^ (c as u32).wrapping_mul(0x9e37_79b9) ^ 0x4355_524c);
        if !(unit(h) < 0.6 * folds) {
            continue;
        }
        let u = |salt: u32| unit(hash_u32(h ^ salt));
        let rho = sc * (12.0 + 18.0 * u(1));
        let w = 3.0 * rho;
        let at = (c as f32 + 0.5) * curl_cell + (u(2) - 0.5) * (curl_cell - 2.2 * w);
        shapes.push(Shape::Curl { at, w, rho, side: if u(3) < 0.5 { -1.0 } else { 1.0 } });
    }
    shapes
}

/// Ribbon `k`'s footprint (its middle `d0 + k ARC_STEP_KM` away), from its one end to the other: along the arc past
/// the window `half` (or REACH_KM) either side, however much of the way its folds and curls take, its warps pinned at
/// the middle. Without `detail`, its meander alone (no folds, curls or bends).
fn footprint(d0: f32, k: u32, folds: f32, half: f32, shared: u32, own: u32, detail: bool) -> Vec<[f32; 3]> {
    let d = d0 + k as f32 * ARC_STEP_KM;
    let sc = (d0 / SHAPE_KM).clamp(SHAPE_MIN, 1.0);
    let reach = REACH_KM.min((d + 200.0 * sc) * (half + 3f32.to_radians()).min(MAX_HALF).tan()) + 100.0 * sc;
    // a fold takes its length of the arc's parameter and goes back by its leg: at Folds 1 about four times the reach
    let span = 4.0 * reach + 1000.0 * sc;
    let shapes = if detail { shapes(folds, sc, span, shared, own, k) } else { Vec::new() };
    let (meander, bend) = (MEANDER_DEPTH_KM * sc * folds.sqrt(), if detail { BEND_DEPTH_KM * sc * folds } else { 0.0 });
    let warp = |t: f32| {
        if !(folds > 0.0) {
            return (0.0, 0.0);
        }
        let m = t / (MEANDER_KM * sc);
        let twist = 1.5 * (fbm1_d(1.9 * m + 5.3, 2.0, shared ^ 0x5457_4953).0 - 0.5);
        let mut y = meander * (2.0 * fbm1_d(m + twist, 3.0, shared).0 - 1.0);
        let b = t / (BEND_KM * sc);
        let mut x = bend * (2.0 * fbm1_d(b, 2.0, own ^ 0x4245_4e58).0 - 1.0);
        y += bend * (2.0 * fbm1_d(b + 11.7, 2.0, own ^ 0x4245_4e59).0 - 1.0);
        for shape in &shapes {
            shape.add(t, &mut x, &mut y);
        }
        (x, y)
    };
    let pin = warp(0.0);
    let point = |t: f32| {
        let (x, y) = warp(t);
        [t + x - pin.0, d + y - pin.1]
    };
    let step = |p: [f32; 2]| STEP_KM * sc * ((p[0] * p[0] + p[1] * p[1]).sqrt() / d).max(1.0);
    let walk = |sign: f32| {
        let (mut t, mut p, mut out) = (0.0f32, point(0.0), Vec::new());
        while sign * p[0] < reach && t < span {
            t += step(p);
            p = point(sign * t);
            out.push(p);
        }
        out
    };
    let (left, right) = (walk(-1.0), walk(1.0));
    let mut points: Vec<[f32; 3]> = Vec::with_capacity(left.len() + right.len() + 1);
    let mut length = 0.0f32;
    let mid = point(0.0);
    for p in left.iter().rev().chain(std::iter::once(&mid)).chain(right.iter()) {
        if let Some(last) = points.last() {
            length += ((p[0] - last[0]).powi(2) + (p[1] - last[1]).powi(2)).sqrt();
        }
        points.push([p[0], p[1], length]);
    }
    // the middle at length 0, so the noises along the ribbon do not move with its far end
    let middle = points[left.len()][2];
    for p in &mut points {
        p[2] -= middle;
    }
    points
}

/// The light a footprint's sheet puts on flat ground, but for its height profile: an optically thin sheet's element
/// sends its emission over the square of its distance, from about h / r up, so about its length over the cube of its
/// ground distance, times its knots (`knot` of its length along the ribbon), summed inside the window `half` (with
/// the window's soft edge). Relative: for a bare lookup's stand-in's gain.
fn light(points: &[[f32; 3]], half: f32, knot: &dyn Fn(f32) -> f32) -> f32 {
    points
        .windows(2)
        .filter(|w| w[0][1] > 0.0 && w[1][1] > 0.0)
        .map(|w| {
            let (x, y) = (0.5 * (w[0][0] + w[1][0]), 0.5 * (w[0][1] + w[1][1]));
            let window = 1.0 - smoothstep((half - WINDOW_EDGE).max(0.0), half, x.atan2(y).abs());
            let r2 = x * x + y * y;
            (w[1][2] - w[0][2]) * window * knot(0.5 * (w[0][2] + w[1][2])) / (r2 * r2.sqrt())
        })
        .sum()
}

/// The knots' brightness at `kv` (their fbm, 0..1) and depth `depth`: (2 kv)^2 / 1.25 at depth 1, about 1 on average
/// (a 3-octave fbm's mean square is about 0.31), 0 to 3.2 at its ends.
fn knot(depth: f32, kv: f32) -> f32 {
    1.0 + depth * (3.2 * kv * kv - 1.0)
}

/// `points`' segments by azimuth bin within the window `half` (`scale` bins a radian, from -half), and the nearest
/// ground distance of a point inside the window.
fn index(points: Vec<[f32; 3]>, half: f32, scale: f32) -> (Footprint, f32) {
    let bin = |beta: f32| (((beta + half) * scale).max(0.0) as usize).min(BINS - 1);
    let ranges: Vec<Option<(usize, usize)>> = points
        .windows(2)
        .map(|w| {
            if !(w[0][1] > 0.0 && w[1][1] > 0.0) {
                return None;
            }
            let (a, b) = (w[0][0].atan2(w[0][1]), w[1][0].atan2(w[1][1]));
            let (lo, hi) = (a.min(b), a.max(b));
            (hi > -half && lo < half).then(|| (bin(lo), bin(hi)))
        })
        .collect();
    let mut start = vec![0u32; BINS + 1];
    for (lo, hi) in ranges.iter().flatten() {
        for b in *lo..=*hi {
            start[b + 1] += 1;
        }
    }
    for b in 0..BINS {
        start[b + 1] += start[b];
    }
    let mut fill = start.clone();
    let mut segments = vec![0u32; start[BINS] as usize];
    for (i, range) in ranges.iter().enumerate() {
        if let Some((lo, hi)) = range {
            for b in *lo..=*hi {
                segments[fill[b] as usize] = i as u32;
                fill[b] += 1;
            }
        }
    }
    let nearest = points
        .iter()
        .filter(|p| p[1] > 0.0 && p[0].atan2(p[1]).abs() < half)
        .map(|p| (p[0] * p[0] + p[1] * p[1]).sqrt())
        .fold(f32::MAX, f32::min);
    (Footprint { points, start, segments }, nearest)
}

/// The aurora of one map, built once per map from the block as clamp() holds it.
#[derive(Clone, Debug)]
pub(super) struct Aurora {
    phi0: f32,
    half: f32,
    /// Azimuth bins a radian.
    bin_scale: f32,
    rays: f32,
    /// How deep the knots go: 0.5 at Folds 0 to 1 at Folds 1.
    knots: f32,
    gain: f32,
    arcs: Vec<Sheet>,
    /// The sheet's face-on radiance by height above the border (BORDER_KM + dh), TABLE_LOW_KM to TOP_KM.
    table: [Vec3f; TABLE],
    middle: Vec3f,
}

impl Aurora {
    /// `gain`: strength x the night's lift x the night factor (none is built at 0).
    pub(super) fn new(p: &AuroraParams, gain: f32) -> Aurora {
        // the border's ground distance, exact on a sphere: R cos e0 / cos(e0 + D0 / R) = R + h0
        let e0 = p.height_deg.to_radians();
        let d0 = R_KM * ((R_KM * e0.cos() / (R_KM + BORDER_KM)).acos() - e0);
        let half = (0.5 * p.width_deg).to_radians().min(MAX_HALF);
        let bin_scale = BINS as f32 / (2.0 * half);
        let seed = hash_u32(p.seed ^ SALT);
        let shared = hash_u32(seed ^ 0x5348_4152);
        let knots = 0.5 + 0.5 * p.folds;
        let arcs = (0..3u32)
            .filter_map(|k| {
                let fade = match k {
                    0 => 1.0,
                    1 => smoothstep(0.1, 0.4, p.folds),
                    _ => smoothstep(0.25, 0.7, p.folds),
                };
                if !(fade > 0.0) {
                    return None;
                }
                let s = hash_u32(seed.wrapping_add(k.wrapping_mul(0x9e37_79b9)));
                let knot_seed = hash_u32(s ^ 0x4b4e_4f54);
                let points = footprint(d0, k, p.folds, half, shared, s, true);
                // the folded ribbon's light with its knots, which a bare lookup leaves out
                let full_light = light(&points, half, &|l| knot(knots, fbm1_d(l / KNOT_KM, 3.0, knot_seed).0));
                let (full, mut nearest) = index(points, half, bin_scale);
                let (bare, bare_light) = if p.folds > 0.0 {
                    let points = footprint(d0, k, p.folds, half, shared, s, false);
                    let bare_light = light(&points, half, &|_| 1.0);
                    let (bare, near) = index(points, half, bin_scale);
                    nearest = nearest.min(near);
                    (Some(bare), bare_light)
                } else {
                    (None, light(&full.points, half, &|_| 1.0))
                };
                let bare_gain = full_light / bare_light;
                let bare_gain = if bare_gain.is_finite() && bare_gain > 0.0 { bare_gain } else { 1.0 };
                // the highest any of its points stands: its top at its nearest
                let a = nearest.max(20.0) / R_KM;
                let top = ((R_KM + TOP_KM) * a.cos() - R_KM).atan2((R_KM + TOP_KM) * a.sin());
                Some(Sheet {
                    full,
                    bare,
                    bare_gain,
                    brightness: 0.6f32.powi(k as i32) * fade,
                    top_sin: top.sin(),
                    ray_seed: hash_u32(s ^ 0x5241_5953),
                    wave_seed: hash_u32(s ^ 0x5741_5645),
                    knot_seed,
                })
            })
            .collect();
        Aurora {
            phi0: p.direction_deg.to_radians(),
            half,
            bin_scale,
            rays: p.rays,
            knots,
            gain,
            arcs,
            table: line_table(p.line_mix()),
            middle: dir_from_az_el(p.direction_deg, p.height_deg),
        }
    }

    /// The arcs' radiance along `d` (map frame), before extinction and the horizon fade; 0 below the horizon and off
    /// the arc. `texel` band-limits the knots, the rays and the border's wave.
    pub(super) fn at(&self, d: Vec3f, texel: f32) -> Vec3f {
        let horizontal = (d.x * d.x + d.z * d.z).sqrt();
        if !(d.y > 0.0) || !(horizontal > 1.0e-6) || !(self.gain > 0.0) {
            return Vec3f::default();
        }
        let length = (horizontal * horizontal + d.y * d.y).sqrt();
        let (sin_e, cos_e) = (d.y / length, horizontal / length);
        let beta = (d.x.atan2(-d.z) - self.phi0 + PI).rem_euclid(TAU) - PI;
        if !(beta.abs() < self.half) {
            return Vec3f::default();
        }
        let window = 1.0 - smoothstep((self.half - WINDOW_EDGE).max(0.0), self.half, beta.abs());
        let (sin_b, cos_b) = beta.sin_cos();
        let mut sum = Vec3f::default();
        for arc in &self.arcs {
            if sin_e > arc.top_sin {
                continue;
            }
            self.hits(arc, sin_e, cos_e, beta, sin_b, cos_b, texel, |hit| {
                sum += self.profile(BORDER_KM + hit.above) * (hit.ray * hit.edge * hit.knot * arc.brightness);
            });
        }
        sum * (window * self.gain)
    }

    /// The arc's middle at its lower border, map frame.
    pub(super) fn middle(&self) -> Vec3f {
        self.middle
    }

    /// Where the azimuth beta from the middle crosses `foot`: for each crossing, its ground distance (km), its length
    /// along the ribbon (km), the cosine of the view's angle to the sheet's normal, and the ribbon's x per km along it.
    fn crossings(&self, foot: &Footprint, beta: f32, sin_b: f32, cos_b: f32, mut each: impl FnMut(f32, f32, f32, f32)) {
        let bin = (((beta + self.half) * self.bin_scale).max(0.0) as usize).min(BINS - 1);
        for &i in &foot.segments[foot.start[bin] as usize..foot.start[bin + 1] as usize] {
            let (p, q) = (foot.points[i as usize], foot.points[i as usize + 1]);
            let (dx, dy, length) = (q[0] - p[0], q[1] - p[1], q[2] - p[2]);
            let cross = sin_b * dy - cos_b * dx;
            let s = (cos_b * p[0] - sin_b * p[1]) / cross;
            if !(s >= 0.0 && s < 1.0 && length > 0.0) {
                continue;
            }
            let r0 = sin_b * (p[0] + s * dx) + cos_b * (p[1] + s * dy);
            if r0 > 0.0 {
                each(r0, p[2] + s * length, cross.abs() / length, dx / length);
            }
        }
    }

    /// Every place where the view at elevation (sin_e, cos_e) and azimuth beta from the middle meets `arc`'s sheet
    /// between its border and its top; a bare lookup's view (`texel` of AURORA_BARE_TEXEL or more) meets its stand-in.
    #[allow(clippy::too_many_arguments)]
    fn hits(&self, arc: &Sheet, sin_e: f32, cos_e: f32, beta: f32, sin_b: f32, cos_b: f32, texel: f32, mut each: impl FnMut(Hit)) {
        let e = sin_e.atan2(cos_e);
        // the height a view at e reaches over ground distance r, exact on a sphere
        let height = |r: f32| {
            let c = (e + r / R_KM).cos();
            if c > 0.02 { R_KM * cos_e / c - R_KM } else { f32::INFINITY }
        };
        let lean = FIELD_TILT / cos_b;
        let (foot, gain) = match &arc.bare {
            _ if texel < AURORA_BARE_TEXEL => (&arc.full, 1.0),
            Some(bare) => (bare, arc.bare_gain),
            None => (&arc.full, arc.bare_gain),
        };
        self.crossings(foot, beta, sin_b, cos_b, |r0, l, cos_a, run| {
            // km along the ribbon a texel spans: an azimuth step of texel / cos e, over the ribbon's slant to the view
            let along = texel * r0 / (cos_e.max(0.05) * cos_a.max(0.05));
            let border = BORDER_KM + WAVE_DEPTH_KM * (fbm1_d(l / WAVE_KM, octaves_for(WAVE_KM, along, 3.0), arc.wave_seed).0 - 0.5);
            // r = r0 + (h(r) - border) x lean, three fixed-point steps from the footprint
            let mut r = r0;
            for _ in 0..3 {
                r = r0 + (height(r) - border).min(TOP_KM) * lean;
            }
            let h = height(r);
            let above = h - border;
            if !(above > -2.0 && h < TOP_KM) {
                return;
            }
            // the path through a thin vertical sheet, capped: 1 / (cos alpha cos e), alpha the view's angle to its normal;
            // where the view grazes a fold's turn it fades out over TIP_SOFT, the curtain's soft edge there
            let k = cos_a * cos_e;
            let edge = gain * smoothstep(0.0, TIP_SOFT, cos_a) / (k * k + 1.0 / (EDGE_MAX * EDGE_MAX)).sqrt();
            // the field line through the hit, counted along the ribbon: the hit less the lean, onto the ribbon
            let x = l + (r - r0) * sin_b * run;
            let ray_along = texel * r / (cos_e.max(0.05) * cos_a.max(0.05));
            // the rays fade out where a texel spans more than one of them, gone at two: edge-on, or a bare lookup (face-on
            // the held and the released bakes keep them alike)
            let keep = smoothstep(0.5, 1.0, RAY_KM / ray_along);
            let q = x / RAY_KM + RAY_WARP * (fbm1_d(x / RAY_WARP_KM, 2.0, arc.ray_seed ^ 0x5741_5250).0 - 0.5);
            // on the full three octaves' scale, so a coarser bake keeps each stretch's mean (its first octave as it was)
            let v = fbm1_frac_full(q, octaves_for(RAY_KM, ray_along, 3.0), 3.0, arc.ray_seed);
            let ray = 1.0 + self.rays * keep * (2.0 * v - 1.0) * self.ray_reach(arc, x, above);
            let kv = fbm1_d(x / KNOT_KM, octaves_for(KNOT_KM, along, 3.0), arc.knot_seed).0;
            // the knots fade out as a texel grows from an eighth to a quarter of one (a bare lookup: its gain carries them)
            let knot = knot(self.knots * smoothstep(4.0, 8.0, KNOT_KM / along), kv);
            each(Hit {
                above,
                edge,
                ray,
                knot,
                #[cfg(test)]
                h,
                #[cfg(test)]
                x,
            });
        });
    }

    /// How strongly the rays streak `arc` `above` km over its border on the field line `x` km along it: weaker near the
    /// border, and gone past each ray's own length (40 to 360 km over the border).
    fn ray_reach(&self, arc: &Sheet, x: f32, above: f32) -> f32 {
        let length = 40.0 + 320.0 * fbm1_d(x / RAY_LENGTH_KM + 3.7, 2.0, arc.ray_seed ^ 0x4c45_4e47).0;
        (0.5 + above.max(0.0) / 120.0).min(1.0) * (1.0 - smoothstep(0.5 * length, length, above))
    }

    /// The sheet's face-on radiance at `h` km (the table read linearly; 0 outside it).
    fn profile(&self, h: f32) -> Vec3f {
        let x = (h - TABLE_LOW_KM) / (TOP_KM - TABLE_LOW_KM) * TABLE as f32 - 0.5;
        if !(x > -0.5 && x < TABLE as f32 - 0.5) {
            return Vec3f::default();
        }
        let x = x.max(0.0);
        let i = (x as usize).min(TABLE - 2);
        let t = (x - i as f32).min(1.0);
        self.table[i] + (self.table[i + 1] - self.table[i]) * t
    }

    /// The brightest line of sight's luminance (a sheet edge-on at its brightest height), gain in. For the tests.
    #[cfg(test)]
    fn brightest(&self) -> f32 {
        self.table.iter().map(|c| crate::sky::luminance(*c)).fold(0.0, f32::max) * EDGE_MAX * self.gain
    }

    /// `crossings`' (ground distance, length along the ribbon) at `beta`. For the tests.
    #[cfg(test)]
    fn crossings_at(&self, arc: &Sheet, beta: f32) -> Vec<(f32, f32)> {
        let (sin_b, cos_b) = beta.sin_cos();
        let mut out = Vec::new();
        self.crossings(&arc.full, beta, sin_b, cos_b, |r0, l, _, _| out.push((r0, l)));
        out
    }

    /// `hits` at azimuth `beta`, collected. For the tests.
    #[cfg(test)]
    fn hits_at(&self, arc: &Sheet, sin_e: f32, cos_e: f32, beta: f32, texel: f32) -> Vec<Hit> {
        let (sin_b, cos_b) = beta.sin_cos();
        let mut out = Vec::new();
        self.hits(arc, sin_e, cos_e, beta, sin_b, cos_b, texel, |hit| out.push(hit));
        out
    }

    /// The direction (map frame) of `arc`'s point at azimuth `beta` (radians) from the middle and `h` km up, on its
    /// nearest crossing there: `hits`' inverse at texel 0. For the tests.
    #[cfg(test)]
    fn sheet_dir(&self, arc: &Sheet, beta: f32, h: f32) -> Vec3f {
        let (r0, l) = self.crossings_at(arc, beta).into_iter().min_by(|p, q| p.0.total_cmp(&q.0)).expect("the ribbon crosses beta");
        let border = BORDER_KM + WAVE_DEPTH_KM * (fbm1_d(l / WAVE_KM, 3.0, arc.wave_seed).0 - 0.5);
        let r = r0 + (h - border).min(TOP_KM) * FIELD_TILT / beta.cos();
        let a = r / R_KM;
        let e = ((R_KM + h) * a.cos() - R_KM).atan2((R_KM + h) * a.sin());
        dir_from_az_el((self.phi0 + beta).to_degrees(), e.to_degrees())
    }
}

/// The sheet's face-on radiance by height (TABLE entries from TABLE_LOW_KM to TOP_KM, the border at BORDER_KM) for the
/// line amounts `mix` (green, red, nitrogen), each x PEAK_RAYLEIGHS on the brightest lines of sight: face-on an
/// EDGE_MAX-th of it.
fn line_table(mix: [f32; 3]) -> [Vec3f; TABLE] {
    let face = PEAK_RAYLEIGHS / EDGE_MAX;
    let hold = GREEN_HOLD_KM * smoothstep(0.5, 2.0, mix[2]);
    let (green, red, violet, pink) = (line_xyz_per_rayleigh(557.7), line_xyz_per_rayleigh(630.0), line_xyz_per_rayleigh(427.8), line_xyz_per_rayleigh(N2_PINK_NM));
    let (orange, blue) = (line_xyz_per_rayleigh(N2_ORANGE_NM), line_xyz_per_rayleigh(TEAL_BLUE_NM));
    std::array::from_fn(|i| {
        let h = TABLE_LOW_KM + (i as f32 + 0.5) * (TOP_KM - TABLE_LOW_KM) / TABLE as f32;
        let dh = h - BORDER_KM;
        let border = smoothstep(-1.5, 0.5, dh);
        let g = smoothstep(2.0 + hold, 12.0 + hold, dh) * (-(dh - 12.0 - hold).max(0.0) / 25.0).exp() * (1.0 - smoothstep(180.0, 220.0, h));
        let high = 1.0 - smoothstep(320.0, 350.0, h);
        let t = TEAL_SHARE * smoothstep(TEAL_FROM_KM, TEAL_KM, dh) * (-(dh - TEAL_KM).max(0.0) / TEAL_FALL_KM).exp() * high;
        let q = (h / 220.0).ln() / 0.18;
        let r = (-0.5 * q * q).exp() * smoothstep(140.0, 170.0, h) * high;
        let w = (dh - N2_KM) / N2_SPREAD_KM;
        let n = (-0.5 * w * w).exp();
        let k = face * border;
        let xyz: [f32; 3] = std::array::from_fn(|c| {
            let green_unit = g * green[c] + t * (TEAL_VIOLET_SHARE * violet[c] + TEAL_BLUE_SHARE * blue[c]);
            let red_unit = r * (red[c] + TOPS_VIOLET * violet[c]);
            k * (mix[0] * green_unit + mix[1] * red_unit + mix[2] * n * (N2_VIOLET_SHARE * violet[c] + N2_PINK_SHARE * pink[c] + N2_ORANGE_SHARE * orange[c]))
        });
        xyz_to_rgb_keep_luminance(xyz)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::atmosphere::SUN_IRRADIANCE;
    use crate::sky::luminance;

    fn g0() -> f32 {
        luminance(vec3f(0.02, 0.03, 0.06)) * 5.0e-8 * SUN_IRRADIANCE
    }

    fn calm(p: AuroraParams) -> Aurora {
        Aurora::new(&AuroraParams { folds: 0.0, rays: 0.0, ..p }, 1.0)
    }

    /// Plan hdri-7 §4, §5 (refs §5.1): at Strength 1, classic, the brightest lines of sight (a sheet seen edge-on)
    /// carry 50 kR of green: about 51 G0, within 10 %.
    #[test]
    fn the_brightest_line_of_sight_is_50_kr_of_green() {
        let a = Aurora::new(&AuroraParams::default(), 1.0);
        let peak = a.brightest() / g0();
        assert!((peak / 51.0 - 1.0).abs() <= 0.1, "{peak} G0");
    }

    /// The colour by height comes from the lines, as the owner's photographs show it (2026-10-24): every look's
    /// brightest, lower edge is lime (green over red over blue), its body 40 km over the border cyan-green to teal
    /// (green over blue over red), and blue 75 km up; no look is ever red or maroon (red over green with little blue)
    /// or grey (its hues cancelling) where it shows at 1 % of its peak. Classic carries at most a trace of violet at the
    /// top; magenta tops run on into a faint magenta-purple; the storm's lower border is pink-violet (red and blue over
    /// green) and green above it.
    #[test]
    fn each_preset_has_its_colours_by_height() {
        let at = |color: &str| Aurora::new(&AuroraParams { color: color.to_string(), ..AuroraParams::default() }, 1.0);
        let height = |i: usize| TABLE_LOW_KM + (i as f32 + 0.5) * (TOP_KM - TABLE_LOW_KM) / TABLE as f32;
        for color in ["classic", "magenta tops", "storm"] {
            let a = at(color);
            let lime = a.table.iter().copied().fold(Vec3f::default(), |b, c| if luminance(c) > luminance(b) { c } else { b });
            assert!(lime.y > lime.x && lime.x > lime.z, "{color} at its brightest: {lime:?}");
            let body = a.profile(BORDER_KM + 40.0);
            assert!(body.y > body.z && body.z > body.x, "{color} 40 km over the border: {body:?}");
            let high = a.profile(BORDER_KM + 75.0);
            assert!(high.z > high.y && high.z > high.x, "{color} 75 km over the border: {high:?}");
            let peak = luminance(lime);
            for i in 0..TABLE {
                let c = a.table[i];
                if luminance(c) < 0.01 * peak {
                    continue;
                }
                let (most, least) = (c.x.max(c.y).max(c.z), c.x.min(c.y).min(c.z));
                assert!(least < 0.55 * most, "{color} at {} km is grey: {c:?}", height(i));
                assert!(!(c.x > c.y && c.x > 2.0 * c.z), "{color} at {} km is red: {c:?}", height(i));
            }
        }
        let classic = at("classic");
        for i in 0..TABLE {
            let c = classic.table[i];
            if height(i) > BORDER_KM + 10.0 {
                assert!(c.x <= 0.6 * c.y.max(c.z), "classic at {} km carries more than a trace of magenta: {c:?}", height(i));
            }
        }
        let tops = at("magenta tops");
        let (high, low) = (tops.profile(230.0), tops.profile(120.0));
        assert!(high.x > high.y && high.z > high.y && high.z > 0.6 * high.x, "magenta tops at 230 km: {high:?}");
        assert!(low.y > low.x && low.y > low.z, "magenta tops at 120 km: {low:?}");
        // refs §5.1: 630 nm at 10 to 15 kR face-on, about 2.4 to 3.6 G0, at the default Strength (3), over the green's
        // end at 220 km: faint beside the green's 46 G0 face-on (the table is at Strength 1)
        let top = (210..=300).map(|h| luminance(tops.profile(h as f32))).fold(0.0, f32::max) * AuroraParams::default().strength / g0();
        println!("magenta tops: {top:.2} G0 over 210 km at the default Strength, at 230 km {high:?}");
        assert!((2.4..=3.6).contains(&top), "magenta tops' brightest above 210 km at the default Strength: {top} G0");
        let storm = at("storm");
        // refs §5.1: the nitrogen fills the bottom of a storm's sheet, the green held off it for 5 to 10 km, so its
        // fringe is several km tall (a few texels at 1024), not a seam
        for dh in [1.0f32, 4.0, 7.0] {
            let border = storm.profile(BORDER_KM + dh);
            assert!(border.x > border.y && border.z > border.y, "the storm's lower border, {dh} km up: {border:?}");
        }
        let above = storm.profile(BORDER_KM + 15.0);
        assert!(above.y > above.x && above.y > above.z, "15 km up: {above:?}");
        // and bright enough to read pink beside the green over it (the first positive bands' 600 nm group), not a
        // maroon line at a thirtieth of it
        let lum_over = |a: &Aurora, hs: std::ops::Range<i32>| hs.map(|dh| luminance(a.profile(BORDER_KM + dh as f32 * 0.5))).fold(0.0, f32::max);
        let (fringe, body) = (lum_over(&storm, 0..16), lum_over(&storm, 0..200));
        println!("the storm's pink fringe {:.2} G0 under its green {:.2} G0", fringe / g0(), body / g0());
        assert!(fringe >= 0.15 * body, "the storm's fringe {fringe} against its body {body}");
    }

    /// The arc stands where its settings say: its lower border at its middle is at Height within half a degree, it sinks
    /// toward its ends, nothing shows outside Width, and Direction turns it.
    #[test]
    fn the_arc_sits_where_its_settings_say() {
        let a = calm(AuroraParams::default());
        let arc = &a.arcs[0];
        let border_el = |beta: f32| {
            let mut e = 0.5f32;
            while e < 60.0 {
                let (sin_e, cos_e) = e.to_radians().sin_cos();
                if a.hits_at(arc, sin_e, cos_e, beta.to_radians(), 0.0).iter().any(|hit| hit.above >= 0.0) {
                    return e;
                }
                e += 0.01;
            }
            f32::NAN
        };
        let middle = border_el(0.0);
        assert!((middle - 15.0).abs() <= 0.5, "the border at the middle stands at {middle} degrees");
        assert!(border_el(40.0) < middle - 2.0, "it sinks toward its ends: {} at 40 degrees", border_el(40.0));
        let up = |az: f32| a.at(dir_from_az_el(az, middle + 3.0), 0.0);
        assert!(luminance(up(0.0)) > 0.0);
        assert_eq!(up(51.0), Vec3f::default(), "outside its 100 degrees");
        assert_eq!(up(-51.0), Vec3f::default());
        let turned = calm(AuroraParams { direction_deg: 90.0, ..AuroraParams::default() });
        for (az, el) in [(0.0f32, 16.0f32), (20.0, 18.0), (-30.0, 14.0)] {
            let (x, y) = (a.at(dir_from_az_el(az, el), 0.0), turned.at(dir_from_az_el(az + 90.0, el), 0.0));
            assert!((x - y).length() <= 1.0e-4 * x.length().max(1.0e-30), "{az}: {x:?} vs {y:?}");
        }
        assert!((a.middle() - dir_from_az_el(0.0, 15.0)).length() < 1.0e-6);
    }

    /// A view ray meets the sheet where the sheet stands (the closed form's round trip; on a folded ribbon, one of the
    /// view's crossings), and a ray (a fixed field line along the calm arc) leans toward the arc's middle as it rises:
    /// the sheet is tilted poleward along the field.
    #[test]
    fn the_sheets_are_met_where_they_stand_and_lean_with_the_field() {
        let met = |a: &Aurora, arc: &Sheet, beta: f32, h: f32| {
            let d = a.sheet_dir(arc, beta.to_radians(), h);
            let hits = a.hits_at(arc, d.y, (d.x * d.x + d.z * d.z).sqrt(), beta.to_radians(), 0.0);
            hits.into_iter().min_by(|p, q| (p.h - h).abs().total_cmp(&(q.h - h).abs())).unwrap_or_else(|| panic!("{beta} {h}: no hit"))
        };
        let a = Aurora::new(&AuroraParams { folds: 0.6, ..AuroraParams::default() }, 1.0);
        for beta in [-45.0f32, -20.0, 0.0, 25.0, 40.0] {
            for h in [105.0f32, 150.0, 250.0] {
                let hit = met(&a, &a.arcs[0], beta, h);
                assert!((hit.h - h).abs() < 2.0, "beta {beta}, {h} km: met at {} km", hit.h);
            }
        }
        let flat = calm(AuroraParams::default());
        let x_at = |h: f32| met(&flat, &flat.arcs[0], 40.0, h).x;
        assert!(x_at(200.0) > x_at(110.0) + 10.0, "a fixed azimuth reaches further along the arc higher up: the ray at a fixed x leans in");
    }

    /// Plan hdri-7 D6, Task 4c: a sheet seen edge-on is brighter than face-on: toward the arc's ends, and at the folds,
    /// where a view passes through the doubled-back sheet more than once. The folds' part sums the first arc's own column
    /// over its crossings (`hits`' edges), so the second and third arcs, which Folds 1 brings in, cannot pass it alone.
    #[test]
    fn a_fold_seen_edge_on_and_the_arcs_ends_are_brighter() {
        let brightest = |a: &Aurora, betas: std::ops::RangeInclusive<i32>| {
            let mut most = 0.0f32;
            for beta in betas {
                for e in 0..160 {
                    most = most.max(luminance(a.at(dir_from_az_el(beta as f32 * 0.5, e as f32 * 0.25), 0.0)));
                }
            }
            most
        };
        let flat = calm(AuroraParams { width_deg: 180.0, ..AuroraParams::default() });
        let (middle, end) = (brightest(&flat, -2..=2), brightest(&flat, 118..=122));
        assert!(end > 1.5 * middle, "the arc's end {end} against its middle {middle}");
        // the first arc's column (its crossings' edge-on factors summed) within 40 degrees of the middle, a few degrees up
        let most_edge = |a: &Aurora| {
            let mut most = 0.0f32;
            for beta in -80..=80 {
                for e in [16.0f32, 20.0, 25.0] {
                    let (sin_e, cos_e) = e.to_radians().sin_cos();
                    let column: f32 = a.hits_at(&a.arcs[0], sin_e, cos_e, (beta as f32 * 0.5).to_radians(), 0.0).iter().map(|hit| hit.edge).sum();
                    most = most.max(column);
                }
            }
            most
        };
        let folded = Aurora::new(&AuroraParams { width_deg: 180.0, folds: 1.0, rays: 0.0, ..AuroraParams::default() }, 1.0);
        let (flat_edge, folded_edge) = (most_edge(&flat), most_edge(&folded));
        println!("the first arc's largest column within 40 degrees: flat {flat_edge:.2}, folded {folded_edge:.2}");
        assert!(folded_edge > 2.0 * flat_edge, "folds seen edge-on brighten the arc: {folded_edge} against {flat_edge}");
    }

    /// Plan hdri-7 §4 (spec: "Rays: the vertical ray streaks"; "Seed"): the rays streak the curtain along the arc and
    /// keep its mean, and the seed lays them out. With no folds (one arc), at 18, 21 and 25 degrees (the border at 15
    /// in the middle), over 20 degrees of azimuth round the middle in 0.05-degree steps: the rays' own modulation (the
    /// luminance at Rays 1 over that at Rays 0, which carries the knots and the border's wave) varies by 0.1 or more (its
    /// standard deviation) along each, and over six seeds the mean stays within 5 %. Another seed changes the arc along a
    /// fixed direction; the same seed repeats it bit for bit.
    #[test]
    fn the_rays_streak_the_curtain_and_the_seed_lays_them_out() {
        let variance = |l: &[f64]| {
            let m = l.iter().sum::<f64>() / l.len() as f64;
            l.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / l.len() as f64
        };
        let (mut calm_sum, mut rayed_sum) = (0.0f64, 0.0f64);
        for seed in 1..=6u32 {
            let at = |rays: f32| Aurora::new(&AuroraParams { folds: 0.0, rays, seed, ..AuroraParams::default() }, 1.0);
            let (calm, rayed) = (at(0.0), at(1.0));
            for el in [18.0f32, 21.0, 25.0] {
                let along = |a: &Aurora| (0..=400).map(|i| luminance(a.at(dir_from_az_el(-10.0 + i as f32 * 0.05, el), 0.0)) as f64).collect::<Vec<f64>>();
                let (l0, l1) = (along(&calm), along(&rayed));
                assert!(l0.iter().all(|l| *l > 0.0), "seed {seed}, {el} degrees: inside the curtain");
                let ratio: Vec<f64> = l1.iter().zip(&l0).map(|(a, b)| a / b).collect();
                let spread = variance(&ratio).sqrt();
                assert!(spread >= 0.1, "seed {seed}, {el} degrees: the rays modulate the curtain by {spread:.3}");
                calm_sum += l0.iter().sum::<f64>();
                rayed_sum += l1.iter().sum::<f64>();
            }
        }
        println!("the rays move the mean by {:+.2} %", 100.0 * (rayed_sum / calm_sum - 1.0));
        assert!((rayed_sum / calm_sum - 1.0).abs() <= 0.05, "the rays move the mean by {:.1} %", 100.0 * (rayed_sum / calm_sum - 1.0));
        let lit = |seed: u32| {
            // 9 degrees over the border's middle: inside the curtain whatever the folds (+-24 km) do to the border
            let c = Aurora::new(&AuroraParams { folds: 0.6, rays: 0.8, seed, ..AuroraParams::default() }, 1.0).at(dir_from_az_el(7.0, 24.0), 0.0);
            [c.x.to_bits(), c.y.to_bits(), c.z.to_bits()]
        };
        assert!(lit(7)[1] != 0 && lit(8)[1] != 0, "the premise: both seeds' arcs show there");
        assert_eq!(lit(7), lit(7), "the same seed repeats it bit for bit");
        assert_ne!(lit(8), lit(7), "another seed lays the folds and rays out anew");
    }

    /// The elevation (degrees) at which a point `h` km up over ground distance `r` km stands.
    fn elevation_at(r: f32, h: f32) -> f32 {
        let a = r / R_KM;
        ((R_KM + h) * a.cos() - R_KM).atan2((R_KM + h) * a.sin()).to_degrees()
    }

    /// The most `arc`'s footprint turns (radians, either way) within 200 km along it, inside the window.
    fn turning(a: &Aurora, arc: &Sheet) -> f32 {
        let p = arc.full.points.clone();
        let inside = |q: &[f32; 3]| q[1] > 0.0 && q[0].atan2(q[1]).abs() < a.half;
        // each segment's heading and its start's length, for runs of segments inside the window
        let mut turns: Vec<(f32, f32)> = Vec::new();
        let mut most = 0.0f32;
        let mut flush = |turns: &mut Vec<(f32, f32)>| {
            for i in 0..turns.len() {
                let mut sum = 0.0f32;
                for j in i + 1..turns.len() {
                    if turns[j].1 - turns[i].1 > 200.0 {
                        break;
                    }
                    sum += turns[j].0;
                    most = most.max(sum.abs());
                }
            }
            turns.clear();
        };
        let mut last: Option<f32> = None;
        for w in p.windows(2) {
            if !(inside(&w[0]) && inside(&w[1])) || w[1][2] <= w[0][2] {
                flush(&mut turns);
                last = None;
                continue;
            }
            let heading = (w[1][1] - w[0][1]).atan2(w[1][0] - w[0][0]);
            let turn = last.map_or(0.0, |h| (heading - h + PI).rem_euclid(TAU) - PI);
            turns.push((turn, w[0][2]));
            last = Some(heading);
        }
        flush(&mut turns);
        most
    }

    /// Task 4c (owner, 2026-10-24: "aurora's look a bit straight"): the ribbon's footprint meanders, doubles back on
    /// itself and now and then curls. Over twelve seeds within 40 degrees of the middle: at the default Folds (0.4) the
    /// first ribbon's border wanders off the calm arc's by 3 degrees or more for most seeds (S-bends), and some azimuth
    /// meets it three or more times (a fold) for at least a third of them; at Folds 1 nearly every seed folds and at
    /// least two curl (a ribbon's heading turns through 300 degrees within 200 km). At Folds 0 it is the calm arc, met
    /// once along every azimuth.
    #[test]
    fn the_ribbons_meander_fold_back_and_curl() {
        let ribbon = |folds: f32, seed: u32| Aurora::new(&AuroraParams { folds, seed, ..AuroraParams::default() }, 1.0);
        let betas = || (-80..=80).map(|i| (i as f32 * 0.5).to_radians());
        for seed in 1..=4u32 {
            let a = ribbon(0.0, seed);
            for beta in betas() {
                assert_eq!(a.crossings_at(&a.arcs[0], beta).len(), 1, "seed {seed}, Folds 0: the calm arc at {} degrees", beta.to_degrees());
            }
        }
        let (mut wander, mut folded, mut folded_full, mut curled) = (0, 0, 0, 0);
        for seed in 1..=12u32 {
            let (a, calm, full) = (ribbon(0.4, seed), ribbon(0.0, seed), ribbon(1.0, seed));
            let (mut lo, mut hi, mut most) = (f32::MAX, f32::MIN, 0usize);
            for beta in betas() {
                let cross = a.crossings_at(&a.arcs[0], beta);
                let near = cross.iter().map(|c| c.0).fold(f32::MAX, f32::min);
                let off = elevation_at(near, BORDER_KM) - elevation_at(calm.crossings_at(&calm.arcs[0], beta)[0].0, BORDER_KM);
                (lo, hi, most) = (lo.min(off), hi.max(off), most.max(cross.len()));
            }
            let full_most = betas().map(|b| full.crossings_at(&full.arcs[0], b).len()).max().unwrap();
            let turn = full.arcs.iter().map(|arc| turning(&full, arc)).fold(0.0, f32::max);
            println!("seed {seed}: Folds 0.4 wanders {:.1} degrees, met {most} times at most; Folds 1 met {full_most} times, turns {:.0} degrees", hi - lo, turn.to_degrees());
            wander += (hi - lo >= 3.0) as usize;
            folded += (most >= 3) as usize;
            folded_full += (full_most >= 3) as usize;
            curled += (turn >= 300f32.to_radians()) as usize;
        }
        assert!(wander >= 8, "S-bends at the default Folds: {wander} of 12 seeds wander 3 degrees");
        assert!(folded >= 4, "folds at the default Folds: {folded} of 12 seeds");
        assert!(folded_full >= 11, "folds at Folds 1: {folded_full} of 12 seeds");
        assert!(curled >= 2, "curls at Folds 1: {curled} of 12 seeds");
    }

    /// Task 4c: often two or three ribbons, of different brightness, roughly side by side: at the default Folds the
    /// second shows at 0.3 to 0.8 of the first's brightness and stands behind it (further away) along at least 85 % of
    /// the azimuths within 40 degrees of the middle; at Folds 1 there are three, each fainter than the one before.
    #[test]
    fn two_or_three_ribbons_side_by_side() {
        for seed in 1..=6u32 {
            let a = Aurora::new(&AuroraParams { seed, ..AuroraParams::default() }, 1.0);
            assert!(a.arcs.len() >= 2, "seed {seed}: {} ribbons at the default Folds", a.arcs.len());
            let share = a.arcs[1].brightness / a.arcs[0].brightness;
            assert!((0.3..=0.8).contains(&share), "seed {seed}: the second ribbon at {share} of the first");
            let near = |arc: &Sheet, beta: f32| a.crossings_at(arc, beta).iter().map(|c| c.0).fold(f32::MAX, f32::min);
            let betas = (-80..=80).map(|i| (i as f32 * 0.5).to_radians());
            let behind = betas.clone().filter(|b| near(&a.arcs[1], *b) > near(&a.arcs[0], *b)).count();
            assert!(behind as f32 >= 0.85 * betas.count() as f32, "seed {seed}: the second ribbon behind the first along {behind} of 161 azimuths");
        }
        let full = Aurora::new(&AuroraParams { folds: 1.0, ..AuroraParams::default() }, 1.0);
        assert_eq!(full.arcs.len(), 3);
        assert!(full.arcs[1].brightness < full.arcs[0].brightness && full.arcs[2].brightness < full.arcs[1].brightness);
    }

    /// Task 4c: the brightness varies along each ribbon, brighter knots and dimmer stretches: 120 km up along the first
    /// ribbon within 40 degrees of the middle (no rays), the knots span at least 2.5 to 1 for most of twelve seeds, and
    /// over them all they keep the curtain's mean within 15 %.
    #[test]
    fn the_brightness_varies_along_each_ribbon() {
        let (mut knotty, mut sum, mut count) = (0, 0.0f64, 0usize);
        for seed in 1..=12u32 {
            let a = Aurora::new(&AuroraParams { rays: 0.0, seed, ..AuroraParams::default() }, 1.0);
            let arc = &a.arcs[0];
            let (mut lo, mut hi) = (f32::MAX, 0.0f32);
            for i in -80..=80 {
                let beta = (i as f32 * 0.5).to_radians();
                let d = a.sheet_dir(arc, beta, 120.0);
                let horizontal = (d.x * d.x + d.z * d.z).sqrt();
                let hits = a.hits_at(arc, d.y, horizontal, beta, 0.0);
                let Some(hit) = hits.iter().min_by(|p, q| (p.h - 120.0).abs().total_cmp(&(q.h - 120.0).abs())) else { continue };
                (lo, hi) = (lo.min(hit.knot), hi.max(hit.knot));
                sum += hit.knot as f64;
                count += 1;
            }
            println!("seed {seed}: the knots from {lo:.2} to {hi:.2}");
            knotty += (hi >= 2.5 * lo) as usize;
        }
        let mean = sum / count as f64;
        println!("the knots' mean {mean:.3}");
        assert!(knotty >= 8, "knots along the ribbon: {knotty} of 12 seeds span 2.5 to 1");
        assert!((mean - 1.0).abs() <= 0.15, "the knots move the mean to {mean}");
    }

    /// Task 4c: a bare lookup (the strip's rings, the dome, the night's sums: too few looks for the folds) meets each
    /// ribbon's meander alone, carrying the folded ribbon's light: over the arc's part of the sky, in 0.25-degree steps,
    /// the light on flat ground at the bare lookups' band limit is the light at a fine one's within 15 %, at Folds 0, 0.4
    /// and 1, over four seeds (at Folds 0 a bare lookup meets the ribbon itself, without its knots, carrying their light).
    #[test]
    fn a_bare_lookup_carries_the_folded_ribbons_light() {
        let ground = |a: &Aurora, texel: f32| {
            let mut sum = 0.0f64;
            for i in -240..=240 {
                for j in 1..=240 {
                    let (az, el) = (i as f32 * 0.25, j as f32 * 0.25);
                    let w = (el.to_radians().sin() * el.to_radians().cos()) as f64;
                    sum += luminance(a.at(dir_from_az_el(az, el), texel)) as f64 * w;
                }
            }
            sum
        };
        for folds in [0.0f32, 0.4, 1.0] {
            for seed in 1..=4u32 {
                let a = Aurora::new(&AuroraParams { folds, rays: 0.0, seed, ..AuroraParams::default() }, 1.0);
                assert!(a.arcs.iter().all(|arc| arc.bare.is_some() == (folds > 0.0)), "the premise: a stand-in for each folded ribbon");
                let (fine, bare) = (ground(&a, TAU / 2048.0), ground(&a, AURORA_BARE_TEXEL));
                println!("Folds {folds}, seed {seed}: a bare lookup carries {:.3} of the fine light", bare / fine);
                assert!((bare / fine - 1.0).abs() <= 0.15, "Folds {folds}, seed {seed}: {bare} against {fine}");
            }
        }
    }

    /// Task 4c: the rays hang from the ribbon: their field lines are counted along it, so on a fold's backward leg their
    /// coordinate runs back as the azimuth grows; and they vary in length: along 2000 km of ribbon at least 15 % of
    /// them end below 150 km over the border (their reach there under a fifth of theirs at 30 km) and at least 15 %
    /// reach it in full.
    #[test]
    fn the_rays_ride_along_the_ribbon_and_vary_in_length() {
        let a = (1..=12u32)
            .map(|seed| Aurora::new(&AuroraParams { folds: 1.0, seed, ..AuroraParams::default() }, 1.0))
            .find(|a| (-80..=80).any(|i| a.crossings_at(&a.arcs[0], (i as f32 * 0.5).to_radians()).len() >= 3))
            .expect("a seed whose first ribbon folds");
        let arc = &a.arcs[0];
        let beta = (-800..=800).map(|i| (i as f32 * 0.05).to_radians()).find(|b| a.crossings_at(arc, *b).len() == 3 && a.crossings_at(arc, *b + 0.001).len() == 3).unwrap();
        let along = |b: f32| {
            let mut c = a.crossings_at(arc, b);
            c.sort_by(|p, q| p.1.total_cmp(&q.1));
            c.iter().map(|c| c.1).collect::<Vec<f32>>()
        };
        let (here, there) = (along(beta), along(beta + 0.001));
        assert!(there[0] > here[0] && there[1] < here[1] && there[2] > here[2], "the legs' lengths along the ribbon at {} degrees: {here:?}, then {there:?}", beta.to_degrees());
        let (mut short, mut long) = (0usize, 0usize);
        for i in 0..2000 {
            let x = i as f32;
            let (low, high) = (a.ray_reach(arc, x, 30.0), a.ray_reach(arc, x, 150.0));
            short += (high < 0.2 * low) as usize;
            long += (high >= 0.9) as usize;
        }
        println!("of 2000 km of rays, {short} end below 150 km and {long} reach it");
        assert!(short >= 300 && long >= 300, "the rays' lengths: {short} short, {long} long");
    }
}
