//! The aurora (plan hdri-7 §4), written from general techniques: thin field-aligned sheets on a curved Earth, their
//! colour from the emission lines. No volume is marched: each view ray meets up to three folded, tilted sheets in
//! closed form.
//!
//! - **Geometry.** The arc's middle lies at `direction_deg`; its lower border (BORDER_KM up) stands at `height_deg`
//!   there, so its ground distance is D0 = R (acos(R cos e0 / (R + h0)) - e0), exact on a sphere. Along a view azimuth
//!   beta from the middle the footprint is at (D0 + f) / cos beta, f the fold at s = D0 tan beta: an arc that sinks to
//!   the horizon at both ends. The sheet leans poleward with height (FIELD_TILT, a field dip of about 77 degrees), so a
//!   ray (a field line at a fixed along-arc x) leans toward the middle as it rises. Folds 0.3 to 1 bring in a second and
//!   a third arc behind the first.
//! - **Brightness.** A thin sheet's light is its column along the view: face-on 0.3 of the brightest (EDGE_MAX, the top
//!   of refs §5.1's 0.1 to 0.3), seen edge-on (a fold, the arc's ends) up to all of it: 50 kR of green at Strength 1.
//! - **Colour.** The lines' heights above the border: nitrogen (427.8 nm and the first positive bands, pink together)
//!   in its bottom 12 km or so, the green 557.7 nm line from a few km up fading over 25 km, held off the bottom 7 km
//!   more where the nitrogen is strong (a storm's harder electrons), the red 630 nm line from about 150 km to 350 km;
//!   summed in XYZ per height and turned into Rec.709 once, luminance kept (`spectrum::xyz_to_rgb_keep_luminance`).
//! - **Band limits.** Every noise reads the lookup's texel (AURORA_BARE_TEXEL for the few looks round the horizon), its
//!   octaves fading to their mean below two texels, so held and released bakes agree in their means.

use makepad_draw::*;
use super::noise::{fbm1_d, hash_u32, octaves_for};
use super::spectrum::{line_xyz_per_rayleigh, xyz_to_rgb_keep_luminance};
use super::{dir_from_az_el, AuroraParams};
use std::f32::consts::{PI, TAU};

/// The band limit of a bare lookup (the strip's rings, the dome, the night's quadrature): the rings' spacing.
pub(super) const AURORA_BARE_TEXEL: f32 = TAU / 64.0;
const R_KM: f32 = 6371.0;
/// The lower border's height, and how far its wave moves it (+-2 km over about 40 km along the arc).
const BORDER_KM: f32 = 100.0;
const WAVE_KM: f32 = 40.0;
const WAVE_DEPTH_KM: f32 = 4.0;
/// The second and third arcs stand this much further poleward.
const ARC_STEP_KM: f32 = 60.0;
/// The sheet's poleward lean per km of height: cot of the field's dip (about 77 degrees).
const FIELD_TILT: f32 = 0.23;
/// Edge-on over face-on, at most: the face-on body is 0.3 of the brightest lines of sight (refs §5.1: 0.1 to 0.3).
const EDGE_MAX: f32 = 10.0 / 3.0;
/// The folds: their wavelength along the arc and their depth at Folds 1.
const FOLD_KM: f32 = 150.0;
const FOLD_DEPTH_KM: f32 = 80.0;
/// The rays' wavelength along the arc.
const RAY_KM: f32 = 8.0;
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
/// The window's soft edge at the arc's ends, and the widest half-width (the footprint's 1 / cos beta).
const WINDOW_EDGE: f32 = 10.0 * PI / 180.0;
const MAX_HALF: f32 = 88.0 * PI / 180.0;
const SALT: u32 = 0x4155_524f;

/// One sheet (an arc of the aurora).
#[derive(Clone, Debug)]
struct Sheet {
    d0: f32,
    brightness: f32,
    /// The sine of the highest elevation any of its points reaches: above it a look skips the arc.
    top_sin: f32,
    fold_seed: u32,
    ray_seed: u32,
    wave_seed: u32,
}

/// Where a view ray meets a sheet: above the local border (km), the edge-on factor, the ray's modulation, and for the
/// tests its height (km) and along-arc position (km).
#[derive(Clone, Copy, Debug)]
struct Hit {
    above: f32,
    edge: f32,
    ray: f32,
    #[cfg(test)]
    h: f32,
    #[cfg(test)]
    x: f32,
}

/// The aurora of one map, built once per map from the block as clamp() holds it.
#[derive(Clone, Debug)]
pub(super) struct Aurora {
    phi0: f32,
    half: f32,
    fold: f32,
    rays: f32,
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
        let fold = p.folds * FOLD_DEPTH_KM;
        let seed = hash_u32(p.seed ^ SALT);
        let arcs = (0..3u32)
            .filter_map(|k| {
                let fade = match k {
                    0 => 1.0,
                    1 => smoothstep(0.3, 0.6, p.folds),
                    _ => smoothstep(0.6, 0.9, p.folds),
                };
                if !(fade > 0.0) {
                    return None;
                }
                let d = d0 + k as f32 * ARC_STEP_KM;
                // the highest any of its points stands: its top at its nearest
                let a = (d - fold).max(20.0) / R_KM;
                let top = ((R_KM + TOP_KM) * a.cos() - R_KM).atan2((R_KM + TOP_KM) * a.sin());
                let s = hash_u32(seed.wrapping_add(k.wrapping_mul(0x9e37_79b9)));
                Some(Sheet { d0: d, brightness: 0.6f32.powi(k as i32) * fade, top_sin: top.sin(), fold_seed: s, ray_seed: hash_u32(s ^ 0x5241_5953), wave_seed: hash_u32(s ^ 0x5741_5645) })
            })
            .collect();
        Aurora {
            phi0: p.direction_deg.to_radians(),
            half: (0.5 * p.width_deg).to_radians().min(MAX_HALF),
            fold,
            rays: p.rays,
            gain,
            arcs,
            table: line_table(p.line_mix()),
            middle: dir_from_az_el(p.direction_deg, p.height_deg),
        }
    }

    /// The arcs' radiance along `d` (map frame), before extinction and the horizon fade; 0 below the horizon and off
    /// the arc. `texel` band-limits the folds, the rays and the border's wave.
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
            if let Some(hit) = self.hit(arc, sin_e, cos_e, sin_b, cos_b, texel) {
                sum += self.profile(BORDER_KM + hit.above) * (hit.ray * hit.edge * arc.brightness);
            }
        }
        sum * (window * self.gain)
    }

    /// The arc's middle at its lower border, map frame.
    pub(super) fn middle(&self) -> Vec3f {
        self.middle
    }

    /// Where the view at elevation (sin_e, cos_e) and azimuth beta from the middle meets `arc`, or None where it passes
    /// under its border or over its top.
    fn hit(&self, arc: &Sheet, sin_e: f32, cos_e: f32, sin_b: f32, cos_b: f32, texel: f32) -> Option<Hit> {
        let e = sin_e.atan2(cos_e);
        let s = arc.d0 * sin_b / cos_b;
        // km of s a texel spans: d s / d beta = d0 / cos² beta, and a texel of azimuth is texel / cos e
        let along = texel * arc.d0 / (cos_b * cos_b * cos_e.max(0.05));
        let (fv, fd) = fbm1_d(s / FOLD_KM, octaves_for(FOLD_KM, along, 4.0), arc.fold_seed);
        let (f, slope) = (self.fold * (fv - 0.5), self.fold * fd / FOLD_KM);
        let border = BORDER_KM + WAVE_DEPTH_KM * (fbm1_d(s / WAVE_KM, octaves_for(WAVE_KM, along, 3.0), arc.wave_seed).0 - 0.5);
        let base = arc.d0 + f;
        // the height a view at e reaches over ground distance r, exact on a sphere
        let height = |r: f32| {
            let c = (e + r / R_KM).cos();
            if c > 0.02 { R_KM * cos_e / c - R_KM } else { f32::INFINITY }
        };
        // r cos beta = base + (h(r) - border) x tilt, three fixed-point steps from the untilted footprint
        let mut r = base / cos_b;
        for _ in 0..3 {
            r = (base + (height(r) - border).min(TOP_KM) * FIELD_TILT) / cos_b;
        }
        let h = height(r);
        let above = h - border;
        if !(above > -2.0 && h < TOP_KM) {
            return None;
        }
        // the path through a thin vertical sheet, capped: 1 / (cos alpha cos e), alpha the view's angle to its normal
        let cos_a = (cos_b - slope * sin_b).abs() / (1.0 + slope * slope).sqrt();
        let k = cos_a * cos_e;
        let edge = 1.0 / (k * k + 1.0 / (EDGE_MAX * EDGE_MAX)).sqrt();
        // a ray is a field line: a fixed along-arc x on the tilted sheet
        let x = r * sin_b;
        let v = fbm1_d(x / RAY_KM, octaves_for(RAY_KM, texel * r / cos_e.max(0.05), 5.0), arc.ray_seed).0;
        let ray = 1.0 + self.rays * (2.0 * v - 1.0) * (0.5 + above.max(0.0) / 120.0).min(1.0);
        Some(Hit {
            above,
            edge,
            ray,
            #[cfg(test)]
            h,
            #[cfg(test)]
            x,
        })
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

    /// The direction (map frame) of `arc`'s point at azimuth `beta` (radians) from the middle and `h` km up: `hit`'s
    /// inverse at texel 0. For the tests.
    #[cfg(test)]
    fn sheet_dir(&self, arc: &Sheet, beta: f32, h: f32) -> Vec3f {
        let (sin_b, cos_b) = beta.sin_cos();
        let s = arc.d0 * sin_b / cos_b;
        let f = self.fold * (fbm1_d(s / FOLD_KM, 4.0, arc.fold_seed).0 - 0.5);
        let border = BORDER_KM + WAVE_DEPTH_KM * (fbm1_d(s / WAVE_KM, 3.0, arc.wave_seed).0 - 0.5);
        let r = (arc.d0 + f + (h - border).min(TOP_KM) * FIELD_TILT) / cos_b;
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
    let orange = line_xyz_per_rayleigh(N2_ORANGE_NM);
    std::array::from_fn(|i| {
        let h = TABLE_LOW_KM + (i as f32 + 0.5) * (TOP_KM - TABLE_LOW_KM) / TABLE as f32;
        let dh = h - BORDER_KM;
        let border = smoothstep(-1.5, 0.5, dh);
        let g = smoothstep(2.0 + hold, 12.0 + hold, dh) * (-(dh - 12.0 - hold).max(0.0) / 25.0).exp() * (1.0 - smoothstep(180.0, 220.0, h));
        let q = (h / 220.0).ln() / 0.18;
        let r = (-0.5 * q * q).exp() * smoothstep(140.0, 170.0, h) * (1.0 - smoothstep(320.0, 350.0, h));
        let w = (dh - N2_KM) / N2_SPREAD_KM;
        let n = (-0.5 * w * w).exp();
        let k = face * border;
        let xyz: [f32; 3] = std::array::from_fn(|c| k * (mix[0] * g * green[c] + mix[1] * r * red[c] + mix[2] * n * (N2_VIOLET_SHARE * violet[c] + N2_PINK_SHARE * pink[c] + N2_ORANGE_SHARE * orange[c])));
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

    /// The colour by height comes from the lines: classic is green wherever it shows; red tops are red high up and
    /// green low down; the storm's lower border is pink-violet (red and blue over green) and green above it.
    #[test]
    fn each_preset_has_its_colours_by_height() {
        let at = |color: &str| Aurora::new(&AuroraParams { color: color.to_string(), ..AuroraParams::default() }, 1.0);
        let classic = at("classic");
        let most = (0..TABLE).map(|i| luminance(classic.table[i])).fold(0.0, f32::max);
        for i in 0..TABLE {
            let c = classic.table[i];
            if luminance(c) >= 0.05 * most {
                assert!(c.y > c.x && c.y > c.z, "classic at table entry {i}: {c:?}");
            }
        }
        let tops = at("red tops");
        let (high, low) = (tops.profile(230.0), tops.profile(120.0));
        assert!(high.x > high.y, "red tops at 230 km: {high:?}");
        assert!(low.y > low.x, "red tops at 120 km: {low:?}");
        // refs §5.1: the red tops read orange, then deep red, at about 2.4 to 3.6 G0 face-on (630 nm at 10 to 15 kR)
        let top = (180..=300).map(|h| luminance(tops.profile(h as f32))).fold(0.0, f32::max) / g0();
        assert!((2.4..=3.6).contains(&top), "red tops' brightest red above 180 km: {top} G0");
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
            let (sin_b, cos_b) = beta.to_radians().sin_cos();
            let mut e = 0.5f32;
            while e < 60.0 {
                let (sin_e, cos_e) = e.to_radians().sin_cos();
                if a.hit(arc, sin_e, cos_e, sin_b, cos_b, 0.0).is_some_and(|hit| hit.above >= 0.0) {
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

    /// A view ray meets the sheet where the sheet stands (the closed form's round trip), and a ray (a fixed along-arc
    /// x) leans toward the arc's middle as it rises: the sheet is tilted poleward along the field.
    #[test]
    fn the_sheets_are_met_where_they_stand_and_lean_with_the_field() {
        let a = Aurora::new(&AuroraParams { folds: 0.6, ..AuroraParams::default() }, 1.0);
        let arc = &a.arcs[0];
        for beta in [-45.0f32, -20.0, 0.0, 25.0, 40.0] {
            for h in [105.0f32, 150.0, 250.0] {
                let d = a.sheet_dir(arc, beta.to_radians(), h);
                let horizontal = (d.x * d.x + d.z * d.z).sqrt();
                let (sin_b, cos_b) = beta.to_radians().sin_cos();
                let hit = a.hit(arc, d.y, horizontal, sin_b, cos_b, 0.0).unwrap_or_else(|| panic!("{beta} {h}: no hit"));
                assert!((hit.h - h).abs() < 2.0, "beta {beta}, {h} km: met at {} km", hit.h);
            }
        }
        let x_at = |h: f32| {
            let d = a.sheet_dir(arc, 40.0f32.to_radians(), h);
            let (sin_b, cos_b) = 40.0f32.to_radians().sin_cos();
            a.hit(arc, d.y, (d.x * d.x + d.z * d.z).sqrt(), sin_b, cos_b, 0.0).unwrap().x
        };
        assert!(x_at(200.0) > x_at(110.0) + 10.0, "a fixed azimuth reaches further along the arc higher up: the ray at a fixed x leans in");
    }

    /// Plan hdri-7 D6: a sheet seen edge-on is brighter than face-on: toward the arc's ends, and at the folds. The folds'
    /// part reads the edge-on factor of the first arc itself (`hit(..).edge`), so the second and third arcs, which Folds
    /// 1 brings in over the first, cannot pass it alone.
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
        // the first arc's edge-on factor over 40 degrees either side of the middle, a few degrees into the curtain
        let most_edge = |a: &Aurora| {
            let mut most = 0.0f32;
            for beta in -80..=80 {
                let (sin_b, cos_b) = (beta as f32 * 0.5).to_radians().sin_cos();
                for e in [16.0f32, 20.0, 25.0] {
                    let (sin_e, cos_e) = e.to_radians().sin_cos();
                    if let Some(hit) = a.hit(&a.arcs[0], sin_e, cos_e, sin_b, cos_b, 0.0) {
                        most = most.max(hit.edge);
                    }
                }
            }
            most
        };
        let folded = Aurora::new(&AuroraParams { width_deg: 180.0, folds: 1.0, rays: 0.0, ..AuroraParams::default() }, 1.0);
        let (flat_edge, folded_edge) = (most_edge(&flat), most_edge(&folded));
        println!("the first arc's most edge-on factor within 40 degrees: flat {flat_edge:.2}, folded {folded_edge:.2}");
        assert!(folded_edge > 1.3 * flat_edge, "folds seen edge-on brighten the arc: {folded_edge} against {flat_edge}");
    }

    /// Plan hdri-7 §4 (spec: "Rays: the vertical ray streaks"; "Seed"): the rays streak the curtain along the arc and
    /// keep its mean, and the seed lays them out. With no folds (one arc), at 18, 21 and 25 degrees (the border at 15
    /// in the middle), over 20 degrees of azimuth round the middle in 0.05-degree steps: the luminance varies at least
    /// four times as much at Rays 1 as at Rays 0 (the border's wave alone) along each, and over six seeds its mean
    /// stays within 5 %. Another seed changes the arc along a fixed direction; the same seed repeats it bit for bit.
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
                let (v0, v1) = (variance(&l0), variance(&l1));
                assert!(v1 >= 4.0 * v0, "seed {seed}, {el} degrees: the rays vary it {v1:.3e}, the calm arc {v0:.3e}");
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
}
