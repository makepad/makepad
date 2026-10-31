//! Environment maps for the engine (phase 2 of the HDRI generator): the
//! generator's params or an HDR file become the engine's own `EnvMap`, and
//! a photographed sky gives up its sun as an `EnvSun` for the renderer's
//! directional light.
//!
//! The renderer lights the world from the map without its key (prepare.rs),
//! because the directional light already carries that energy. A generated
//! map leaves exactly the key's own light out ([`bake_generated_env_map`]:
//! the same map baked without it, N3), so the lights around a wide studio
//! key stay; a sun found in a file has no such map, so its covering cone is
//! filled with the sky around it ([`remove_sun`]). `load_env_map(.., true)`
//! fills it too, so a loaded file lights the same way whichever path
//! registers it.

use makepad_render_material::ibl::{self, EnvMap};
use super::*;
use super::image::{load_equirect, PathOrigin};
use crate::sky::luminance;

/// Bakes `params` into an engine environment map `width` x `width / 2`
/// (texel centres, ibl's equirect) and reports its key light. `run`
/// spreads the rows as for [`Env::bake_par`]: `pool.fan_out` inside a
/// Heavy job, a plain loop on the UI thread. The sun is [`Env::sun`]:
/// world space, the rotation applied, already x 2^intensity_ev.
pub fn bake_env_map(
    params: &HdriParams,
    width: usize,
    run: impl FnOnce(usize, &(dyn Fn(usize) + Sync)),
) -> (EnvMap, Option<EnvSun>) {
    let (map, sun, _) = bake_env_map_with_daylight(params, width, run);
    (map, sun)
}

/// [`bake_env_map`] and the map's own sun for the daylight switches,
/// `Environment.daylight_sun`: [`Env::sun_dir`], the sky's sun wherever it is
/// (below the horizon while the moon is the key, or at twilight with no key at
/// all), world space like the key; `None` for a studio. A host that bakes a
/// map for the engine reports both.
pub fn bake_env_map_with_daylight(
    params: &HdriParams,
    width: usize,
    run: impl FnOnce(usize, &(dyn Fn(usize) + Sync)),
) -> (EnvMap, Option<EnvSun>, Option<Vec3f>) {
    // A shadow caster's bloom is rastered at this bake's width (R5).
    let env = Env::new_for_width(params, &ImageSet::default(), width, &|| false).expect("an Env nothing cancels is built");
    let (sun, daylight) = (env.sun(), env.sun_dir());
    (env.bake_par(width, run), sun, daylight)
}

/// A generated map with everything the engine takes from it.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedEnvMap {
    /// The map as the dome shows it, key and all ([`bake_env_map`]'s).
    pub map: EnvMap,
    /// The same map without exactly its key's own light
    /// ([`Env::bake_keyless_par`]), the source of the renderer's lighting copy
    /// while the key is the one the world lights with; `None` without a key.
    pub keyless: Option<EnvMap>,
    /// The key ([`Env::sun`]), world space, x 2^intensity_ev.
    pub sun: Option<EnvSun>,
    /// The sky's own sun for the daylight switches ([`Env::sun_dir`]).
    pub daylight: Option<Vec3f>,
}

/// [`bake_env_map_with_daylight`] and the map without its key: what a host
/// hands the renderer for a generated map
/// (`Renderer::register_generated_environment`; a procedural preset the
/// renderer bakes itself goes the same way). `run` spreads the rows of the
/// bake and then of the key's reach, so it is called twice. No pictures: an
/// image light draws plain. See [`bake_generated_env_map_with`].
pub fn bake_generated_env_map(params: &HdriParams, width: usize, run: impl Fn(usize, &(dyn Fn(usize) + Sync))) -> GeneratedEnvMap {
    bake_generated_env_map_with(params, &ImageSet::default(), width, run)
}

/// [`bake_generated_env_map`] with the pictures the params name by key taken
/// from `images` (1b: image lights). A key the set lacks draws as it does in
/// the app: the plain light. [`ImageStore`] fills such a set from files.
pub fn bake_generated_env_map_with(params: &HdriParams, images: &ImageSet, width: usize, run: impl Fn(usize, &(dyn Fn(usize) + Sync))) -> GeneratedEnvMap {
    // Nothing cancels it, so it is always made.
    bake_generated_env_map_cancellable(params, images, width, run, &|| false).expect("a bake nothing cancels is made")
}

/// [`bake_generated_env_map_with`] for a job a newer request can overtake
/// (the sandbox's map job): its `Env` is built through
/// [`Env::new_for_width_par`] at `width` (a caster's rasters are laid out for
/// it, their rows spread by `run`),
/// so the first build of a seed's cloud volume noise (about 2.2 s in
/// release) and the bloom's pre-pass stop when `cancelled` says so, and the
/// bake is then None (D15, L:229). The rows are `run`'s to skip; a bake that
/// was not stopped is `bake_generated_env_map_with`'s, bit for bit.
pub fn bake_generated_env_map_cancellable(
    params: &HdriParams,
    images: &ImageSet,
    width: usize,
    run: impl Fn(usize, &(dyn Fn(usize) + Sync)),
    cancelled: &dyn Fn() -> bool,
) -> Option<GeneratedEnvMap> {
    let env = Env::new_for_width_par(params, images, width, cancelled, &run)?;
    let map = env.bake_par(width, &run);
    let keyless = env.bake_keyless_par(&map, &run);
    Some(GeneratedEnvMap { map, keyless, sun: env.sun(), daylight: env.sun_dir() })
}

/// A host's loaded pictures: the set [`Env::new_with`] reads, and the keys
/// whose file could not be read. Those are not tried again, so a map that
/// is re-baked often (the sandbox's day cycle) does not go to the disk for
/// a missing file on every bake. `Env` itself still opens no file, and
/// neither does the store: the host reads (`load_missing`'s `read`) and
/// hands `set` to the bake.
#[derive(Clone, Debug, Default)]
pub struct ImageStore {
    pub set: ImageSet,
    failed: std::collections::HashSet<String>,
}

impl ImageStore {
    /// Reads every picture `params` names that is neither loaded nor failed.
    /// `read` gives a key's bytes: the host resolves the key (the app's keys
    /// are full paths; the sandbox reads a relative key inside the game's
    /// folder) and bounds the read; an `Err` (a missing file, a key the host
    /// refuses) is a failed file. A light's picture is decoded as the app
    /// decodes it (`image::decode_image`: the file's orientation, its alpha),
    /// shrunk to `MAX_LIGHT_IMAGE_SIDE`; a base image as an equirect
    /// (`image::decode_equirect`: 2:1, rolled to the engine convention,
    /// opaque). Returns one line per file that failed in this call.
    pub fn load_missing(&mut self, params: &HdriParams, read: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Vec<String> {
        let mut problems = Vec::new();
        // The base image (1b, B15): an equirect, rolled to the engine
        // convention, at full size (`image::decode_equirect`). Before the
        // light pictures, so a file named both as the base image and as a
        // light's picture is the panorama, as in the app.
        if let Some(key) = params.base_image.as_ref().map(|base| base.key.clone()).filter(|key| !key.is_empty()) {
            if !self.set.contains(&key) && !self.failed.contains(&key) {
                match read(&key).and_then(|bytes| super::image::decode_equirect(&bytes)) {
                    Ok(map) => {
                        self.set.insert(key, map);
                    }
                    Err(error) => {
                        problems.push(format!("base image {key}: {error}"));
                        self.failed.insert(key);
                    }
                }
            }
        }
        for key in super::image_light::referenced_keys(params) {
            if self.set.contains(&key) || self.failed.contains(&key) {
                continue;
            }
            match read(&key).and_then(|bytes| super::image::decode_image(&bytes)) {
                Ok(map) => {
                    let map = super::image_light::shrink_to(map, super::image_light::MAX_LIGHT_IMAGE_SIDE);
                    self.set.insert(key, map);
                }
                Err(error) => {
                    problems.push(format!("picture {key}: {error}"));
                    self.failed.insert(key);
                }
            }
        }
        problems
    }
}

/// Loads an equirect as an engine environment map (EXR, Radiance .hdr, or
/// PNG/JPG as linearised LDR; `image::load_equirect` checks 2:1 and rolls
/// the file's +X-centred columns to the engine convention). With `detect`,
/// the brightest compact region becomes the sun ([`detect_sun`]) and its
/// covering cone is filled with the sky around it ([`remove_sun`]), so the
/// renderer's directional light carries it exactly once. The returned map
/// therefore shows NO disc: a host that also draws the map as its dome
/// loads with `image::load_equirect`, calls [`detect_sun`] and declares the
/// sun on `Environment.sun` instead; the renderer fills the cone in its
/// lighting copy and the dome keeps the disc (the sandbox does this).
pub fn load_env_map(path: &std::path::Path, detect: bool) -> Result<(EnvMap, Option<EnvSun>), String> {
    // The host chose the path (D5: a preset's path rule is the app's to apply).
    let mut env = load_equirect(path, PathOrigin::Picked)?;
    let sun = if detect { detect_sun(&env) } else { None };
    if let Some(sun) = &sun {
        remove_sun(&mut env, sun);
    }
    Ok((env, sun))
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
pub(super) fn rows_near(dir: Vec3f, radius: f32, h: usize) -> std::ops::Range<usize> {
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
    fn serial(n: usize, f: &(dyn Fn(usize) + Sync)) {
        for i in 0..n {
            f(i)
        }
    }
    fn max_luminance(env: &EnvMap) -> f32 {
        env.data.iter().map(|c| luminance(vec3f(c[0], c[1], c[2]))).fold(0.0, f32::max)
    }
    /// A map's whole emission, ∫ L dΩ per channel, with exact texel solid angles.
    fn emission(env: &EnvMap) -> Vec3f {
        let mut sum = [0.0f64; 3];
        for y in 0..env.height {
            let area = area_at(y, env.width, env.height) as f64;
            for t in &env.data[y * env.width..(y + 1) * env.width] {
                for k in 0..3 {
                    sum[k] += t[k] as f64 * area;
                }
            }
        }
        vec3f(sum[0] as f32, sum[1] as f32, sum[2] as f32)
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
    fn a_baked_map_is_the_env_and_its_sun() {
        let p = HdriParams::default();
        let (map, sun) = bake_env_map(&p, 64, serial);
        assert_eq!((map.width, map.height), (64, 32));
        let env = Env::new(&p);
        assert_eq!(map, env.bake(64), "the same texels as the serial bake");
        assert_eq!(sun, env.sun());
        // The default params are a midsummer noon sky: its sun is up.
        assert!(sun.is_some(), "the default noon sky has its sun up");
        assert!(sun.unwrap().dir.y > 0.0);
    }

    /// R5, M5: the bakes that know their width build their Env for it
    /// (`Env::new_for_width`), so a shadow caster's rasters are laid out for
    /// the bake: the map is that Env's, not the reference width's. The caster
    /// is soft, and a softness raster's cells are no finer than the bake's
    /// texel; its bloom's raster is the reference width's at 64 (no map below
    /// it rasters a bloom more coarsely, D6).
    #[test]
    fn a_generated_bake_rasters_a_caster_at_its_width() {
        let mut p = HdriParams::default();
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.azimuth_deg = 200.0;
        p.sky.sun.elevation_deg = 30.0;
        p.lights.push(LightParams {
            azimuth_deg: 200.0,
            elevation_deg: 47.0,
            width_deg: 40.0,
            height_deg: 30.0,
            gobo: Some(GoboParams { kind: "leaves".to_string(), density: 0.6, distance: 0.0, seed: 3, ..GoboParams::default() }),
            caster: Some(CasterParams { softness: 0.3, bloom_strength: 1.0, bloom_length_deg: 20.0, ..CasterParams::default() }),
            ..LightParams::new_caster()
        });
        let at_64 = Env::new_for_width(&p, &ImageSet::default(), 64, &|| false).expect("nothing cancels it").bake(64);
        assert!(bake_generated_env_map(&p, 64, serial).map == at_64, "the generated bake builds its Env at its width");
        assert!(bake_env_map(&p, 64, serial).0 == at_64, "bake_env_map builds its Env at its width");
        assert!(Env::new(&p).bake(64) != at_64, "the reference width's raster is another");
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
    fn a_baked_clear_sky_gives_up_the_sun_it_was_baked_with() {
        // The generator's own maps through the photograph's route: the sun
        // `detect_sun` finds is the one `Env::sun` reported, in direction and, up to
        // the tenth of its energy its cone leaves outside, in irradiance.
        for name in ["Clear noon", "Golden hour", "Sunset"] {
            let params = crate::hdri::presets::preset(name).unwrap();
            let (map, key) = bake_env_map(&params, 512, serial);
            let key = key.expect("the sun is up");
            let sun = detect_sun(&map).unwrap_or_else(|| panic!("{name}: the baked sun is a sun"));
            assert!(sun.validate().is_ok(), "{name}: {sun:?}");
            assert!(angle_deg(sun.dir, key.dir) < 0.7, "{name}: found {:?}, baked at {:?}", az_el_from_dir(sun.dir), az_el_from_dir(key.dir));
            let ratio = luminance(sun.irradiance()) / luminance(key.irradiance());
            assert!(ratio > 0.85 && ratio < 1.05, "{name}: the detected irradiance is {ratio} of the baked sun's");
            assert!(sun.facing > 0.999, "{name}: {}", sun.facing);
        }
        // A closed overcast hides the disc: its key is dimmed to nothing and the map has no sun.
        let (map, _) = bake_env_map(&crate::hdri::presets::preset("Overcast").unwrap(), 512, serial);
        assert!(detect_sun(&map).is_none());
    }

    /// M6: a detected sun's radiance is the energy it gathered over the cone
    /// as its f32 cosine gives it, so `irradiance()` hands that energy back,
    /// also for a sun of one texel, whose cone keeps few digits of 1 - cos,
    /// and for one at the zenith of a fine map, whose cosine rounds to 1: its
    /// cone is the narrowest an f32 can name instead of none.
    #[test]
    fn a_detected_sun_carries_its_energy_through_its_f32_cone() {
        for (w, y) in [(1024usize, 40usize), (1024, 256), (2048, 0)] {
            let h = w / 2;
            let mut map = EnvMap::constant(w, [1.0, 1.0, 1.0]);
            map.data[y * w + 300] = [1.0e5, 1.0e5, 1.0e5, 1.0];
            let sun = detect_sun(&map).expect("one hot texel is a sun");
            assert!(sun.validate().is_ok(), "{sun:?}");
            let want = (1.0e5 - 1.0) * area_at(y, w, h);
            let got = sun.irradiance().y;
            assert!((got / want - 1.0).abs() < 1.0e-6, "{w} wide, row {y}: {got} for {want} ({sun:?})");
        }
    }

    /// N1: a bake reports the sky's own sun beside its key: under the
    /// moonlit preset the key is the moon and the sun is 30 degrees down; a
    /// studio reports none.
    #[test]
    fn a_bake_reports_the_skys_own_sun_for_the_daylight_switches() {
        let moonlit = crate::hdri::presets::preset("Moonlit night").unwrap();
        let (map, key, daylight) = bake_env_map_with_daylight(&moonlit, 32, serial);
        assert_eq!((map, key), bake_env_map(&moonlit, 32, serial), "the same map and key");
        assert_eq!(daylight, Env::new(&moonlit).sun_dir());
        assert!(daylight.is_some_and(|d| d.y < -0.4) && key.is_some_and(|k| k.dir.y > 0.4), "{daylight:?} {key:?}");
        let (_, _, studio) = bake_env_map_with_daylight(&crate::hdri::presets::preset("Three-point").unwrap(), 32, serial);
        assert_eq!(studio, None);
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

    /// `params` with the backdrop black and every Add light but the key taken
    /// out: what is left is the key as the map draws it, behind its flags.
    fn the_key_alone(params: &HdriParams) -> HdriParams {
        let key = params.lights.iter().position(|l| l.enabled && l.key && l.blend() == Blend::Add).expect("a key light");
        let mut studio = params.studio.clone();
        studio.top = [0.0; 3];
        studio.horizon = [0.0; 3];
        studio.floor = [0.0; 3];
        HdriParams {
            studio,
            lights: params.lights.iter().enumerate().filter(|(i, l)| *i == key || l.blend() == Blend::Multiply).map(|(_, l)| l.clone()).collect(),
            ..params.clone()
        }
    }

    #[test]
    fn removing_a_studio_key_leaves_none_of_it_in_the_map() {
        // The cone `radiance` is averaged over does not hold a rect's corners or a
        // soft light's edge: the covering cone does, and remove_sun fills that one.
        let mut narrow_leaves_more = false;
        for name in ["Three-point", "Top softbox", "Rim pair", "Overcast dome", "Ring light"] {
            let alone = the_key_alone(&crate::hdri::presets::preset(name).unwrap());
            let (mut map, sun) = bake_env_map(&alone, 256, serial);
            let sun = sun.expect("a key light");
            assert!(sun.cos_cover < sun.cos_radius, "{name}: the reach box is wider than the cone, {sun:?}");
            let whole = emission(&map);
            assert!(whole.y > 0.0, "{name}");
            let mut narrow = map.clone();
            remove_sun(&mut narrow, &EnvSun { cos_cover: sun.cos_radius, ..sun });
            narrow_leaves_more |= emission(&narrow).y > 0.005 * whole.y;
            remove_sun(&mut map, &sun);
            let left = emission(&map);
            for (got, all) in [(left.x, whole.x), (left.y, whole.y), (left.z, whole.z)] {
                assert!(got < 0.005 * all, "{name}: {got} of the key's {all} is left in the map");
            }
        }
        assert!(narrow_leaves_more, "the cone radiance is averaged over leaves part of a wide key behind: the test can see it");
    }

    #[test]
    fn a_loaded_hdr_file_finds_its_sun_and_fills_the_disc_only_when_asked() {
        let sun_dir = dir_from_az_el(200.0, 25.0);
        let (map, _) = sky_with_disc(128, 0.5, sun_dir, 4.0, 2000.0);
        let path = std::env::temp_dir().join(format!("makepad_hdri_envmap_{}.hdr", std::process::id()));
        // encode_hdr rolls to the file convention; load_equirect rolls back.
        std::fs::write(&path, super::super::export::encode_hdr(&map)).unwrap();
        let with = load_env_map(&path, true);
        let without = load_env_map(&path, false);
        let _ = std::fs::remove_file(&path);
        let ((with, sun), (without, none)) = (with.unwrap(), without.unwrap());
        assert_eq!((with.width, with.height), (128, 64));
        let sun = sun.expect("the disc is detected");
        assert!(angle_deg(sun.dir, sun_dir) < 2.0, "{:?}", az_el_from_dir(sun.dir));
        assert!(none.is_none());
        // detect = true also fills the cone (the renderer's light carries it);
        // detect = false keeps the file as it is (RGBE holds 1 % precision).
        assert!(max_luminance(&with) < 0.51, "{}", max_luminance(&with));
        assert!(max_luminance(&without) > 1900.0);
        let far = ibl::dir_to_equirect_uv(arr(dir_from_az_el(20.0, 25.0)));
        let i = ((far[1] * 64.0) as usize) * 128 + (far[0] * 128.0) as usize;
        assert!((with.data[i][1] - 0.5).abs() < 0.01 && (without.data[i][1] - 0.5).abs() < 0.01);
    }

    /// The same through OpenEXR (the spec: `load_env_map` "for .hdr and
    /// .exr"): the decoder is A8's, this pins that the route reaches it and
    /// that float precision keeps the sky exact.
    #[test]
    fn a_loaded_exr_file_finds_its_sun_too() {
        use super::super::export::{encode_exr, ExrPrecision};
        let sun_dir = dir_from_az_el(200.0, 25.0);
        let (map, _) = sky_with_disc(128, 0.5, sun_dir, 4.0, 2000.0);
        let path = std::env::temp_dir().join(format!("makepad_hdri_envmap_{}.exr", std::process::id()));
        let (bytes, report) = encode_exr(&map, ExrPrecision::Float).unwrap();
        assert_eq!(report.clipped, 0);
        std::fs::write(&path, bytes).unwrap();
        let with = load_env_map(&path, true);
        let without = load_env_map(&path, false);
        let _ = std::fs::remove_file(&path);
        let ((with, sun), (without, none)) = (with.unwrap(), without.unwrap());
        assert_eq!((with.width, with.height), (128, 64));
        let sun = sun.expect("the disc is detected");
        assert!(angle_deg(sun.dir, sun_dir) < 2.0, "{:?}", az_el_from_dir(sun.dir));
        assert!(none.is_none());
        assert!(max_luminance(&with) < 0.51 && max_luminance(&without) > 1900.0);
        let far = ibl::dir_to_equirect_uv(arr(dir_from_az_el(20.0, 25.0)));
        let i = ((far[1] * 64.0) as usize) * 128 + (far[0] * 128.0) as usize;
        assert!((with.data[i][1] - 0.5).abs() < 1.0e-4 && (without.data[i][1] - 0.5).abs() < 1.0e-4);
    }

    #[test]
    fn a_malformed_map_is_no_sun_and_is_left_alone() {
        let short = EnvMap { width: 8, height: 4, data: vec![[1.0; 4]; 3] };
        assert!(detect_sun(&short).is_none());
        let mut copy = short.clone();
        remove_sun(&mut copy, &EnvSun { dir: vec3f(0.0, 1.0, 0.0), radiance: vec3f(1.0, 1.0, 1.0), cos_radius: 0.9, facing: 1.0, cos_cover: 0.9 });
        assert_eq!(copy, short);
    }

    /// A studio map with one large image light whose picture is `key`.
    fn picture_light(key: &str) -> HdriParams {
        let mut p = HdriParams::default();
        p.mode = "studio".to_string();
        p.lights.push(LightParams {
            width_deg: 60.0,
            height_deg: 60.0,
            image: Some(LightImageParams { key: key.to_string(), ..LightImageParams::default() }),
            ..LightParams::default()
        });
        p
    }

    #[test]
    fn a_host_bakes_with_the_pictures_its_params_name() {
        let dir = std::env::temp_dir().join(format!("makepad_hdri_envmap_images_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A picture that is black on its left half and white on its right.
        let png = Cx::encode_rgba_as_png(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]).unwrap();
        std::fs::write(dir.join("half.png"), png).unwrap();
        let params = picture_light("half.png");
        // The host reads a key's bytes (here: from the test's folder; the
        // sandbox reads inside the game's folder, bounded); the store decodes.
        let read = |key: &str| std::fs::read(dir.join(key)).map_err(|e| e.to_string());
        let mut store = ImageStore::default();
        // The first call reads the file: the picture is in the set under its key.
        assert!(store.load_missing(&params, &read).is_empty());
        assert!(store.set.contains("half.png"));
        // With the pictures the light shows its picture; without them it is plain.
        let with = bake_generated_env_map_with(&params, &store.set, 64, serial);
        let plain = bake_generated_env_map(&params, 64, serial);
        assert!(with.map != plain.map, "the picture shows in the bake");
        assert!(plain == bake_generated_env_map_with(&params, &ImageSet::default(), 64, serial), "bake_generated_env_map is the empty set");
        // A loaded picture is not read again: the file may go.
        std::fs::remove_file(dir.join("half.png")).unwrap();
        assert!(store.load_missing(&params, &read).is_empty());
        assert!(store.set.contains("half.png"));
        // A file that cannot be read is reported once, and its light stays plain.
        let missing = picture_light("gone.png");
        let problems = store.load_missing(&missing, &read);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("gone.png"), "{problems:?}");
        assert!(store.load_missing(&missing, &read).is_empty(), "a failed file is not tried again");
        assert!(bake_generated_env_map_with(&missing, &store.set, 64, serial) == bake_generated_env_map(&missing, 64, serial));
        // A key the host refuses to read (the sandbox refuses one outside the
        // game's folder) fails like a missing file: reported, its light plain.
        let outside = picture_light("../outside.png");
        let refuse = |key: &str| -> Result<Vec<u8>, String> { Err(format!("`{key}` must be a relative path inside the game's folder")) };
        let problems = store.load_missing(&outside, &refuse);
        assert!(problems.len() == 1 && problems[0].contains("inside the game's folder"), "{problems:?}");
        assert!(!store.set.contains("../outside.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_host_reads_the_base_image_its_params_name() {
        let dir = std::env::temp_dir().join(format!("makepad_hdri_envmap_base_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A panorama: bright above the horizon, dark below it.
        let pano = EnvMap::from_fn(32, |d| if d[1] > 0.0 { [3.0, 3.0, 3.0] } else { [0.2, 0.2, 0.2] });
        std::fs::write(dir.join("pano.hdr"), crate::hdri::export::encode_hdr(&pano)).unwrap();
        let mut params = HdriParams::default();
        params.mode = "studio".to_string();
        params.base_image = Some(BaseImageParams { key: "pano.hdr".to_string(), ..BaseImageParams::default() });
        let read = |key: &str| std::fs::read(dir.join(key)).map_err(|e| e.to_string());
        let mut store = ImageStore::default();
        assert!(store.load_missing(&params, &read).is_empty());
        // Loaded as an equirect: 2:1, in the engine convention (up is still the bright half).
        let map = store.set.get("pano.hdr").expect("the base image is in the set");
        assert_eq!((map.width, map.height), (32, 16));
        assert!(map.sample([0.0, 1.0, 0.0])[1] > 2.5 && map.sample([0.0, -1.0, 0.0])[1] < 0.5);
        // The bake shows it; without the set the studio backdrop shows.
        let with = bake_generated_env_map_with(&params, &store.set, 64, serial).map;
        assert!(with != bake_generated_env_map(&params, 64, serial).map, "the base image shows in the bake");
        let up = with.sample([0.0, 1.0, 0.0]);
        assert!((up[1] - 3.0).abs() < 0.1, "{up:?}");
        // A file that is not there is reported once.
        params.base_image.as_mut().unwrap().key = "gone.hdr".to_string();
        let problems = store.load_missing(&params, &read);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("gone.hdr"), "{problems:?}");
        assert!(store.load_missing(&params, &read).is_empty(), "a failed file is not tried again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// D13: an EXR whose alpha is 0 is a base image in the colour it stores
    /// (it loaded black since B14, L:124), in the set and in the bake.
    #[test]
    fn a_base_image_with_alpha_0_keeps_its_colour() {
        use makepad_openexr::{write_to_vec, Compression, ExrChannel, ExrImage, ExrPart};
        // An 8 x 4 sky of colour 2.5, written with A = 0 everywhere.
        let plane = vec![2.5f32; 32];
        let part = ExrPart::new(None, 8, 4, Compression::None, vec![
            ExrChannel::float("A", vec![0.0; 32]),
            ExrChannel::float("B", plane.clone()),
            ExrChannel::float("G", plane.clone()),
            ExrChannel::float("R", plane),
        ]);
        let bytes = write_to_vec(&ExrImage::single(part)).unwrap();
        let params = HdriParams {
            mode: "studio".to_string(),
            base_image: Some(BaseImageParams { key: "sky.exr".to_string(), ..BaseImageParams::default() }),
            ..HdriParams::default()
        };
        let mut store = ImageStore::default();
        assert!(store.load_missing(&params, &|_: &str| Ok(bytes.clone())).is_empty());
        let map = store.set.get("sky.exr").expect("the base image is in the set");
        assert!(map.data.iter().all(|t| *t == [2.5, 2.5, 2.5, 1.0]), "{:?}", map.data[0]);
        let baked = bake_generated_env_map_with(&params, &store.set, 32, serial).map;
        assert!(baked.data.iter().all(|t| (t[1] - 2.5).abs() < 1.0e-5), "{:?}", baked.data[0]);
    }

    /// D15 (L:229), the hosts' half: a generated bake a newer request
    /// overtakes (the sandbox's map job) stops in the first build of its
    /// seed's cloud noise, as the app's preview does (`Env::new_cancellable`),
    /// and is None; one that is not stopped is `bake_generated_env_map_with`'s
    /// to the bit. (A seed no other test uses: the kept noise is shared.)
    #[test]
    fn a_stopped_generated_bake_stops_in_its_first_cloud_noise() {
        let mut sky = HdriParams { seed: 0x4837_0232, ..HdriParams::default() };
        sky.sky.clouds.coverage = 0.6;
        sky.sky.volumetric = Some(VolumetricParams { steps: 16, ..VolumetricParams::default() });
        let images = ImageSet::default();
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let stop = || {
            asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            true
        };
        assert!(bake_generated_env_map_cancellable(&sky, &images, 16, serial, &stop).is_none(), "a stopped bake was made");
        assert!(asked.load(std::sync::atomic::Ordering::Relaxed) > 0);
        let made = bake_generated_env_map_cancellable(&sky, &images, 16, serial, &|| false).expect("not stopped");
        assert!(made == bake_generated_env_map_with(&sky, &images, 16, serial));
    }
}
