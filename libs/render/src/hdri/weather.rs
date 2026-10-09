//! Weather over the sky (phase 1b).
//!
//! A weather **kind** is a preset over the sky's existing fields, applied when
//! the map is built and never written back into the params: it raises the
//! haze, closes the cloud deck and softens it ([`weathered_sky`]), and brings
//! floors for the weather's own effects ([`effects`]). The kinds follow
//! apps/weather's `SkyKind` classes (clear, overcast, rain, snow, fog, thunder
//! as "storm") plus "hazy" ([`kind_for_sky_kind`]).
//!
//! The effects, in the order they are drawn over the dry sky:
//! - **Ground.** Under a closed deck the ground gets the deck's light, not the
//!   open sky's. Snow is a ground albedo of 0.8 (through the weathered ground
//!   colour). Wet ground mirrors the sky by water's Fresnel reflectance, as a
//!   dull sheen on damp ground and as a flat mirror in fbm puddles.
//! - **Rain.** Haze that greys the distance, with virga: shafts hanging under
//!   the deck toward the horizon, darker curtains over the haze and what is
//!   behind it. An environment map holds no streaks.
//! - **Fog.** A veil lying on the ground: thin overhead, opaque toward the
//!   horizon, glowing toward the sun; dense at 1, when only the ground at the
//!   feet shows through it.
//! - **Lightning.** One seeded bolt from the deck to the horizon and a flash
//!   lobe around its top, standing inside the rain.
//!
//! The rain haze and the fog are lit by the dome as the map draws it (under a
//! deck, the deck's own light), greyed as the clouds grey it.
//!
//! Directions are in the map's own frame (before the map rotation); radiance
//! is in the atmosphere's units.

use makepad_draw::*;
use super::noise::{fbm2, hash01};
use super::{dir_from_az_el, SkyParams, WeatherKind, WeatherParams};
use crate::sky::luminance;

/// Snow on the ground: albedo 0.8.
const SNOW_ALBEDO: [f32; 3] = [0.8, 0.8, 0.8];
/// The viewer's height above the ground: how near the ground below is.
const EYE_M: f32 = 2.0;

/// Fog's optical depth straight up is FOG_ZENITH times its thickened amount
/// (`thickened`); the layer is FOG_HEIGHT_M thick and a level path through it
/// is capped at FOG_RANGE_M (the Earth curves away).
const FOG_ZENITH: f32 = 2.5;
const FOG_HEIGHT_M: f32 = 150.0;
const FOG_RANGE_M: f32 = 6000.0;
/// Fog thickens steeply over the last stretch of its range: its depth goes
/// with fog x (1 + FOG_DENSE x fog^16). The kinds' amounts (0.7 and under)
/// stay within 2.5 % of linear, about 330 m of sight at the ground at 0.7,
/// and fog 1 is dense fog, 11 times as deep as 0.7: about 30 m of sight.
const FOG_DENSE: f32 = 7.0;
/// Fog's forward scattering (Henyey-Greenstein g) and the share of the sun's
/// light it spreads evenly by its many scatterings.
const FOG_G: f32 = 0.6;
const FOG_DIFFUSE: f32 = 0.6;

/// Rain 1 has this optical depth straight up, through a slab up to the deck.
const RAIN_ZENITH: f32 = 1.2;
const RAIN_RANGE_M: f32 = 12_000.0;
/// Rain is a dull grey of the ambient; falling snow is nearly white.
const RAIN_ALBEDO: f32 = 0.6;
const SNOWFALL_ALBEDO: f32 = 0.9;
/// The most a shaft of virga darkens the rain haze and what is behind it.
const VIRGA_DARKEN: f32 = 0.5;
/// Radius of the circle the shaft noise is read on (cells): the azimuth wraps
/// around it without a seam. 5 gives shafts some ten degrees apart.
const VIRGA_RING: f32 = 5.0;
/// How far the wind leans the shafts (radians of azimuth per unit of sine).
const VIRGA_SLANT: f32 = 0.6;

/// Segments of the bolt's main channel, its sideways kink per segment
/// (radians), and its forks.
const BOLT_STEPS: usize = 18;
const BOLT_KINK: f32 = 0.03;
const BOLT_FORKS: usize = 2;
const FORK_WEIGHT: f32 = 0.4;
/// Radiance of the channel's core and of its halo, and their widths (radians:
/// 0.12 and 0.5 degrees). The halo keeps the channel a line in a small preview.
const BOLT_CORE: f32 = 2000.0;
const BOLT_HALO: f32 = 30.0;
const CORE_SIGMA: f32 = 0.0021;
const HALO_SIGMA: f32 = 0.009;
/// Nothing of the channel lies further than this from the bolt's azimuth (radians).
const BOLT_REACH: f32 = 0.6;
/// The flash: the cloud around the channel's top, lit from inside. Radiance at
/// its centre, its width (14 degrees) and the cosine past which it is nothing.
const FLASH: f32 = 3.0;
const FLASH_SIGMA: f32 = 0.25;
const FLASH_COS_MIN: f32 = 0.64;
/// Lightning is blue-white.
const BOLT_COLOR: [f32; 3] = [0.8, 0.88, 1.0];

/// Water's reflectance head-on (Schlick's F0).
const WATER_F0: f32 = 0.02;
/// Wet ground is darker (water fills its pores) and has a dull sheen.
const WET_DARKEN: f32 = 0.45;
const WET_SHEEN: f32 = 0.5;
/// What shows of the ground through a puddle.
const PUDDLE_BOTTOM: f32 = 0.35;
/// Metres per cell of the puddle noise, and the distances between which the
/// puddles blur into their mean share (they are below a texel out there).
const PUDDLE_M: f32 = 1.5;
const PUDDLE_FADE_START_M: f32 = 30.0;
const PUDDLE_FADE_END_M: f32 = 200.0;
/// sin(3 deg): the band below the horizon over which the atmosphere blends its
/// ground into the horizon haze (atmosphere.rs, GROUND_BLEND_DEG).
const GROUND_BLEND: f32 = 0.0523;

/// What a kind asks of the sky: floors and a ceiling over the sky's own
/// fields, floors under the weather's effects, and how much light the deck
/// passes to the clouds' undersides and to the ground.
struct KindPreset {
    haze_min: f32,
    coverage_min: f32,
    sharpness_max: f32,
    fog: f32,
    rain: f32,
    wet_ground: f32,
    snow: bool,
    lightning: bool,
    cloud_gain: f32,
    ground_light: f32,
}

/// Fair weather: nothing asked.
const FAIR: KindPreset = KindPreset {
    haze_min: 0.0,
    coverage_min: 0.0,
    sharpness_max: 1.0,
    fog: 0.0,
    rain: 0.0,
    wet_ground: 0.0,
    snow: false,
    lightning: false,
    cloud_gain: 1.0,
    ground_light: 1.0,
};

fn preset(kind: WeatherKind) -> KindPreset {
    match kind {
        WeatherKind::Clear => FAIR,
        WeatherKind::Hazy => KindPreset { haze_min: 3.0, fog: 0.08, ground_light: 0.9, ..FAIR },
        WeatherKind::Overcast => {
            KindPreset { haze_min: 1.5, coverage_min: 0.95, sharpness_max: 0.25, fog: 0.03, ground_light: 0.45, ..FAIR }
        }
        WeatherKind::Fog => KindPreset {
            haze_min: 2.0,
            coverage_min: 0.9,
            sharpness_max: 0.2,
            fog: 0.7,
            wet_ground: 0.2,
            ground_light: 0.5,
            ..FAIR
        },
        WeatherKind::Rain => KindPreset {
            haze_min: 2.0,
            coverage_min: 1.0,
            sharpness_max: 0.2,
            fog: 0.1,
            rain: 0.6,
            wet_ground: 0.7,
            cloud_gain: 0.7,
            ground_light: 0.3,
            ..FAIR
        },
        WeatherKind::Storm => KindPreset {
            haze_min: 2.5,
            coverage_min: 1.0,
            sharpness_max: 0.3,
            fog: 0.12,
            rain: 0.9,
            wet_ground: 0.9,
            lightning: true,
            cloud_gain: 0.45,
            ground_light: 0.18,
            ..FAIR
        },
        WeatherKind::Snow => KindPreset {
            haze_min: 1.5,
            coverage_min: 0.95,
            sharpness_max: 0.2,
            fog: 0.2,
            rain: 0.35,
            snow: true,
            cloud_gain: 0.85,
            ground_light: 0.55,
            ..FAIR
        },
    }
}

/// The weather's own effects after the kind's floors: what is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effects {
    pub fog: f32,
    pub rain: f32,
    pub wet_ground: f32,
    pub snow: bool,
    pub lightning: bool,
}

/// The effects of a weather block: each explicit number or tick, or the
/// kind's own amount when that is more. So a kind alone is complete, and the
/// numbers only add to it.
pub fn effects(w: &WeatherParams) -> Effects {
    let kind = preset(w.kind());
    Effects {
        fog: unit(w.fog).max(kind.fog),
        rain: unit(w.rain).max(kind.rain),
        wet_ground: unit(w.wet_ground).max(kind.wet_ground),
        snow: w.snow || kind.snow,
        lightning: w.lightning || kind.lightning,
    }
}

/// A copy of the sky settings with the weather's kind folded into the fields
/// it presets: the haze and the cloud coverage raised to the kind's floors,
/// the sharpness lowered to its ceiling, the ground white under snow. The
/// layers are built from this copy; the stored params stay the user's. With
/// no weather, or kind "clear" and no snow, the copy equals the input.
pub fn weathered_sky(sky: &SkyParams) -> SkyParams {
    let mut out = sky.clone();
    let Some(weather) = &sky.weather else {
        return out;
    };
    let kind = preset(weather.kind());
    out.atmosphere.haze = out.atmosphere.haze.max(kind.haze_min);
    out.clouds.coverage = out.clouds.coverage.max(kind.coverage_min);
    out.clouds.sharpness = out.clouds.sharpness.min(kind.sharpness_max);
    if effects(weather).snow {
        out.atmosphere.ground_color = SNOW_ALBEDO;
    }
    out
}

/// The kind for one of apps/weather's `SkyKind` discriminants
/// (apps/weather/src/model.rs: Clear = 0, Partly = 1, Overcast = 2, Rain = 3,
/// Snow = 4, Fog = 5, Thunder = 6, Unknown = 7), for a host that feeds live
/// weather. Partly cloudy is fair weather with the caller's own cloud cover.
pub fn kind_for_sky_kind(index: u32) -> WeatherKind {
    match index {
        2 => WeatherKind::Overcast,
        3 => WeatherKind::Rain,
        4 => WeatherKind::Snow,
        5 => WeatherKind::Fog,
        6 => WeatherKind::Storm,
        _ => WeatherKind::Clear,
    }
}

/// One stretch of the bolt in its own plane: x across (radians of great
/// circle from the bolt's azimuth), y elevation (radians).
struct BoltSegment {
    a: [f32; 2],
    b: [f32; 2],
    /// 1 for the main channel, less along a fork.
    weight: f32,
}

struct Bolt {
    /// Where it strikes, radians clockwise from north.
    azimuth: f32,
    /// Elevation of the channel's top, radians.
    top: f32,
    /// The centre of the flash, just above the top.
    flash_dir: Vec3f,
    segments: Vec<BoltSegment>,
}

/// The seeded bolt: a channel that walks sideways with a lean that carries
/// over (long kinks and short jitters) while it drops from the deck to just
/// over the horizon, and two weaker forks that leave it on the way.
fn build_bolt(seed: u32) -> Bolt {
    let roll = |index: usize, stream: i32| hash01(index as i32, stream, 0, seed ^ 0x0b01_7000);
    let azimuth = roll(0, 0) * std::f32::consts::TAU;
    let top = (22.0 + 12.0 * roll(1, 0)).to_radians();
    let foot = (0.5 + 1.5 * roll(2, 0)).to_radians();
    let drop = (top - foot) / BOLT_STEPS as f32;
    let mut points = Vec::with_capacity(BOLT_STEPS + 1);
    let (mut x, mut lean) = (0.0f32, 0.0f32);
    for i in 0..=BOLT_STEPS {
        points.push([x, top - drop * i as f32]);
        lean = 0.6 * lean + (roll(i, 1) - 0.5) * BOLT_KINK;
        x += lean;
    }
    let mut segments: Vec<BoltSegment> =
        points.windows(2).map(|pair| BoltSegment { a: pair[0], b: pair[1], weight: 1.0 }).collect();
    for fork in 0..BOLT_FORKS {
        let start = 3 + (roll(fork, 2) * (BOLT_STEPS - 8) as f32) as usize;
        let side = if roll(fork, 3) < 0.5 { -1.0 } else { 1.0 };
        let length = 4 + (roll(fork, 4) * 4.0) as usize;
        let mut from = points[start];
        for j in 0..length {
            let index = fork * 16 + j;
            let to = [
                from[0] + side * BOLT_KINK * (0.6 + roll(index, 5)),
                from[1] - drop * (0.5 + 0.5 * roll(index, 6)),
            ];
            // A fork thins out toward its tip.
            segments.push(BoltSegment { a: from, b: to, weight: FORK_WEIGHT * (1.0 - 0.6 * j as f32 / length as f32) });
            from = to;
        }
    }
    Bolt { azimuth, top, flash_dir: dir_from_az_el(azimuth.to_degrees(), top.to_degrees() + 4.0), segments }
}

/// The weather of one map, ready to draw over the dry sky.
pub struct Weather {
    effects: Effects,
    /// How much of the clouds' light a rain or storm deck keeps.
    cloud_gain: f32,
    /// How much of the open sky's light reaches the ground under the deck.
    ground_light: f32,
    ground_color: [f32; 3],
    /// The rain slab reaches up to the cloud base, metres.
    rain_height: f32,
    /// The cloud cover the map is drawn with (the kind's floor or the sky's
    /// own, whichever is more): it greys the fog's light as it greys the deck's.
    coverage: f32,
    sun_dir: Vec3f,
    seed: u32,
    bolt: Option<Bolt>,
}

impl Weather {
    /// `sky` is the user's own (clamped) sky block, weather included; the
    /// kind's floors are worked out here. None when the weather draws nothing:
    /// no weather block, or fair weather with every effect at zero.
    pub fn new(sky: &SkyParams, sun_dir: Vec3f) -> Option<Weather> {
        let params = sky.weather.as_ref()?;
        let kind = preset(params.kind());
        let drawn = effects(params);
        let idle = drawn.fog <= 0.0 && drawn.rain <= 0.0 && drawn.wet_ground <= 0.0 && !drawn.snow && !drawn.lightning;
        if idle && kind.cloud_gain == 1.0 && kind.ground_light == 1.0 {
            return None;
        }
        let sun = sun_dir.normalize();
        let sun = if sun.is_finite() && sun.length() > 0.5 { sun } else { vec3f(0.0, 1.0, 0.0) };
        // Rain falls from the cloud base: the volume's when there is one, else the deck's.
        let base = sky.volumetric.as_ref().map_or(sky.clouds.altitude_m, |vol| vol.base_m);
        Some(Weather {
            effects: drawn,
            cloud_gain: kind.cloud_gain,
            ground_light: kind.ground_light,
            ground_color: if drawn.snow { SNOW_ALBEDO } else { sky.atmosphere.ground_color },
            rain_height: if base.is_finite() { base.clamp(200.0, 12_000.0) } else { 2000.0 },
            coverage: unit(sky.clouds.coverage).max(kind.coverage_min),
            sun_dir: sun,
            seed: params.seed,
            bolt: if drawn.lightning { Some(build_bolt(params.seed)) } else { None },
        })
    }

    pub fn effects(&self) -> Effects {
        self.effects
    }

    /// Factor on the clouds' own light: below 1 under rain and storm.
    pub fn cloud_gain(&self) -> f32 {
        self.cloud_gain
    }

    /// The ground colour the atmosphere was built with (`weathered_sky`):
    /// white under snow, else the sky's own.
    pub fn ground_color(&self) -> [f32; 3] {
        self.ground_color
    }

    /// True when the ground mirrors the sky (so the sun's reflection is as
    /// small a bright spot as the sun).
    pub fn mirrors(&self) -> bool {
        self.effects.wet_ground > 0.0
    }

    /// The ground below the horizon under this weather. `ground` is what the
    /// dry sky drew along `d`; `sky_at` answers what the sky shows along a
    /// direction above the horizon (it is asked for the mirror image of `d`;
    /// `Env` passes the sky as the weather shows it, through the rain and fog,
    /// so the mirror is no brighter than the sky above it).
    /// Above the horizon `ground` passes unchanged.
    pub fn ground(&self, d: Vec3f, ground: Vec3f, sky_at: &dyn Fn(Vec3f) -> Vec3f) -> Vec3f {
        if !(d.y < 0.0) {
            return ground;
        }
        let down = (-d.y).min(1.0);
        // The atmosphere lit its ground with the open sky and the sun. Under a
        // closed deck less arrives; the dimming follows the atmosphere's own
        // blend from horizon haze to ground.
        let c = ground * (1.0 + (self.ground_light - 1.0) * smooth(0.0, GROUND_BLEND, down));
        let wet = self.effects.wet_ground;
        if wet <= 0.0 {
            return c;
        }
        // Schlick's Fresnel for water: a mirror at grazing angles, 2 % head-on.
        let fresnel = WATER_F0 + (1.0 - WATER_F0) * (1.0 - down).powi(5);
        let mirror = sky_at(vec3f(d.x, -d.y, d.z));
        // Standing water: fbm over the ground plane, an eye height below.
        let reach = EYE_M / down.max(1.0e-3);
        let puddle_noise = fbm2(d.x * reach / PUDDLE_M, d.z * reach / PUDDLE_M, 4, self.seed ^ 0x9d01_e5);
        // More of the ground is under water the wetter it is.
        let threshold = 0.72 - 0.34 * wet;
        let puddle = smooth(threshold, threshold + 0.06, puddle_noise);
        // Far away the puddles are smaller than a texel: their mean share
        // instead (the fbm is near normal, mean 0.49, deviation 0.12), so the
        // horizon does not sparkle.
        let share = smooth(-2.0, 2.0, (0.49 - (threshold + 0.03)) / 0.12);
        let distance = reach * (1.0 - down * down).max(0.0).sqrt();
        let detail = 1.0 - smooth(PUDDLE_FADE_START_M, PUDDLE_FADE_END_M, distance);
        let puddle = share + (puddle - share) * detail;
        // Damp ground between the puddles: darker, with a dull sheen. A puddle:
        // a flat mirror over what shows of its bottom.
        let damp = c * (1.0 - WET_DARKEN * wet) + mirror * (fresnel * WET_SHEEN * wet);
        let water = c * (PUDDLE_BOTTOM * (1.0 - fresnel)) + mirror * fresnel;
        damp.mix(water, puddle)
    }

    /// The lightning along `d`: the bolt's channel and the flash around its
    /// top, already dimmed by half the veil's depth (the bolt stands inside
    /// the rain, not behind it). Zero without a bolt.
    pub fn lightning(&self, d: Vec3f) -> Vec3f {
        let Some(bolt) = &self.bolt else {
            return Vec3f::default();
        };
        let mut light = 0.0f32;
        let cos_flash = d.dot(bolt.flash_dir);
        if cos_flash > FLASH_COS_MIN {
            let angle = cos_flash.min(1.0).acos() / FLASH_SIGMA;
            light += FLASH * (-0.5 * angle * angle).exp();
        }
        let elevation = d.y.clamp(-1.0, 1.0).asin();
        if elevation > -0.02 && elevation < bolt.top + 0.05 {
            let tau = std::f32::consts::TAU;
            let mut turn = d.x.atan2(-d.z) - bolt.azimuth;
            // Into -pi..pi: the bolt may stand on either side of north.
            turn -= tau * (turn / tau).round();
            let x = turn * elevation.cos();
            if x.abs() < BOLT_REACH {
                let mut channel = 0.0f32;
                for segment in &bolt.segments {
                    let distance = segment_distance([x, elevation], segment.a, segment.b);
                    let glow = BOLT_CORE * gauss(distance, CORE_SIGMA) + BOLT_HALO * gauss(distance, HALO_SIGMA);
                    channel = channel.max(segment.weight * glow);
                }
                light += channel;
            }
        }
        if !(light > 0.0) {
            return Vec3f::default();
        }
        vec3f(BOLT_COLOR[0], BOLT_COLOR[1], BOLT_COLOR[2]) * (light * self.haze_transmittance(d).sqrt())
    }

    /// The veil between the eye and what is drawn along `d`: rain haze, its
    /// virga shafts over the haze and what is behind it, then fog.
    /// `sun_irradiance` is the sun's light under the clouds (already through
    /// the cover along the sun), `ambient` the mean radiance of the dome above
    /// the veil as the map draws it, the clouds and their light included
    /// (`Env` measures it once per map).
    pub fn veil(&self, d: Vec3f, c: Vec3f, sun_irradiance: Vec3f, ambient: Vec3f) -> Vec3f {
        let mut c = c;
        // The air under a deck is lit by what that deck passes down, as the
        // clouds' own undersides are: `ambient` is that deck's light, the
        // kind's cloud light included, so a storm's rain is dark.
        let grey = luminance(ambient);
        if self.effects.rain > 0.0 {
            let through = (-self.rain_depth(d)).exp();
            let haze = grey * if self.effects.snow { SNOWFALL_ALBEDO } else { RAIN_ALBEDO };
            c = c * through + vec3f(haze, haze, haze) * (1.0 - through);
            // Shafts of rain under the deck are denser curtains of it: they
            // darken the distance and the rain haze before it alike.
            c = c * (1.0 - self.virga(d));
        }
        if self.effects.fog > 0.0 {
            let through = (-self.fog_depth(d)).exp();
            // Fog is lit by the dome, greyed as the clouds grey it (half on a
            // clear day, all of it under a closed deck), and by the sun:
            // evenly, from the many scatterings inside it, and with a glow
            // toward the sun.
            let sky = ambient.mix(vec3f(grey, grey, grey), 0.5 + 0.5 * self.coverage);
            let even = FOG_DIFFUSE * self.sun_dir.y.max(0.0) / std::f32::consts::PI;
            let glow = hg_phase(d.dot(self.sun_dir), FOG_G);
            c = c * through + (sky + sun_irradiance * (even + glow)) * (1.0 - through);
        }
        c
    }

    /// The fraction of light that passes the veil along `d`: the rain, the
    /// shaft of virga hanging there and the fog, as `veil` dims what is behind
    /// them. It dims the key light, so a sun behind a shaft hands the engine
    /// what the map shows of it (D1).
    pub fn transmittance_toward(&self, d: Vec3f) -> f32 {
        self.haze_transmittance(d) * (1.0 - self.virga(d))
    }

    /// The fraction of light that passes the rain and the fog along `d`, the
    /// shafts apart. Half its depth dims the bolt, which stands inside the
    /// rain, shafts and all.
    fn haze_transmittance(&self, d: Vec3f) -> f32 {
        (-(self.fog_depth(d) + self.rain_depth(d))).exp()
    }

    fn fog_depth(&self, d: Vec3f) -> f32 {
        slab_depth(d.y, thickened(self.effects.fog) * FOG_ZENITH, FOG_HEIGHT_M, FOG_RANGE_M)
    }

    fn rain_depth(&self, d: Vec3f) -> f32 {
        slab_depth(d.y, self.effects.rain * RAIN_ZENITH, self.rain_height, RAIN_RANGE_M)
    }

    /// How much a shaft of virga darkens along `d`: 0..VIRGA_DARKEN. Shafts
    /// are a function of the azimuth alone (leaned a little by the wind), so
    /// they hang straight; they show between the horizon and the deck.
    fn virga(&self, d: Vec3f) -> f32 {
        if self.effects.rain <= 0.0 || !(d.y > 0.0) {
            return 0.0;
        }
        let band = smooth(0.0, 0.04, d.y) * (1.0 - smooth(0.2, 0.45, d.y));
        if band <= 0.0 {
            return 0.0;
        }
        let azimuth = d.x.atan2(-d.z) + VIRGA_SLANT * d.y;
        // Read on a circle, so the azimuth wraps without a seam.
        let noise = fbm2(VIRGA_RING * azimuth.cos(), VIRGA_RING * azimuth.sin(), 4, self.seed ^ 0x51a_f7);
        VIRGA_DARKEN * self.effects.rain.min(1.0) * smooth(0.5, 0.72, noise) * band
    }
}

/// Optical depth along a ray of elevation sine `sin_elevation` through a level
/// slab `height_m` thick whose depth straight up is `zenith_depth`. A level
/// path is capped at `range_m`; below the horizon the ray ends on the ground,
/// an eye height below, once the atmosphere's ground blend (GROUND_BLEND) is
/// past. The two branches meet at the horizon.
fn slab_depth(sin_elevation: f32, zenith_depth: f32, height_m: f32, range_m: f32) -> f32 {
    if !(zenith_depth > 0.0) {
        return 0.0;
    }
    let path = if sin_elevation >= 0.0 {
        height_m / (sin_elevation + height_m / range_m)
    } else {
        // Just below the horizon the atmosphere still draws distant haze (its
        // ground blend): that is as far off as the horizon itself. The path
        // shortens to the ground an eye height below as the ground takes over.
        let ground = EYE_M / (-sin_elevation + EYE_M / range_m);
        range_m + (ground - range_m) * smooth(0.0, GROUND_BLEND, -sin_elevation)
    };
    zenith_depth / height_m * path
}

/// A fog amount as the depth it gives, per FOG_ZENITH: linear over the kinds'
/// amounts, then steeply denser toward 1 (FOG_DENSE).
fn thickened(fog: f32) -> f32 {
    fog * (1.0 + FOG_DENSE * fog.powi(16))
}

/// Distance from point `p` to the segment a-b, all in the bolt's plane.
fn segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let (apx, apy) = (p[0] - a[0], p[1] - a[1]);
    let length2 = abx * abx + aby * aby;
    let t = if length2 > 0.0 { ((apx * abx + apy * aby) / length2).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (apx - abx * t, apy - aby * t);
    (dx * dx + dy * dy).sqrt()
}

fn gauss(distance: f32, sigma: f32) -> f32 {
    let q = distance / sigma;
    (-q * q).exp()
}

/// Henyey-Greenstein phase function, normalised over the sphere (1/sr).
fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let gg = g * g;
    let denom = (1.0 + gg - 2.0 * g * cos_theta).max(1.0e-6);
    (1.0 - gg) / (4.0 * std::f32::consts::PI * denom * denom.sqrt())
}

/// smoothstep under its own name, so no glob can make it ambiguous.
fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn unit(x: f32) -> f32 {
    if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sun() -> Vec3f {
        dir_from_az_el(180.0, 40.0)
    }

    /// The default sky under one weather kind, everything else at its default.
    fn sky_with(kind: &str) -> SkyParams {
        let mut sky = SkyParams::default();
        sky.weather = Some(WeatherParams { kind: kind.to_string(), ..WeatherParams::default() });
        sky
    }

    fn weather(kind: &str) -> Weather {
        Weather::new(&sky_with(kind), sun()).expect("this kind draws something")
    }

    #[test]
    fn kinds_are_floors_over_the_skys_own_fields() {
        let plain = SkyParams::default();
        assert_eq!(weathered_sky(&plain), plain, "no weather, no change");
        assert_eq!(weathered_sky(&sky_with("clear")), sky_with("clear"));
        let overcast = weathered_sky(&sky_with("overcast"));
        assert!(overcast.clouds.coverage >= 0.95 && overcast.clouds.sharpness <= 0.25 && overcast.atmosphere.haze >= 1.5);
        // A floor never lowers what the user set higher.
        let mut hazy = sky_with("hazy");
        hazy.atmosphere.haze = 6.0;
        assert_eq!(weathered_sky(&hazy).atmosphere.haze, 6.0);
        assert_eq!(weathered_sky(&sky_with("hazy")).atmosphere.haze, 3.0);
        // The stored weather block is not rewritten, and folding twice changes nothing more.
        let storm = weathered_sky(&sky_with("storm"));
        assert_eq!(storm.weather, sky_with("storm").weather);
        assert_eq!(weathered_sky(&storm), storm);
        // Snow lies on the ground at albedo 0.8, from the kind or from the tick.
        assert_eq!(weathered_sky(&sky_with("snow")).atmosphere.ground_color, [0.8, 0.8, 0.8]);
        let mut dusting = sky_with("clear");
        dusting.weather.as_mut().unwrap().snow = true;
        assert_eq!(weathered_sky(&dusting).atmosphere.ground_color, [0.8, 0.8, 0.8]);
        assert_eq!(weathered_sky(&sky_with("rain")).atmosphere.ground_color, plain.atmosphere.ground_color);
    }

    #[test]
    fn a_kind_alone_brings_its_effects_and_the_numbers_only_add() {
        let of = |kind: &str| effects(&WeatherParams { kind: kind.to_string(), ..WeatherParams::default() });
        assert_eq!(of("clear"), Effects { fog: 0.0, rain: 0.0, wet_ground: 0.0, snow: false, lightning: false });
        assert!(of("fog").fog >= 0.5);
        assert!(of("rain").rain >= 0.5 && of("rain").wet_ground >= 0.5 && !of("rain").lightning);
        assert!(of("storm").lightning && of("storm").rain > of("rain").rain);
        assert!(of("snow").snow && !of("rain").snow);
        // On clear the numbers act alone.
        let wet = effects(&WeatherParams { fog: 0.4, wet_ground: 1.0, lightning: true, ..WeatherParams::default() });
        assert_eq!((wet.fog, wet.wet_ground, wet.lightning), (0.4, 1.0, true));
        // On a kind they add; they never take away from it.
        let rain = effects(&WeatherParams { kind: "rain".to_string(), rain: 0.1, fog: 0.9, ..WeatherParams::default() });
        assert_eq!((rain.rain, rain.fog), (of("rain").rain, 0.9));
    }

    #[test]
    fn kinds_follow_the_weather_apps_sky_kinds() {
        // apps/weather/src/model.rs: Clear = 0, Partly = 1, Overcast = 2, Rain = 3,
        // Snow = 4, Fog = 5, Thunder = 6, Unknown = 7.
        let kinds: Vec<WeatherKind> = (0..8).map(kind_for_sky_kind).collect();
        assert_eq!(
            kinds,
            [
                WeatherKind::Clear,
                WeatherKind::Clear,
                WeatherKind::Overcast,
                WeatherKind::Rain,
                WeatherKind::Snow,
                WeatherKind::Fog,
                WeatherKind::Storm,
                WeatherKind::Clear
            ]
        );
        assert_eq!(kind_for_sky_kind(99), WeatherKind::Clear);
    }

    #[test]
    fn fair_weather_draws_nothing() {
        assert!(Weather::new(&SkyParams::default(), sun()).is_none());
        assert!(Weather::new(&sky_with("clear"), sun()).is_none());
        assert!(Weather::new(&sky_with("hazy"), sun()).is_some());
        let rain = weather("rain");
        let storm = weather("storm");
        assert!(storm.cloud_gain() < rain.cloud_gain() && rain.cloud_gain() < 1.0);
        assert_eq!(weather("hazy").cloud_gain(), 1.0);
        assert!(rain.mirrors() && !weather("hazy").mirrors());
        assert_eq!(weather("snow").ground_color(), [0.8, 0.8, 0.8]);
        assert_eq!(rain.ground_color(), SkyParams::default().atmosphere.ground_color);
        assert_eq!(rain.effects(), effects(sky_with("rain").weather.as_ref().unwrap()));
    }

    #[test]
    fn fog_veils_the_horizon_before_the_zenith_and_glows_toward_the_sun() {
        let fog = weather("fog");
        let up = vec3f(0.0, 1.0, 0.0);
        assert!(fog.transmittance_toward(dir_from_az_el(0.0, 0.5)) < 0.01, "the horizon is lost in fog");
        let overhead = fog.transmittance_toward(up);
        assert!(overhead > 0.05 && overhead < 0.5, "the sky overhead is dimmed, not gone: {overhead}");
        // Looking down, the ground an eye height below is close: little fog in between.
        assert!(fog.transmittance_toward(vec3f(0.0, -1.0, 0.0)) > 0.9);
        // Continuous through the horizon.
        let mist = Weather::new(&{ let mut s = sky_with("clear"); s.weather.as_mut().unwrap().fog = 0.02; s }, sun()).unwrap();
        let (above, below) = (
            mist.transmittance_toward(vec3f(1.0, 1.0e-6, 0.0).normalize()),
            mist.transmittance_toward(vec3f(1.0, -1.0e-6, 0.0).normalize()),
        );
        assert!((above - below).abs() < 0.01, "{above} vs {below}");
        // What is behind thick fog is replaced by the fog's own light...
        let (sun_e, ambient) = (vec3f(10.0, 10.0, 10.0), vec3f(0.3, 0.4, 0.6));
        let red = vec3f(100.0, 0.0, 0.0);
        let level = fog.veil(dir_from_az_el(0.0, 0.5), red, sun_e, ambient);
        assert!(level.x < 5.0 && level.z > 0.1, "{level:?}");
        // ...and overhead part of it still shows.
        assert!(fog.veil(up, red, sun_e, ambient).x > 5.0);
        // Fog scatters forward: it is brighter toward the sun than away from it.
        let toward = luminance(fog.veil(dir_from_az_el(180.0, 1.0), Vec3f::default(), sun_e, ambient));
        let away = luminance(fog.veil(dir_from_az_el(0.0, 1.0), Vec3f::default(), sun_e, ambient));
        assert!(toward > 1.5 * away, "{toward} vs {away}");
        // With the sun gone the fog is the sky's own grey.
        let unlit = fog.veil(dir_from_az_el(0.0, 0.5), red, Vec3f::default(), ambient);
        assert!(unlit.x > 0.2 && unlit.x < 0.5 && unlit.z > unlit.x, "{unlit:?}");
    }

    /// Fog 1 is dense fog, about 30 m of sight at the ground: the deck
    /// overhead and the ground past ten metres or so are lost in it, and only
    /// the ground at the feet shows (an eye 2 m up looks down through 2 m of
    /// it). The kinds' own amounts, 0.7 and under, keep their depth (B17's fix
    /// round: fog 1 was only 1.4 times as deep as fog 0.7, and hid nothing more).
    #[test]
    fn the_densest_fog_leaves_only_the_ground_at_the_feet() {
        let mut sky = sky_with("fog");
        sky.weather.as_mut().unwrap().fog = 1.0;
        let dense = Weather::new(&sky, sun()).unwrap();
        let up = vec3f(0.0, 1.0, 0.0);
        let overhead = dense.transmittance_toward(up);
        assert!(overhead < 1.0e-3, "the deck overhead is lost: {overhead}");
        let (ten, five) = (dense.transmittance_toward(dir_from_az_el(0.0, -10.0)), dense.transmittance_toward(dir_from_az_el(0.0, -5.0)));
        assert!(ten < 0.35 && five < 0.1, "the ground 11 m and 23 m off: {ten}, {five}");
        let feet = dense.transmittance_toward(vec3f(0.0, -1.0, 0.0));
        assert!(feet > 0.5, "the ground at the feet still shows: {feet}");
        // The kind's own fog 0.7 is within 3 % of its linear depth.
        let kind = weather("fog").fog_depth(up);
        let linear = slab_depth(1.0, 0.7 * FOG_ZENITH, FOG_HEIGHT_M, FOG_RANGE_M);
        assert!((kind / linear - 1.0).abs() < 0.03, "{kind} vs {linear}");
    }

    #[test]
    fn rain_greys_the_distance_and_hangs_virga_under_the_deck() {
        let rain = weather("rain");
        assert!(rain.transmittance_toward(dir_from_az_el(90.0, 1.0)) < 0.2);
        assert!(rain.transmittance_toward(vec3f(0.0, 1.0, 0.0)) > 0.3);
        // Virga hangs between the horizon and the deck: none below the horizon, none overhead.
        assert_eq!(rain.virga(dir_from_az_el(10.0, -5.0)), 0.0);
        assert_eq!(rain.virga(vec3f(0.0, 1.0, 0.0)), 0.0);
        let shafts: Vec<f32> = (0..360).map(|az| rain.virga(dir_from_az_el(az as f32, 8.0))).collect();
        let lo = shafts.iter().cloned().fold(f32::MAX, f32::min);
        let hi = shafts.iter().cloned().fold(0.0, f32::max);
        assert!(hi > 0.15 && lo < 0.02, "shafts and gaps: {lo}..{hi}");
        assert!(shafts.iter().all(|v| (0.0..=VIRGA_DARKEN).contains(v)));
        // The azimuth wraps without a seam.
        let seam = (rain.virga(dir_from_az_el(359.99, 8.0)) - rain.virga(dir_from_az_el(0.01, 8.0))).abs();
        assert!(seam < 0.02, "{seam}");
        // Fog has no shafts.
        assert_eq!(weather("fog").virga(dir_from_az_el(10.0, 8.0)), 0.0);
    }

    #[test]
    fn a_storm_draws_one_seeded_bolt_and_its_flash() {
        let storm = weather("storm");
        let bolt = storm.bolt.as_ref().expect("a storm has its bolt");
        assert!(bolt.segments.len() > BOLT_STEPS, "a channel and its forks");
        // The direction of a point of the bolt's own plane (x across, y elevation; radians).
        let at = |x: f32, y: f32| dir_from_az_el((bolt.azimuth + x / y.cos()).to_degrees(), y.to_degrees());
        // On the channel the bolt outshines everything; just above its top only the flash is left.
        let on_channel = luminance(storm.lightning(at(0.0, bolt.top)));
        let above = luminance(storm.lightning(at(0.0, bolt.top + 0.06)));
        assert!(above > 0.0, "the flash lights the cloud around the channel's top");
        assert!(on_channel > 50.0 * above, "{on_channel} vs {above}");
        // Far from the bolt there is no light at all.
        assert_eq!(storm.lightning(dir_from_az_el(bolt.azimuth.to_degrees() + 180.0, 20.0)), Vec3f::default());
        // The same seed draws the same bolt; another seed strikes elsewhere.
        assert_eq!(weather("storm").bolt.as_ref().unwrap().azimuth, bolt.azimuth);
        let mut elsewhere = sky_with("storm");
        elsewhere.weather.as_mut().unwrap().seed = 2;
        let other = Weather::new(&elsewhere, sun()).unwrap();
        assert_ne!(other.bolt.as_ref().unwrap().azimuth, bolt.azimuth);
        // Rain alone has no lightning; the tick adds a bolt to any weather.
        assert!(weather("rain").bolt.is_none());
        assert_eq!(weather("rain").lightning(at(0.0, bolt.top)), Vec3f::default());
        let mut ticked = sky_with("clear");
        ticked.weather.as_mut().unwrap().lightning = true;
        assert!(Weather::new(&ticked, sun()).unwrap().bolt.is_some());
    }

    #[test]
    fn wet_ground_mirrors_the_sky_at_grazing_angles_and_stands_in_puddles() {
        let mut soaked = sky_with("clear");
        soaked.weather.as_mut().unwrap().wet_ground = 1.0;
        let wet = Weather::new(&soaked, sun()).unwrap();
        let ground = vec3f(0.2, 0.2, 0.2);
        // A blue sky to mirror; it is only ever asked about directions above the horizon.
        let sky = |m: Vec3f| {
            assert!(m.y > 0.0, "the mirror looks up");
            vec3f(0.0, 0.0, 10.0)
        };
        // Straight down water mirrors 2 %: the ground is darker (wet), hardly bluer.
        let down = wet.ground(vec3f(0.0, -1.0, 0.0), ground, &sky);
        assert!(down.x < 0.2 && down.z < 0.4, "{down:?}");
        // At a grazing angle the sky takes over.
        let grazing = wet.ground(dir_from_az_el(30.0, -1.0), ground, &sky);
        assert!(grazing.z > 4.0, "{grazing:?}");
        // Puddles: around the feet some spots mirror far more than others.
        let near: Vec<f32> = (0..72).map(|i| wet.ground(dir_from_az_el(i as f32 * 5.0, -20.0), ground, &sky).z).collect();
        let lo = near.iter().cloned().fold(f32::MAX, f32::min);
        let hi = near.iter().cloned().fold(0.0, f32::max);
        assert!(hi > 1.3 * lo, "puddles and damp ground: {lo}..{hi}");
        // Above the horizon nothing is ground.
        assert_eq!(wet.ground(dir_from_az_el(30.0, 10.0), ground, &sky), ground);
        // Dry weather leaves the ground alone, apart from the light a closed deck takes.
        assert_eq!(weather("hazy").ground(vec3f(0.0, -1.0, 0.0), ground, &sky), ground * 0.9);
        let under_storm = weather("storm").ground(vec3f(0.0, -1.0, 0.0), ground, &|_| Vec3f::default());
        assert!(under_storm.x < 0.5 * ground.x, "{under_storm:?}");
    }

    #[test]
    fn the_weather_dims_the_key() {
        let toward_sun = sun();
        assert!(weather("fog").transmittance_toward(toward_sun) < 0.5);
        let hazy = weather("hazy").transmittance_toward(toward_sun);
        assert!(hazy > 0.6 && hazy < 1.0, "{hazy}");
        assert!(weather("storm").transmittance_toward(toward_sun) < weather("rain").transmittance_toward(toward_sun));
    }

    /// D1 (L:260): a rain shaft in front of a low sun dims its disc in the
    /// map (`veil`), so it dims the key by as much: what passes the veil
    /// along a direction is what `transmittance_toward` gives, shaft and all.
    /// The bolt keeps its own dimming (the haze alone, as before).
    #[test]
    fn a_rain_shaft_dims_the_key_as_it_dims_the_disc() {
        let mut sky = SkyParams::default();
        sky.weather = Some(WeatherParams { rain: 0.8, fog: 0.2, ..WeatherParams::default() });
        let rain = Weather::new(&sky, sun()).expect("rain is weather");
        let ring: Vec<Vec3f> = (0..360).map(|az| dir_from_az_el(az as f32, 8.0)).collect();
        let shaft = ring.iter().copied().max_by(|a, b| rain.virga(*a).total_cmp(&rain.virga(*b))).unwrap();
        assert!(rain.virga(shaft) > 0.1, "a shaft hangs on the ring: {}", rain.virga(shaft));
        for d in ring {
            let through = rain.veil(d, vec3f(1.0, 1.0, 1.0), Vec3f::default(), Vec3f::default()).y;
            assert!((rain.transmittance_toward(d) - through).abs() < 1.0e-6, "{through} through the veil, {} toward the key", rain.transmittance_toward(d));
        }
        assert!(rain.transmittance_toward(shaft) < (-(rain.fog_depth(shaft) + rain.rain_depth(shaft))).exp() * 0.95);
    }
}

#[cfg(test)]
mod env_tests {
    use super::*;
    use crate::hdri::{az_el_from_dir, vec, Env, HdriParams, KeyTerm, WEATHER_KIND_NAMES};

    fn sky_params(kind: Option<&str>) -> HdriParams {
        let mut p = HdriParams::default();
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.elevation_deg = 40.0;
        p.sky.sun.azimuth_deg = 180.0;
        if let Some(kind) = kind {
            p.sky.weather = Some(WeatherParams { kind: kind.to_string(), ..WeatherParams::default() });
        }
        p
    }

    #[test]
    fn clear_weather_bakes_the_plain_sky_bit_for_bit() {
        assert_eq!(Env::new(&sky_params(Some("clear"))).bake(32), Env::new(&sky_params(None)).bake(32));
    }

    #[test]
    fn a_kind_closes_the_sky_without_rewriting_the_params() {
        let clear = Env::new(&sky_params(None));
        let rain = Env::new(&sky_params(Some("rain")));
        // The kind raised the cover as if the user had: the disc is hidden and the key is gone under it.
        let at_sun = dir_from_az_el(180.0, 40.0);
        assert!(luminance(rain.radiance(at_sun)) < 1.0e-3 * luminance(clear.radiance(at_sun)));
        let key = rain.sun().expect("the sun is still the key, only dimmed");
        assert!(luminance(key.radiance) < 0.05 * luminance(clear.sun().unwrap().radiance));
        // The stored params are the user's: the coverage slider still reads 0.
        assert_eq!(rain.params().sky.clouds.coverage, 0.0);
        assert_eq!(rain.params().sky.atmosphere.haze, clear.params().sky.atmosphere.haze);
        // Fog dims the key too, through the veil.
        let foggy = Env::new(&sky_params(Some("fog"))).sun().expect("still the key");
        assert!(luminance(foggy.radiance) < 0.5 * luminance(clear.sun().unwrap().radiance));
    }

    #[test]
    fn a_storm_is_darker_than_rain_and_brightest_at_its_bolt() {
        let rain = Env::new(&sky_params(Some("rain")));
        let p = sky_params(Some("storm"));
        let storm = Env::new(&p);
        let up = vec3f(0.0, 1.0, 0.0);
        assert!(luminance(storm.radiance(up)) < luminance(rain.radiance(up)));
        // The bolt and its flash are the brightest thing in a storm map.
        let bolt_azimuth = Weather::new(&p.sky, dir_from_az_el(180.0, 40.0)).unwrap().bolt.as_ref().unwrap().azimuth.to_degrees();
        let map = storm.bake(64);
        let (azimuth, elevation) = az_el_from_dir(vec(map.brightest_direction()));
        let apart = ((azimuth - bolt_azimuth + 540.0).rem_euclid(360.0) - 180.0).abs();
        assert!(apart < 30.0 && elevation > -2.0 && elevation < 50.0, "brightest at ({azimuth}, {elevation}), the bolt at {bolt_azimuth}");
        // The same settings bake the same storm.
        assert_eq!(map, Env::new(&p).bake(64));
    }

    #[test]
    fn snow_lies_bright_and_wet_ground_changes_only_the_ground() {
        let down = dir_from_az_el(90.0, -30.0);
        let overcast = Env::new(&sky_params(Some("overcast")));
        let snow = Env::new(&sky_params(Some("snow")));
        assert!(luminance(snow.radiance(down)) > 2.0 * luminance(overcast.radiance(down)));
        // Wet ground on a clear day: the sky is untouched, the ground is not.
        let dry = Env::new(&sky_params(None));
        let mut soaked = sky_params(Some("clear"));
        soaked.sky.weather.as_mut().unwrap().wet_ground = 1.0;
        let wet = Env::new(&soaked);
        for &(az, el) in &[(0.0f32, 5.0f32), (180.0, 40.0), (270.0, 80.0)] {
            let d = dir_from_az_el(az, el);
            assert_eq!(wet.radiance(d), dry.radiance(d), "({az}, {el})");
        }
        let straight_down = vec3f(0.0, -1.0, 0.0);
        assert!(luminance(wet.radiance(straight_down)) < luminance(dry.radiance(straight_down)), "wet ground is darker head-on");
        let grazing = dir_from_az_el(0.0, -2.0);
        assert_ne!(wet.radiance(grazing), dry.radiance(grazing), "and mirrors the sky at a grazing angle");
    }

    #[test]
    fn every_kind_bakes_a_finite_map() {
        for kind in WEATHER_KIND_NAMES.iter().copied() {
            let map = Env::new(&sky_params(Some(kind))).bake(32);
            assert!(map.data.iter().all(|t| t.iter().all(|v| v.is_finite() && *v >= 0.0)), "{kind}");
        }
    }

    #[test]
    fn the_veil_has_no_seam_at_the_horizon() {
        // Half a degree above and below the horizon the atmosphere draws the
        // same distant haze, so the weather must veil both alike: no bright
        // strip under the horizon. Storm is left out: its bolt's foot stands
        // on the horizon.
        for kind in ["overcast", "fog", "rain", "snow"] {
            let env = Env::new(&sky_params(Some(kind)));
            let mean = |el: f32| (0..36).map(|i| luminance(env.radiance(dir_from_az_el(i as f32 * 10.0 + 3.0, el)))).sum::<f32>() / 36.0;
            let (above, below) = (mean(0.5), mean(-0.5));
            assert!((below / above - 1.0).abs() < 0.25, "{kind}: {above} above the horizon, {below} below it");
        }
    }

    /// Under a closed deck the veil is grey: the fog's light is greyed as the
    /// deck greys the dome's (B17's visual check: every horizon read sky blue
    /// under the fixed half greying, blue over red 1.47 to 1.63).
    #[test]
    fn the_veil_under_a_closed_deck_is_grey() {
        for kind in ["overcast", "fog", "rain", "storm", "snow"] {
            let env = Env::new(&sky_params(Some(kind)));
            for az in [30.0f32, 120.0, 300.0] {
                let c = env.radiance(dir_from_az_el(az, 1.0));
                assert!(c.z < 1.15 * c.x, "{kind} at the horizon, azimuth {az}: {c:?}");
            }
        }
    }

    /// The veil is lit by the dome as the map draws it (B17's fix round): on a
    /// fair sky that is the open dome's mean, as the brief's veil had it, and
    /// under a deck it is the deck's light, measured here on a finer grid of
    /// the dry sky than the map's own.
    #[test]
    fn the_veil_is_lit_by_the_dome_as_the_map_draws_it() {
        let hazy = Env::new(&sky_params(Some("hazy")));
        let (dome, open) = (luminance(hazy.weather_dome), luminance(hazy.cloud_ambient));
        assert!((dome / open - 1.0).abs() < 0.02, "a fair sky's dome is the open dome: {dome} vs {open}");
        for kind in ["overcast", "fog", "rain", "storm", "snow"] {
            let env = Env::new(&sky_params(Some(kind)));
            let atmo = env.atmo.as_ref().unwrap();
            let (rows, columns) = (24, 96);
            let mut sum = Vec3f::default();
            for j in 0..rows {
                let elevation = ((j as f32 + 0.5) / rows as f32).asin().to_degrees();
                for i in 0..columns {
                    sum += env.sky_layers(dir_from_az_el((i as f32 + 0.25) * 360.0 / columns as f32, elevation), atmo, Some(KeyTerm::Sun), None, true);
                }
            }
            let fine = luminance(sum * (1.0 / (rows * columns) as f32));
            let dome = luminance(env.weather_dome);
            assert!((dome / fine - 1.0).abs() < 0.05, "{kind}: the veil's dome {dome}, the dry sky's mean {fine}");
        }
    }

    /// Kind fog: the horizon is the fog's own light all round, no darker than
    /// the sky overhead, where the deck still shows through the thinner fog
    /// (B17's fix round: lit by the clear dome under a deck 45 % brighter than
    /// that, the horizon was the darkest band of the sky, sRGB 141 against
    /// 160 at the zenith).
    #[test]
    fn fog_lies_white_on_the_horizon() {
        let env = Env::new(&sky_params(Some("fog")));
        let ring = |el: f32| (0..36).map(|i| luminance(env.radiance(dir_from_az_el(i as f32 * 10.0 + 3.0, el)))).sum::<f32>() / 36.0;
        let (horizon, overhead) = (ring(1.0), ring(80.0));
        assert!(horizon > 0.98 * overhead, "{horizon} at the horizon, {overhead} overhead");
    }

    /// Fog 1 hides everything but its own light, its glow and the ground at
    /// the feet: wet ground no longer shows 3 degrees down (38 m off), which
    /// it brightens by about a quarter under the kind's own fog, and on a fair
    /// sky the sun's key is gone into it.
    #[test]
    fn fog_one_hides_the_distance_and_the_key() {
        let foggy = |kind: Option<&str>, fog: f32, wet: f32| {
            let mut p = sky_params(kind);
            let w = p.sky.weather.get_or_insert_with(WeatherParams::default);
            w.fog = fog;
            w.wet_ground = wet;
            Env::new(&p)
        };
        let ring = |env: &Env, el: f32| (0..36).map(|i| luminance(env.radiance(dir_from_az_el(i as f32 * 10.0 + 3.0, el)))).sum::<f32>() / 36.0;
        let change = |fog: f32| ring(&foggy(Some("fog"), fog, 1.0), -3.0) / ring(&foggy(Some("fog"), fog, 0.0), -3.0) - 1.0;
        let (kind, dense) = (change(0.0), change(1.0));
        assert!(kind.abs() > 0.05, "the kind's own fog shows the puddles: {kind}");
        assert!(dense.abs() < 0.01, "fog 1 hides them: {dense}");
        let clear = luminance(Env::new(&sky_params(None)).sun().unwrap().radiance);
        let key = foggy(Some("clear"), 1.0, 0.0).sun().map_or(0.0, |sun| luminance(sun.radiance));
        assert!(key < 1.0e-3 * clear, "{key} against {clear} on a clear day");
    }

    /// The virga shows in the map: the shafts darken the rain haze in front of
    /// the distance too, not only the distance behind it, which the haze hides
    /// (B17's visual check: 2 to 5 % along the ring when they stood behind it).
    #[test]
    fn rain_hangs_visible_shafts_under_the_deck() {
        let env = Env::new(&sky_params(Some("rain")));
        let ring: Vec<f32> = (0..360).map(|az| luminance(env.radiance(dir_from_az_el(az as f32, 15.0)))).collect();
        let lo = ring.iter().cloned().fold(f32::MAX, f32::min);
        let hi = ring.iter().cloned().fold(0.0, f32::max);
        assert!(hi > 1.08 * lo, "shafts and gaps 15 degrees up: {lo}..{hi}");
    }

    /// The wet ground mirrors the sky as the weather shows it, through the
    /// rain and the fog, so the ground a few degrees down is no brighter than
    /// the sky a few degrees up. (A mirror of the dry sky behind the veil
    /// reads 0.27 at -5 degrees under a storm, against 0.22 at +5.)
    #[test]
    fn the_wet_ground_mirrors_no_more_than_the_veiled_sky() {
        for kind in ["rain", "storm"] {
            let env = Env::new(&sky_params(Some(kind)));
            let mean = |el: f32| (0..36).map(|i| luminance(env.radiance(dir_from_az_el(i as f32 * 10.0 + 3.0, el)))).sum::<f32>() / 36.0;
            for el in [5.0f32, 10.0] {
                let (above, below) = (mean(el), mean(-el));
                assert!(below < above, "{kind} at {el} degrees: {above} above the horizon, {below} below it");
            }
        }
    }

    /// Plan 2's N3 under weather: the engine's lighting copy is the map
    /// without exactly the sun's disc (a whole bake through the lookup without
    /// the key), and the wet ground's reflection of the sun stays in it: that
    /// is light from below, which the directional light does not carry.
    #[test]
    fn a_wet_skys_lighting_copy_keeps_the_suns_reflection() {
        let mut p = sky_params(Some("clear"));
        p.sky.weather.as_mut().unwrap().wet_ground = 1.0;
        // A 2 degree disc: at 64 wide the refinement's 8 x 8 sub-samples lie
        // 0.7 degrees apart, and the real 0.53 degree disc at (180, 40) falls
        // between them, so neither the map nor the copy would show it (nor
        // its reflection).
        p.sky.sun_disc.size_deg = 2.0;
        let env = Env::new(&p);
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f);
        let map = env.bake(64);
        let keyless = env.bake_keyless_par(&map, serial).expect("the sun is the key");
        assert!(Some(&keyless) == env.bake_without_key(64).as_ref(), "a whole bake through the lookup without the key");
        let sun = dir_from_az_el(180.0, 40.0);
        let mirrored = vec3f(sun.x, -sun.y, sun.z);
        assert!(keyless.sample(crate::hdri::arr(sun))[0] < map.sample(crate::hdri::arr(sun))[0], "the disc left the copy");
        assert_eq!(keyless.sample(crate::hdri::arr(mirrored)), map.sample(crate::hdri::arr(mirrored)), "its reflection in the wet ground stayed");
    }
}
