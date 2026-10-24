//! The Milky Way (plan hdri-7 §2, Task 3b), written from general techniques: the band of unresolved stars as a disk
//! whose brightness falls off as a Lorentzian in the sine of the galactic latitude (broad wings) and grows toward the
//! galactic centre, a boxy bulge round the centre a little south of the plane, star clouds and a grain of unresolved
//! stars from band-limited 3D noise, and dust as the clumps of a domain-warped fbm (dark clouds of every size it holds)
//! with ridged filaments feathering their edges, deepest along a main lane and in ragged clouds above and below the
//! plane near the centre, all in galactic coordinates (no
//! seam, no pole pinch); and the two Magellanic Clouds and Andromeda as small patches south of the plane. All of it
//! lives in star space, so it turns with the stars.
//!
//! **Scale.** The constants are surface brightnesses (mag/arcsec²) on the stars' own anchor
//! (`night::mag_arcsec2_to_radiance`): at Strength 1, as the eye sees it (Realism 1), the core region is about 4.4x the
//! default glow's zenith (G0), mid longitudes about 2, the anticentre about 1.1, the poles near 0 (the glow already
//! stands for the faint starlight there). The dust's transmittance has a floor of a quarter (the Great Rift is 1 to
//! 1.5 mag deep: stars in front of it fill it in).
//!
//! **Realism** (the nebulas' setting, so one control sets the whole deep sky's look): at 1 the eye's band, near grey;
//! at 0 the band as a long exposure photographs it, MW_PHOTO_GAIN times brighter (geometric between), its core and
//! bright star clouds cream to yellow-orange with a little pink, its arms bluish white, and what the dust lets through
//! reddened.

use makepad_draw::*;
use super::night::mag_arcsec2_to_radiance;
use super::noise::{fbm3_frac_full, hash01, hash_u32, octaves_for, perlin3};
use super::studio::kelvin_to_rgb;
use super::MilkyWayParams;
use crate::sky::luminance;
use std::sync::OnceLock;

/// The disk's ridge toward the anticentre (about 1.1 G0) and toward the centre (about 4.7 G0), mag/arcsec².
const MW_ANTICENTRE_MU: f32 = 21.79;
const MW_INNER_MU: f32 = 20.21;
/// The bulge's peak at Core 1 (about 8.7 G0, under the dust that crosses it), mag/arcsec².
const MW_BULGE_MU: f32 = 19.55;
/// The brightest the band draws before its dust (the brightest star clouds, about 8.9 G0), x Core when Core is over 1:
/// the band runs straight up to MW_KNEE of it and bends smoothly toward it above, so no star cloud passes it.
const MW_PEAK_MU: f32 = 19.52;
const MW_KNEE: f32 = 0.6;
/// The disk's half width at half maximum, in sin b (6 degrees: a FWHM of 12).
const MW_DISK_SIN: f32 = 0.105;
/// The disk's ridge runs from the anticentre's level to the centre's as ((1 + cos l) / 2)^MW_DISK_POWER.
const MW_DISK_POWER: f32 = 2.0;
/// The bulge: a superellipse of power MW_BULGE_BOX (boxy, a peanut once the dust lane cuts it), sigmas of 12 degrees
/// along the plane and 7 across it (radians), its middle 1.5 degrees south of the plane.
const MW_BULGE_SIGMA_L: f32 = 0.21;
const MW_BULGE_SIGMA_B: f32 = 0.122;
const MW_BULGE_B: f32 = -0.026;
const MW_BULGE_BOX: f32 = 2.6;
/// The star clouds: noise of about one 9.5-degree cell per 1/6 rad, three octaves (to 2.4 degrees), 1 + 1.1 (n - 0.5).
const MW_CLOUD_SCALE: f32 = 6.0;
const MW_CLOUD_FULL: f32 = 3.0;
const MW_CLOUD_DEPTH: f32 = 1.1;
/// The grain of unresolved stars as a long exposure shows it (the eye sees the band smooth): 1 + 1.2 (n - 0.5) of an
/// fbm of 1.2 degrees and finer (three octaves), its mean 1 at every width; its first octave shows from four texels a
/// wavelength (none at a held 512 bake).
const MW_GRAIN_WAVE: f32 = 0.021;
const MW_GRAIN_FULL: f32 = 3.0;
const MW_GRAIN_DEPTH: f32 = 1.2;
/// The clouds and the grain fade out between these sines of the latitude (22 and 30 degrees).
const MW_CLOUD_SIN_B: (f32, f32) = (0.375, 0.5);
/// The dust is drawn within 25 degrees of the plane (sin 25), fading out from 20.
const MW_DUST_SIN_B: f32 = 0.4226;
const MW_DUST_FADE_SIN_B: f32 = 0.342;
/// The domain warp that bends the lanes: one 15-degree octave each, moving the latitude by up to +-3.4 degrees and the
/// longitude by up to +-5.7.
const MW_WARP_WAVE: f32 = 0.26;
const MW_WARP_B: f32 = 0.06;
const MW_WARP_L: f32 = 0.1;
/// The dust's features are MW_DUST_ACROSS times shorter across the plane than along it.
const MW_DUST_ACROSS: f32 = 1.6;
/// The dark clouds' mass: fbm of 6-degree cells, six octaves (to 0.2 degrees), dark where it is high
/// (smoothstep(MW_CLUMP.0, MW_CLUMP.1, n)).
const MW_MASS_WAVE: f32 = 0.105;
const MW_MASS_FULL: f32 = 6.0;
const MW_CLUMP: (f32, f32) = (0.46, 0.7);
/// The filaments at the clouds' edges: ridged noise of 8 degrees, four octaves (to 1 degree along, 0.6 across), each
/// octave's ridge (1 - |perlin|)^MW_RIDGE_POW, octave weights falling by MW_RIDGE_GAIN.
const MW_RIDGE_WAVE: f32 = 0.14;
const MW_RIDGE_FULL: usize = 4;
const MW_RIDGE_POW: i32 = 8;
const MW_RIDGE_GAIN: f32 = 0.65;
/// The main lane (the Great Rift, and the thin lane along the whole plane): b +1 degree, about 3 wide, deepest over
/// the inner longitudes; its optical depth at Dust 1 runs with the clouds' mass.
const MW_RIFT_B: f32 = 0.0175;
const MW_RIFT_W: f32 = 0.05;
const MW_RIFT_TAU: f32 = 1.4;
/// The ragged dark clouds above and below the plane: within about 5 degrees of it round the band, 15 near the centre;
/// their optical depth, and their filaments'.
const MW_DARK_W: f32 = 0.09;
const MW_DARK_W_CORE: f32 = 0.26;
const MW_DARK_TAU: f32 = 1.6;
const MW_FILAMENT_TAU: f32 = 1.6;
const MW_DUST_FLOOR: f32 = 0.25;
/// A long exposure's band over the eye's, at Realism 0.
const MW_PHOTO_GAIN: f32 = 14.0;
/// How much more the dust takes of blue than of green, and of green than of red, at Realism 0 (T^(1 -+ r)).
const MW_REDDEN: f32 = 0.2;
/// Salts for the filaments', the clouds', the grain's, the warp's, the patches' and the dust's mass's noise, from the
/// map's seed.
const SALT_DUST: u32 = 0x4d57_4455;
const SALT_CLOUDS: u32 = 0x4d57_434c;
const SALT_GRAIN: u32 = 0x4d57_4752;
const SALT_WARP: u32 = 0x4d57_5752;
const SALT_PATCH: u32 = 0x4d57_5041;
const SALT_MASS: u32 = 0x4d57_4d41;

/// The small patches south of the plane: (l, b, sigma along, sigma across (degrees), position angle (degrees from the
/// longitude's direction), peak mag/arcsec², colour temperature (K; 0 for an H II region's pink)). The Large Magellanic
/// Cloud's disc and bar and the Tarantula in it, the Small Magellanic Cloud, Andromeda.
const PATCHES: [(f32, f32, f32, f32, f32, f32, f32); 5] = [
    (280.5, -32.9, 3.0, 2.4, 30.0, 22.2, 7500.0),
    (280.3, -33.3, 1.7, 0.55, 15.0, 21.7, 7000.0),
    (279.5, -31.7, 0.3, 0.25, 0.0, 21.2, 0.0),
    (302.8, -44.3, 1.4, 0.8, 45.0, 22.2, 7500.0),
    (121.2, -21.6, 0.9, 0.25, 35.0, 21.2, 5500.0),
];
/// The patches lie south of this sine of the latitude (-14.5 degrees); their noise is 1.5 degrees, three octaves.
const PATCH_SIN_B: f32 = -0.25;
const PATCH_SCALE: f32 = 38.0;
/// An H II region's pink, as a long exposure shows it.
const PINK: Vec3f = vec3f(1.0, 0.35, 0.5);

/// A patch, placed in star space.
#[derive(Clone, Debug)]
struct Patch {
    centre: Vec3f,
    e1: Vec3f,
    e2: Vec3f,
    inv_a: f32,
    inv_b: f32,
    cos_reach: f32,
    light: f32,
    rgb: Vec3f,
    seed: u32,
}

/// The band, built once per map from the block as clamp() holds it.
#[derive(Clone, Debug)]
pub(super) struct MilkyWay {
    /// The galactic axes in star space: the centre (l 0), the pole, l 90.
    centre: Vec3f,
    pole: Vec3f,
    l90: Vec3f,
    /// Strength x the night's lift x the astronomical-twilight fade x the photograph's gain: 0 draws no light (the dust
    /// still dims the stars).
    gain: f32,
    anticentre: f32,
    inner: f32,
    bulge: f32,
    peak: f32,
    dust: f32,
    /// The band's colours toward the arms and in the core and the bright clouds (luminance 1), Realism blended in.
    cool_rgb: Vec3f,
    /// How much of the photograph's grain shows (1 at Realism 0, 0 at 1).
    grain: f32,
    warm_rgb: Vec3f,
    /// The dust's reddening exponent (0 at Realism 1).
    redden: f32,
    dust_seed: u32,
    cloud_seed: u32,
    grain_seed: u32,
    warp_seed: u32,
    mass_seed: u32,
    patches: Vec<Patch>,
}

/// White plus `saturation` of a blackbody's colour, at luminance 1.
fn tint(kelvin: f32, saturation: f32) -> Vec3f {
    let white = Vec3f::all(1.0);
    let c = white + (kelvin_to_rgb(kelvin) - white) * saturation;
    c / luminance(c)
}

/// The mean of a ridge (1 - |perlin3|)^MW_RIDGE_POW over space: what an octave a lookup cannot resolve counts as.
/// Measured once over 16384 hashed points (the same bits every run).
fn ridge_mean() -> f32 {
    static MEAN: OnceLock<f32> = OnceLock::new();
    *MEAN.get_or_init(|| {
        let n = 16384;
        let mut sum = 0.0f64;
        for i in 0..n {
            let p = |k: i32| 97.0 * hash01(i, k, 0, 0x5249_4447);
            sum += (1.0 - perlin3(p(0), p(1), p(2), 0x5249_4447).abs()).powi(MW_RIDGE_POW) as f64;
        }
        (sum / n as f64) as f32
    })
}

/// Ridged noise at (x, y, z), `full` octaves of (1 - |perlin3|)^MW_RIDGE_POW at lacunarity 2, weights falling by
/// MW_RIDGE_GAIN, normalised to [0, 1]: each octave keeps its weight in the full sum, and one a lookup of `octaves`
/// (fractional, `octaves_for`) leaves out counts as its mean, the last fading in from it by its fraction, so a coarse
/// lookup's mean is a fine one's.
fn ridged(x: f32, y: f32, z: f32, octaves: f32, full: usize, seed: u32) -> f32 {
    let mean = ridge_mean();
    let (mut sum, mut total, mut amplitude, mut f) = (0.0f32, 0.0f32, 1.0f32, 1.0f32);
    for i in 0..full {
        let w = (octaves - i as f32).clamp(0.0, 1.0);
        let r = if w > 0.0 {
            let o = 17.13 * i as f32;
            mean + w * ((1.0 - perlin3(x * f + o, y * f + 0.37 * o, z * f - 0.61 * o, seed.wrapping_add((i as u32).wrapping_mul(0x9e37_79b9))).abs()).powi(MW_RIDGE_POW) - mean)
        } else {
            mean
        };
        sum += amplitude * r;
        total += amplitude;
        amplitude *= MW_RIDGE_GAIN;
        f *= 2.0;
    }
    sum / total
}

impl MilkyWay {
    /// `axes`: the galactic centre, pole and l = 90 in star space; `seed`: the map's; `gain`: strength x lift x
    /// ASTRO_FADE (0 skips the light); `realism`: the nebulas' Realism (0 photographed .. 1 the eye's).
    pub(super) fn new(p: &MilkyWayParams, seed: u32, axes: [Vec3f; 3], gain: f32, realism: f32) -> MilkyWay {
        let r = if realism.is_nan() { 0.0 } else { realism.clamp(0.0, 1.0) };
        // the art colours' share given to the eye's, as the nebulas blend theirs
        let chroma = 1.0 - (1.0 - r) * (1.0 - r);
        let blend = |art: Vec3f, eye: Vec3f| {
            let c = art + (eye - art) * chroma;
            c / luminance(c)
        };
        let warm_art = {
            let c = tint(4100.0, 0.55) + (PINK / luminance(PINK) - tint(4100.0, 0.55)) * 0.08;
            c / luminance(c)
        };
        let [centre, pole, l90] = axes;
        let patches = PATCHES
            .iter()
            .enumerate()
            .map(|(i, &(l, b, sa, sb, angle, mu, kelvin))| {
                let (l, b, angle) = (l.to_radians(), b.to_radians(), angle.to_radians());
                let mid = (centre * (b.cos() * l.cos()) + l90 * (b.cos() * l.sin()) + pole * b.sin()).normalize();
                let along = (l90 * l.cos() - centre * l.sin()).normalize();
                let across = Vec3f::cross(mid, along).normalize();
                let (e1, e2) = (along * angle.cos() + across * angle.sin(), across * angle.cos() - along * angle.sin());
                let art = if kelvin > 0.0 { tint(kelvin, 0.6) } else { PINK / luminance(PINK) };
                Patch {
                    centre: mid,
                    e1,
                    e2,
                    inv_a: 1.0 / sa.to_radians(),
                    inv_b: 1.0 / sb.to_radians(),
                    cos_reach: (3.0 * sa.max(sb)).to_radians().cos(),
                    light: mag_arcsec2_to_radiance(mu),
                    rgb: blend(art, tint(6000.0, 0.15)),
                    seed: hash_u32(seed ^ SALT_PATCH ^ i as u32),
                }
            })
            .collect();
        MilkyWay {
            centre,
            pole,
            l90,
            gain: gain * MW_PHOTO_GAIN.powf(1.0 - r),
            anticentre: mag_arcsec2_to_radiance(MW_ANTICENTRE_MU),
            inner: mag_arcsec2_to_radiance(MW_INNER_MU),
            bulge: mag_arcsec2_to_radiance(MW_BULGE_MU) * p.core,
            peak: mag_arcsec2_to_radiance(MW_PEAK_MU) * p.core.max(1.0),
            dust: p.dust,
            cool_rgb: blend(tint(12000.0, 0.7), tint(6500.0, 0.15)),
            warm_rgb: blend(warm_art, tint(4800.0, 0.15)),
            redden: MW_REDDEN * (1.0 - chroma),
            grain: 1.0 - chroma,
            dust_seed: hash_u32(seed ^ SALT_DUST),
            cloud_seed: hash_u32(seed ^ SALT_CLOUDS),
            grain_seed: hash_u32(seed ^ SALT_GRAIN),
            warp_seed: hash_u32(seed ^ SALT_WARP),
            mass_seed: hash_u32(seed ^ SALT_MASS),
            patches,
        }
    }

    /// At star-space `s`: the band's radiance (gain included, before extinction and the horizon fade) and the dust's
    /// transmittance for what lies behind (1 off the band). `texel` (radians) band-limits the noise, on the scale of its
    /// finest lookup (`fbm3_frac_full`, `ridged`), so a coarse bake's star clouds and dust lanes are as deep as a fine
    /// one's.
    pub(super) fn at(&self, s: Vec3f, texel: f32) -> (Vec3f, f32) {
        let (x, y, z, cos_l) = self.galactic(s);
        let t = self.dust_at(x, y, z, cos_l, texel);
        if !(self.gain > 0.0) {
            return (Vec3f::default(), t);
        }
        let along = (0.5 * (1.0 + cos_l)).powf(MW_DISK_POWER);
        let q = y / MW_DISK_SIN;
        let disk = (self.anticentre + (self.inner - self.anticentre) * along) / (1.0 + q * q);
        let bulge = self.bulge_at(x, y);
        // star clouds, linear in their noise (a coarse lookup keeps a fine one's mean), none off the band
        let fade = 1.0 - smoothstep(MW_CLOUD_SIN_B.0, MW_CLOUD_SIN_B.1, y.abs());
        let mut clouds = 1.0f32;
        if fade > 0.0 {
            let k = MW_CLOUD_SCALE;
            let n = fbm3_frac_full(x * k, y * k, z * k, octaves_for(1.0 / k, texel, MW_CLOUD_FULL), MW_CLOUD_FULL, self.cloud_seed);
            clouds = (1.0 + fade * MW_CLOUD_DEPTH * (n - 0.5)).max(0.1);
        }
        let mut band = self.knee((disk + bulge) * clouds);
        // the photograph's grain over the knee, linear in its noise, where the band shows; it shows from four texels a
        // wavelength, fully from eight
        let shown = if texel > 0.0 { (MW_GRAIN_WAVE / (4.0 * texel) - 1.0).clamp(0.0, 1.0) } else { 1.0 };
        if self.grain > 0.0 && fade > 0.0 && shown > 0.0 {
            let k = 1.0 / MW_GRAIN_WAVE;
            let g = fbm3_frac_full(x * k, y * k, z * k, octaves_for(MW_GRAIN_WAVE, texel, MW_GRAIN_FULL), MW_GRAIN_FULL, self.grain_seed);
            band *= 1.0 + self.grain * fade * shown * MW_GRAIN_DEPTH * (g - 0.5);
        }
        // warm toward the centre and in the bright clouds, cool toward the arms
        let warmth = (smoothstep(0.5, 0.99, x) + 0.6 * (clouds - 1.0)).clamp(0.0, 1.0);
        let mut light = (self.cool_rgb + (self.warm_rgb - self.cool_rgb) * warmth) * band;
        if y < PATCH_SIN_B {
            light += self.patches_at(s, texel);
        }
        // the dust takes more blue than red
        let reddened = if self.redden > 0.0 && t < 1.0 { vec3f(t.powf(1.0 - self.redden), t, t.powf(1.0 + 1.5 * self.redden)) } else { Vec3f::all(t) };
        (light * reddened * self.gain, t)
    }

    /// The dust's transmittance alone at star-space `s`, as `at` gives it.
    pub(super) fn transmittance(&self, s: Vec3f, texel: f32) -> f32 {
        let (x, y, z, cos_l) = self.galactic(s);
        self.dust_at(x, y, z, cos_l, texel)
    }

    /// Star-space `s` in galactic coordinates: x toward the centre, y = sin b, z toward l 90, and cos l (0 at the poles).
    fn galactic(&self, s: Vec3f) -> (f32, f32, f32, f32) {
        let (x, y, z) = (self.centre.dot(s), self.pole.dot(s), self.l90.dot(s));
        let flat = x * x + z * z;
        let cos_l = if flat > 1.0e-8 { x / flat.sqrt() } else { 0.0 };
        (x, y, z, cos_l)
    }

    /// The bulge at galactic (x, y = sin b): boxy, centred MW_BULGE_B off the plane; none more than 50 degrees out.
    fn bulge_at(&self, x: f32, y: f32) -> f32 {
        if !(x > 0.64) || !(self.bulge > 0.0) {
            return 0.0;
        }
        // 2 (1 - x) is the squared angle from the centre for small angles; less the latitude's, the longitude's
        let u = (2.0 * (1.0 - x) - y * y).max(0.0).sqrt() / MW_BULGE_SIGMA_L;
        let v = ((y - MW_BULGE_B) / MW_BULGE_SIGMA_B).abs();
        let r2 = (u.powf(MW_BULGE_BOX) + v.powf(MW_BULGE_BOX)).powf(2.0 / MW_BULGE_BOX);
        self.bulge * (-0.5 * r2).exp()
    }

    /// The band's soft knee: straight up to MW_KNEE of the peak, then bending toward the peak (continuous in value and
    /// slope), never reaching it.
    fn knee(&self, v: f32) -> f32 {
        let k = MW_KNEE * self.peak;
        if !(v > k) {
            return v;
        }
        let room = self.peak - k;
        k + room * (1.0 - (-(v - k) / room).exp())
    }

    /// The patches' light at star-space `s` (their colours in), before the gain.
    fn patches_at(&self, s: Vec3f, texel: f32) -> Vec3f {
        let mut c = Vec3f::default();
        for p in &self.patches {
            if !(s.dot(p.centre) >= p.cos_reach) {
                continue;
            }
            let (u, v) = (s.dot(p.e1) * p.inv_a, s.dot(p.e2) * p.inv_b);
            let k = PATCH_SCALE;
            let n = fbm3_frac_full(s.x * k, s.y * k, s.z * k, octaves_for(1.0 / k, texel, 3.0), 3.0, p.seed);
            c += p.rgb * (p.light * (-0.5 * (u * u + v * v)).exp() * (1.0 + 1.4 * (n - 0.5)).max(0.0));
        }
        c
    }

    /// The dust's transmittance at galactic (x, y = sin b, z): exp(-Dust x tau) over its floor, none past 25 degrees
    /// from the plane, fading in from 20.
    fn dust_at(&self, x: f32, y: f32, z: f32, cos_l: f32, texel: f32) -> f32 {
        let ay = y.abs();
        if !(self.dust > 0.0) || !(ay < MW_DUST_SIN_B) {
            return 1.0;
        }
        let tau = self.dust_tau(x, y, z, cos_l, texel) * (1.0 - smoothstep(MW_DUST_FADE_SIN_B, MW_DUST_SIN_B, ay));
        (-self.dust * tau).exp().max(MW_DUST_FLOOR)
    }

    /// The dust's optical depth at Dust 1: dark clouds where a domain-warped fbm's mass is high (clumps of every size
    /// it holds), deepest along the main lane (b about +1 degree, over the inner longitudes) and spread a few degrees
    /// either side of the plane (15 near the centre), with ridged filaments feathering their edges.
    fn dust_tau(&self, x: f32, y: f32, z: f32, cos_l: f32, texel: f32) -> f32 {
        let kw = 1.0 / MW_WARP_WAVE;
        let w1 = fbm3_frac_full(x * kw, y * kw, z * kw, 1.0, 1.0, self.warp_seed) - 0.5;
        let w2 = fbm3_frac_full(x * kw + 7.31, y * kw + 2.17, z * kw, 1.0, 1.0, self.warp_seed ^ 1) - 0.5;
        let yw = y + 2.0 * MW_WARP_B * w1;
        let a = 2.0 * MW_WARP_L * w2;
        let (xw, zw) = (x - a * z, z + a * x);
        let k = 1.0 / MW_MASS_WAVE;
        let mass = fbm3_frac_full(xw * k, yw * (k * MW_DUST_ACROSS), zw * k, octaves_for(MW_MASS_WAVE / MW_DUST_ACROSS, texel, MW_MASS_FULL), MW_MASS_FULL, self.mass_seed);
        let clumps = smoothstep(MW_CLUMP.0, MW_CLUMP.1, mass);
        // the ridges are thinner than their wavelength: an octave shows from four texels across its wavelength
        let edge = smoothstep(MW_CLUMP.0 - 0.14, MW_CLUMP.0, mass) * (1.0 - clumps);
        let filaments = if edge > 0.0 {
            let k = 1.0 / MW_RIDGE_WAVE;
            let octaves = octaves_for(0.5 * MW_RIDGE_WAVE / MW_DUST_ACROSS, texel, MW_RIDGE_FULL as f32);
            edge * smoothstep(0.3, 0.6, ridged(xw * k, yw * (k * MW_DUST_ACROSS), zw * k, octaves, MW_RIDGE_FULL, self.dust_seed))
        } else {
            0.0
        };
        let rb = (yw - MW_RIFT_B) / MW_RIFT_W;
        let lane = (-rb * rb).exp() * (0.3 + 0.7 * smoothstep(0.0, 0.6, cos_l));
        let wide = MW_DARK_W + (MW_DARK_W_CORE - MW_DARK_W) * smoothstep(0.6, 0.95, x);
        let fb = yw / wide;
        let near = (-fb * fb).exp();
        MW_RIFT_TAU * lane * (0.3 + 1.4 * clumps) + near * (MW_DARK_TAU * clumps + MW_FILAMENT_TAU * filaments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dust never takes more than three quarters (foreground stars fill the rift in), is none off the band (|b| over
    /// 25 degrees) and none at Dust 0.
    #[test]
    fn the_dust_has_a_floor_and_stays_on_the_band() {
        let axes = [vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)];
        let deep = MilkyWay::new(&MilkyWayParams { dust: 2.0, ..MilkyWayParams::default() }, 7, axes, 1.0, 0.0);
        let none = MilkyWay::new(&MilkyWayParams { dust: 0.0, ..MilkyWayParams::default() }, 7, axes, 1.0, 0.0);
        for i in 0..2000 {
            let (l, b) = ((i as f32 * 0.618_034).fract() * std::f32::consts::TAU, ((i as f32 * 0.414_214).fract() - 0.5) * std::f32::consts::PI);
            let s = vec3f(b.cos() * l.cos(), b.sin(), b.cos() * l.sin());
            let t = deep.at(s, 0.0).1;
            assert!((0.25..=1.0).contains(&t), "{t}");
            if b.abs() > 25.5f32.to_radians() {
                assert_eq!(t, 1.0, "off the band at b {}", b.to_degrees());
            }
            assert_eq!(none.at(s, 0.0).1, 1.0);
        }
        assert_eq!(MilkyWay::new(&MilkyWayParams::default(), 7, axes, 0.0, 0.0).at(axes[0], 0.0).0, Vec3f::default(), "gain 0 draws nothing");
    }
}
