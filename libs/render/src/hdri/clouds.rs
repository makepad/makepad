//! A 2D cloud deck and a cirrus veil for the HDRI sky.
//!
//! The deck is a flat plane `altitude_m` above the viewer. A view ray meets
//! it at a point whose fbm value, remapped by coverage and sharpness, is the
//! cloud density there. Self-shadowing is the 2D trick from Inigo Quilez's
//! dynamic-clouds article: a few steps from that point toward the sun, and
//! the density met on the way dims the light. Lighting is two
//! Henyey-Greenstein lobes on the sun's irradiance (the narrow forward one is
//! the silver lining) plus the sky's ambient. Cirrus is a thinner, higher
//! layer of stretched, domain-warped fbm.
//!
//! After iq's articles on fbm, domain warping and 2D dynamic clouds; the
//! techniques are reimplemented, no code is copied.
//!
//! Directions are in the map's own frame (before the map rotation), the
//! frame `ibl::dir_to_equirect_uv` maps. Radiance is in the atmosphere's
//! units (see `atmosphere.rs`, "Radiance scale").

use makepad_draw::*;
use super::noise::{fbm2, hash01, hash_u32};
use super::CloudParams;

/// Size of one base noise cell at `scale` 1, in metres: from the default
/// 2 km deck a handful of cumulus cells fill the sky overhead.
const CELL_M: f32 = 3000.0;
/// Cirrus cells are larger: the layer is higher and streakier.
const CIRRUS_CELL_FACTOR: f32 = 2.0;
const CUMULUS_OCTAVES: u32 = 6;
/// Far from the zenith one pixel spans many fine octaves, which alias into
/// sparkle. Between these distances (in cells from the point overhead) the
/// deck fades down to its broad octaves.
const BROAD_OCTAVES: u32 = 3;
const DETAIL_FADE_START: f32 = 4.0;
const DETAIL_FADE_END: f32 = 16.0;
const SHADOW_OCTAVES: u32 = 4;
const SHADOW_STEPS: usize = 4;
/// Horizontal reach of the shadow march per unit of tan(sun zenith), in
/// cells: the deck is taken to be about a third of a cell thick.
const SHADOW_REACH: f32 = 0.35;
/// Cap on tan(sun zenith), so a grazing sun does not march off to infinity.
const SHADOW_MAX_TAN: f32 = 4.0;
/// Mean density 1 along the march dims the light to e^-3.
const SHADOW_DENSITY: f32 = 3.0;
/// Optical thickness of a density-1 cumulus pixel; alpha is normalised so
/// density 1 is exactly opaque.
const CUMULUS_THICKNESS: f32 = 5.0;
const CIRRUS_OCTAVES: u32 = 4;
const CIRRUS_BROAD_OCTAVES: u32 = 2;
const CIRRUS_ABOVE_DECK_M: f32 = 6000.0;
const CIRRUS_MIN_ALTITUDE_M: f32 = 8000.0;
/// Cirrus never gets more opaque than this: it is a veil, not a deck.
const CIRRUS_OPACITY: f32 = 0.55;
/// sin(elevation) at which the layer reaches full opacity (about 7 deg):
/// below it the layer thins into the horizon haze instead of piling up at
/// infinity.
const HORIZON_FADE: f32 = 0.12;
/// Lower rays hit the flat deck at this sine's distance (about 1.1 deg).
const MIN_SIN_ELEVATION: f32 = 0.02;
/// Thick clouds scatter many times, which one phase lobe underestimates.
const SUN_GAIN: f32 = 1.5;
const AMBIENT_GAIN: f32 = 1.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct CloudSample {
    /// Coverage in this direction, 0..1 (0 at and below the horizon).
    pub alpha: f32,
    /// Self-shadowing, 0..1: 1 is fully sun-lit.
    pub light: f32,
    /// Cosine between the view and the sun, for the phase function.
    pub cos_sun: f32,
}

pub struct CloudLayer {
    coverage: f32,
    sharpness: f32,
    cirrus: f32,
    /// Metres per noise cell.
    cell: f32,
    altitude: f32,
    cirrus_altitude: f32,
    seed: u32,
    cirrus_seed: u32,
    warp_seed: u32,
    sun_dir: Vec3f,
    /// Offset of one shadow-march step, in cells.
    shadow_step: Vec2f,
    /// Unit direction the cirrus streaks run along, as (x, z).
    wind: Vec2f,
}

impl CloudLayer {
    /// None when there is nothing to draw (coverage and cirrus both 0).
    pub fn new(p: &CloudParams, seed: u32, sun_dir: Vec3f) -> Option<CloudLayer> {
        // As clamp() holds it: its ranges and defaults are written once (L:231).
        let p = &p.clamped();
        let coverage = p.coverage;
        let cirrus = p.cirrus;
        if coverage <= 0.0 && cirrus <= 0.0 {
            return None;
        }
        let sun = sun_dir.normalize();
        let sun = if sun.is_finite() && sun.length() > 0.5 { sun } else { vec3f(0.0, 1.0, 0.0) };
        // Light crossing the deck toward a point moves sideways by the deck's
        // thickness x tan(sun zenith); with the sun overhead the march stays
        // on the point and measures the cloud's own thickness.
        let horizontal = (sun.x * sun.x + sun.z * sun.z).sqrt();
        let shadow_step = if horizontal > 1.0e-6 {
            let tan_zenith = (horizontal / sun.y.max(1.0e-3)).min(SHADOW_MAX_TAN);
            let per_step = SHADOW_REACH * tan_zenith / SHADOW_STEPS as f32;
            vec2f(sun.x / horizontal * per_step, sun.z / horizontal * per_step)
        } else {
            vec2f(0.0, 0.0)
        };
        let wind_angle = hash01(0, 0, 0, hash_u32(seed ^ 0x5bd1_e995)) * std::f32::consts::TAU;
        let altitude = p.altitude_m;
        Some(CloudLayer {
            coverage,
            sharpness: p.sharpness,
            cirrus,
            cell: CELL_M * p.scale,
            altitude,
            cirrus_altitude: (altitude + CIRRUS_ABOVE_DECK_M).max(CIRRUS_MIN_ALTITUDE_M),
            seed,
            cirrus_seed: hash_u32(seed.wrapping_add(0x68e3_1da4)),
            warp_seed: hash_u32(seed.wrapping_add(0xb529_7a4d)),
            sun_dir: sun,
            shadow_step,
            wind: vec2f(wind_angle.cos(), wind_angle.sin()),
        })
    }

    /// The visible deck and veil toward `dir`: alpha, self-shadowing and the
    /// sun's cosine. Alpha fades to 0 at the horizon (see `HORIZON_FADE`), so
    /// it is what the viewer sees, not what the sun sees: use `cover_toward`
    /// for light that is blocked.
    pub fn sample(&self, dir: Vec3f) -> CloudSample {
        let length = dir.length();
        if length.is_nan() || length <= 0.0 {
            return CloudSample { alpha: 0.0, light: 1.0, cos_sun: 0.0 };
        }
        let d = dir / length;
        let cos_sun = d.dot(self.sun_dir);
        if d.y <= 0.0 {
            return CloudSample { alpha: 0.0, light: 1.0, cos_sun };
        }
        let (alpha, light) = self.layers(d, smoothstep(0.0, HORIZON_FADE, d.y), true);
        CloudSample { alpha: alpha.clamp(0.0, 1.0), light, cos_sun }
    }

    /// How much of whatever lies behind the layers they hide along `dir`, 0..1:
    /// the alpha `sample` gives without the horizon fade. The fade thins the
    /// visible deck into the horizon haze, but a light behind the deck is not
    /// shown through that haze: at 2 degrees a full overcast is still opaque to
    /// the sun. The key light and the discs of the sun and moon use this; 0 at
    /// and below the horizon and for a direction that is not one.
    pub fn cover_toward(&self, dir: Vec3f) -> f32 {
        let length = dir.length();
        if !length.is_finite() || length <= 0.0 {
            return 0.0;
        }
        let d = dir / length;
        if d.y <= 0.0 {
            return 0.0;
        }
        self.layers(d, 1.0, false).0.clamp(0.0, 1.0)
    }

    /// Deck and veil along the unit direction `d` above the horizon: their
    /// combined alpha, with each layer's opacity scaled by `fade`, and the
    /// light that reaches them (1 unless `shaded`, which runs the deck's
    /// self-shadow march).
    fn layers(&self, d: Vec3f, fade: f32, shaded: bool) -> (f32, f32) {
        let sin_el = d.y.max(MIN_SIN_ELEVATION);
        let mut alpha = 0.0f32;
        let mut light = 1.0f32;
        if self.coverage > 0.0 {
            let p = deck_point(d, sin_el, self.altitude, self.cell);
            let density = self.cumulus_density(p);
            if density > 0.0 {
                alpha = deck_opacity(density) * fade;
                if shaded {
                    light = self.self_shadow(p);
                }
            }
        }
        if self.cirrus > 0.0 {
            let q = deck_point(d, sin_el, self.cirrus_altitude, self.cell * CIRRUS_CELL_FACTOR);
            let veil = self.cirrus_density(q) * CIRRUS_OPACITY * fade;
            // The deck is lower, so it is in front: cirrus shows through its gaps.
            let behind = veil * (1.0 - alpha);
            let total = alpha + behind;
            if total > 0.0 {
                // Cirrus is thin and fully lit.
                light = (alpha * light + behind) / total;
            }
            alpha = total;
        }
        (alpha, light)
    }

    /// Cloud radiance for a sample: sun irradiance x phase x self-shadow,
    /// plus the sky's ambient, greyed more the more of the sky is cloud.
    pub fn shade(&self, s: &CloudSample, sun_irradiance: Vec3f, ambient: Vec3f) -> Vec3f {
        // A broad lobe for the body, a narrow forward one for the silver
        // lining that rims a cloud standing in front of the sun.
        let phase = 0.75 * hg_phase(s.cos_sun, 0.2) + 0.25 * hg_phase(s.cos_sun, 0.85);
        // Clouds are white: the underside sees the blue dome but scatters it
        // back half greyed. And the dome it sees is blue only through the
        // gaps, other cloud everywhere else: the more of the sky is cloud the
        // less blue is left to carry, and a closed deck has none (a full
        // overcast is grey, not the clear sky's blue turned down).
        let grey = crate::sky::luminance(ambient);
        let greyed = 0.5 + 0.5 * self.coverage;
        let sky = ambient.mix(vec3f(grey, grey, grey), greyed);
        sun_irradiance * (SUN_GAIN * phase * s.light) + sky * AMBIENT_GAIN
    }

    fn cumulus_density(&self, p: Vec2f) -> f32 {
        let noise = fbm_lod(p, CUMULUS_OCTAVES, BROAD_OCTAVES, detail_at(p.length()), self.seed);
        self.cover(noise)
    }

    /// Coverage and sharpness turn a noise value into a density in 0..1.
    fn cover(&self, noise: f32) -> f32 {
        // Coverage slides the threshold down through the noise's range (fbm
        // sits mostly in 0.2..0.8); sharpness narrows the band that turns
        // noise into cloud.
        let threshold = 0.78 - 0.56 * self.coverage;
        let width = 0.02 + 0.2 * (1.0 - self.sharpness);
        let density = smoothstep(threshold - width, threshold + width, noise);
        // The last 15 % of coverage closes the remaining gaps into overcast.
        let close = smoothstep(0.85, 1.0, self.coverage);
        density + (1.0 - density) * close
    }

    /// The deck's density (0..1) at a point of its plane, in noise cells east
    /// and south of the point overhead. The volumetric clouds (volumetric.rs)
    /// stand on this footprint, so both modes put their clouds in the same
    /// places. Four octaves, as the deck's own self-shadow reads it: the
    /// volume adds the finer detail itself.
    pub(crate) fn deck_density(&self, cells: Vec2f) -> f32 {
        self.cover(fbm2(cells.x, cells.y, SHADOW_OCTAVES, self.seed))
    }

    /// Metres per noise cell of the deck.
    pub(crate) fn cell_m(&self) -> f32 {
        self.cell
    }

    /// iq's 2D self-shadow: the mean density met a few steps toward the sun.
    fn self_shadow(&self, p: Vec2f) -> f32 {
        if self.shadow_step.x == 0.0 && self.shadow_step.y == 0.0 {
            let own = self.cover(fbm2(p.x, p.y, SHADOW_OCTAVES, self.seed));
            return (-SHADOW_DENSITY * own).exp();
        }
        let mut sum = 0.0f32;
        for i in 1..=SHADOW_STEPS {
            let q = p + self.shadow_step * i as f32;
            sum += self.cover(fbm2(q.x, q.y, SHADOW_OCTAVES, self.seed));
        }
        (-SHADOW_DENSITY * sum / SHADOW_STEPS as f32).exp()
    }

    fn cirrus_density(&self, q: Vec2f) -> f32 {
        // Into the wind's frame, stretched along the wind so the streaks come
        // out long and thin.
        let along = q.x * self.wind.x + q.y * self.wind.y;
        let across = q.y * self.wind.x - q.x * self.wind.y;
        let s = vec2f(along * 0.3, across * 2.0);
        // Domain warp: a second, broad fbm pushes the lookup around so the
        // streaks curl instead of running dead straight.
        let wx = fbm2(s.x * 0.5, s.y * 0.5, 3, self.warp_seed) - 0.5;
        let wy = fbm2(s.x * 0.5 + 5.2, s.y * 0.5 + 1.3, 3, self.warp_seed) - 0.5;
        let warped = vec2f(s.x + 1.5 * wx, s.y + 1.5 * wy);
        let noise = fbm_lod(warped, CIRRUS_OCTAVES, CIRRUS_BROAD_OCTAVES, detail_at(q.length()), self.cirrus_seed);
        let threshold = 0.7 - 0.25 * self.cirrus;
        smoothstep(threshold, threshold + 0.2, noise) * (0.4 + 0.6 * self.cirrus)
    }
}

/// The deck's opacity at a density (0..1): Beer-Lambert through the deck,
/// rescaled so density 1 is opaque. A thin footprint draws translucent (0.1
/// is 40 % opaque). The volume (volumetric.rs) reads it too, so that its
/// columns hide as much of the sky as the deck draws on their footprint.
pub(crate) fn deck_opacity(density: f32) -> f32 {
    let opacity = (1.0 - (-CUMULUS_THICKNESS * density).exp()) / (1.0 - (-CUMULUS_THICKNESS).exp());
    opacity.min(1.0)
}

/// Where a view ray meets a flat deck `altitude` metres up, in noise cells.
fn deck_point(d: Vec3f, sin_el: f32, altitude: f32, cell: f32) -> Vec2f {
    let reach = altitude / (sin_el * cell);
    vec2f(d.x * reach, d.z * reach)
}

/// 1 near the point overhead, 0 far away, for `fbm_lod`.
fn detail_at(distance_cells: f32) -> f32 {
    1.0 - smoothstep(DETAIL_FADE_START, DETAIL_FADE_END, distance_cells)
}

/// fbm whose fine octaves fade out with `detail` (1 = all `fine` octaves,
/// 0 = only the `broad` ones).
fn fbm_lod(p: Vec2f, fine: u32, broad: u32, detail: f32, seed: u32) -> f32 {
    if detail >= 1.0 {
        return fbm2(p.x, p.y, fine, seed);
    }
    let low = fbm2(p.x, p.y, broad, seed);
    if detail <= 0.0 {
        return low;
    }
    low + (fbm2(p.x, p.y, fine, seed) - low) * detail
}

/// Henyey-Greenstein phase function, normalised over the sphere (1/sr): the
/// one the deck, the cloud volume (`volumetric.rs`) and the fog's glow
/// (`weather.rs`) scatter by (L:231).
pub(super) fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let gg = g * g;
    let denom = (1.0 + gg - 2.0 * g * cos_theta).max(1.0e-6);
    (1.0 - gg) / (4.0 * std::f32::consts::PI * denom * denom.sqrt())
}

/// Local copy (as in sky.rs), so no glob can make the name ambiguous.
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::dir_from_az_el;

    fn sun() -> Vec3f {
        dir_from_az_el(180.0, 40.0)
    }

    fn clouds(coverage: f32, cirrus: f32) -> CloudParams {
        CloudParams { coverage, cirrus, ..CloudParams::default() }
    }

    /// Directions over the upper sky, clear of the horizon fade.
    fn sky_dirs() -> Vec<Vec3f> {
        let mut dirs = Vec::new();
        for el in (15..=85).step_by(10) {
            for az in (0..360).step_by(20) {
                dirs.push(dir_from_az_el(az as f32, el as f32));
            }
        }
        dirs
    }

    fn mean_alpha(layer: &CloudLayer) -> f32 {
        let dirs = sky_dirs();
        dirs.iter().map(|&d| layer.sample(d).alpha).sum::<f32>() / dirs.len() as f32
    }

    #[test]
    fn no_cover_means_no_layer() {
        assert!(CloudLayer::new(&clouds(0.0, 0.0), 1, sun()).is_none());
        assert!(CloudLayer::new(&clouds(0.3, 0.0), 1, sun()).is_some());
        assert!(CloudLayer::new(&clouds(0.0, 0.3), 1, sun()).is_some());
    }

    #[test]
    fn alpha_is_zero_at_and_below_the_horizon() {
        let layer = CloudLayer::new(&clouds(1.0, 1.0), 7, sun()).unwrap();
        for az in (0..360).step_by(30) {
            for el in [0.0f32, -0.5, -10.0, -90.0] {
                let s = layer.sample(dir_from_az_el(az as f32, el));
                assert_eq!(s.alpha, 0.0, "az {az} el {el}");
            }
        }
        // Full cover is opaque overhead.
        assert!(layer.sample(vec3f(0.0, 1.0, 0.0)).alpha > 0.99);
        // Nothing below the horizon is covered either.
        assert_eq!(layer.cover_toward(vec3f(0.0, -1.0, 0.0)), 0.0);
    }

    #[test]
    fn the_cover_toward_a_direction_has_no_horizon_fade() {
        // A full overcast is opaque toward the horizon too; only the visible
        // deck thins into the haze there.
        let overcast = CloudLayer::new(&clouds(1.0, 0.0), 7, sun()).unwrap();
        for el in [0.5f32, 1.0, 2.0, 3.0, 5.0, 8.0, 40.0] {
            for az in (0..360).step_by(45) {
                let cover = overcast.cover_toward(dir_from_az_el(az as f32, el));
                assert!(cover > 0.999, "az {az} el {el}: cover {cover}");
            }
        }
        // The deck keeps its fade: about a fifth at 2 degrees, two fifths at 3.
        for (el, most) in [(2.0f32, 0.3f32), (3.0, 0.5)] {
            let d = dir_from_az_el(0.0, el);
            assert!(overcast.sample(d).alpha < most, "el {el}: alpha {}", overcast.sample(d).alpha);
        }
        // Where the fade is 1 (above about 7 degrees) the cover is the visible
        // alpha, bit for bit, cirrus included.
        let mixed = CloudLayer::new(&clouds(0.5, 0.5), 7, sun()).unwrap();
        for d in sky_dirs() {
            assert_eq!(mixed.cover_toward(d).to_bits(), mixed.sample(d).alpha.to_bits());
        }
        // Below that it is never less, deck and veil alike.
        for layer in [&mixed, &CloudLayer::new(&clouds(0.0, 1.0), 5, sun()).unwrap()] {
            for el in [0.3f32, 1.0, 2.0, 4.0, 6.0] {
                for az in (0..360).step_by(30) {
                    let d = dir_from_az_el(az as f32, el);
                    assert!(layer.cover_toward(d) >= layer.sample(d).alpha, "az {az} el {el}");
                }
            }
        }
        // None at or below the horizon, and none toward a non-direction.
        for d in [dir_from_az_el(30.0, 0.0), dir_from_az_el(30.0, -20.0), vec3f(0.0, -1.0, 0.0)] {
            assert_eq!(overcast.cover_toward(d), 0.0);
        }
        for d in [Vec3f::default(), vec3f(f32::NAN, 1.0, 0.0), vec3f(f32::INFINITY, 1.0, 0.0)] {
            assert_eq!(overcast.cover_toward(d), 0.0, "{d:?}");
        }
    }

    #[test]
    fn coverage_grows_the_cloud_fraction() {
        let fraction = |c: f32| mean_alpha(&CloudLayer::new(&clouds(c, 0.0), 3, sun()).unwrap());
        let (low, mid, high) = (fraction(0.2), fraction(0.5), fraction(0.8));
        assert!(low < mid && mid < high, "{low} {mid} {high}");
        assert!(high > 0.5, "0.8 coverage should hide most of the sky: {high}");
    }

    #[test]
    fn clouds_are_deterministic_per_seed() {
        let a = CloudLayer::new(&clouds(0.5, 0.5), 42, sun()).unwrap();
        let b = CloudLayer::new(&clouds(0.5, 0.5), 42, sun()).unwrap();
        for d in sky_dirs() {
            let (sa, sb) = (a.sample(d), b.sample(d));
            assert_eq!(sa.alpha.to_bits(), sb.alpha.to_bits());
            assert_eq!(sa.light.to_bits(), sb.light.to_bits());
        }
    }

    #[test]
    fn different_seeds_give_different_clouds() {
        let a = CloudLayer::new(&clouds(0.5, 0.5), 42, sun()).unwrap();
        let b = CloudLayer::new(&clouds(0.5, 0.5), 43, sun()).unwrap();
        let difference: f32 = sky_dirs().iter().map(|&d| (a.sample(d).alpha - b.sample(d).alpha).abs()).sum();
        assert!(difference > 1.0, "seeds 42 and 43 gave nearly the same clouds: {difference}");
    }

    #[test]
    fn the_lining_is_brightest_toward_the_sun() {
        let layer = CloudLayer::new(&clouds(0.5, 0.0), 1, sun()).unwrap();
        let sun_e = vec3f(10.0, 10.0, 10.0);
        let at = |cos_sun: f32| {
            layer.shade(&CloudSample { alpha: 1.0, light: 0.5, cos_sun }, sun_e, Vec3f::default()).y
        };
        assert!(at(0.99) > 5.0 * at(0.0), "{} vs {}", at(0.99), at(0.0));
        assert!(at(0.0) > 0.0);
        // The sky alone lights the underside, half greyed toward white.
        let ambient = layer.shade(
            &CloudSample { alpha: 1.0, light: 0.0, cos_sun: 0.0 },
            Vec3f::default(),
            vec3f(0.1, 0.2, 0.4),
        );
        assert!(ambient.z > ambient.x && ambient.x > 0.05, "{ambient:?}");
    }

    #[test]
    fn the_more_of_the_dome_is_cloud_the_less_blue_the_underside_has() {
        // A cloud sees the blue dome through the gaps and other cloud
        // everywhere else, so a closed deck has no blue to carry.
        let ambient = vec3f(0.1, 0.2, 0.4);
        let underside = |coverage: f32| {
            let layer = CloudLayer::new(&clouds(coverage, 0.0), 1, sun()).unwrap();
            layer.shade(&CloudSample { alpha: 1.0, light: 0.0, cos_sun: 0.0 }, Vec3f::default(), ambient)
        };
        let blue = |coverage: f32| {
            let c = underside(coverage);
            c.z / c.x
        };
        let (scattered, broken, mostly, closed) = (blue(0.1), blue(0.4), blue(0.7), blue(1.0));
        assert!(scattered > broken && broken > mostly && mostly > closed, "{scattered} {broken} {mostly} {closed}");
        // Scattered cloud keeps the half-greyed underside it always had...
        assert!(scattered > 1.8, "{scattered}");
        // ...and a closed deck is the grey of the dome's luminance, nothing else.
        let grey = crate::sky::luminance(ambient);
        let c = underside(1.0);
        for channel in [c.x, c.y, c.z] {
            assert!((channel - grey).abs() < 1.0e-5, "{c:?} against {grey}");
        }
    }

    #[test]
    fn cirrus_is_a_thin_fully_lit_veil() {
        let layer = CloudLayer::new(&clouds(0.0, 1.0), 5, sun()).unwrap();
        let mut most = 0.0f32;
        for d in sky_dirs() {
            let s = layer.sample(d);
            assert!(s.alpha <= CIRRUS_OPACITY + 1.0e-6, "cirrus stays translucent: {}", s.alpha);
            if s.alpha > 0.0 {
                assert!((s.light - 1.0).abs() < 1.0e-6, "cirrus is not self-shadowed");
            }
            most = most.max(s.alpha);
        }
        assert!(most > 0.05, "some cirrus should show: {most}");
    }
}

#[cfg(test)]
mod env_tests {
    use super::*;
    use crate::hdri::atmosphere::Atmosphere;
    use crate::hdri::{dir_from_az_el, Env, HdriParams};
    use crate::sky::luminance;

    fn cloudy(coverage: f32, seed: u32) -> HdriParams {
        let mut p = HdriParams {
            mode: "sky".to_string(),
            seed,
            ..Default::default()
        };
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.elevation_deg = 40.0;
        p.sky.sun.azimuth_deg = 180.0;
        p.sky.clouds.coverage = coverage;
        p.sky.clouds.cirrus = 0.0;
        p
    }

    #[test]
    fn no_cover_leaves_the_clear_sky() {
        let p = cloudy(0.0, 1);
        let env = Env::new(&p);
        let atmo = Atmosphere::new(dir_from_az_el(180.0, 40.0), &p.sky.atmosphere, &p.sky.sun_disc);
        for &(az, el) in &[(0.0f32, 10.0f32), (90.0, 45.0), (180.0, 40.0), (270.0, 80.0), (45.0, -20.0)] {
            let d = dir_from_az_el(az, el);
            let want = atmo.sky(d) + atmo.sun_disc(d);
            let got = env.radiance(d);
            assert!(
                (got - want).length() <= 1.0e-4 * want.length().max(1.0e-3),
                "({az}, {el}): {got:?} vs {want:?}"
            );
        }
    }

    #[test]
    fn full_cover_dims_the_sun_key_and_hides_the_disc() {
        let clear = Env::new(&cloudy(0.0, 1));
        let overcast = Env::new(&cloudy(1.0, 1));
        let clear_key = clear.sun().expect("the sun is up");
        let overcast_key = overcast.sun().expect("the sun is still the key, only dimmed");
        assert!(
            luminance(overcast_key.radiance) < 0.05 * luminance(clear_key.radiance),
            "{:?} vs {:?}",
            overcast_key.radiance,
            clear_key.radiance
        );
        let at_sun = dir_from_az_el(180.0, 40.0);
        assert!(luminance(overcast.radiance(at_sun)) < 1.0e-3 * luminance(clear.radiance(at_sun)));
        let zenith = vec3f(0.0, 1.0, 0.0);
        let change = (overcast.radiance(zenith) - clear.radiance(zenith)).length();
        assert!(change > 0.01 * luminance(clear.radiance(zenith)), "the deck shows overhead");
    }

    #[test]
    fn the_overcast_preset_is_a_grey_deck() {
        // Linear blue over red of the deck, away from the horizon haze: the
        // clear dome's blue is 4 to 1 here, and a deck under a deck is grey.
        let env = Env::new(&crate::hdri::presets::preset("Overcast").expect("the Overcast preset"));
        for (az, el) in [(0.0f32, 60.0f32), (0.0, 30.0), (90.0, 30.0), (200.0, 45.0), (300.0, 20.0)] {
            let c = env.radiance(dir_from_az_el(az, el));
            let blue = c.z / c.x;
            assert!((0.85..1.1).contains(&blue), "({az}, {el}): {c:?} has blue over red {blue}");
        }
    }

    /// A manual sun due south at `el` degrees.
    fn sun_at(coverage: f32, el: f32) -> HdriParams {
        let mut p = cloudy(coverage, 1);
        p.sky.sun.elevation_deg = el;
        p
    }

    #[test]
    fn full_cover_hides_a_low_sun_as_it_hides_a_high_one() {
        // The visible deck is only 20 % opaque at 2 degrees and 40 % at 3, but
        // the sun's key and disc are behind all of it: the horizon fade is
        // for the haze the deck thins into, not for the light it blocks.
        for el in [2.0f32, 3.0, 5.0, 10.0] {
            let clear = Env::new(&sun_at(0.0, el));
            let overcast = Env::new(&sun_at(1.0, el));
            let clear_key = clear.sun().expect("the sun is up");
            let overcast_key = overcast.sun().expect("the sun is still the key, only dimmed");
            assert!(
                luminance(overcast_key.radiance) < 1.0e-3 * luminance(clear_key.radiance),
                "el {el}: key {:?} vs {:?}",
                overcast_key.radiance,
                clear_key.radiance
            );
            let at_sun = dir_from_az_el(180.0, el);
            assert!(
                luminance(overcast.radiance(at_sun)) < 1.0e-2 * luminance(clear.radiance(at_sun)),
                "el {el}: the disc shows through: {:?} vs {:?}",
                overcast.radiance(at_sun),
                clear.radiance(at_sun)
            );
        }
    }

    #[test]
    fn the_key_is_dimmed_by_the_unfaded_cover() {
        // Partial cover, a 3 degree sun: the key follows cover_toward the sun,
        // not the deck's faded alpha.
        let sun_dir = dir_from_az_el(180.0, 3.0);
        let clear_key = luminance(Env::new(&sun_at(0.0, 3.0)).sun().unwrap().radiance);
        let mut unlike_the_faded_alpha = 0;
        for seed in 0..8 {
            let mut p = sun_at(0.5, 3.0);
            p.seed = seed;
            let layer = CloudLayer::new(&p.sky.clouds, seed, sun_dir).unwrap();
            let key = luminance(Env::new(&p).sun().unwrap().radiance) / clear_key;
            let want = 1.0 - layer.cover_toward(sun_dir);
            assert!((key - want).abs() < 1.0e-4, "seed {seed}: key {key}, 1 - cover {want}");
            if ((1.0 - layer.sample(sun_dir).alpha) - want).abs() > 0.05 {
                unlike_the_faded_alpha += 1;
            }
        }
        assert!(unlike_the_faded_alpha > 0, "no seed tells the cover from the faded alpha");
    }

    #[test]
    fn full_cover_hides_a_low_moon_and_its_key() {
        // The moon is a disc behind the deck like the sun: same cover.
        let night = |coverage: f32| {
            let mut p = sun_at(coverage, -30.0);
            p.sky.night.moon = true;
            p.sky.night.moon_elevation_deg = 3.0;
            p.sky.night.moon_azimuth_deg = 180.0;
            Env::new(&p)
        };
        let (clear, overcast) = (night(0.0), night(1.0));
        let clear_key = clear.sun().expect("the moon is the key");
        let overcast_key = overcast.sun().expect("and stays the key, dimmed");
        assert!(luminance(clear_key.radiance) > 0.0);
        assert!(luminance(overcast_key.radiance) < 1.0e-3 * luminance(clear_key.radiance));
        // What is left of the disc's texel is the airglow and the moonlit air
        // in front of the cloud (about 1 % here), not the moon: before the
        // cover was unfaded 60 % of it showed through.
        let at_moon = dir_from_az_el(180.0, 3.0);
        assert!(
            luminance(overcast.radiance(at_moon)) < 5.0e-2 * luminance(clear.radiance(at_moon)),
            "{:?} vs {:?}",
            overcast.radiance(at_moon),
            clear.radiance(at_moon)
        );
    }

    #[test]
    fn a_cloudy_bake_is_deterministic_per_seed() {
        let p = cloudy(0.5, 42);
        assert_eq!(Env::new(&p).bake(32), Env::new(&p).bake(32));
        let mut other = p.clone();
        other.seed = 43;
        assert_ne!(Env::new(&p).bake(32), Env::new(&other).bake(32));
    }
}
