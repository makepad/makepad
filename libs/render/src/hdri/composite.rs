//! HDRI compositing and sky replacement (phase 1b).
//!
//! A loaded equirect (a photographed panorama, a render) becomes the base
//! layer under the lights, in one of two ways:
//!
//! - **Compositing** (`replace_sky` off): the image is the whole base, in
//!   either mode, and the lights are drawn over it. The generated sky is not
//!   seen, so its weather and its cloud volume are not built ([`whole_base`],
//!   the plan owner's D6).
//! - **Sky replacement** (`replace_sky` on): above a horizon line the mode's
//!   own base shows (the generated sky), below it the image. The line is an
//!   elevation mask with a feather, optionally times a sky key that keeps
//!   whatever in the image stands against its sky. The image's exposure is
//!   matched to the new sky: its ground was lit by its own sky, so both skies
//!   are metered as the light a level surface receives and the image is
//!   scaled by the ratio. The weather's rain haze and fog lie over the image
//!   below the line as over the sky above it (D6).
//!
//! The image's sun is found with `envmap::detect_sun`. `remove_sun` cuts it
//! out of the map (`envmap::remove_sun`, run on a small copy, supplies what
//! goes in its place), and "match sun" writes its direction into the manual
//! sun, so the generated sky's sun stands where the photograph's did. Left
//! in, its texels are area-averaged by the bake as the generated sun's are
//! (`BaseImage::drawn_sun`, D6).
//!
//! `Env::new_with` runs per preview bake, so nothing here may read every
//! texel of a large image each time: the sun search and the small copies are
//! worked out once per image and kept for as long as the image lives
//! ([`image_facts`]).
//!
//! Directions are in the map's own frame (before the map rotation). Images
//! are in the engine's equirect convention (`image::load_equirect` rolls
//! files to it).

use makepad_render_material::ibl::EnvMap;
use super::*;
use super::image_light::PictureMemo;
use crate::sky::luminance;
use std::sync::{Arc, OnceLock};

/// The base layer is metered on a bake this wide (64 x 32) for the exposure
/// match: `Env` bakes its own base at this size and `irradiance_above` reads it.
pub const SKY_METER_WIDTH: usize = 64;

/// Widest copy kept for the sun cut. At this size `remove_sun`'s fill is
/// cheap, and its cone is still round.
const PATCH_WIDTH: usize = 1024;
/// The copy the exposure match and the sky key meter: 128 x 64.
const METER_WIDTH: usize = 128;
/// The widest level of an image's peaks (`Peaks`), a power of two: blocks of
/// about 0.7 degrees.
const PEAK_WIDTH: usize = 512;
/// The sky key reads the image's own sky level in this many degrees above the
/// mask's line.
const SKY_BAND_DEG: f32 = 30.0;
/// The exposure match never scales the image by more than 2^10 either way.
const MATCH_LIMIT: f32 = 1024.0;

/// What is known about one image, worked out once (see [`image_facts`]).
pub struct ImageFacts {
    /// The image's sun, when it has one (`envmap::detect_sun` on the full
    /// map), in the image's own frame and units.
    pub sun: Option<EnvSun>,
    /// A copy at most PATCH_WIDTH wide with the sun's cone filled
    /// (`envmap::remove_sun`); None without a sun.
    patch: Option<EnvMap>,
    /// A METER_WIDTH-wide copy for the metering.
    meter: EnvMap,
    /// The most luminance over regions of the image (`Peaks`), worked out the
    /// first time a caster's bloom asks (`BaseImage::peak`).
    peaks: OnceLock<Peaks>,
}

impl ImageFacts {
    fn measure(map: &EnvMap) -> ImageFacts {
        if !well_formed(map) {
            return ImageFacts { sun: None, patch: None, meter: EnvMap::constant(2, [0.0; 3]), peaks: OnceLock::new() };
        }
        let sun = detect_sun(map);
        // remove_sun widens the cone by a texel of the map it fills, which is
        // what a downsized copy needs: the disc has smeared into its neighbours.
        let patch = sun.as_ref().map(|sun| {
            let mut small = map.resized(map.width.min(PATCH_WIDTH));
            remove_sun(&mut small, sun);
            small
        });
        ImageFacts { sun, patch, meter: map.resized(METER_WIDTH), peaks: OnceLock::new() }
    }

    /// The luminance a level surface receives from the image's upper
    /// hemisphere, its sun included (the meter copy is a solid-angle weighted
    /// box average, so the sun's energy is in it).
    pub fn irradiance(&self) -> f32 {
        irradiance_above(&self.meter, 0.0)
    }
}

/// Upper bounds of an image's luminance over regions of the sphere, for a
/// caster's bloom (`BaseImage::peak`, fix round 3 of S3's review): level 0
/// holds the most luminance of the image's texels in each of its blocks (at
/// most PEAK_WIDTH x PEAK_WIDTH / 2, a power of two wide), each later level the
/// most of 2 x 2 of the one before. In the image's own frame and units.
struct Peaks {
    /// (width, height, the most of each block, row by row) per level.
    levels: Vec<(usize, usize, Vec<f32>)>,
    /// The image's texel, radians: a bilinear read reaches texels up to about
    /// this far from where it reads.
    texel: f32,
}

impl Peaks {
    fn new(map: &EnvMap) -> Peaks {
        let (w, h) = (map.width.max(1), map.height.max(1));
        let mut gw = 2usize;
        while gw * 2 <= w.min(PEAK_WIDTH) {
            gw *= 2;
        }
        let gh = (gw / 2).max(1);
        let mut top = vec![0.0f32; gw * gh];
        if well_formed(map) {
            for y in 0..h {
                let by = y * gh / h;
                for x in 0..w {
                    let lum = luminance(clean(map.data[y * w + x]));
                    let block = &mut top[by * gw + x * gw / w];
                    *block = block.max(lum);
                }
            }
        }
        let mut levels = vec![(gw, gh, top)];
        while let Some((lw, lh, below)) = levels.last().filter(|(lw, lh, _)| *lw > 1 || *lh > 1) {
            let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
            let mut next = vec![0.0f32; nw * nh];
            for y in 0..*lh {
                for x in 0..*lw {
                    let at = &mut next[(y / 2).min(nh - 1) * nw + (x / 2).min(nw - 1)];
                    *at = at.max(below[y * lw + x]);
                }
            }
            levels.push((nw, nh, next));
        }
        let pi = std::f32::consts::PI;
        Peaks { levels, texel: (pi / h as f32).max(std::f32::consts::TAU / w as f32) }
    }

    /// The most luminance the image may show within `radius` (radians) of
    /// `dir` (its own frame), a bilinear read's reach included: never less
    /// than what any read there returns.
    fn max_in(&self, dir: Vec3f, radius: f32) -> f32 {
        let (pi, tau) = (std::f32::consts::PI, std::f32::consts::TAU);
        let (gw, gh) = (self.levels[0].0 as i64, self.levels[0].1 as i64);
        let r = radius.max(0.0) + 1.5 * self.texel;
        let uv = ibl::dir_to_equirect_uv(arr(dir));
        let theta = uv[1] * pi;
        let row = |t: f32| ((t / pi * gh as f32).floor() as i64).clamp(0, gh - 1);
        let (y0, y1) = (row(theta - r), row(theta + r));
        // The cap's reach in longitude, asin(sin r / sin theta), while it holds
        // neither pole; all of it otherwise.
        let reach = r.sin() / theta.sin();
        let (x0, x1) = if theta - r <= 0.0 || theta + r >= pi || !(reach < 1.0) {
            (0, gw - 1)
        } else {
            let du = reach.asin() / tau;
            (((uv[0] - du) * gw as f32).floor() as i64, ((uv[0] + du) * gw as f32).floor() as i64)
        };
        let (x0, x1) = if x1 - x0 + 1 >= gw { (0, gw - 1) } else { (x0, x1) };
        // The level at which the region spans at most four entries a side.
        let span = (x1 - x0).max(y1 - y0);
        let mut k = 0usize;
        while (span >> k) > 3 && k + 1 < self.levels.len() {
            k += 1;
        }
        let (lw, lh, values) = &self.levels[k];
        let (lw, lh) = (*lw as i64, *lh as i64);
        let scale = 1i64 << k;
        let mut most = 0.0f32;
        for y in y0.div_euclid(scale)..=y1.div_euclid(scale) {
            let y = y.clamp(0, lh - 1);
            for x in x0.div_euclid(scale)..=x1.div_euclid(scale) {
                most = most.max(values[(y * lw + x.rem_euclid(lw)) as usize]);
            }
        }
        most
    }
}

/// The facts of every live image (`image_light::PictureMemo`).
static FACTS: PictureMemo<ImageFacts> = PictureMemo::new();

/// The facts of one image: its sun, the sun-free patch and the meter copy.
/// Worked out on the first call (it reads every texel) and kept while the
/// image's `Arc` lives, so later `Env::new_with` calls with the same image
/// cost a lookup. Call it once off the UI thread after loading an image.
pub fn image_facts(map: &Arc<EnvMap>) -> Arc<ImageFacts> {
    FACTS.get(map, ImageFacts::measure)
}

/// Drops what is kept of every picture whose last `Arc` is gone: a base
/// image's facts, a gobo picture's summed-area table, a light picture's mean
/// and its softened mask's table (`image_light::PictureMemo`). Each memo does
/// so whenever it adds an entry; a host that frees a picture calls this (the
/// app after every batch, its base images going by the plan owner's D4; the
/// sandbox when it lets go of a map's pictures), so what is kept of it goes
/// with it, not at the next picture's first bake. Of a 2048 picture, a
/// gobo's table is 33.6 MB and a softened mask's 134.3 MB.
pub fn drop_dead_facts() {
    FACTS.drop_dead();
    super::gobo::drop_dead_tables();
    super::image_light::drop_dead_light_facts();
}

/// The luminance a level surface receives from the rows of `map` more than
/// `min_elevation_deg` (never less than 0) above the horizon: the sum of
/// L x cos(zenith angle) x solid angle. A constant sky of luminance L gives
/// pi x L. The new sky and the image are both metered with this one function,
/// so their ratio does not depend on how good the estimate is.
pub fn irradiance_above(map: &EnvMap, min_elevation_deg: f32) -> f32 {
    let (w, h) = (map.width, map.height);
    if !well_formed(map) {
        return 0.0;
    }
    let floor = if min_elevation_deg.is_finite() { min_elevation_deg.max(0.0) } else { 0.0 };
    let pi = std::f32::consts::PI;
    let mut sum = 0.0f64;
    for y in 0..h {
        // Rows run down from the zenith.
        let theta = (y as f32 + 0.5) / h as f32 * pi;
        if 90.0 - theta.to_degrees() <= floor {
            break;
        }
        let weight = theta.cos() * (std::f32::consts::TAU / w as f32) * (pi / h as f32) * theta.sin();
        let mut row = 0.0f64;
        for x in 0..w {
            row += luminance(clean(map.data[y * w + x])) as f64;
        }
        sum += row * weight as f64;
    }
    sum as f32
}

/// Where the sun was cut: inside `cos_inner` the patch shows, outside
/// `cos_outer` the image, with a ramp between.
#[derive(Clone, Copy)]
struct SunCut {
    dir: Vec3f,
    cos_outer: f32,
    cos_inner: f32,
}

/// A base image, ready to sample.
pub struct BaseImage {
    map: Arc<EnvMap>,
    facts: Arc<ImageFacts>,
    yaw_deg: f32,
    /// 2^ev x the exposure match.
    gain: f32,
    cut: Option<SunCut>,
    replace_sky: bool,
    horizon_deg: f32,
    feather_deg: f32,
    sky_key: f32,
    /// Mean luminance of the image's own sky just above the mask's line, in
    /// the image's units: what the sky key compares a texel with.
    sky_level: f32,
}

impl BaseImage {
    /// None when `p.key` is empty, names no image of `images`, or names a
    /// malformed one. `sky_irradiance` is what a level surface receives from
    /// the base the image is composited over (see `Env::base_irradiance`);
    /// with `replace_sky` on, the image is scaled so its own sky would have
    /// put the same light on the ground.
    pub fn new(p: &BaseImageParams, images: &ImageSet, sky_irradiance: Option<f32>) -> Option<BaseImage> {
        // As clamp() holds it: its ranges and defaults are written once (L:231).
        let p = &p.clamped();
        let map = accepted(p, images)?.clone();
        let facts = image_facts(&map);
        let mut gain = 2.0f32.powf(p.ev);
        if let (true, Some(sky)) = (p.replace_sky, sky_irradiance) {
            let own = facts.irradiance();
            if sky.is_finite() && sky > 0.0 && own > 1.0e-9 {
                gain *= (sky / own).clamp(1.0 / MATCH_LIMIT, MATCH_LIMIT);
            }
        }
        let cut = match (&facts.sun, &facts.patch) {
            (Some(sun), Some(patch)) if p.remove_sun => {
                // remove_sun filled the sun's covering cone (plan 2: its whole
                // reach, min(cos_cover, cos_radius)) widened by one texel of the
                // patch; inside that the patch is the fill. One texel further out
                // the patch is the image again (only softer), so the ramp between
                // the two leaves no seam.
                let pi = std::f32::consts::PI;
                let texel = pi / patch.height.max(1) as f32;
                let radius = sun.cos_cover.min(sun.cos_radius).clamp(-1.0, 1.0).acos();
                Some(SunCut {
                    dir: sun.dir,
                    cos_inner: (radius + texel).min(pi).cos(),
                    cos_outer: (radius + 2.0 * texel).min(pi).cos(),
                })
            }
            _ => None,
        };
        let (horizon_deg, feather_deg) = (p.horizon_deg, p.feather_deg);
        let line = horizon_deg + 0.5 * feather_deg;
        let sky_level = band_luminance(&facts.meter, line, line + SKY_BAND_DEG);
        Some(BaseImage {
            map,
            facts,
            yaw_deg: p.yaw_deg,
            gain,
            cut,
            replace_sky: p.replace_sky,
            horizon_deg,
            feather_deg,
            sky_key: p.sky_key,
            sky_level,
        })
    }

    /// True for sky replacement; false when the image is the whole base.
    pub fn replaces_sky(&self) -> bool {
        self.replace_sky
    }

    /// The image along `d` (map frame): turned by its yaw, at its exposure
    /// (2^ev and the match), with the sun cut when asked for.
    pub fn radiance(&self, d: Vec3f) -> Vec3f {
        self.texel(d) * self.gain
    }

    /// How much of the mode's own base shows along `d`: 0 = the image, 1 = the
    /// base (the generated sky). Always 0 without sky replacement.
    pub fn mask(&self, d: Vec3f) -> f32 {
        let above = self.above(d);
        if above <= 0.0 || self.sky_key <= 0.0 {
            return above;
        }
        above * self.keyed(self.texel(d))
    }

    /// The image under the mask, `base` over it: mix(image, base, mask).
    pub fn compose(&self, d: Vec3f, base: Vec3f) -> Vec3f {
        self.compose_veiled(d, base, &|photo| photo)
    }

    /// `compose` with the image seen through `veil` (the weather's rain haze
    /// and fog, D6): mix(veil(image), base, mask). The mask reads the image
    /// itself, so the sky key keeps what it kept.
    pub fn compose_veiled(&self, d: Vec3f, base: Vec3f, veil: &dyn Fn(Vec3f) -> Vec3f) -> Vec3f {
        let above = self.above(d);
        // All base and no key to consult: the image is not even read.
        if above >= 1.0 && self.sky_key <= 0.0 {
            return base;
        }
        let texel = self.texel(d);
        let mask = above * self.keyed(texel);
        veil(texel * self.gain).mix(base, mask)
    }

    /// The image's sun in the map's own frame (its yaw applied), at the
    /// image's exposure. Reported whether or not the sun was cut from the map:
    /// it is the key light an engine lights with. The cones and the facing
    /// share are angles about the sun's own centre (`detect_sun`'s), so only
    /// the direction turns and only the radiance scales.
    pub fn sun(&self) -> Option<EnvSun> {
        let sun = self.facts.sun.as_ref()?;
        Some(EnvSun {
            dir: rotate_y(sun.dir, self.yaw_deg),
            radiance: sun.radiance * self.gain,
            ..*sun
        })
    }

    /// The image's sun as the map draws it, for the bake's refinement (D6): its
    /// direction in the map's own frame and the radius of the cone that holds
    /// its reach (`detect_sun`'s, at most its own cone). None without a sun,
    /// or when it is cut from the map.
    pub fn drawn_sun(&self) -> Option<(Vec3f, f32)> {
        if self.cut.is_some() {
            return None;
        }
        self.sun().map(|sun| (sun.dir, sun.cos_cover.min(sun.cos_radius).clamp(-1.0, 1.0).acos()))
    }

    /// What a caster's bloom must look for in the image within `radius`
    /// (radians) of `d` (map frame): the most luminance the map may draw of it
    /// there, at its exposure (never less than it draws), and the spacing its
    /// detail asks for (radians): half its texel, so its bilinear read between
    /// texels is followed. The image's peaks are worked out on the first call
    /// and kept with its facts.
    pub fn peak(&self, d: Vec3f, radius: f32) -> (f32, f32) {
        let peaks = self.facts.peaks.get_or_init(|| Peaks::new(&self.map));
        (peaks.max_in(rotate_y(d, -self.yaw_deg), radius) * self.gain, 0.5 * peaks.texel)
    }

    /// The image's own value along `d`, in the image's units.
    fn texel(&self, d: Vec3f) -> Vec3f {
        // The image turned by yaw_deg shows, along d, what it holds along d
        // turned back (rotate_y's sign, as the map's own rotation).
        let local = rotate_y(d, -self.yaw_deg);
        let mut c = clean(self.map.sample(arr(local)));
        if let (Some(cut), Some(patch)) = (&self.cut, &self.facts.patch) {
            let cos = local.normalize().dot(cut.dir);
            if cos > cut.cos_outer {
                c = c.mix(clean(patch.sample(arr(local))), smooth(cut.cos_outer, cut.cos_inner, cos));
            }
        }
        c
    }

    /// The elevation mask alone: 0 below the feather, 1 above it.
    fn above(&self, d: Vec3f) -> f32 {
        if !self.replace_sky {
            return 0.0;
        }
        let horizontal = (d.x * d.x + d.z * d.z).sqrt();
        let elevation = d.y.atan2(horizontal).to_degrees();
        let half = 0.5 * self.feather_deg;
        smooth(self.horizon_deg - half, self.horizon_deg + half, elevation)
    }

    /// The sky key's factor on the mask for an image value: 1 where the image
    /// looks like sky, down to 1 - sky_key where it does not.
    fn keyed(&self, texel: Vec3f) -> f32 {
        if self.sky_key <= 0.0 {
            return 1.0;
        }
        // Sky is the bright part of a photograph: whatever sits well under the
        // level of the image's own sky is a tree, a roof, a hill.
        let bright = smooth(0.2 * self.sky_level, 0.5 * self.sky_level, luminance(texel));
        // And sky is blue or neutral; foliage and brick are not.
        let warm = texel.x.max(texel.y);
        let cool = if warm > 1.0e-6 { smooth(0.7, 0.95, texel.z / warm) } else { 0.0 };
        let likeness = bright * (0.4 + 0.6 * cool);
        1.0 - self.sky_key * (1.0 - likeness)
    }
}

/// True when the params' base image is the whole base and `images` holds it
/// (well formed), so the map draws it in place of the mode's own base: the
/// generated sky is not seen, and its weather and its cloud volume are
/// neither built nor drawn (the plan owner's D6). Exactly when
/// `BaseImage::new` makes an image without sky replacement. The panel greys
/// the Weather and Volumetric controls by it.
pub fn whole_base(params: &HdriParams, images: &ImageSet) -> bool {
    params.base_image.as_ref().is_some_and(|base| !base.replace_sky && accepted(base, images).is_some())
}

/// The picture a base image block names, when `BaseImage::new` takes it: a
/// key that is not empty, names a picture of `images`, and that picture is
/// well formed. One test for `new`, `whole_base` and `image_sun_az_el`, so a
/// condition added to one cannot leave the others drawing what it refuses
/// (L:319).
fn accepted<'a>(p: &BaseImageParams, images: &'a ImageSet) -> Option<&'a Arc<EnvMap>> {
    if p.key.is_empty() {
        return None;
    }
    images.images.get(&p.key).filter(|map| well_formed(map))
}

/// Where the base image's sun stands in the map's own frame (the image's yaw
/// applied): (azimuth, elevation) in degrees, as `az_el_from_dir` gives them.
/// None without an image in `images` or without a sun in it.
pub fn image_sun_az_el(p: &BaseImageParams, images: &ImageSet) -> Option<(f32, f32)> {
    let p = &p.clamped();
    let sun = image_facts(accepted(p, images)?).sun?;
    Some(az_el_from_dir(rotate_y(sun.dir, p.yaw_deg)))
}

/// "Match sun": points the generated sky's sun at the base image's sun, in
/// manual mode (a clock cannot be pointed). With sky replacement the new sky's
/// light then falls the way the photograph's shadows already do. False, and
/// nothing changed, when there is no base image or no sun in it.
pub fn match_sun(params: &mut HdriParams, images: &ImageSet) -> bool {
    let Some((azimuth, elevation)) = params.base_image.as_ref().and_then(|base| image_sun_az_el(base, images)) else {
        return false;
    };
    let sun = &mut params.sky.sun;
    sun.mode = "manual".to_string();
    sun.azimuth_deg = azimuth;
    sun.elevation_deg = elevation;
    true
}

/// A map whose texel list covers its stated size.
fn well_formed(map: &EnvMap) -> bool {
    map.width >= 2 && map.height >= 1 && map.data.len() >= map.width * map.height
}

/// Bad files carry NaN, infinities and negative texels; they count as black.
fn clean(c: [f32; 4]) -> Vec3f {
    let f = |v: f32| if v.is_finite() && v > 0.0 { v } else { 0.0 };
    vec3f(f(c[0]), f(c[1]), f(c[2]))
}

/// Mean luminance of the rows whose elevation lies in `lo_deg..=hi_deg`
/// (solid-angle weighted); 0 when no row does.
fn band_luminance(map: &EnvMap, lo_deg: f32, hi_deg: f32) -> f32 {
    let (w, h) = (map.width, map.height);
    let (mut sum, mut weight) = (0.0f64, 0.0f64);
    for y in 0..h {
        let theta = (y as f32 + 0.5) / h as f32 * std::f32::consts::PI;
        let elevation = 90.0 - theta.to_degrees();
        if elevation < lo_deg || elevation > hi_deg {
            continue;
        }
        let area = theta.sin() as f64;
        for x in 0..w {
            sum += luminance(clean(map.data[y * w + x])) as f64 * area;
            weight += area;
        }
    }
    if weight > 0.0 {
        (sum / weight) as f32
    } else {
        0.0
    }
}

/// smoothstep that is a plain step when the edges coincide (a zero feather,
/// a black sky level), where the usual formula divides by zero.
fn smooth(a: f32, b: f32, x: f32) -> f32 {
    if !(b - a > 1.0e-12) {
        return if x >= b { 1.0 } else { 0.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    /// A photograph stand-in, 64 x 32: a bluish sky above the horizon, a grey ground below it.
    fn photo(sky: f32, ground: f32) -> EnvMap {
        EnvMap::from_fn(64, |d| if d[1] > 0.0 { [sky, sky, 1.2 * sky] } else { [ground, ground, ground] })
    }

    fn set_of(key: &str, map: EnvMap) -> ImageSet {
        let mut images = ImageSet::default();
        images.images.insert(key.to_string(), Arc::new(map));
        images
    }

    fn base(key: &str) -> BaseImageParams {
        BaseImageParams { key: key.to_string(), ..BaseImageParams::default() }
    }

    /// The direction through the centre of texel (x, y) of a 64 x 32 map.
    fn texel_dir(x: usize, y: usize) -> Vec3f {
        vec(ibl::equirect_uv_to_dir([(x as f32 + 0.5) / 64.0, (y as f32 + 0.5) / 32.0]))
    }

    /// `photo(1.0, 0.3)` with one texel as hot as a sun.
    fn photo_with_sun(x: usize, y: usize) -> EnvMap {
        let mut map = photo(1.0, 0.3);
        map.data[y * 64 + x] = [2.0e4, 2.0e4, 2.0e4, 1.0];
        map
    }

    #[test]
    fn an_empty_or_unknown_key_is_no_base_image() {
        let images = set_of("photo", photo(1.0, 0.3));
        assert!(BaseImage::new(&base(""), &images, None).is_none());
        assert!(BaseImage::new(&base("another"), &images, None).is_none());
        assert!(BaseImage::new(&base("photo"), &ImageSet::default(), None).is_none());
        assert!(BaseImage::new(&base("photo"), &images, None).is_some());
        // A map whose texel list is short is refused, not read out of bounds.
        let broken = set_of("photo", EnvMap { width: 8, height: 4, data: vec![[1.0; 4]; 3] });
        assert!(BaseImage::new(&base("photo"), &broken, None).is_none());
    }

    #[test]
    fn the_image_shows_at_its_exposure_and_turns_with_its_yaw() {
        let lamp = dir_from_az_el(90.0, 20.0);
        let map = EnvMap::from_fn(64, |d| if vec(d).dot(lamp) > 0.97 { [50.0; 3] } else { [1.0; 3] });
        let images = set_of("room", map);
        let plain = BaseImage::new(&base("room"), &images, None).unwrap();
        assert!(close(plain.radiance(lamp).y, 50.0, 0.01), "{:?}", plain.radiance(lamp));
        assert!(close(plain.radiance(dir_from_az_el(270.0, 20.0)).y, 1.0, 1.0e-4));
        // Compositing: the image is the whole base, whatever stood there before.
        assert!(!plain.replaces_sky());
        assert_eq!(plain.mask(lamp), 0.0);
        assert_eq!(plain.compose(lamp, vec3f(7.0, 7.0, 7.0)), plain.radiance(lamp));
        // One stop up doubles it.
        let brighter = BaseImage::new(&BaseImageParams { ev: 1.0, ..base("room") }, &images, None).unwrap();
        assert!(close(brighter.radiance(lamp).y, 100.0, 0.02));
        // Yaw has the map rotation's sign: content at azimuth 90 shows at 90 - 30.
        let turned = BaseImage::new(&BaseImageParams { yaw_deg: 30.0, ..base("room") }, &images, None).unwrap();
        assert!(close(turned.radiance(dir_from_az_el(60.0, 20.0)).y, 50.0, 0.01));
        assert!(close(turned.radiance(lamp).y, 1.0, 1.0e-3), "the lamp moved away from azimuth 90");
    }

    #[test]
    fn the_horizon_mask_feathers_between_image_and_sky() {
        let images = set_of("photo", photo(4.0, 0.5));
        let p = BaseImageParams { replace_sky: true, horizon_deg: 0.0, feather_deg: 10.0, ..base("photo") };
        let image = BaseImage::new(&p, &images, None).unwrap();
        assert!(image.replaces_sky());
        let at = |elevation: f32| image.mask(dir_from_az_el(40.0, elevation));
        assert_eq!(at(-5.5), 0.0);
        assert_eq!(at(5.5), 1.0);
        assert!(close(at(0.0), 0.5, 1.0e-3), "{}", at(0.0));
        let mut previous = 0.0;
        for step in -60..=60 {
            let m = at(step as f32 * 0.1);
            assert!(m >= previous, "the mask rises through the feather");
            previous = m;
        }
        // Above the feather the base passes untouched, below it the image does.
        let sky = vec3f(0.2, 0.4, 0.9);
        assert_eq!(image.compose(dir_from_az_el(40.0, 30.0), sky), sky);
        let down = dir_from_az_el(40.0, -30.0);
        assert_eq!(image.compose(down, sky), image.radiance(down));
        assert!(close(image.radiance(down).y, 0.5, 1.0e-4));
        // The line moves with horizon_deg, and a zero feather is a hard cut.
        let raised =
            BaseImage::new(&BaseImageParams { horizon_deg: 20.0, feather_deg: 0.0, ..p.clone() }, &images, None).unwrap();
        assert_eq!(raised.mask(dir_from_az_el(40.0, 19.0)), 0.0);
        assert_eq!(raised.mask(dir_from_az_el(40.0, 21.0)), 1.0);
    }

    #[test]
    fn the_exposure_match_scales_the_image_to_the_new_sky() {
        // A constant sky of luminance L puts pi x L on a level surface.
        let white = EnvMap::constant(128, [1.0, 1.0, 1.0]);
        assert!(close(irradiance_above(&white, 0.0), std::f32::consts::PI, 0.02));
        assert_eq!(irradiance_above(&white, 90.0), 0.0);
        let images = set_of("photo", photo(4.0, 0.5));
        let own = image_facts(&images.images["photo"]).irradiance();
        assert!(close(own, std::f32::consts::PI * luminance(vec3f(4.0, 4.0, 4.8)), 0.4), "{own}");
        let p = BaseImageParams { replace_sky: true, ..base("photo") };
        let down = dir_from_az_el(200.0, -40.0);
        // A new sky a quarter as bright: the photograph's ground follows it down.
        let matched = BaseImage::new(&p, &images, Some(0.25 * own)).unwrap();
        assert!(close(matched.radiance(down).y, 0.125, 1.0e-3), "{:?}", matched.radiance(down));
        // ev trims on top of the match.
        let trimmed = BaseImage::new(&BaseImageParams { ev: 1.0, ..p.clone() }, &images, Some(0.25 * own)).unwrap();
        assert!(close(trimmed.radiance(down).y, 0.25, 2.0e-3));
        // Without sky replacement nothing is matched: the image is the whole base at its own exposure.
        let whole = BaseImage::new(&base("photo"), &images, Some(0.25 * own)).unwrap();
        assert!(close(whole.radiance(down).y, 0.5, 1.0e-4));
        // A broken meter never blows the image up.
        let unmetered = BaseImage::new(&p, &images, Some(f32::NAN)).unwrap();
        assert!(close(unmetered.radiance(down).y, 0.5, 1.0e-4));
    }

    #[test]
    fn the_sky_key_keeps_what_stands_against_the_sky() {
        // A dark green tree against a bright sky.
        let tree = dir_from_az_el(0.0, 15.0);
        let map = EnvMap::from_fn(64, |d| {
            if vec(d).dot(tree) > 0.94 {
                [0.2, 0.3, 0.1]
            } else if d[1] > 0.0 {
                [4.0, 4.0, 4.8]
            } else {
                [0.5, 0.5, 0.5]
            }
        });
        let images = set_of("photo", map);
        let p = BaseImageParams { replace_sky: true, feather_deg: 0.0, sky_key: 1.0, ..base("photo") };
        let keyed = BaseImage::new(&p, &images, None).unwrap();
        let open = dir_from_az_el(180.0, 15.0);
        assert!(keyed.mask(tree) < 0.05, "the tree stays: {}", keyed.mask(tree));
        assert!(keyed.mask(open) > 0.95, "the open sky is replaced: {}", keyed.mask(open));
        // Without the key the elevation alone decides, and the tree goes with the sky.
        let plain = BaseImage::new(&BaseImageParams { sky_key: 0.0, ..p.clone() }, &images, None).unwrap();
        assert_eq!(plain.mask(tree), 1.0);
        // Half a key replaces half of it.
        let half = BaseImage::new(&BaseImageParams { sky_key: 0.5, ..p.clone() }, &images, None).unwrap();
        assert!(close(half.mask(tree), 0.5, 0.05), "{}", half.mask(tree));
        // Below the line the key changes nothing.
        assert_eq!(keyed.mask(dir_from_az_el(0.0, -20.0)), 0.0);
    }

    #[test]
    fn the_images_sun_is_found_cut_and_reported() {
        let images = set_of("photo", photo_with_sun(48, 8));
        let sun_dir = texel_dir(48, 8);
        let sun = image_facts(&images.images["photo"]).sun.expect("a 20000:1 texel is a sun");
        assert!(sun.dir.dot(sun_dir) > 0.9995, "{:?} vs {:?}", sun.dir, sun_dir);
        // Left in, the disc is in the map; cut, the sky around it shows in its place.
        let kept = BaseImage::new(&base("photo"), &images, None).unwrap();
        assert!(kept.radiance(sun_dir).y > 1.0e4);
        let cut = BaseImage::new(&BaseImageParams { remove_sun: true, ..base("photo") }, &images, None).unwrap();
        let filled = cut.radiance(sun_dir);
        assert!(close(filled.x, 1.0, 0.05) && close(filled.z, 1.2, 0.06), "{filled:?}");
        // Away from the cone nothing changed.
        let far = dir_from_az_el(300.0, 40.0);
        assert_eq!(cut.radiance(far), kept.radiance(far));
        // Either way the sun is reported, with the image's exposure and yaw applied.
        let turned = BaseImage::new(
            &BaseImageParams { remove_sun: true, ev: 1.0, yaw_deg: 40.0, ..base("photo") },
            &images,
            None,
        )
        .unwrap();
        let reported = turned.sun().expect("the image's sun");
        assert!(reported.dir.dot(rotate_y(sun.dir, 40.0)) > 0.99999);
        assert!(close(reported.radiance.y / sun.radiance.y, 2.0, 1.0e-3));
        assert_eq!((reported.cos_radius, reported.facing, reported.cos_cover), (sun.cos_radius, sun.facing, sun.cos_cover), "detect_sun's cones and facing kept");
        assert!(reported.validate().is_ok(), "{reported:?}");
        assert!(kept.sun().is_some());
        // An image without a sun reports none, and remove_sun cuts nothing from it.
        let dull = set_of("dull", photo(1.0, 0.3));
        let none = BaseImage::new(&BaseImageParams { remove_sun: true, ..base("dull") }, &dull, None).unwrap();
        assert!(none.sun().is_none());
        assert!(close(none.radiance(sun_dir).y, 1.0, 1.0e-4));
    }

    #[test]
    fn match_sun_points_the_manual_sun_at_the_images_sun() {
        let images = set_of("photo", photo_with_sun(48, 8));
        let (azimuth, elevation) = az_el_from_dir(texel_dir(48, 8));
        let mut params = HdriParams::default();
        assert!(!match_sun(&mut params, &images), "no base image, nothing to match");
        params.base_image = Some(BaseImageParams { yaw_deg: 40.0, ..base("photo") });
        assert_eq!(params.sky.sun.mode, "time");
        assert!(match_sun(&mut params, &images));
        assert_eq!(params.sky.sun.mode, "manual");
        // The yaw turns the image's sun with the image: azimuth a shows at a - 40.
        assert!(
            close(params.sky.sun.azimuth_deg, azimuth - 40.0, 1.5),
            "{} vs {}",
            params.sky.sun.azimuth_deg,
            azimuth - 40.0
        );
        assert!(close(params.sky.sun.elevation_deg, elevation, 1.5));
        assert_eq!(image_sun_az_el(params.base_image.as_ref().unwrap(), &images).map(|(az, _)| az), Some(params.sky.sun.azimuth_deg));
        assert!(params.clamp().is_empty(), "the matched sun is inside the documented ranges");
        // An image without a sun, or one that is not loaded, matches nothing and changes nothing.
        let mut dull = HdriParams::default();
        dull.base_image = Some(base("dull"));
        assert!(!match_sun(&mut dull, &set_of("dull", photo(1.0, 0.3))));
        assert!(!match_sun(&mut dull, &ImageSet::default()));
        assert_eq!(dull.sky.sun, SunParams::default());
    }

    #[test]
    fn facts_are_worked_out_once_per_image() {
        let map = Arc::new(photo_with_sun(20, 6));
        let first = image_facts(&map);
        assert!(Arc::ptr_eq(&first, &image_facts(&map)), "the second call reads the memo");
        // An equal image in another allocation is another image.
        let twin = Arc::new(photo_with_sun(20, 6));
        assert!(!Arc::ptr_eq(&first, &image_facts(&twin)));
        assert_eq!(image_facts(&twin).sun, first.sun);
    }

    /// D4: an image's facts (its 8 MB patch at most) go with the image when
    /// the host asks, not at the next image's first bake.
    #[test]
    fn a_freed_images_facts_go_when_asked() {
        let map = Arc::new(photo_with_sun(20, 8));
        let facts = Arc::downgrade(&image_facts(&map));
        assert!(facts.upgrade().is_some(), "kept while the image lives");
        drop_dead_facts();
        assert!(facts.upgrade().is_some(), "a live image keeps its facts");
        drop(map);
        drop_dead_facts();
        assert!(facts.upgrade().is_none(), "freed with the image");
    }
}

#[cfg(test)]
mod env_tests {
    use super::*;

    /// A photograph stand-in: a bluish sky of 4, a grey ground of 0.5.
    fn photo() -> EnvMap {
        EnvMap::from_fn(64, |d| if d[1] > 0.0 { [4.0, 4.0, 4.8] } else { [0.5, 0.5, 0.5] })
    }

    fn images() -> ImageSet {
        let mut images = ImageSet::default();
        images.images.insert("photo".to_string(), Arc::new(photo()));
        images
    }

    fn sky_params() -> HdriParams {
        let mut p = HdriParams::default();
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.elevation_deg = 50.0;
        p.sky.sun.azimuth_deg = 200.0;
        p
    }

    fn close3(a: Vec3f, b: Vec3f, relative: f32) -> bool {
        (a - b).length() <= relative * b.length().max(1.0e-3)
    }

    #[test]
    fn an_image_is_the_whole_base_under_the_lights() {
        let images = images();
        let mut p = sky_params();
        p.base_image = Some(BaseImageParams { key: "photo".to_string(), ..BaseImageParams::default() });
        let env = Env::new_with(&p, &images);
        // (200, 50) is where the generated sun would stand: the image covers it.
        for &(az, el) in &[(10.0f32, 60.0f32), (200.0, 50.0), (120.0, -30.0)] {
            let d = dir_from_az_el(az, el);
            let want = if el > 0.0 { vec3f(4.0, 4.0, 4.8) } else { vec3f(0.5, 0.5, 0.5) };
            assert!(close3(env.radiance(d), want, 1.0e-3), "({az}, {el}): {:?}", env.radiance(d));
        }
        // The lights are an overlay on it, as on any base.
        let mut light = LightParams::default();
        light.azimuth_deg = 120.0;
        light.elevation_deg = -30.0;
        p.lights = vec![light.clone()];
        let lit = Env::new_with(&p, &images).radiance(dir_from_az_el(120.0, -30.0));
        let added = lit - vec3f(0.5, 0.5, 0.5);
        assert!(close3(added, super::super::studio::light_color(&light), 1.0e-3), "{added:?}");
        // The map's intensity and rotation carry the image with them.
        p.lights.clear();
        p.intensity_ev = 1.0;
        p.rotation_deg = 90.0;
        let turned = Env::new_with(&p, &images);
        assert!(close3(turned.radiance(dir_from_az_el(10.0, 60.0)), vec3f(8.0, 8.0, 9.6), 1.0e-3));
        // No such image (Env::new carries none): the mode's own base shows, as if base_image were not set.
        let mut plain = p.clone();
        plain.base_image = None;
        let d = dir_from_az_el(10.0, 60.0);
        assert_eq!(Env::new(&p).radiance(d), Env::new(&plain).radiance(d));
    }

    #[test]
    fn sky_replacement_keeps_the_ground_and_matches_it_to_the_new_sky() {
        let images = images();
        let plain = sky_params();
        let sky = Env::new(&plain);
        let mut p = plain.clone();
        p.base_image = Some(BaseImageParams {
            key: "photo".to_string(),
            replace_sky: true,
            feather_deg: 4.0,
            ..BaseImageParams::default()
        });
        let env = Env::new_with(&p, &images);
        // Above the feather: the generated sky, sun disc and all, bit for bit.
        for &(az, el) in &[(10.0f32, 60.0f32), (200.0, 50.0), (300.0, 8.0)] {
            let d = dir_from_az_el(az, el);
            assert_eq!(env.radiance(d), sky.radiance(d), "({az}, {el})");
        }
        // Below it: the photograph's ground, scaled by new sky over old sky as
        // a level surface meters them.
        let new_sky = irradiance_above(&sky.bake(SKY_METER_WIDTH), 0.0);
        let old_sky = image_facts(&images.images["photo"]).irradiance();
        let gain = new_sky / old_sky;
        assert!(gain.is_finite() && gain > 0.0, "{gain}");
        let ground = env.radiance(dir_from_az_el(120.0, -30.0));
        assert!(close3(ground, vec3f(0.5, 0.5, 0.5) * gain, 0.02), "{ground:?} vs gain {gain}");
        // The generated sun stands above the mask and is still the key, and the
        // daylight report (plan 2's N1).
        assert_eq!(env.sun(), sky.sun());
        assert_eq!(env.sun_dir(), sky.sun_dir());
        // The photograph has no sun of its own: nothing to match.
        let mut matched = p.clone();
        assert!(!match_sun(&mut matched, &images));
        assert_eq!(matched, p);
    }

    #[test]
    fn a_whole_base_image_brings_its_own_sun_or_none() {
        let mut hot = photo();
        hot.data[8 * 64 + 48] = [2.0e4, 2.0e4, 2.0e4, 1.0];
        let mut images = images();
        images.images.insert("sunny".to_string(), Arc::new(hot));
        let mut p = sky_params();
        p.base_image = Some(BaseImageParams { key: "sunny".to_string(), remove_sun: true, ..BaseImageParams::default() });
        p.intensity_ev = 1.0;
        p.rotation_deg = 30.0;
        let env = Env::new_with(&p, &images);
        let found = image_facts(&images.images["sunny"]).sun.expect("the hot texel");
        let key = env.sun().expect("the image's sun is the key");
        // World space: the map's rotation and intensity applied, as for any key.
        assert!(key.dir.dot(rotate_y(found.dir, 30.0)) > 0.99999);
        assert!((key.radiance.y / found.radiance.y - 2.0).abs() < 1.0e-3);
        assert!(key.validate().is_ok(), "{key:?}");
        // A photograph's sun is a file's sun (plan 2): the map reports no sky's
        // sun for the daylight switches (N1), and it has no keyless bake (N3),
        // so a host's renderer fills the sun's covering cone instead.
        assert!(env.sun_dir().is_none(), "the generated sky is not shown");
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f);
        let generated = crate::hdri::envmap::bake_generated_env_map_with(&p, &images, 64, serial);
        assert!(generated.sun.is_some() && generated.keyless.is_none() && generated.daylight.is_none(), "{:?} {:?}", generated.sun, generated.daylight);
        // Cut from the map: the brightest thing left is the photograph's sky (4.8 x 2).
        let map = env.bake(64);
        assert!(map.data.iter().all(|t| t[0] < 20.0), "the disc is gone from the map");
        // An image without a sun leaves no key: the generated sun is not drawn under it.
        p.base_image = Some(BaseImageParams { key: "photo".to_string(), ..BaseImageParams::default() });
        assert!(Env::new_with(&p, &images).sun().is_none());
    }

    /// D6 (M1): under an image that is the whole base the generated sky is
    /// not drawn, so its weather and its cloud volume are not built (nor the
    /// dome measured for them); `whole_base` says so, for the panel too. Under
    /// sky replacement both are built, and so they are when the image is not
    /// loaded (the mode's own base shows then).
    #[test]
    fn a_whole_base_image_builds_no_weather_and_no_volume() {
        let images = images();
        let mut p = sky_params();
        p.sky.clouds.coverage = 0.5;
        p.sky.volumetric = Some(VolumetricParams { steps: 16, ..VolumetricParams::default() });
        p.sky.weather = Some(WeatherParams { kind: "storm".to_string(), ..WeatherParams::default() });
        p.base_image = Some(BaseImageParams { key: "photo".to_string(), ..BaseImageParams::default() });
        assert!(whole_base(&p, &images));
        let env = Env::new_with(&p, &images);
        assert!(env.weather.is_none() && env.volumetric.is_none(), "neither is built under the photograph");
        assert_eq!(env.dome, Vec3f::default(), "and no dome is measured for them");
        // Not loaded here: the generated sky shows, with its weather and its volume.
        assert!(!whole_base(&p, &ImageSet::default()));
        let unloaded = Env::new(&p);
        assert!(unloaded.weather.is_some() && unloaded.volumetric.is_some());
        // Sky replacement draws the generated sky above the line: both are built.
        p.base_image.as_mut().unwrap().replace_sky = true;
        assert!(!whole_base(&p, &images));
        let replaced = Env::new_with(&p, &images);
        assert!(replaced.weather.is_some() && replaced.volumetric.is_some());
        // No key, or one naming nothing in the set, is no whole base.
        p.base_image = Some(BaseImageParams::default());
        assert!(!whole_base(&p, &images));
        p.base_image = None;
        assert!(!whole_base(&p, &images));
    }

    /// D6 (L:252): under sky replacement the weather's rain haze and fog lie
    /// over the photograph too: what shows below the line is the photograph
    /// through the veil, so on the horizon, where fog is opaque, the photo's
    /// ground meets the fogged sky without a seam. It was drawn crisp under
    /// an opaque fog horizon.
    #[test]
    fn the_fog_and_the_rain_haze_lie_over_the_photograph() {
        let images = images();
        for kind in ["fog", "rain"] {
            let mut p = sky_params();
            p.sky.weather = Some(WeatherParams { kind: kind.to_string(), ..WeatherParams::default() });
            p.base_image = Some(BaseImageParams { key: "photo".to_string(), replace_sky: true, feather_deg: 0.0, ..BaseImageParams::default() });
            let env = Env::new_with(&p, &images);
            let image = env.base_image.as_ref().expect("the photograph is loaded");
            let weather = env.weather.as_ref().expect("the weather is drawn");
            // Below the line: the photograph through the veil, exactly.
            for &(az, el) in &[(30.0f32, -2.0f32), (150.0, -20.0), (260.0, -60.0)] {
                let d = dir_from_az_el(az, el);
                let want = weather.veil(d, image.radiance(d), env.weather_sun, env.dome);
                assert!(close3(env.radiance(d), want, 1.0e-5), "{kind} ({az}, {el}): {:?} vs {want:?}", env.radiance(d));
            }
            // At the line the two sides are the veil's own light.
            let ring = |el: f32| (0..36).map(|i| luminance(env.radiance(dir_from_az_el(i as f32 * 10.0 + 3.0, el)))).sum::<f32>() / 36.0;
            let (above, below) = (ring(0.2), ring(-0.2));
            assert!((below / above - 1.0).abs() < 0.1, "{kind}: {above} above the line, {below} below it");
        }
    }

    /// D6 (L:220): an image's own sun is area-averaged as the generated one
    /// is, so the map holds its energy whatever the bake's texels: a 4 degree
    /// sun of a 512 wide photograph, sampled at the texel centres alone of a
    /// 48 to 64 wide map (5.6 to 7.5 degree texels), held 2.71 times its
    /// energy at 48 wide. Under the whole base the generated sun and moon,
    /// which are not drawn, are no hot spots.
    #[test]
    fn the_photographed_sun_is_area_averaged() {
        let sun = dir_from_az_el(100.0, 35.0);
        let lit = |d: [f32; 3]| vec(d).dot(sun) > 2.0f32.to_radians().cos();
        let sky = |d: [f32; 3]| if d[1] > 0.0 { [0.6, 0.7, 0.9] } else { [0.12, 0.1, 0.08] };
        let mut images = ImageSet::default();
        images.images.insert("sunny".to_string(), Arc::new(EnvMap::from_fn(512, |d| if lit(d) { [5.0e3; 3] } else { sky(d) })));
        images.images.insert("dull".to_string(), Arc::new(EnvMap::from_fn(512, sky)));
        let energy = |map: &EnvMap| -> f64 {
            let (w, h) = (map.width, map.height);
            (0..w * h)
                .map(|i| {
                    let theta = ((i / w) as f64 + 0.5) / h as f64 * std::f64::consts::PI;
                    map.data[i][1] as f64 * theta.sin() * (std::f64::consts::TAU / w as f64) * (std::f64::consts::PI / h as f64)
                })
                .sum()
        };
        let own = energy(&images.images["sunny"]) - energy(&images.images["dull"]);
        let mut p = sky_params();
        for width in [48usize, 56, 64] {
            for yaw in [0.0f32, 1.3, 2.9] {
                p.base_image = Some(BaseImageParams { key: "sunny".to_string(), yaw_deg: yaw, ..BaseImageParams::default() });
                let with = energy(&Env::new_with(&p, &images).bake(width));
                p.base_image = Some(BaseImageParams { key: "dull".to_string(), yaw_deg: yaw, ..BaseImageParams::default() });
                let without = energy(&Env::new_with(&p, &images).bake(width));
                let ratio = (with - without) / own;
                assert!((ratio - 1.0).abs() < 0.1, "{width} wide, yaw {yaw}: the map holds {ratio:.3} of the sun's energy");
            }
        }
        p.base_image = Some(BaseImageParams { key: "sunny".to_string(), ..BaseImageParams::default() });
        let hot = Env::new_with(&p, &images).hot_spots();
        assert_eq!(hot.len(), 1, "the photograph's sun alone: {hot:?}");
        assert!(hot[0].0.dot(sun) > 0.9999, "{hot:?}");
        // Cut from the map, it is not drawn: nothing to refine.
        p.base_image.as_mut().unwrap().remove_sun = true;
        assert!(Env::new_with(&p, &images).hot_spots().is_empty());
    }
}
