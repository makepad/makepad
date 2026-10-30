//! Environment maps for the renderer: the IBL lane texture, dome and meters a
//! map is prepared into ([`prepare`]), a generated map with its key
//! ([`GeneratedEnvMap`]), and a photographed sky's sun ([`detect_sun`],
//! [`remove_sun`]) for the renderer's directional light.
//!
//! Conventions:
//! - Y is up, +X is east and -Z is north.
//! - Azimuths are degrees clockwise from north (0 = -Z, 90 = +X), as in NOAA
//!   and `ibl::dir_deg`.
//! - Equirect: `ibl`'s: `u = 0.5 + atan2(x, -z) / 2pi`, `v = acos(y) / pi`.
//!   -Z (north) sits at the image centre, row 0 is the zenith and texels are
//!   evaluated at their centres.
//! - `rotation_deg` turns the content about +Y, positive counter-clockwise
//!   seen from above, exactly as `EnvMap::procedural` and `Ibl.rotation_deg`
//!   do: +90 moves a light at -Z to -X ([`rotate_y`]).
//! - `ibl` works in `[f32; 3]`; this module works in `Vec3f` and converts at
//!   the boundary with [`arr`] and [`vec`].

// `!(x > 0.0)` and its kin are written negated on purpose: a NaN fails the comparison, so
// it takes the guard's branch.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use makepad_draw::*;
use makepad_render_material::ibl::{self, EnvMap};
use crate::sky::luminance;

pub mod prepare;

/// The environment's key light is the engine's own type (makepad-scene),
/// so a baked or detected sun goes straight into `World.environment.sun`.
pub use makepad_scene::EnvSun;

/// `ibl` works in `[f32; 3]`; the layers work in `Vec3f`. These two
/// functions are the whole boundary between them.
pub fn arr(v: Vec3f) -> [f32; 3] {
    [v.x, v.y, v.z]
}

pub fn vec(a: [f32; 3]) -> Vec3f {
    vec3f(a[0], a[1], a[2])
}

/// Unit direction for an azimuth (degrees clockwise from north) and an
/// elevation (degrees above the horizon): (sin az cos el, sin el, -cos az cos el).
pub fn dir_from_az_el(azimuth_deg: f32, elevation_deg: f32) -> Vec3f {
    let (az, el) = (azimuth_deg.to_radians(), elevation_deg.to_radians());
    let horizontal = el.cos();
    vec3f(az.sin() * horizontal, el.sin(), -az.cos() * horizontal)
}

/// Azimuth in [0, 360) and elevation in [-90, 90], in degrees. The input need
/// not be normalised. Straight up or down (or no direction at all) has no
/// azimuth and reports 0.
pub fn az_el_from_dir(dir: Vec3f) -> (f32, f32) {
    let horizontal = (dir.x * dir.x + dir.z * dir.z).sqrt();
    // The negated test also catches NaN.
    if !(horizontal > 1.0e-6 * dir.y.abs()) {
        let elevation = if dir.y > 0.0 {
            90.0
        } else if dir.y < 0.0 {
            -90.0
        } else {
            0.0
        };
        return (0.0, elevation);
    }
    // atan2 for both angles: no normalisation is needed, and it stays
    // accurate near the poles where asin(y) does not.
    let elevation = dir.y.atan2(horizontal).to_degrees();
    let mut azimuth = dir.x.atan2(-dir.z).to_degrees();
    if azimuth < 0.0 {
        azimuth += 360.0;
    }
    // A tiny negative angle plus 360 rounds to 360, which is outside [0, 360).
    if azimuth >= 360.0 {
        azimuth = 0.0;
    }
    (azimuth, elevation)
}

/// Turns `dir` about +Y by `deg`, positive counter-clockwise seen from above
/// (ibl's sign: +90 takes -Z to -X, the turn `EnvMap::procedural` and the
/// shader's `mat_ibl_dir` apply to a `rotation_deg`); the elevation does not
/// change. In compass terms the azimuth falls by `deg`.
pub fn rotate_y(dir: Vec3f, deg: f32) -> Vec3f {
    let (s, c) = deg.to_radians().sin_cos();
    vec3f(dir.x * c + dir.z * s, dir.y, dir.z * c - dir.x * s)
}

/// A generated map with everything the engine takes from it: what a host hands
/// `Renderer::register_generated_environment`.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedEnvMap {
    /// The map as the dome shows it, key and all (a generator's bake).
    pub map: EnvMap,
    /// The same map without exactly its key's own light
    /// (a generator bakes it), the source of the renderer's lighting copy
    /// while the key is the one the world lights with; `None` without a key.
    pub keyless: Option<EnvMap>,
    /// The key, world space, at the map's own scale.
    pub sun: Option<EnvSun>,
    /// The sky's own sun for the daylight switches (`Environment.daylight_sun`).
    pub daylight: Option<Vec3f>,
}

/// Luminance of the map's solid-angle-weighted mean radiance: the auto-exposure meter. An
/// empty map meters 0 rather than NaN.
pub fn mean_luminance(env: &EnvMap) -> f32 {
    if env.width == 0 || env.height == 0 || env.data.len() < env.width * env.height {
        return 0.0;
    }
    let l = crate::sky::luminance(vec(env.mean()));
    if l.is_finite() {
        l
    } else {
        0.0
    }
}

/// The brightest full-resolution texel is sought this far around the
/// box-averaged seed (`EnvMap::brightest_direction` averages to 64 x 32,
/// whose cells span 5.6 deg).
const SUN_SEED_DEG: f32 = 6.0;
/// How far from the peak the sun's excess energy is gathered, in degrees.
const SUN_SEARCH_DEG: f32 = 20.0;
/// Width in degrees of the ring just outside the search disc that measures
/// the sky behind the sun.
const SUN_BACKGROUND_DEG: f32 = 5.0;
/// The share of the excess energy the sun's cone holds.
const SUN_ENERGY_SHARE: f32 = 0.9;
/// How much a sun must outshine the sky around it. A clear-sky sun in an
/// unclipped HDR is 10^4 to 10^5 times its surroundings. An 8-bit map's
/// clipped sun (about 2x) and an overcast glow stay well below this.
const SUN_MIN_CONTRAST: f32 = 20.0;
/// A cone wider than this is a bright region (a window, a lit cloud), not
/// a sun.
const SUN_MAX_RADIUS_DEG: f32 = 10.0;

/// Finds the sun in an HDR equirect. The steps:
/// 1. Seed with `EnvMap::brightest_direction` (a 64 x 32 box average, so a
///    single hot pixel cannot mislead it), then take the brightest
///    full-resolution texel within 6 deg of the seed as the peak.
/// 2. Measure the sky level in a ring 20-25 deg around the peak.
/// 3. Find the energy centre of everything above that level within 20 deg
///    (a clipped, flat-topped sun's first maximal texel sits on its rim,
///    so the cone grows from the centre, not from the peak).
/// 4. Grow a cone outward from that centre until it holds 90 % of the
///    excess energy.
///
/// `radiance` is the gathered energy over the cone's solid angle, so
/// `radiance x solid angle` is the light's irradiance; `facing` is the
/// gathered texels' cosine-weighted share, Σ L cosθ dΩ / Σ L dΩ about the
/// centre; `cos_cover` is the cosine of the farthest texel gathered, the cone
/// that holds all of them (never above the cone's own `cos_radius`). `None`
/// when nothing outshines its surroundings (overcast, 8-bit) or when the
/// bright region is not compact.
pub fn detect_sun(env: &EnvMap) -> Option<EnvSun> {
    let (w, h) = (env.width, env.height);
    if w < 2 || h < 1 || env.data.len() < w * h {
        return None;
    }
    // 1. The peak texel near the seed.
    let seed = vec(env.brightest_direction());
    let seed_radius = SUN_SEED_DEG.to_radians();
    let cos_seed = seed_radius.cos();
    let mut peak = None;
    let mut peak_lum = 0.0f32;
    for y in rows_near(seed, seed_radius, h) {
        for x in 0..w {
            let d = texel_dir(x, y, w, h);
            if d.dot(seed) < cos_seed {
                continue;
            }
            let l = luminance(finite_rgb(env.data[y * w + x]));
            if l > peak_lum {
                peak_lum = l;
                peak = Some(d);
            }
        }
    }
    let peak_dir = peak?;
    // 2. Every texel within the search radius, and the sky level in the
    // ring just outside it.
    let cos_search = SUN_SEARCH_DEG.to_radians().cos();
    let outer = (SUN_SEARCH_DEG + SUN_BACKGROUND_DEG).to_radians();
    let cos_outer = outer.cos();
    let mut near: Vec<(Vec3f, f32, Vec3f)> = Vec::new();
    let mut ring_sum = Vec3f::default();
    let mut ring_area = 0.0f32;
    for y in rows_near(peak_dir, outer, h) {
        let area = row_solid_angle(y, w, h);
        for x in 0..w {
            let d = texel_dir(x, y, w, h);
            let c = d.dot(peak_dir);
            if c < cos_outer {
                continue;
            }
            let rgb = finite_rgb(env.data[y * w + x]);
            if c >= cos_search {
                near.push((rgb, area, d));
            } else {
                ring_sum += rgb * area;
                ring_area += area;
            }
        }
    }
    let background = if ring_area > 0.0 { ring_sum * (1.0 / ring_area) } else { Vec3f::default() };
    if peak_lum <= SUN_MIN_CONTRAST * luminance(background) {
        return None;
    }
    let excess = |rgb: Vec3f| {
        vec3f(
            (rgb.x - background.x).max(0.0),
            (rgb.y - background.y).max(0.0),
            (rgb.z - background.z).max(0.0),
        )
    };
    // 3. The energy centre of the bright region.
    let mut total = 0.0f32;
    let mut centre = Vec3f::default();
    for &(rgb, area, d) in &near {
        let l = luminance(excess(rgb)) * area;
        total += l;
        centre += d * l;
    }
    if !(total > 0.0) || !(centre.length() > 0.0) {
        return None;
    }
    let centre = centre.normalize();
    // 4. Nearest first from the centre, until the cone holds 90 % of the
    // excess.
    let mut ordered: Vec<(f32, Vec3f, f32)> =
        near.iter().map(|&(rgb, area, d)| (d.dot(centre), rgb, area)).collect();
    ordered.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut gathered = 0.0f32;
    let mut gathered_cos = 0.0f32;
    let mut energy = Vec3f::default();
    let mut omega = 0.0f32;
    let mut cos_edge = 1.0f32;
    for &(cos, rgb, area) in &ordered {
        let e = excess(rgb);
        let l = luminance(e) * area;
        gathered += l;
        gathered_cos += l * cos;
        energy += e * area;
        omega += area;
        cos_edge = cos;
        if gathered >= SUN_ENERGY_SHARE * total {
            break;
        }
    }
    // The cone is the cap with the gathered texels' area: omega = 2π(1 - cos r).
    // At least one texel was gathered, so omega > 0; a texel at the zenith of a
    // fine map is so small that its cosine rounds to 1 in f32, a cone with no
    // solid angle, so the narrowest cone an f32 cosine names stands for it.
    let cos_radius = (1.0 - omega / std::f32::consts::TAU).clamp(-1.0, 1.0 - 0.5 * f32::EPSILON);
    if cos_radius < SUN_MAX_RADIUS_DEG.to_radians().cos() {
        return None;
    }
    // The gathered energy is at least 90 % of a positive total, so `gathered`
    // is positive; the share of it a surface facing the centre receives is the
    // cosine-weighted mean.
    let facing = (gathered_cos / gathered).clamp(0.0, 1.0);
    // The cap's cosine comes from an area and the texel's from a direction, so
    // either may be the smaller: the covering cone is the wider of the two.
    let cos_cover = cos_edge.clamp(-1.0, 1.0).min(cos_radius);
    // The energy over the cone as its f32 cosine names it, the cone every
    // consumer of the key computes: `irradiance()` gives the energy back.
    Some(EnvSun { dir: centre, radiance: EnvSun::cone_radiance(energy, cos_radius), cos_radius, facing, cos_cover })
}

/// Fills the sun's covering cone (`cos_cover`: the cone that holds the key's
/// whole reach, a studio key's corners and soft edge included) with the mean
/// of the ring just outside it, leaving the sky that was behind the sun. The
/// cone is widened by one texel for two reasons: a downsampled map has
/// smeared the disc into the texels around it, and a sun smaller than a texel
/// must still take its own texel with it. The ring is a quarter of the cone's
/// radius wide and never narrower than two texels, so even a tiny sun
/// averages a real ring. Filling an already filled cone changes nothing (the
/// ring mean is the fill); a malformed map or sun is left alone.
pub fn remove_sun(env: &mut EnvMap, sun: &EnvSun) {
    let (w, h) = (env.width, env.height);
    if w < 2 || h < 1 || env.data.len() < w * h || !sun.cos_cover.is_finite() || !sun.cos_radius.is_finite() || !sun.dir.is_finite() {
        return;
    }
    let dir = sun.dir.normalize();
    // A zero or NaN direction names no cone.
    if !(dir.length() > 0.5) {
        return;
    }
    let pi = std::f32::consts::PI;
    let texel = pi / h as f32;
    // Never narrower than the cone `radiance` is averaged over, whatever a host
    // that builds its own key puts in `cos_cover`.
    let radius = sun.cos_cover.min(sun.cos_radius).clamp(-1.0, 1.0).acos();
    let fill = (radius + texel).min(pi);
    let outer = (fill + (fill * 0.25).max(2.0 * texel)).min(pi);
    let (cos_fill, cos_outer) = (fill.cos(), outer.cos());
    let rows = rows_near(dir, outer, h);
    let mut sum = Vec3f::default();
    let mut area = 0.0f32;
    for y in rows.clone() {
        let a = row_solid_angle(y, w, h);
        for x in 0..w {
            let c = texel_dir(x, y, w, h).dot(dir);
            if c < cos_fill && c >= cos_outer {
                sum += finite_rgb(env.data[y * w + x]) * a;
                area += a;
            }
        }
    }
    // A cone that covers the whole sphere has no outside to borrow from.
    if area <= 0.0 {
        return;
    }
    let ring = sum * (1.0 / area);
    for y in rows {
        for x in 0..w {
            if texel_dir(x, y, w, h).dot(dir) >= cos_fill {
                env.data[y * w + x] = [ring.x, ring.y, ring.z, 1.0];
            }
        }
    }
}

/// Direction through the centre of texel (x, y) of a w x h equirect, in
/// ibl's convention.
fn texel_dir(x: usize, y: usize, w: usize, h: usize) -> Vec3f {
    vec(ibl::equirect_uv_to_dir([(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32]))
}

/// Solid angle of one texel in row y: its longitude slice (2π/w) times its
/// band of cos(polar angle). The texels of a whole map sum to 4π exactly.
fn row_solid_angle(y: usize, w: usize, h: usize) -> f32 {
    let pi = std::f32::consts::PI;
    let top = (pi * y as f32 / h as f32).cos();
    let bottom = (pi * (y + 1) as f32 / h as f32).cos();
    std::f32::consts::TAU / w as f32 * (top - bottom)
}

/// The rows a cone of `radius` (radians) around `dir` can reach: the rows
/// within its polar angle +- radius. Every column of those rows still needs
/// the angle test, because near a pole the cone wraps all the way round.
pub fn rows_near(dir: Vec3f, radius: f32, h: usize) -> std::ops::Range<usize> {
    let pi = std::f32::consts::PI;
    let theta = dir.y.clamp(-1.0, 1.0).acos();
    let lo = (((theta - radius) / pi) * h as f32).floor().max(0.0) as usize;
    let hi = ((((theta + radius) / pi) * h as f32).ceil().max(0.0) as usize).min(h);
    lo.min(h)..hi
}

/// Bad files carry NaN, infinities and negative texels. They count as black.
fn finite_rgb(c: [f32; 4]) -> Vec3f {
    let f = |v: f32| if v.is_finite() && v > 0.0 { v } else { 0.0 };
    vec3f(f(c[0]), f(c[1]), f(c[2]))
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{PI, TAU};

    // Texel geometry is written out here so the tests do not lean on the
    // helpers they check. Same formulas as ibl::from_fn (texel centres).
    fn dir_at(x: usize, y: usize, w: usize, h: usize) -> Vec3f {
        vec(ibl::equirect_uv_to_dir([(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32]))
    }
    fn area_at(y: usize, w: usize, h: usize) -> f32 {
        let top = (PI * y as f32 / h as f32).cos();
        let bottom = (PI * (y + 1) as f32 / h as f32).cos();
        TAU / w as f32 * (top - bottom)
    }
    fn angle_deg(a: Vec3f, b: Vec3f) -> f32 {
        a.normalize().dot(b.normalize()).clamp(-1.0, 1.0).acos().to_degrees()
    }
    fn max_luminance(env: &EnvMap) -> f32 {
        env.data.iter().map(|c| luminance(vec3f(c[0], c[1], c[2]))).fold(0.0, f32::max)
    }

    /// A grey sky of `sky` with a disc of `radius_deg` around `dir` whose
    /// texels hold `disc`. Returns the map and the disc's energy above the
    /// sky (the sun's true irradiance on a facing surface).
    fn sky_with_disc(w: usize, sky: f32, dir: Vec3f, radius_deg: f32, disc: f32) -> (EnvMap, f32) {
        let h = w / 2;
        let cos_r = radius_deg.to_radians().cos();
        let mut data = Vec::with_capacity(w * h);
        let mut energy = 0.0;
        for y in 0..h {
            for x in 0..w {
                let inside = dir_at(x, y, w, h).dot(dir) >= cos_r;
                let v = if inside { disc } else { sky };
                if inside {
                    energy += (disc - sky) * area_at(y, w, h);
                }
                data.push([v, v, v, 1.0]);
            }
        }
        (EnvMap { width: w, height: h, data }, energy)
    }

    #[test]
    fn a_synthetic_sun_is_found_where_it_was_put() {
        let sun_dir = dir_from_az_el(120.0, 35.0);
        let (map, energy) = sky_with_disc(256, 1.0, sun_dir, 2.0, 5000.0);
        let sun = detect_sun(&map).expect("a 5000:1 disc is a sun");
        assert!(angle_deg(sun.dir, sun_dir) < 1.0, "found at {:?}", az_el_from_dir(sun.dir));
        let radius = sun.cos_radius.clamp(-1.0, 1.0).acos().to_degrees();
        assert!(radius > 0.5 && radius < 3.0, "cone radius {radius} deg");
        // radiance x solid angle is the energy the cone gathered: at least 90 %.
        let irradiance = sun.radiance.y * TAU * (1.0 - sun.cos_radius);
        assert!(
            irradiance >= 0.895 * energy && irradiance <= 1.005 * energy,
            "irradiance {irradiance} vs the disc's {energy}"
        );
        assert!((sun.irradiance().y - irradiance).abs() < 1.0e-3 * energy, "EnvSun::irradiance agrees");
    }

    #[test]
    fn a_detected_suns_facing_and_covering_cone_are_what_it_gathered() {
        // A flat 8 degree disc, the widest a detected sun is allowed to grow: its
        // cone is a cap, whose cosine-weighted share is (1 + cos r) / 2 exactly.
        let (map, _) = sky_with_disc(512, 1.0, dir_from_az_el(210.0, 40.0), 8.0, 5000.0);
        let sun = detect_sun(&map).expect("a 5000:1 disc is a sun");
        assert!(sun.validate().is_ok(), "{sun:?}");
        let cap = 0.5 * (1.0 + sun.cos_radius);
        assert!((sun.facing - cap).abs() < 0.002, "facing {} vs the cap's {cap}", sun.facing);
        assert!(sun.facing < 1.0, "a cone eight degrees wide is not a point");
        // The covering cone holds every texel the sun gathered: its cosine is the
        // last one's, never above the cone's own.
        assert!(sun.cos_cover <= sun.cos_radius);
        let cover_deg = sun.cos_cover.acos().to_degrees();
        let radius_deg = sun.cos_radius.acos().to_degrees();
        assert!(cover_deg >= radius_deg && cover_deg < radius_deg + 2.0, "cover {cover_deg} deg, cone {radius_deg} deg");
        // A point-like sun faces almost fully.
        let (point, _) = sky_with_disc(256, 1.0, dir_from_az_el(120.0, 35.0), 1.0, 5000.0);
        let sun = detect_sun(&point).expect("a 5000:1 disc is a sun");
        assert!(sun.facing > 0.999 && sun.facing <= 1.0, "facing {}", sun.facing);
        assert!(sun.validate().is_ok(), "{sun:?}");
        // One hot texel: the cone is the texel's own, and the key still validates.
        let mut one = EnvMap::constant(256, [1.0, 1.0, 1.0]);
        one.data[40 * 256 + 100] = [1.0e5, 1.0e5, 1.0e5, 1.0];
        let sun = detect_sun(&one).expect("one hot texel is a sun");
        assert!(sun.facing > 0.9999 && sun.validate().is_ok(), "{sun:?}");
    }

    #[test]
    fn a_sun_of_two_distant_texels_is_covered_whole() {
        // The cone `radiance` is averaged over has the area of the texels gathered,
        // which for two hot texels six apart is far smaller than the cone that reaches
        // both: the covering cone is the one that does, and removing the sun clears them.
        let (w, h) = (256, 128);
        let mut map = EnvMap::constant(w, [1.0, 1.0, 1.0]);
        for x in [97, 103] {
            map.data[40 * w + x] = [1.0e5, 1.0e5, 1.0e5, 1.0];
        }
        let sun = detect_sun(&map).expect("two hot texels are a sun");
        assert!(sun.validate().is_ok(), "{sun:?}");
        assert!(sun.cos_cover < sun.cos_radius, "{sun:?}");
        for x in [97, 103] {
            assert!(dir_at(x, 40, w, h).dot(sun.dir) >= sun.cos_cover - 1.0e-6, "texel {x} is inside the covering cone");
        }
        remove_sun(&mut map, &sun);
        assert!(max_luminance(&map) < 1.01, "both are gone, brightest texel {}", max_luminance(&map));
    }

    #[test]
    fn a_one_texel_sun_has_a_finite_cone() {
        let (w, h) = (256, 128);
        let mut map = EnvMap::constant(w, [1.0, 1.0, 1.0]);
        map.data[40 * w + 100] = [1.0e5, 1.0e5, 1.0e5, 1.0];
        let sun = detect_sun(&map).expect("one hot texel is a sun");
        // 0.1 deg, not tighter: acos of an f32 dot this close to 1 is only good to about 0.03 deg.
        assert!(angle_deg(sun.dir, dir_at(100, 40, w, h)) < 0.1);
        assert!(sun.cos_radius < 1.0, "a zero cone would make the radiance infinite");
        assert!((sun.radiance.y - (1.0e5 - 1.0)).abs() < 1.0e3, "{:?}", sun.radiance);
    }

    #[test]
    fn overcast_and_clipped_skies_have_no_sun() {
        let overcast = EnvMap::from_fn(128, |d| {
            let v = 0.5 + 0.5 * d[1].max(0.0);
            [v, v, v]
        });
        assert!(detect_sun(&overcast).is_none(), "a bright zenith is not a sun");
        // An 8-bit sky: the sun clipped to 1.0 over a 0.8 sky.
        let (ldr, _) = sky_with_disc(128, 0.8, dir_from_az_el(90.0, 30.0), 4.0, 1.0);
        assert!(detect_sun(&ldr).is_none());
        assert!(detect_sun(&EnvMap::constant(16, [0.0, 0.0, 0.0])).is_none(), "black has no sun");
    }

    #[test]
    fn a_bright_region_wider_than_a_sun_is_not_one() {
        // A 13 deg glow over a 1.0 sky, brightest at its centre, well inside
        // the 20 deg search. Its 90 % cone is about 12 deg, past the 10 deg a
        // sun may have.
        let centre = dir_from_az_el(45.0, 40.0);
        let cos_r = 13.0f32.to_radians().cos();
        let map = EnvMap::from_fn(256, |d| {
            let c = vec(d).dot(centre);
            let v = if c >= cos_r { 1000.0 + 100.0 * (c - cos_r) / (1.0 - cos_r) } else { 1.0 };
            [v, v, v]
        });
        assert!(detect_sun(&map).is_none());
    }

    #[test]
    fn removing_the_sun_leaves_the_sky_behind_it() {
        let sun_dir = dir_from_az_el(300.0, 20.0);
        let (mut map, _) = sky_with_disc(256, 1.0, sun_dir, 2.0, 5000.0);
        let sun = detect_sun(&map).unwrap();
        remove_sun(&mut map, &sun);
        let max = max_luminance(&map);
        assert!(max < 1.01, "the disc is gone, brightest texel {max}");
        // A texel 30 deg away was never touched.
        let far = dir_from_az_el(330.0, 20.0);
        let uv = ibl::dir_to_equirect_uv(arr(far));
        let (x, y) = ((uv[0] * 256.0) as usize, (uv[1] * 128.0) as usize);
        assert_eq!(map.data[y * 256 + x], [1.0, 1.0, 1.0, 1.0]);
        // Idempotent: filling an already filled cone changes nothing.
        let again = map.clone();
        remove_sun(&mut map, &sun);
        assert_eq!(map, again);
        // A sun with no cone (NaN) or no direction is ignored.
        let broken = EnvSun { dir: vec3f(0.0, 0.0, 0.0), radiance: vec3f(1.0, 1.0, 1.0), cos_radius: 0.9, facing: 1.0, cos_cover: 0.9 };
        remove_sun(&mut map, &broken);
        assert_eq!(map, again);
        let nan = EnvSun { dir: sun_dir, radiance: vec3f(1.0, 1.0, 1.0), cos_radius: 0.9, facing: 1.0, cos_cover: f32::NAN };
        remove_sun(&mut map, &nan);
        assert_eq!(map, again);
    }

    #[test]
    fn a_malformed_map_is_no_sun_and_is_left_alone() {
        let short = EnvMap { width: 8, height: 4, data: vec![[1.0; 4]; 3] };
        assert!(detect_sun(&short).is_none());
        let mut copy = short.clone();
        remove_sun(&mut copy, &EnvSun { dir: vec3f(0.0, 1.0, 0.0), radiance: vec3f(1.0, 1.0, 1.0), cos_radius: 0.9, facing: 1.0, cos_cover: 0.9 });
        assert_eq!(copy, short);
    }
}
