//! The Milky Way (plan hdri-7 §2), written from general techniques: the band of unresolved stars as a disk whose
//! brightness falls off as a Lorentzian in the sine of the galactic latitude (broad wings) and grows toward the
//! galactic centre, a Gaussian bulge round the centre, star clouds and dust lanes from band-limited 3D noise in
//! galactic coordinates (no seam, no pole pinch). All of it lives in star space, so it turns with the stars.
//!
//! **Scale.** The constants are surface brightnesses (mag/arcsec²) on the stars' own anchor
//! (`night::mag_arcsec2_to_radiance`): at Strength 1 the core region is about 4.4x the default glow's zenith (G0),
//! mid longitudes about 2, the anticentre about 1.1, the poles near 0 (the glow already stands for the faint
//! starlight there). The dust's transmittance has a floor of a quarter (the Great Rift is 1 to 1.5 mag deep: stars in
//! front of it fill it in).

use makepad_draw::*;
use super::night::mag_arcsec2_to_radiance;
use super::noise::{fbm3_frac, hash_u32, octaves_for};
use super::studio::kelvin_to_rgb;
use super::MilkyWayParams;
use crate::sky::luminance;

/// The disk's ridge toward the anticentre (about 1.1 G0) and toward the centre (about 4.7 G0), mag/arcsec².
const MW_ANTICENTRE_MU: f32 = 21.79;
const MW_INNER_MU: f32 = 20.21;
/// The bulge's peak at Core 1 (about 6 G0), mag/arcsec².
const MW_BULGE_MU: f32 = 19.95;
/// The brightest the band draws before its dust (the brightest star clouds, about 8.9 G0), x Core when Core is over 1.
/// It caps the band with its star clouds: a cap before them let the clouds lift the brightest patch to about 11.7 G0.
const MW_PEAK_MU: f32 = 19.52;
/// The disk's half width at half maximum, in sin b (6 degrees: a FWHM of 12).
const MW_DISK_SIN: f32 = 0.105;
/// The disk's ridge runs from the anticentre's level to the centre's as ((1 + cos l) / 2)^MW_DISK_POWER.
const MW_DISK_POWER: f32 = 2.0;
/// The bulge's Gaussian sigma (10 degrees, radians): exp(-(1 - x) / sigma²) is exp(-theta² / (2 sigma²)).
const MW_BULGE_SIGMA: f32 = 0.1745;
/// The star clouds: noise of about one 9.5-degree cell per 1/6 rad, at most two octaves, +-35 %.
const MW_CLOUD_SCALE: f32 = 6.0;
const MW_CLOUD_DEPTH: f32 = 0.35;
/// The dust is drawn within 25 degrees of the plane (sin 25); its noise is 20 degrees long along l and four times
/// shorter across b.
const MW_DUST_SIN_B: f32 = 0.4226;
const MW_DUST_WAVE: f32 = 0.349;
/// The Great Rift's optical depth (about 1.2 mag at Dust 1, along b +1 degree from l 0 to about 80) and a dark
/// patch's (about 0.9 mag, Coalsack-like, where the noise is high near the plane); the transmittance's floor.
const MW_RIFT_TAU: f32 = 1.2;
const MW_PATCH_TAU: f32 = 0.8;
const MW_DUST_FLOOR: f32 = 0.25;
/// Salts for the dust's and the clouds' noise, from the map's seed.
const SALT_DUST: u32 = 0x4d57_4455;
const SALT_CLOUDS: u32 = 0x4d57_434c;

/// The band, built once per map from the block as clamp() holds it.
#[derive(Clone, Debug)]
pub(super) struct MilkyWay {
    /// The galactic axes in star space: the centre (l 0), the pole, l 90.
    centre: Vec3f,
    pole: Vec3f,
    l90: Vec3f,
    /// Strength x the night's lift x the astronomical-twilight fade: 0 draws no light (the dust still dims the stars).
    gain: f32,
    anticentre: f32,
    inner: f32,
    bulge: f32,
    peak: f32,
    dust: f32,
    /// Integrated starlight's warm white (4800 K at half saturation), warmer toward the core (4300 K); luminance 1.
    outer_rgb: Vec3f,
    core_rgb: Vec3f,
    dust_seed: u32,
    cloud_seed: u32,
}

impl MilkyWay {
    /// `axes`: the galactic centre, pole and l = 90 in star space; `seed`: the map's; `gain`: strength x lift x
    /// ASTRO_FADE (0 skips the light).
    pub(super) fn new(p: &MilkyWayParams, seed: u32, axes: [Vec3f; 3], gain: f32) -> MilkyWay {
        let tint = |kelvin: f32| {
            let white = Vec3f::all(1.0);
            let c = white + (kelvin_to_rgb(kelvin) - white) * 0.5;
            c / luminance(c)
        };
        MilkyWay {
            centre: axes[0],
            pole: axes[1],
            l90: axes[2],
            gain,
            anticentre: mag_arcsec2_to_radiance(MW_ANTICENTRE_MU),
            inner: mag_arcsec2_to_radiance(MW_INNER_MU),
            bulge: mag_arcsec2_to_radiance(MW_BULGE_MU) * p.core,
            peak: mag_arcsec2_to_radiance(MW_PEAK_MU) * p.core.max(1.0),
            dust: p.dust,
            outer_rgb: tint(4800.0),
            core_rgb: tint(4300.0),
            dust_seed: hash_u32(seed ^ SALT_DUST),
            cloud_seed: hash_u32(seed ^ SALT_CLOUDS),
        }
    }

    /// At star-space `s`: the band's radiance (gain included, before extinction and the horizon fade) and the dust's
    /// transmittance for what lies behind (1 off the band). `texel` (radians) band-limits the noise.
    pub(super) fn at(&self, s: Vec3f, texel: f32) -> (Vec3f, f32) {
        let (x, y, z) = (self.centre.dot(s), self.pole.dot(s), self.l90.dot(s));
        let flat = x * x + z * z;
        let cos_l = if flat > 1.0e-8 { x / flat.sqrt() } else { 0.0 };
        let t = self.dust_at(x, y, z, cos_l, texel);
        if !(self.gain > 0.0) {
            return (Vec3f::default(), t);
        }
        let along = (0.5 * (1.0 + cos_l)).powf(MW_DISK_POWER);
        let q = y / MW_DISK_SIN;
        let disk = (self.anticentre + (self.inner - self.anticentre) * along) / (1.0 + q * q);
        let bulge = self.bulge * (-(1.0 - x) / (MW_BULGE_SIGMA * MW_BULGE_SIGMA)).exp();
        let k = MW_CLOUD_SCALE;
        let octaves = octaves_for(1.0 / k, texel, 2.0);
        let n = on_full_scale(fbm3_frac(x * k, y * k, z * k, octaves, self.cloud_seed), octaves, 2.0);
        let clouds = (1.0 + 2.0 * (n - 0.5)).clamp(1.0 - MW_CLOUD_DEPTH, 1.0 + MW_CLOUD_DEPTH);
        // the peak caps the band with its star clouds, so no cloud lifts the brightest patch over it
        let band = ((disk + bulge) * clouds).min(self.peak);
        let rgb = self.outer_rgb + (self.core_rgb - self.outer_rgb) * smoothstep(0.5, 1.0, x);
        (rgb * (band * t * self.gain), t)
    }

    /// The dust's transmittance at galactic (x, y = sin b, z): the Great Rift along b about +1 degree over the inner
    /// longitudes (cos l over 0.2), dark patches where its noise is high near the plane; a quarter at the least.
    fn dust_at(&self, x: f32, y: f32, z: f32, cos_l: f32, texel: f32) -> f32 {
        if !(self.dust > 0.0) || !(y.abs() < MW_DUST_SIN_B) {
            return 1.0;
        }
        let k = 1.0 / MW_DUST_WAVE;
        let octaves = octaves_for(MW_DUST_WAVE / 4.0, texel, 6.0);
        let n = on_full_scale(fbm3_frac(x * k, y * (4.0 * k), z * k, octaves, self.dust_seed), octaves, 6.0);
        let r = (y - 0.0175) / 0.044;
        let rift = smoothstep(0.2, 0.5, cos_l) * (-r * r).exp();
        let w = y / 0.09;
        let patches = smoothstep(0.55, 0.75, n) * (-w * w).exp();
        let tau = MW_RIFT_TAU * rift * (0.6 + 0.8 * n) + MW_PATCH_TAU * patches;
        (-self.dust * tau).exp().max(MW_DUST_FLOOR)
    }
}

/// `n`, an fbm of `octaves` (`fbm3_frac`, normalised by the octaves it sums), on the scale of `max` octaves: the octaves
/// a coarse texel leaves out count as their mean (one half), so the coarse octaves keep the weight they have at the
/// finest lookup. Normalised by its own octaves the dust's coarse octave weighs 0.72 at 512 (1.8 octaves) and 0.59 at
/// 1024 (2.8), and the held dust lanes were a fifth deeper than the released ones (plan hdri-7 §5, D10). `n` itself at
/// `max` octaves, and at a whole number of them exactly the fbm normalised by `max`'s total.
fn on_full_scale(n: f32, octaves: f32, max: f32) -> f32 {
    if !(octaves < max) {
        return n;
    }
    let total = |o: f32| {
        let whole = o.floor();
        let next = 0.5f32.powi(whole as i32);
        2.0 * (1.0 - next) + (o - whole) * next
    };
    0.5 + (n - 0.5) * (total(octaves) / total(max))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dust never takes more than three quarters (foreground stars fill the rift in), is none off the band (|b| over
    /// 25 degrees) and none at Dust 0.
    #[test]
    fn the_dust_has_a_floor_and_stays_on_the_band() {
        let axes = [vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)];
        let deep = MilkyWay::new(&MilkyWayParams { dust: 2.0, ..MilkyWayParams::default() }, 7, axes, 1.0);
        let none = MilkyWay::new(&MilkyWayParams { dust: 0.0, ..MilkyWayParams::default() }, 7, axes, 1.0);
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
        assert_eq!(MilkyWay::new(&MilkyWayParams::default(), 7, axes, 0.0).at(axes[0], 0.0).0, Vec3f::default(), "gain 0 draws nothing");
    }
}
