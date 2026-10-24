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
//! - **Ground.** The ground is lit by the sky the map draws: under a closed
//!   deck by the deck's light, not by the clear sky's sun (`Env` measures the
//!   gain once per map, the plan owner's D7, wherever anything covers the
//!   sky, with or without a weather: [`relit_ground`]). Snow is a ground
//!   albedo of 0.8 (through the weathered ground colour). Wet ground mirrors
//!   the sky by water's Fresnel reflectance, the bolt included, as a dull
//!   sheen on damp ground and as a flat mirror in fbm puddles.
//! - **Rain.** Haze that greys the distance, with virga: shafts hanging under
//!   the deck toward the horizon, darker curtains over the haze and what is
//!   behind it. An environment map holds no streaks.
//! - **Fog.** A veil lying on the ground: thin overhead, opaque toward the
//!   horizon, glowing toward the sun; dense at 1, when only the ground at the
//!   feet shows through it. It spreads round the sky the sunlight it takes
//!   out of the sun's beam, and no more (D7).
//! - **Lightning.** One seeded bolt from the deck to the horizon and a flash
//!   lobe around its top that fades out at the horizon, standing inside the
//!   rain. A bake averages the channel over each texel, so the bolt's light
//!   does not depend on the bake's width (D7).
//!
//! The rain haze and the fog are lit by the dome as the map draws it (under a
//! deck, the deck's own light), greyed as the clouds grey it.
//!
//! Directions are in the map's own frame (before the map rotation); radiance
//! is in the atmosphere's units.

use makepad_draw::*;
use super::clouds::hg_phase;
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
/// light it spreads evenly by its many scatterings: the shape of its glow,
/// whose whole light is what the fog takes out of the sun's beam (D7).
const FOG_G: f32 = 0.6;
const FOG_DIFFUSE: f32 = 0.6;
/// The sphere the fog's glow is summed over once per map: rows equally spaced
/// in the sine of the elevation (equal solid angles), fine enough for the
/// slab's steep edge at the horizon, by columns of azimuth.
const FOG_SUM_ROWS: usize = 256;
const FOG_SUM_COLUMNS: usize = 64;

/// Rain 1 has this optical depth straight up, through a slab up to the deck.
const RAIN_ZENITH: f32 = 1.2;
const RAIN_RANGE_M: f32 = 12_000.0;
/// Rain is a dull grey of the ambient; falling snow is nearly white.
const RAIN_ALBEDO: f32 = 0.6;
const SNOWFALL_ALBEDO: f32 = 0.9;
/// The share of the light the rain and the fog take out of a light's beam that they scatter forward, close round it
/// (plan hdri-6 R4: lights shine through): a light seen through them keeps exp(-(1 - LIGHT_FORWARD) tau) of its light
/// (`light_through`), its glow, where a surface keeps exp(-tau). Fog and rain droplets are many wavelengths across and
/// scatter mostly forward; 0.75 is the brief's start, which shows the Rain kind's city lights 1-2 km off.
pub(crate) const LIGHT_FORWARD: f32 = 0.75;
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
/// How a sum over directions (a window key's integral, D1) reads the channel:
/// as a bake of texels CHANNEL_FOOTPRINT wide draws it (`lightning`'s
/// `texel`), the pieces added up as every bake adds them (a point lookup
/// joins them with round caps, which holds less light where the channel kinks
/// sharply: 1.5 % less of one seed's bolt), sampled CHANNEL_STEP apart where
/// it runs (`fine_step_near`). Averaged over four times the spacing, jittered
/// samples hold the bolt's light to 0.6 % for ten seeds; at half the core's
/// width for both they hold it to 0.55 % for four times the cost, and over the
/// footprint alone, at a key's own spacing, to 3 %. CHANNEL_REACH is how far
/// from the centre line the averaged core reaches: e^-16 of its peak 4 widths
/// out, plus half the footprint (the halo is smooth at a key's own spacing).
pub const CHANNEL_FOOTPRINT: f32 = 4.0 * CORE_SIGMA;
const CHANNEL_STEP: f32 = CORE_SIGMA;
const CHANNEL_REACH: f32 = 4.0 * CORE_SIGMA + 0.5 * CHANNEL_FOOTPRINT;
/// The bolt's plane (x = turn x cos(elevation), y = elevation) shears the sky
/// by turn x sin(elevation), under 0.5 where the channel runs: a distance in
/// it is at most this many times the angle between the two directions.
const CHANNEL_PLANE_SLACK: f32 = 1.5;
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
/// passes to the clouds' undersides. (The ground is lit by the sky the map
/// draws: `Env::measure_ground_gain`, `relit_ground`.)
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
};

fn preset(kind: WeatherKind) -> KindPreset {
    match kind {
        WeatherKind::Clear => FAIR,
        WeatherKind::Hazy => KindPreset { haze_min: 3.0, fog: 0.08, ..FAIR },
        WeatherKind::Overcast => KindPreset { haze_min: 1.5, coverage_min: 0.95, sharpness_max: 0.25, fog: 0.03, ..FAIR },
        WeatherKind::Fog => KindPreset {
            haze_min: 2.0,
            coverage_min: 0.9,
            sharpness_max: 0.2,
            fog: 0.7,
            wet_ground: 0.2,
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
    // As clamp() holds it: its ranges and defaults are written once (L:231).
    let w = &w.clamped();
    let kind = preset(w.kind());
    Effects {
        fog: w.fog.max(kind.fog),
        rain: w.rain.max(kind.rain),
        wet_ground: w.wet_ground.max(kind.wet_ground),
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
    /// The unit direction of a-b in the bolt's plane, and its length: a
    /// texel's extent along and across the segment is measured on them.
    along: [f32; 2],
    length: f32,
    /// 1 for the main channel, less along a fork.
    weight: f32,
}

impl BoltSegment {
    fn new(a: [f32; 2], b: [f32; 2], weight: f32) -> BoltSegment {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = (dx * dx + dy * dy).sqrt();
        let along = if length > 0.0 { [dx / length, dy / length] } else { [0.0, -1.0] };
        BoltSegment { a, b, along, length, weight }
    }

    /// Where `p` lies against the segment's line: how far along it from a,
    /// and how far across it.
    fn place(&self, p: [f32; 2]) -> (f32, f32) {
        let (rx, ry) = (p[0] - self.a[0], p[1] - self.a[1]);
        (rx * self.along[0] + ry * self.along[1], (ry * self.along[0] - rx * self.along[1]).abs())
    }

    /// How much of a texel centred `t` along the segment lies beside the
    /// segment's own run, the texel's square seen along the segment as the
    /// two sides `sides` long it projects to: the share of the texel this
    /// segment's piece of the channel fills, so the pieces of one channel add
    /// up to it along the texel.
    fn cover(&self, t: f32, sides: [f32; 2]) -> f32 {
        (trapezoid_step(t, sides) - trapezoid_step(t - self.length, sides)).clamp(0.0, 1.0)
    }
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
    let mut segments: Vec<BoltSegment> = points.windows(2).map(|pair| BoltSegment::new(pair[0], pair[1], 1.0)).collect();
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
            segments.push(BoltSegment::new(from, to, FORK_WEIGHT * (1.0 - 0.6 * j as f32 / length as f32)));
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
    ground_color: [f32; 3],
    /// The rain slab reaches up to the cloud base, metres.
    rain_height: f32,
    /// The cloud cover the map is drawn with (the kind's floor or the sky's
    /// own, whichever is more): it greys the fog's light as it greys the deck's.
    coverage: f32,
    sun_dir: Vec3f,
    /// The fog's glow per unit of the sun's irradiance under the clouds: the
    /// most that keeps its light within what the fog takes out of the beam
    /// (`fog_glow_share`, D7).
    fog_glow: f32,
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
        if idle && kind.cloud_gain == 1.0 {
            return None;
        }
        let sun = sun_dir.normalize();
        let sun = if sun.is_finite() && sun.length() > 0.5 { sun } else { vec3f(0.0, 1.0, 0.0) };
        // Rain falls from the cloud base: the volume's when there is one, else
        // the deck's, each as clamp() holds it (L:231).
        let clouds = sky.clouds.clamped();
        let base = sky.volumetric.as_ref().map_or(clouds.altitude_m, |vol| vol.clamped().base_m);
        let mut weather = Weather {
            effects: drawn,
            cloud_gain: kind.cloud_gain,
            ground_color: if drawn.snow { SNOW_ALBEDO } else { sky.atmosphere.ground_color },
            rain_height: base,
            coverage: clouds.coverage.max(kind.coverage_min),
            sun_dir: sun,
            fog_glow: 0.0,
            seed: params.seed,
            bolt: if drawn.lightning { Some(build_bolt(params.seed)) } else { None },
        };
        weather.fog_glow = weather.fog_glow_share();
        Some(weather)
    }

    /// The fog's glow per unit of the sun's irradiance (D7). The fog takes
    /// out of the sun's beam the share 1 - T of what reaches it through the
    /// rain and its shafts, and spreads that round the sky in the glow's
    /// shape: even, from its many scatterings, and forward toward the sun.
    /// Seen through the fog, the glow along d is its shape times 1 - T(d). The
    /// factor is the largest that keeps both measures of the light: the
    /// glow's whole light (summed over the sphere) is no more than the light
    /// taken out of the beam, so fog brightens no map, and the light it puts
    /// on a level surface no more than the beam's share of that, so it
    /// brightens no ground either. Untied, the glow brightened a fair day's
    /// map by up to 2.2 times and put up to 1.9 times the beam's lost light
    /// on the ground (L:259).
    fn fog_glow_share(&self) -> f32 {
        let sun = self.sun_dir;
        if self.effects.fog <= 0.0 || !(sun.y > 0.0) {
            return 0.0;
        }
        let taken = (-self.rain_depth(sun)).exp() * (1.0 - self.virga(sun)) * (1.0 - (-self.fog_depth(sun)).exp());
        let even = FOG_DIFFUSE * sun.y / std::f32::consts::PI;
        // The sun's horizontal direction, for the azimuth about the zenith.
        let level = (sun.x * sun.x + sun.z * sun.z).sqrt();
        let (mut whole, mut down) = (0.0f64, 0.0f64);
        for j in 0..FOG_SUM_ROWS {
            let y = -1.0 + 2.0 * (j as f32 + 0.5) / FOG_SUM_ROWS as f32;
            let across = (1.0 - y * y).max(0.0).sqrt();
            let veiled = 1.0 - (-self.fog_depth(vec3f(0.0, y, 0.0))).exp();
            let mut row = 0.0f32;
            for i in 0..FOG_SUM_COLUMNS {
                let phi = (i as f32 + 0.5) / FOG_SUM_COLUMNS as f32 * std::f32::consts::TAU;
                row += even + hg_phase(y * sun.y + across * level * phi.cos(), FOG_G);
            }
            whole += (row * veiled) as f64;
            if y > 0.0 {
                down += (row * veiled * y) as f64;
            }
        }
        // Each direction stands for an equal share of the sphere's 4 pi.
        let cell = 4.0 * std::f64::consts::PI / (FOG_SUM_ROWS * FOG_SUM_COLUMNS) as f64;
        let (whole, down) = (whole * cell, down * cell);
        if !(whole > 0.0 && down > 0.0) {
            return 0.0;
        }
        (taken as f64 * (1.0 / whole).min(sun.y as f64 / down)) as f32
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

    /// True when the veil does anything: rain or fog (`veil_with` skips each at 0, and then passes `c` itself).
    pub fn veils(&self) -> bool {
        self.effects.rain > 0.0 || self.effects.fog > 0.0
    }

    /// The ground below the horizon under this weather. `ground` is what the
    /// dry sky drew along `d`, lit by the clear sky; `gain` is what this map's
    /// sky does to that light (`Env` measures it: under a closed deck the
    /// deck's light over the clear sky's, D7); `sky_at` answers what the sky
    /// shows along a direction above the horizon (it is asked for the mirror
    /// image of `d`; `Env` passes the sky as the weather shows it, through the
    /// rain and fog and with the bolt, so the mirror is no brighter than the
    /// sky above it). Above the horizon `ground` passes unchanged.
    pub fn ground(&self, d: Vec3f, ground: Vec3f, gain: Vec3f, sky_at: &dyn Fn(Vec3f) -> Vec3f) -> Vec3f {
        self.ground_with(d, ground, gain, sky_at, Puddles::Field)
    }

    /// [`Self::ground`] with the puddles at their mean share everywhere (the
    /// share the far ground already draws), so a few looks round the horizon
    /// read the wet ground's mean, not whether they hit a puddle: what the
    /// skyline's strip takes for its ground (`Env::skyline_light`'s ground
    /// ring, the skirt's hold). The ground is linear in the puddles' weight,
    /// so this is the mean of `ground` over the puddles. A dry ground is
    /// `ground`'s, bit for bit.
    pub fn ground_even(&self, d: Vec3f, ground: Vec3f, gain: Vec3f, sky_at: &dyn Fn(Vec3f) -> Vec3f) -> Vec3f {
        self.ground_with(d, ground, gain, sky_at, Puddles::Even)
    }

    /// `ground` with no puddles: the damp ground between them, its dull sheen included. What the skyline's strip takes
    /// for its own ground under a wet ground: its near rise, and below the horizon the ground between the puddles (plan
    /// hdri-6 §5, fix wave 1). A dry ground is `ground`'s, bit for bit.
    pub fn ground_damp(&self, d: Vec3f, ground: Vec3f, gain: Vec3f, sky_at: &dyn Fn(Vec3f) -> Vec3f) -> Vec3f {
        self.ground_with(d, ground, gain, sky_at, Puddles::Dry)
    }

    /// `ground` all under water: a puddle's bottom and what it mirrors by Fresnel, wherever `d` looks. What the map
    /// shows in the puddles it lays over the skyline's strip where the strip gives way to its floor under a wet ground
    /// (`Env`'s `under_band`). A dry ground is `ground`'s, bit for bit.
    pub(crate) fn ground_water(&self, d: Vec3f, ground: Vec3f, gain: Vec3f, sky_at: &dyn Fn(Vec3f) -> Vec3f) -> Vec3f {
        self.ground_with(d, ground, gain, sky_at, Puddles::Full)
    }

    fn ground_with(&self, d: Vec3f, ground: Vec3f, gain: Vec3f, sky_at: &dyn Fn(Vec3f) -> Vec3f, puddles: Puddles) -> Vec3f {
        if !(d.y < 0.0) {
            return ground;
        }
        let down = (-d.y).min(1.0);
        let c = relit_ground(d, ground, gain);
        let wet = self.effects.wet_ground;
        if wet <= 0.0 {
            return c;
        }
        // Schlick's Fresnel for water: a mirror at grazing angles, 2 % head-on.
        let fresnel = WATER_F0 + (1.0 - WATER_F0) * (1.0 - down).powi(5);
        let mirror = sky_at(vec3f(d.x, -d.y, d.z));
        let puddle = self.puddle_cover(d, puddles);
        // Damp ground between the puddles: darker, with a dull sheen. A puddle:
        // a flat mirror over what shows of its bottom.
        let damp = c * (1.0 - WET_DARKEN * wet) + mirror * (fresnel * WET_SHEEN * wet);
        let water = c * (PUDDLE_BOTTOM * (1.0 - fresnel)) + mirror * fresnel;
        damp.mix(water, puddle)
    }

    /// How much of what the ground along `d` mirrors `ground_with` lets through (`ground`, `ground_even`, `ground_damp`
    /// or `ground_water` by `puddles`): the damp ground's sheen between the puddles, Fresnel's term in them, by their
    /// share. The ground is affine in the mirror, and this is its slope; 0 above the horizon or on a dry ground. What
    /// `Env` takes out of the floor's veil where the mirror shows the skyline's strip, which is veiled over its own
    /// distance instead (plan hdri-6 R1).
    pub(crate) fn mirror_share(&self, d: Vec3f, puddles: Puddles) -> f32 {
        let wet = self.effects.wet_ground;
        if wet <= 0.0 || !(d.y < 0.0) {
            return 0.0;
        }
        let down = (-d.y).min(1.0);
        let fresnel = WATER_F0 + (1.0 - WATER_F0) * (1.0 - down).powi(5);
        let damp = fresnel * WET_SHEEN * wet;
        damp + (fresnel - damp) * self.puddle_cover(d, puddles)
    }

    /// The puddles' share of the ground along `d` (0..1; 0 above the horizon or on a dry ground).
    pub(crate) fn puddle_cover(&self, d: Vec3f, puddles: Puddles) -> f32 {
        let wet = self.effects.wet_ground;
        if wet <= 0.0 || !(d.y < 0.0) || puddles == Puddles::Dry {
            return 0.0;
        }
        if puddles == Puddles::Full {
            return 1.0;
        }
        let down = (-d.y).min(1.0);
        // Far away the puddles are smaller than a texel: their mean share
        // instead, so the horizon does not sparkle.
        let share = self.puddle_share();
        if puddles == Puddles::Even {
            return share;
        }
        // Standing water: fbm over the ground plane, an eye height below.
        let reach = EYE_M / down.max(1.0e-3);
        let puddle_noise = fbm2(d.x * reach / PUDDLE_M, d.z * reach / PUDDLE_M, 4, self.seed ^ 0x9d01_e5);
        // More of the ground is under water the wetter it is (`puddle_threshold`), and past 0.9 the water rises between
        // the puddles (`puddle_flood`), as in their share.
        let threshold = puddle_threshold(wet);
        let puddle = flooded(smooth(threshold, threshold + 0.06, puddle_noise), puddle_flood(wet));
        let distance = reach * (1.0 - down * down).max(0.0).sqrt();
        let detail = 1.0 - smooth(PUDDLE_FADE_START_M, PUDDLE_FADE_END_M, distance);
        share + (puddle - share) * detail
    }

    /// The puddles' mean share (the far ground's), 0 on a dry ground, 1 at Wet ground 1.00 and only there.
    pub fn puddle_share(&self) -> f32 {
        let wet = self.effects.wet_ground;
        if wet <= 0.0 {
            return 0.0;
        }
        // the fbm is near normal (mean 0.49, deviation 0.12): the share of it over the threshold, its edge's middle; then
        // the water risen between them (`puddle_flood`)
        let threshold = puddle_threshold(wet);
        flooded(smooth(-2.0, 2.0, (0.49 - (threshold + 0.03)) / 0.12), puddle_flood(wet))
    }

    /// The lightning along `d`: the bolt's channel and the flash around its
    /// top, already dimmed by half the veil's depth where they stand (the bolt
    /// stands inside the rain, not behind it). Zero without a bolt. `texel` is
    /// the angular size of a bake's texels (radians): the channel, far thinner
    /// than a texel below 4K, is averaged over the texel there, so the bolt
    /// holds its light whatever the bake's width (D7). 0, a lookup along one
    /// exact direction, is the channel itself.
    pub fn lightning(&self, d: Vec3f, texel: f32) -> Vec3f {
        let Some(bolt) = &self.bolt else {
            return Vec3f::default();
        };
        let mut light = 0.0f32;
        let cos_flash = d.dot(bolt.flash_dir);
        if cos_flash > FLASH_COS_MIN {
            let angle = cos_flash.min(1.0).acos() / FLASH_SIGMA;
            // The flash is the cloud lit from inside: it fades out over the
            // atmosphere's blend from the horizon haze into the ground (D7).
            light += FLASH * (-0.5 * angle * angle).exp() * (1.0 - smooth(0.0, GROUND_BLEND, -d.y));
        }
        if light > 0.0 {
            light *= self.haze_transmittance(d).sqrt();
        }
        let texel = if texel.is_finite() { texel.max(0.0) } else { 0.0 };
        let elevation = d.y.clamp(-1.0, 1.0).asin();
        if elevation > -0.02 - texel && elevation < bolt.top + 0.05 + texel {
            let tau = std::f32::consts::TAU;
            let mut turn = d.x.atan2(-d.z) - bolt.azimuth;
            // Into -pi..pi: the bolt may stand on either side of north.
            turn -= tau * (turn / tau).round();
            // In the bolt's plane a texel is texel x cos(elevation) wide and texel high.
            let across = elevation.cos();
            let x = turn * across;
            if x.abs() < BOLT_REACH + texel {
                let p = [x, elevation];
                if texel > 0.0 {
                    // A bake's texel (D7): each segment's piece of the
                    // channel, averaged over the texel's square (which
                    // projects to two sides across the segment and two along
                    // it), and the pieces add up, so a wiggle of the channel
                    // inside one texel does not widen it. Each piece is dimmed
                    // by the haze where it stands: a texel that reaches below
                    // the horizon sees the bolt's foot through the haze over
                    // the foot, not through the few metres down to the ground.
                    let (wide, high) = (texel * across, texel);
                    for segment in &bolt.segments {
                        // How far along the segment's line and across it (its
                        // ends are the cover's).
                        let (t, off) = segment.place(p);
                        let (u, v) = (segment.along[0].abs(), segment.along[1].abs());
                        let cover = segment.cover(t, [wide * u, high * v]);
                        if !(cover > 0.0) {
                            continue;
                        }
                        let sides = [wide * v, high * u];
                        let glow = BOLT_CORE * texel_gauss(off, CORE_SIGMA, sides) + BOLT_HALO * texel_gauss(off, HALO_SIGMA, sides);
                        let seen = segment.weight * glow * cover;
                        if seen > 0.0 {
                            let standing = segment.a[1] + segment.along[1] * t.clamp(0.0, segment.length);
                            light += seen * self.haze_through(standing.sin()).sqrt();
                        }
                    }
                } else {
                    // One direction: the nearest segment's profile, which joins
                    // the segments with round caps.
                    let mut channel = 0.0f32;
                    for segment in &bolt.segments {
                        let distance = segment_distance(p, segment.a, segment.b);
                        let glow = BOLT_CORE * gauss(distance, CORE_SIGMA) + BOLT_HALO * gauss(distance, HALO_SIGMA);
                        channel = channel.max(segment.weight * glow);
                    }
                    if channel > 0.0 {
                        light += channel * self.haze_transmittance(d).sqrt();
                    }
                }
            }
        }
        if !(light > 0.0) {
            return Vec3f::default();
        }
        vec3f(BOLT_COLOR[0], BOLT_COLOR[1], BOLT_COLOR[2]) * light
    }

    /// The spacing a sum over directions must sample this weather's drawing
    /// at within `radius` (radians) of `d`: the channel's (`CHANNEL_STEP`)
    /// where the bolt's channel, or its mirror image in wet ground, may pass;
    /// None where the weather draws nothing thinner than its haze and flash.
    /// A window filled with sky as the key integrates the sky it lets in so
    /// (D1, the review's item 4), reading it averaged over
    /// `CHANNEL_FOOTPRINT`: at its cells' own spacing, 0.3 degrees, the 0.12
    /// degree channel held 0.95 to 1.06 of the light the bake draws.
    /// Conservative: it may say Some a little further out.
    pub fn fine_step_near(&self, d: Vec3f, radius: f32) -> Option<f32> {
        let bolt = self.bolt.as_ref()?;
        let within = CHANNEL_PLANE_SLACK * radius.max(0.0) + CHANNEL_REACH;
        let near = |d: Vec3f| {
            let elevation = d.y.clamp(-1.0, 1.0).asin();
            let tau = std::f32::consts::TAU;
            let mut turn = d.x.atan2(-d.z) - bolt.azimuth;
            turn -= tau * (turn / tau).round();
            let p = [turn * elevation.cos(), elevation];
            bolt.segments.iter().any(|segment| segment_distance(p, segment.a, segment.b) < within)
        };
        (near(d) || (self.mirrors() && near(vec3f(d.x, -d.y, d.z)))).then_some(CHANNEL_STEP)
    }

    /// The veil between the eye and what is drawn along `d`: rain haze, its
    /// virga shafts over the haze and what is behind it, then fog.
    /// `sun_irradiance` is the sun's light under the clouds (already through
    /// the cover along the sun), `ambient` the mean radiance of the dome above
    /// the veil as the map draws it, the clouds and their light included
    /// (`Env` measures it once per map).
    pub fn veil(&self, d: Vec3f, c: Vec3f, sun_irradiance: Vec3f, ambient: Vec3f) -> Vec3f {
        self.veil_with(d, c, sun_irradiance, ambient, true, f32::INFINITY)
    }

    /// `veil` without the rain's shafts: the rain haze and the fog only, over their whole path along `d`. What stands in
    /// front of the shafts, the skyline's strip, is veiled so (plan hdri-6 §5b) where its distance is not known
    /// (`veil_within`).
    pub fn veil_without_virga(&self, d: Vec3f, c: Vec3f, sun_irradiance: Vec3f, ambient: Vec3f) -> Vec3f {
        self.veil_with(d, c, sun_irradiance, ambient, false, f32::INFINITY)
    }

    /// `veil_without_virga` in front of what stands `far_m` (metres) along `d`: the rain haze and the fog over their path
    /// to it, each slab's path capped at `far_m`, never more than the whole path; the same colours scatter in. What the
    /// skyline's strip shows is veiled so, over its own distance (plan hdri-6 R1): the city 1-3 km off stays visible
    /// under rain, the far ranges veiled more. At or beyond the path this is `veil_without_virga`, bit for bit (the
    /// capped path is the path itself); still affine in `c`.
    pub fn veil_within(&self, d: Vec3f, c: Vec3f, sun_irradiance: Vec3f, ambient: Vec3f, far_m: f32) -> Vec3f {
        self.veil_with(d, c, sun_irradiance, ambient, false, far_m)
    }

    /// The share of a light's own light that reaches the eye through the rain and the fog from `far_m` (metres) along `d`
    /// (plan hdri-6 R4): exp(-(1 - LIGHT_FORWARD) tau), tau their optical depth over the path `veil_within` takes (each
    /// slab's path capped at `far_m`, never more than the whole path; INFINITY: the whole path). What they scatter
    /// forward stays round the light, so a light keeps more than a surface's exp(-tau), never more than F of what it
    /// lost; the haze in front of it is the surface's, added once (`Env::base_at`). 1 where nothing veils.
    pub fn light_through(&self, d: Vec3f, far_m: f32) -> f32 {
        let mut tau = 0.0;
        if self.effects.rain > 0.0 {
            tau += self.rain_depth_within(d, far_m);
        }
        if self.effects.fog > 0.0 {
            tau += self.fog_depth_within(d, far_m);
        }
        (-(1.0 - LIGHT_FORWARD) * tau).exp()
    }

    /// `veil`'s body; `shafts` lays the rain's virga over the haze, and the slabs' paths are capped at `far_m`.
    fn veil_with(&self, d: Vec3f, c: Vec3f, sun_irradiance: Vec3f, ambient: Vec3f, shafts: bool, far_m: f32) -> Vec3f {
        let mut c = c;
        // The air under a deck is lit by what that deck passes down, as the
        // clouds' own undersides are: `ambient` is that deck's light, the
        // kind's cloud light included, so a storm's rain is dark.
        let grey = luminance(ambient);
        if self.effects.rain > 0.0 {
            let through = (-self.rain_depth_within(d, far_m)).exp();
            let haze = grey * if self.effects.snow { SNOWFALL_ALBEDO } else { RAIN_ALBEDO };
            c = c * through + vec3f(haze, haze, haze) * (1.0 - through);
            // Shafts of rain under the deck are denser curtains of it: they
            // darken the distance and the rain haze before it alike.
            if shafts {
                c = c * (1.0 - self.virga(d));
            }
        }
        if self.effects.fog > 0.0 {
            let through = (-self.fog_depth_within(d, far_m)).exp();
            // Fog is lit by the dome, greyed as the clouds grey it (half on a
            // clear day, all of it under a closed deck), and by the sun:
            // evenly, from the many scatterings inside it, and with a glow
            // toward the sun.
            let sky = ambient.mix(vec3f(grey, grey, grey), 0.5 + 0.5 * self.coverage);
            // The glow's whole light is the sunlight the fog takes out of the
            // beam (`fog_glow_share`): it spreads the sun's light, it makes none.
            let even = FOG_DIFFUSE * self.sun_dir.y.max(0.0) / std::f32::consts::PI;
            let glow = hg_phase(d.dot(self.sun_dir), FOG_G);
            c = c * through + (sky + sun_irradiance * (self.fog_glow * (even + glow))) * (1.0 - through);
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

    /// `haze_transmittance` along a ray of elevation sine `sin_elevation`:
    /// the rain and the fog are level slabs, so the elevation is all it needs.
    fn haze_through(&self, sin_elevation: f32) -> f32 {
        self.haze_transmittance(vec3f(0.0, sin_elevation, 0.0))
    }

    fn fog_depth(&self, d: Vec3f) -> f32 {
        self.fog_depth_within(d, f32::INFINITY)
    }

    fn rain_depth(&self, d: Vec3f) -> f32 {
        self.rain_depth_within(d, f32::INFINITY)
    }

    /// `fog_depth` and `rain_depth` over the path along `d` capped at `far_m` (`slab_depth`).
    fn fog_depth_within(&self, d: Vec3f, far_m: f32) -> f32 {
        slab_depth(d.y, thickened(self.effects.fog) * FOG_ZENITH, FOG_HEIGHT_M, FOG_RANGE_M, far_m)
    }

    fn rain_depth_within(&self, d: Vec3f, far_m: f32) -> f32 {
        slab_depth(d.y, self.effects.rain * RAIN_ZENITH, self.rain_height, RAIN_RANGE_M, far_m)
    }

    /// How much a shaft of virga darkens along `d`: 0..VIRGA_DARKEN. Shafts
    /// are a function of the azimuth alone (leaned a little by the wind), so
    /// they hang straight; they show between the horizon and the deck.
    pub(super) fn virga(&self, d: Vec3f) -> f32 {
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

/// The puddle noise's threshold at wetness `wet`. Up to 0.5 the old 0.72 - 0.34 wet, bit for bit (the fog kind's 0.2).
/// Toward 0.9 it drops 0.04 more (a little more water under the rain and storm kinds). Past 0.9 the water rises between
/// the puddles instead (`puddle_flood`).
pub(crate) fn puddle_threshold(wet: f32) -> f32 {
    // up to 0.5 the extra term is 0, and a - 0 is a: the old threshold bit for bit
    0.72 - 0.34 * wet - 0.04 * smooth(0.5, 0.9, wet)
}

/// How far the water has risen over the ground between the puddles at wetness `wet`: 0 up to 0.9, all of it at 1.00,
/// growing with each step between (((wet - 0.9) / 0.1)^2: a quarter at 0.95, 0.81 at 0.99). The puddles' share then
/// grows step by step to the whole floor, reached only at 1.00 (plan hdri-6 §5); the threshold alone would cover the
/// floor from about 0.96 (its noise share saturates there), and the slider's last steps would change nothing.
pub(crate) fn puddle_flood(wet: f32) -> f32 {
    let top = ((wet - 0.9) / 0.1).clamp(0.0, 1.0);
    top * top
}

/// A cover `cover` with the water risen by `flood` (`puddle_flood`) over the rest: `cover` itself at 0 (bit for bit),
/// exactly 1 at 1.
fn flooded(cover: f32, flood: f32) -> f32 {
    if flood > 0.0 { 1.0 - (1.0 - cover) * (1.0 - flood) } else { cover }
}

/// How a wet ground's puddles are read: their field (the map's own), their mean share everywhere (`ground_even`), none
/// (`ground_damp`), or everywhere (`ground_water`: the floor's water, what shows where the skyline's strip gives way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Puddles {
    Field,
    Even,
    Dry,
    Full,
}

/// The ground the dry sky drew along `d` (`ground`), lit by the clear sky,
/// taken to this map's light by `gain` (`Env` measures it once per map: under
/// a closed deck the deck's light over the clear sky's, D7; exactly 1 when
/// nothing covers the sky). The atmosphere and the night lit their ground
/// with the clear sky and its discs; the gain follows the atmosphere's own
/// blend from the horizon haze into the ground. Above the horizon `ground`
/// passes unchanged. With or without a weather: the clouds alone can close
/// the deck.
pub fn relit_ground(d: Vec3f, ground: Vec3f, gain: Vec3f) -> Vec3f {
    if !(d.y < 0.0) {
        return ground;
    }
    let down = (-d.y).min(1.0);
    let one = Vec3f::all(1.0);
    ground * (one + (gain - one) * smooth(0.0, GROUND_BLEND, down))
}

/// Optical depth along a ray of elevation sine `sin_elevation` through a level
/// slab `height_m` thick whose depth straight up is `zenith_depth`, over its
/// path (`slab_path`) up to `far_m`: what stands nearer is seen through only
/// that much of the slab. `f32::INFINITY` (or any `far_m` at least the path)
/// is the whole path, bit for bit (`capped`).
fn slab_depth(sin_elevation: f32, zenith_depth: f32, height_m: f32, range_m: f32, far_m: f32) -> f32 {
    if !(zenith_depth > 0.0) {
        return 0.0;
    }
    zenith_depth / height_m * capped(slab_path(sin_elevation, height_m, range_m), far_m)
}

/// `path` capped at `far_m`: `far_m` where it is shorter, else `path` itself, the very float (also a NaN path, which
/// `f32::min` would turn into `far_m`; a NaN `far_m` caps nothing).
fn capped(path: f32, far_m: f32) -> f32 {
    if far_m < path { far_m } else { path }
}

/// The path (m) a ray of elevation sine `sin_elevation` runs through a level
/// slab `height_m` thick. A level path is capped at `range_m`; below the
/// horizon the ray ends on the ground, an eye height below, once the
/// atmosphere's ground blend (GROUND_BLEND) is past. The two branches meet at
/// the horizon.
fn slab_path(sin_elevation: f32, height_m: f32, range_m: f32) -> f32 {
    if sin_elevation >= 0.0 {
        height_m / (sin_elevation + height_m / range_m)
    } else {
        // Just below the horizon the atmosphere still draws distant haze (its
        // ground blend): that is as far off as the horizon itself. The path
        // shortens to the ground an eye height below as the ground takes over.
        let ground = EYE_M / (-sin_elevation + EYE_M / range_m);
        range_m + (ground - range_m) * smooth(0.0, GROUND_BLEND, -sin_elevation)
    }
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

/// `gauss` across a line, averaged over a texel whose square projects onto
/// the line's normal as two sides `sides` long: the profile convolved with
/// both boxes (the square's shadow on the normal is their convolution, a
/// trapezoid), at `distance` from the line (D7). Its integral across the line
/// is `gauss`'s, sigma x sqrt(pi), at every texel size, so a bake holds the
/// line's light whatever its width; sides under a twentieth of sigma are the
/// profile itself. Computed through the profile's second antiderivative,
/// four terms in f64 (they cancel to the few digits that remain).
fn texel_gauss(distance: f32, sigma: f32, sides: [f32; 2]) -> f32 {
    let (a, b) = (sides[0].max(sides[1]) as f64, sides[0].min(sides[1]) as f64);
    let s = sigma as f64;
    if !(a > 0.05 * s) {
        return gauss(distance, sigma);
    }
    let z = distance.abs() as f64;
    // Nothing of the line reaches the trapezoid (e^-36).
    if (z - 0.5 * (a + b)) / s > 6.0 {
        return 0.0;
    }
    // The profile's first antiderivative is s sqrt(pi)/2 erf(x/s); its second:
    let second = |x: f64| x * erf(x / s) + s / std::f64::consts::PI.sqrt() * (-(x / s) * (x / s)).exp();
    let k = 0.5 * s * std::f64::consts::PI.sqrt();
    let value = if b > 0.05 * s {
        k * (second(z + 0.5 * (a + b)) - second(z + 0.5 * (a - b)) - second(z - 0.5 * (a - b)) + second(z - 0.5 * (a + b))) / (a * b)
    } else {
        // One box: the first antiderivative's difference.
        k * (erf((z + 0.5 * a) / s) - erf((z - 0.5 * a) / s)) / a
    };
    value.max(0.0) as f32
}

/// The share of a texel that lies past a point of a line, the texel centred
/// `t` beyond the point and its square projected onto the line as two sides
/// `sides` long (a trapezoid of unit area): 0 well before the point, 1 well
/// past it, a smooth piecewise ramp between. The cover of a run [0, L] is the
/// step at t less the step at t - L.
fn trapezoid_step(t: f32, sides: [f32; 2]) -> f32 {
    let (a, b) = (sides[0].max(sides[1]) as f64, sides[0].min(sides[1]) as f64);
    let t = t as f64;
    if !(a > 0.0) {
        return if t >= 0.0 { 1.0 } else { 0.0 };
    }
    let step = if b > 1.0e-6 * a {
        // The step's second antiderivative is max(x, 0)^2 / 2.
        let q = |x: f64| 0.5 * x.max(0.0) * x.max(0.0);
        (q(t + 0.5 * (a + b)) - q(t + 0.5 * (a - b)) - q(t - 0.5 * (a - b)) + q(t - 0.5 * (a + b))) / (a * b)
    } else {
        (t / a + 0.5).clamp(0.0, 1.0)
    };
    step.clamp(0.0, 1.0) as f32
}

/// The error function (Abramowitz and Stegun 7.1.26, within 1.5e-7).
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let value = 1.0 - poly * (-x * x).exp();
    if x < 0.0 { -value } else { value }
}

/// smoothstep under its own name, so no glob can make it ambiguous.
fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
        let linear = slab_depth(1.0, 0.7 * FOG_ZENITH, FOG_HEIGHT_M, FOG_RANGE_M, f32::INFINITY);
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
        let on_channel = luminance(storm.lightning(at(0.0, bolt.top), 0.0));
        let above = luminance(storm.lightning(at(0.0, bolt.top + 0.06), 0.0));
        assert!(above > 0.0, "the flash lights the cloud around the channel's top");
        assert!(on_channel > 50.0 * above, "{on_channel} vs {above}");
        // Far from the bolt there is no light at all.
        assert_eq!(storm.lightning(dir_from_az_el(bolt.azimuth.to_degrees() + 180.0, 20.0), 0.0), Vec3f::default());
        // The same seed draws the same bolt; another seed strikes elsewhere.
        assert_eq!(weather("storm").bolt.as_ref().unwrap().azimuth, bolt.azimuth);
        let mut elsewhere = sky_with("storm");
        elsewhere.weather.as_mut().unwrap().seed = 2;
        let other = Weather::new(&elsewhere, sun()).unwrap();
        assert_ne!(other.bolt.as_ref().unwrap().azimuth, bolt.azimuth);
        // Rain alone has no lightning; the tick adds a bolt to any weather.
        assert!(weather("rain").bolt.is_none());
        assert_eq!(weather("rain").lightning(at(0.0, bolt.top), 0.0), Vec3f::default());
        let mut ticked = sky_with("clear");
        ticked.weather.as_mut().unwrap().lightning = true;
        assert!(Weather::new(&ticked, sun()).unwrap().bolt.is_some());
    }

    /// D7 (L:245): the flash is the cloud round the bolt's top lit from
    /// inside, so it fades out over the atmosphere's ground blend below the
    /// horizon (3 degrees). It reached about 24 degrees down, where the near
    /// ground glowed at several times the storm ground's own light.
    #[test]
    fn the_flash_fades_at_the_horizon() {
        let storm = weather("storm");
        let bolt = storm.bolt.as_ref().unwrap();
        // Twenty degrees beside the channel only the flash lights the sky.
        let beside = bolt.azimuth.to_degrees() + 20.0;
        let at = |el: f32| luminance(storm.lightning(dir_from_az_el(beside, el), 0.0));
        assert!(at(0.0) > 0.0 && at(5.0) > 0.0, "the flash lights the sky by the bolt");
        for el in [-3.5f32, -5.0, -10.0, -20.0] {
            assert_eq!(at(el), 0.0, "{el} degrees down");
        }
        // No step: from 4 degrees down to 4 up no 0.01 degrees changes it by
        // 3 % of the most it shows there.
        let sweep: Vec<f32> = (0..=800).map(|i| at(-4.0 + 0.01 * i as f32)).collect();
        let most = sweep.iter().cloned().fold(0.0, f32::max);
        for (i, pair) in sweep.windows(2).enumerate() {
            assert!((pair[1] - pair[0]).abs() < 0.03 * most, "{} degrees: {} -> {}", -4.0 + 0.01 * (i + 1) as f32, pair[0], pair[1]);
        }
    }

    /// D7 (M7): the bolt's channel, 0.12 degrees across with a 0.5 degree
    /// halo, is averaged over each texel of a bake (`lightning`'s `texel`),
    /// so a storm map holds the bolt's light at every width; sampled at the
    /// texel centres alone it held -41 to +23 % of it from 256 to 512 wide
    /// (the review's probe; 0.45 at 64 wide here). The light of the bolt and
    /// its flash in a bake of each width from 96 to 1024, against a 2048
    /// wide one, for six seeds, and what a studio window filled with sky shows
    /// of it from 128 to 1024, for two. (At 64 wide, 5.6 degree texels, it
    /// holds 0.94 to 1.03: the channel kinks more finely than such a texel.)
    #[test]
    fn the_bolts_light_does_not_change_with_the_bake_width() {
        /// How far a bake's bolt may stray from the finest one's: a straight
        /// channel holds its light exactly, a kinked one to the grid's
        /// sampling of its joints (2.5 % at 128 wide, 1.3 % from 192).
        const BOLT_LIGHT_TOLERANCE: f64 = 0.03;
        let pi = std::f32::consts::PI;
        for seed in [1u32, 2, 3, 4, 5, 6] {
            let mut sky = sky_with("storm");
            sky.weather.as_mut().unwrap().seed = seed;
            let storm = Weather::new(&sky, sun()).unwrap();
            let light = |width: usize| -> f64 {
                let height = width / 2;
                let texel = std::f32::consts::TAU / width as f32;
                let mut sum = 0.0f64;
                for y in 0..height {
                    let v = (y as f32 + 0.5) / height as f32;
                    let area = (texel * (pi / height as f32) * (v * pi).sin()) as f64;
                    for x in 0..width {
                        let d = vec3f_of(makepad_render_material::ibl::equirect_uv_to_dir([(x as f32 + 0.5) / width as f32, v]));
                        sum += luminance(storm.lightning(d, texel)) as f64 * area;
                    }
                }
                sum
            };
            let finest = light(2048);
            for width in [96usize, 128, 192, 256, 384, 512, 1024] {
                let ratio = light(width) / finest;
                assert!((ratio - 1.0).abs() < BOLT_LIGHT_TOLERANCE, "seed {seed}, {width} wide: {ratio:.4} of the bolt's light");
            }
        }
        // And through a studio window filled with sky, which shows the
        // lightless Sky-mode copy behind the studio (the review's item 4): a
        // bake hands the copy its texel too. Point-sampled there, the window
        // held 0.59 to 1.16 of the finest bake's light from 128 to 1024 wide
        // (0.975 to 1.002 now). The light over the window's box (a black
        // backdrop, one pane, no frame: the sky and the bolt it shows), at each
        // width against the finest.
        for seed in [1u32, 2] {
            let mut sky = sky_with("storm");
            sky.weather.as_mut().unwrap().seed = seed;
            let bolt = Weather::new(&sky, sun()).unwrap().bolt.unwrap();
            let (azimuth, elevation) = (bolt.azimuth.to_degrees(), 17.0f32);
            let mut p = crate::hdri::HdriParams { mode: "studio".to_string(), sky, ..crate::hdri::HdriParams::default() };
            p.studio.top = [0.0; 3];
            p.studio.horizon = [0.0; 3];
            p.studio.floor = [0.0; 3];
            p.lights = vec![crate::hdri::LightParams {
                azimuth_deg: azimuth,
                elevation_deg: elevation,
                width_deg: 36.0,
                height_deg: 40.0,
                softness: 0.05,
                intensity_ev: 0.0,
                gobo: Some(crate::hdri::GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, fill_sky: true, ..crate::hdri::GoboParams::default() }),
                ..crate::hdri::LightParams::default()
            }];
            let env = crate::hdri::Env::new(&p);
            let light = |width: usize| -> f64 {
                let height = width / 2;
                let texel = std::f32::consts::TAU / width as f32;
                let at = crate::hdri::Sampling::bake(width, None);
                let mut sum = 0.0f64;
                for y in 0..height {
                    let v = (y as f32 + 0.5) / height as f32;
                    let el = 90.0 - v * 180.0;
                    if !(-5.0..=elevation + 24.0).contains(&el) {
                        continue;
                    }
                    let area = (texel * (pi / height as f32) * (v * pi).sin()) as f64;
                    for x in 0..width {
                        let d = vec3f_of(makepad_render_material::ibl::equirect_uv_to_dir([(x as f32 + 0.5) / width as f32, v]));
                        let mut turn = d.x.atan2(-d.z).to_degrees() - azimuth;
                        turn -= 360.0 * (turn / 360.0).round();
                        if turn.abs() > 24.0 {
                            continue;
                        }
                        sum += luminance(env.radiance_with(d, None, at)) as f64 * area;
                    }
                }
                sum
            };
            let finest = light(2048);
            for width in [128usize, 256, 512, 1024] {
                let ratio = light(width) / finest;
                assert!((ratio - 1.0).abs() < BOLT_LIGHT_TOLERANCE, "seed {seed}, a studio window {width} wide: {ratio:.4} of its light");
            }
        }
    }

    fn vec3f_of(d: [f32; 3]) -> Vec3f {
        vec3f(d[0], d[1], d[2])
    }

    /// `mirror_share` is the slope of the ground in what it mirrors (`ground_with` is affine in it), for every way the
    /// puddles are read, on a damp, a puddled, a flooding and a flooded floor, near the horizon and near the feet.
    #[test]
    fn the_mirror_share_is_the_grounds_slope_in_its_mirror() {
        for wet_ground in [0.3, 0.7, 0.95, 1.0] {
            let mut sky = sky_with("clear");
            sky.weather.as_mut().unwrap().wet_ground = wet_ground;
            let w = Weather::new(&sky, sun()).unwrap();
            let one = Vec3f::all(1.0);
            for puddles in [Puddles::Field, Puddles::Even, Puddles::Dry] {
                for i in 0..48 {
                    let d = dir_from_az_el(i as f32 * 7.5, -[0.3, 1.0, 2.0, 5.0, 12.0, 40.0][i % 6]);
                    let at = |m: f32| w.ground_with(d, vec3f(0.2, 0.15, 0.1), one, &|_| Vec3f::all(m), puddles);
                    let slope = (at(10.0) - at(0.0)).x / 10.0;
                    let share = w.mirror_share(d, puddles);
                    assert!((slope - share).abs() <= 1e-5, "wet {wet_ground}, {puddles:?}, {d:?}: the slope {slope}, the share {share}");
                }
            }
            assert_eq!(w.mirror_share(dir_from_az_el(10.0, 5.0), Puddles::Field), 0.0, "above the horizon nothing is mirrored");
        }
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
        let one = Vec3f::all(1.0);
        let down = wet.ground(vec3f(0.0, -1.0, 0.0), ground, one, &sky);
        assert!(down.x < 0.2 && down.z < 0.4, "{down:?}");
        // At a grazing angle the sky takes over.
        let grazing = wet.ground(dir_from_az_el(30.0, -1.0), ground, one, &sky);
        assert!(grazing.z > 4.0, "{grazing:?}");
        // Puddles: around the feet some spots mirror far more than others (below Wet ground 1.00, which is all water).
        let mut puddled = sky_with("clear");
        puddled.weather.as_mut().unwrap().wet_ground = 0.7;
        let wet = Weather::new(&puddled, sun()).unwrap();
        let near: Vec<f32> = (0..72).map(|i| wet.ground(dir_from_az_el(i as f32 * 5.0, -20.0), ground, one, &sky).z).collect();
        let lo = near.iter().cloned().fold(f32::MAX, f32::min);
        let hi = near.iter().cloned().fold(0.0, f32::max);
        assert!(hi > 1.3 * lo, "puddles and damp ground: {lo}..{hi}");
        // Above the horizon nothing is ground.
        assert_eq!(wet.ground(dir_from_az_el(30.0, 10.0), ground, vec3f(0.2, 0.3, 0.4), &sky), ground);
        // Dry weather leaves the ground alone, apart from the light this sky
        // puts on it (the gain `Env` measures), past the horizon's blend.
        assert_eq!(weather("hazy").ground(vec3f(0.0, -1.0, 0.0), ground, one, &sky), ground);
        let gain = vec3f(0.2, 0.3, 0.4);
        let lit = weather("overcast").ground(vec3f(0.0, -1.0, 0.0), ground, gain, &|_| Vec3f::default());
        assert!((lit - ground * gain).length() < 1.0e-6, "{lit:?}");
        let at_the_horizon = weather("hazy").ground(dir_from_az_el(0.0, -0.01), ground, gain, &sky);
        assert!((at_the_horizon - ground).length() < 1.0e-3, "{at_the_horizon:?}");
    }

    /// Plan hdri-6 §5: below 1.00 the puddles grow with wetness as before. Up to 0.5 the threshold is the old one, bit for
    /// bit; the rain and storm kinds get a little more water; at 1.00 the whole floor is water, and only there.
    #[test]
    fn the_puddles_grow_as_before_and_fill_the_floor_at_one() {
        let old = |wet: f32| 0.72 - 0.34 * wet;
        for i in 0..=50 {
            let w = i as f32 / 100.0;
            assert_eq!(puddle_threshold(w).to_bits(), old(w).to_bits(), "{w}");
        }
        let share = |t: f32| smooth(-2.0, 2.0, (0.49 - (t + 0.03)) / 0.12);
        let (rain_old, rain) = (share(old(0.7)), share(puddle_threshold(0.7)));
        let (storm_old, storm) = (share(old(0.9)), share(puddle_threshold(0.9)));
        println!("rain {rain_old:.3} -> {rain:.3}, storm {storm_old:.3} -> {storm:.3}");
        assert!(rain > rain_old && rain < rain_old + 0.1, "rain: {rain_old} -> {rain}");
        assert!(storm > storm_old && storm < storm_old + 0.15, "storm: {storm_old} -> {storm}");
        // past the storm's 0.9 the water rises between the puddles, over all of the ground at 1.00
        assert_eq!((puddle_flood(0.9), puddle_flood(1.0)), (0.0, 1.0));
        assert_eq!((flooded(0.37, 0.0).to_bits(), flooded(0.37, 1.0)), (0.37f32.to_bits(), 1.0));
        let mut last = f32::MAX;
        for i in 0..=100 {
            let t = puddle_threshold(i as f32 / 100.0);
            assert!(t < last);
            last = t;
        }
    }

    /// Plan hdri-6 §5, "reaching full cover only at 1.00": the slider's last steps still add water. Up to 0.99 the far
    /// ground (the puddles' mean share, which the skyline's strip takes as its wetness) keeps some ground between the
    /// puddles, each step from 0.95 on adds at least a hundredth of the floor, and near the feet some looks are still
    /// visibly less than water; at 1.00 it is all water.
    #[test]
    fn the_floor_fills_step_by_step_and_is_full_only_at_one() {
        let at = |w: f32| {
            let mut sky = sky_with("clear");
            sky.weather.as_mut().unwrap().wet_ground = w;
            Weather::new(&sky, sun()).unwrap()
        };
        let mut last = at(0.94).puddle_share();
        for i in 95..=99 {
            let share = at(i as f32 / 100.0).puddle_share();
            println!("Wet ground 0.{i}: the far share {share:.4}");
            assert!(share >= last + 0.01 && share <= 0.98, "Wet ground 0.{i}: the far share {last} -> {share}");
            last = share;
        }
        assert_eq!(at(1.0).puddle_share(), 1.0);
        // near looks, where the puddles are drawn in full (closer than PUDDLE_FADE_START_M)
        let near = at(0.99);
        let covers: Vec<f32> = (0..720).map(|i| near.puddle_cover(dir_from_az_el(i as f32 * 0.5, -20.0 - (i % 7) as f32 * 5.0), Puddles::Field)).collect();
        let (least, mean) = (covers.iter().cloned().fold(1.0, f32::min), covers.iter().sum::<f32>() / covers.len() as f32);
        println!("Wet ground 0.99 near the feet: the least cover {least:.3}, the mean {mean:.4}");
        assert!(least < 0.9 && mean < 0.99, "Wet ground 0.99 is all water near the feet: the least cover {least}, the mean {mean}");
    }

    /// At Wet ground 1.00 no direction below the horizon is dry ground: near and far it is under water, and the ground
    /// there is the water's (its bottom through it, the mirror by Fresnel).
    #[test]
    fn at_wet_ground_one_the_whole_floor_is_water() {
        let mut soaked = sky_with("clear");
        soaked.weather.as_mut().unwrap().wet_ground = 1.0;
        let wet = Weather::new(&soaked, sun()).unwrap();
        let (ground, one) = (vec3f(0.2, 0.2, 0.2), Vec3f::all(1.0));
        let sky = |_: Vec3f| vec3f(0.5, 0.6, 2.0);
        let mut state = 7u32;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / 16_777_216.0
        };
        for _ in 0..4000 {
            let d = dir_from_az_el(360.0 * next(), -0.05 - 89.9 * next());
            assert_eq!((wet.puddle_cover(d, Puddles::Field), wet.puddle_cover(d, Puddles::Even)), (1.0, 1.0), "{d:?} is dry");
            let down = (-d.y).min(1.0);
            let fresnel = WATER_F0 + (1.0 - WATER_F0) * (1.0 - down).powi(5);
            let want = relit_ground(d, ground, one) * (PUDDLE_BOTTOM * (1.0 - fresnel)) + sky(d) * fresnel;
            assert!((wet.ground(d, ground, one, &sky) - want).length() < 1e-6, "{d:?}");
        }
        assert_eq!(wet.puddle_share(), 1.0);
        assert_eq!(wet.puddle_cover(dir_from_az_el(10.0, 5.0), Puddles::Field), 0.0, "above the horizon nothing is ground");
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

    /// Plan hdri-6 §5b: the veil without the shafts is the veil wherever no shaft hangs (below the horizon, high up, under
    /// fog alone), and only the shafts differ.
    #[test]
    fn the_veil_without_virga_is_the_veil_where_no_shaft_hangs() {
        let (sun_light, dome) = (vec3f(3.0, 2.8, 2.5), vec3f(0.3, 0.35, 0.5));
        let c = vec3f(0.7, 0.5, 0.3);
        let rain = weather("rain");
        for d in [dir_from_az_el(30.0, -5.0), dir_from_az_el(30.0, 40.0)] {
            assert_eq!(rain.veil_without_virga(d, c, sun_light, dome), rain.veil(d, c, sun_light, dome), "{d:?}");
        }
        let fog = weather("fog");
        for az in 0..36 { let d = dir_from_az_el(az as f32 * 10.0, 8.0); assert_eq!(fog.veil_without_virga(d, c, sun_light, dome), fog.veil(d, c, sun_light, dome)); }
        let differ = (0..360).any(|az| { let d = dir_from_az_el(az as f32, 8.0); rain.virga(d) > 0.05 && rain.veil_without_virga(d, c, sun_light, dome) != rain.veil(d, c, sun_light, dome) });
        assert!(differ, "the premise: a shaft hangs at 8 degrees and only the veil with it darkens there");
    }

    /// Plan hdri-6 R4: a light seen through the rain and the fog keeps exp(-(1 - LIGHT_FORWARD) tau) of its light, tau
    /// the slabs' depth over the path `veil_within` takes (here from their own constants over 2 km at 1 degree, and the
    /// whole path at INFINITY): more than a surface's exp(-tau), never more than F of what it lost (what it keeps over
    /// exp(-tau) is light scattered forward, not made). A weather that veils nothing passes it whole.
    #[test]
    fn a_light_keeps_its_forward_scattered_light() {
        let (sun_light, dome) = (vec3f(3.0, 2.8, 2.5), vec3f(0.3, 0.35, 0.5));
        for kind in ["rain", "storm", "fog", "snow"] {
            let w = weather(kind);
            for (el, far) in [(1.0f32, 2000.0f32), (1.0, f32::INFINITY), (8.0, 500.0), (-2.0, 1000.0), (30.0, f32::INFINITY)] {
                let d = dir_from_az_el(30.0, el);
                let tau = w.rain_depth_within(d, far) + w.fog_depth_within(d, far);
                let t = (w.veil_within(d, Vec3f::all(1.0), sun_light, dome, far) - w.veil_within(d, Vec3f::default(), sun_light, dome, far)).x;
                let l = w.light_through(d, far);
                assert!((l - (-(1.0 - LIGHT_FORWARD) * tau).exp()).abs() <= 1e-6, "{kind} at {el} deg, {far} m: {l}, tau {tau}");
                assert!(l >= t && l - t <= LIGHT_FORWARD * (1.0 - t) + 1e-6, "{kind} at {el} deg, {far} m: the light keeps {l}, a surface {t}");
            }
        }
        let rain = weather("rain");
        let d = dir_from_az_el(30.0, 1.0);
        let t2k = (-(rain.effects.rain * RAIN_ZENITH / rain.rain_height * 2000.0 + thickened(rain.effects.fog) * FOG_ZENITH / FOG_HEIGHT_M * 2000.0)).exp();
        assert!((rain.light_through(d, 2000.0) - t2k.powf(1.0 - LIGHT_FORWARD)).abs() <= 1e-5, "the rain kind over 2 km");
        let mut clear = weather("rain");
        (clear.effects.rain, clear.effects.fog) = (0.0, 0.0);
        assert_eq!(clear.light_through(d, 2000.0), 1.0, "nothing veils");
    }

    /// Plan hdri-6 R1: the veil over what stands `far_m` along `d` is the shaft-free veil with each slab's path capped at
    /// `far_m`: its transmittance the rain's and the fog's depth over that path from their own constants, its in-scatter
    /// the same colours. At or beyond the path it is `veil_without_virga`, bit for bit.
    #[test]
    fn a_veil_within_a_distance_takes_the_slab_path_to_it() {
        let (sun_light, dome) = (vec3f(3.0, 2.8, 2.5), vec3f(0.3, 0.35, 0.5));
        let rain = weather("rain");
        let d = dir_from_az_el(30.0, 1.0);
        let far = 2000.0f32;
        // the premise: both slabs' paths at 1 degree run past 2 km
        let (fog_path, rain_path) = (slab_path(d.y, FOG_HEIGHT_M, FOG_RANGE_M), slab_path(d.y, rain.rain_height, RAIN_RANGE_M));
        assert!(fog_path > far && rain_path > far, "{fog_path} {rain_path}");
        let (t_rain, t_fog) = ((-(rain.effects.rain * RAIN_ZENITH / rain.rain_height * far)).exp(), (-(thickened(rain.effects.fog) * FOG_ZENITH / FOG_HEIGHT_M * far)).exp());
        let within = |c: Vec3f| rain.veil_within(d, c, sun_light, dome, far);
        let zero = within(Vec3f::default());
        for c in [vec3f(1.0, 1.0, 1.0), vec3f(5.0, 0.5, 0.1)] {
            let slope = within(c) - zero;
            let want = c * (t_rain * t_fog);
            assert!((slope - want).length() <= 1e-5 * want.length(), "{c:?}: {slope:?} passes, the slabs' depth over 2 km {want:?}");
        }
        // the in-scatter: the rain's grey haze, then the fog's own light, over their capped paths
        let grey = luminance(dome);
        let sky = dome.mix(vec3f(grey, grey, grey), 0.5 + 0.5 * rain.coverage);
        let even = FOG_DIFFUSE * rain.sun_dir.y.max(0.0) / std::f32::consts::PI;
        let fog_light = sky + sun_light * (rain.fog_glow * (even + hg_phase(d.dot(rain.sun_dir), FOG_G)));
        let want = Vec3f::all(grey * RAIN_ALBEDO * (1.0 - t_rain)) * t_fog + fog_light * (1.0 - t_fog);
        assert!((zero - want).length() <= 1e-5 * want.length(), "the in-scatter {zero:?}, the veil's colours over 2 km {want:?}");
        // far beyond today's path today's veil, and so at the path itself: the cap hands the same float back
        let c = vec3f(0.7, 0.5, 0.3);
        for path in [fog_path, rain_path] {
            for far in [path, path * 2.0, f32::INFINITY, f32::NAN] {
                assert_eq!(capped(path, far).to_bits(), path.to_bits(), "{path} capped at {far}");
            }
            assert_eq!(capped(path, far), far);
        }
        assert!(capped(f32::NAN, f32::INFINITY).is_nan(), "a NaN path stays one, as before the cap");
        for d in [d, dir_from_az_el(200.0, 8.0), dir_from_az_el(30.0, -0.5), dir_from_az_el(30.0, -5.0), dir_from_az_el(30.0, 40.0)] {
            let path = slab_path(d.y, FOG_HEIGHT_M, FOG_RANGE_M).max(slab_path(d.y, rain.rain_height, RAIN_RANGE_M));
            for far in [path, 1.0e6, f32::INFINITY] {
                assert_eq!(rain.veil_within(d, c, sun_light, dome, far), rain.veil_without_virga(d, c, sun_light, dome), "{d:?} at {far} m");
            }
        }
        for kind in ["fog", "storm", "snow"] {
            let w = weather(kind);
            assert_eq!(w.veil_within(d, c, sun_light, dome, 1.0e6), w.veil_without_virga(d, c, sun_light, dome), "{kind}");
        }
        // nearer, a thinner veil, and nothing at all at the eye
        assert!(within(c).x > rain.veil_without_virga(d, c, sun_light, dome).x);
        assert_eq!(rain.veil_within(d, c, sun_light, dome, 0.0), c);
        // still affine in c (`skyline::over_veiled` relies on it)
        let (x, y, a, b) = (vec3f(2.0, 0.3, 0.1), vec3f(0.1, 4.0, 1.0), 0.7, -0.4);
        let (lhs, rhs) = (within(x * a + y * b), within(x) * a + within(y) * b - zero * (a + b - 1.0));
        assert!((lhs - rhs).length() <= 1e-5 * (1.0 + lhs.length()), "{lhs:?} vs {rhs:?}");
    }
}

#[cfg(test)]
mod env_tests {
    use super::*;
    use crate::hdri::{az_el_from_dir, vec, Env, HdriParams, KeyTerm, Sampling, WEATHER_KIND_NAMES};

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

    /// `dome_light`, which lights the veil (B17's fix round), is the dry
    /// sky's mean over the upper hemisphere as the map draws it: on a fair
    /// sky the open dome's mean, as the brief's veil had it, and under a deck
    /// the deck's light, measured here on a finer grid of the dry sky than
    /// the map's own. (What the veil makes of it is pinned by
    /// `the_rain_haze_is_lit_by_the_dome_through_its_transmittance`.)
    #[test]
    fn dome_light_is_the_dry_skys_mean_over_the_upper_hemisphere() {
        let hazy = Env::new(&sky_params(Some("hazy")));
        let (dome, open) = (luminance(hazy.dome), luminance(hazy.cloud_ambient));
        assert!((dome / open - 1.0).abs() < 0.02, "a fair sky's dome is the open dome: {dome} vs {open}");
        for kind in ["overcast", "fog", "rain", "storm", "snow"] {
            let env = Env::new(&sky_params(Some(kind)));
            let atmo = env.atmo.as_ref().unwrap();
            let (rows, columns) = (24, 96);
            let mut sum = Vec3f::default();
            for j in 0..rows {
                let elevation = ((j as f32 + 0.5) / rows as f32).asin().to_degrees();
                for i in 0..columns {
                    sum += env.sky_layers(dir_from_az_el((i as f32 + 0.25) * 360.0 / columns as f32, elevation), atmo, Some(KeyTerm::Sun), Sampling::default());
                }
            }
            let fine = luminance(sum * (1.0 / (rows * columns) as f32));
            let dome = luminance(env.dome);
            assert!((dome / fine - 1.0).abs() < 0.05, "{kind}: the veil's dome {dome}, the dry sky's mean {fine}");
        }
    }

    /// M2: the rain's haze is lit by the dome as the map draws it, through
    /// its own transmittance: along any direction the rain leaves T of what
    /// is behind it and adds 1 - T of a grey of the dome's luminance times
    /// the rain's albedo (falling snow is whiter); the shafts and the fog act
    /// on that. Pinned at the zenith and 30 degrees up for rain, storm and
    /// snow: a haze lit 1.48 times too bright passed every other test while
    /// the zeniths rose 9 to 17 % (the review's mutation).
    #[test]
    fn the_rain_haze_is_lit_by_the_dome_through_its_transmittance() {
        for kind in ["rain", "storm", "snow"] {
            let p = sky_params(Some(kind));
            let env = Env::new(&p);
            let atmo = env.atmo.as_ref().unwrap();
            let weather = env.weather.as_ref().unwrap();
            // The fog over the haze on its own: the same weather, its rain taken out once it is built.
            let mut fog = Weather::new(&p.sky, atmo.sun_dir()).unwrap();
            fog.effects.rain = 0.0;
            let albedo = if kind == "snow" { SNOWFALL_ALBEDO } else { RAIN_ALBEDO };
            let haze = albedo * luminance(env.dome);
            for el in [90.0f32, 30.0] {
                let d = dir_from_az_el(17.0, el);
                let through = (-weather.rain_depth(d)).exp();
                let dry = env.sky_layers(d, atmo, None, Sampling::default());
                let rained = (dry * through + vec3f(haze, haze, haze) * (1.0 - through)) * (1.0 - weather.virga(d));
                let want = fog.veil(d, rained, env.weather_sun, env.dome) + weather.lightning(d, 0.0);
                let got = env.radiance(d);
                assert!((got - want).length() <= 1.0e-4 * want.length(), "{kind} at {el} degrees: {got:?} vs {want:?}");
                assert!(through < 0.9, "{kind} at {el} degrees: the rain hazes the sky ({through})");
            }
        }
    }

    /// D7 (L:259): fog on a fair day takes the sun's light out of its beam
    /// and spreads it round the sky, no more: it made light (the map's mean
    /// rose up to 2.2 times; a 70 degree sun under fog 0.7 put 1.21 E on a
    /// level surface against 0.94 E on a clear day, the review's figures).
    /// The map's mean does not rise with fog, at any sun and any amount, and
    /// the fog's glow puts on a level surface no more than the light the fog
    /// took out of the beam's share of it (within the bake's sampling, 2 %).
    /// The disc is hidden in the bake and its light added as the map shows
    /// it, through the veil: a 64 wide bake's refinement cannot hold a 0.53
    /// degree disc (its sub-samples lie 0.7 degrees apart), and it would then
    /// count the fog's glow but not the sun the glow came from.
    #[test]
    fn fog_on_a_fair_day_does_not_brighten_the_map() {
        for elevation in [5.0f32, 15.0, 40.0, 70.0] {
            let light = |fog: f32, glow: bool| {
                let mut p = sky_params(None);
                p.sky.sun.elevation_deg = elevation;
                p.sky.sun_disc.visible = false;
                if fog > 0.0 {
                    p.sky.weather = Some(WeatherParams { fog, ..WeatherParams::default() });
                }
                let mut env = Env::new(&p);
                if !glow {
                    env.weather.as_mut().unwrap().fog_glow = 0.0;
                }
                let map = env.bake(64);
                let atmo = env.atmo.as_ref().unwrap();
                let sun = atmo.sun_dir();
                let e = luminance(atmo.sun_irradiance());
                let weather = env.weather.as_ref();
                let disc = e * weather.map_or(1.0, |weather| weather.transmittance_toward(sun));
                let mean = crate::hdri::image::mean_luminance(&map) + disc / (4.0 * std::f32::consts::PI);
                let level = crate::hdri::composite::irradiance_above(&map, 0.0);
                // What the fog took out of the beam's light on a level surface.
                let lost = e * sun.y * weather.map_or(0.0, |weather| 1.0 - (-weather.fog_depth(sun)).exp());
                (mean, level, lost)
            };
            let (mean, _, _) = light(0.0, true);
            for fog in [0.08f32, 0.3, 0.7, 1.0] {
                let (foggy, level, lost) = light(fog, true);
                assert!(foggy <= mean, "sun at {elevation}, fog {fog}: the map's mean {foggy} against {mean} on a clear day");
                let (_, glowless, _) = light(fog, false);
                let glow = level - glowless;
                assert!(glow <= 1.02 * lost, "sun at {elevation}, fog {fog}: the glow puts {glow} on a level surface, the beam lost {lost} there");
            }
        }
    }

    /// D7 (L:261): under a closed deck the ground is lit by the light the
    /// deck passes down, the dome the map draws (`dome_light`), not by the
    /// clear sky's sun: it read about 0.8 of its sky under overcast and 9
    /// times it under snow (0.46 against 0.05). A Lambert ground of albedo a
    /// under a dome of mean radiance L reads about a x L. Whatever closes the
    /// deck: a kind, or the shipped Overcast preset's own coverage with no
    /// weather at all (its ground read 0.282 under a dome of 0.45).
    #[test]
    fn the_ground_under_a_closed_deck_is_lit_by_the_deck() {
        let mut decks: Vec<(&str, HdriParams)> = ["overcast", "snow"].iter().map(|kind| (*kind, sky_params(Some(kind)))).collect();
        decks.push(("the Overcast preset", crate::hdri::presets::preset("Overcast").unwrap()));
        for (name, p) in decks {
            let env = Env::new(&p);
            let albedo = env.weather.as_ref().map_or(p.sky.atmosphere.ground_color, |weather| weather.ground_color());
            let albedo = luminance(vec(albedo));
            let ground = luminance(env.radiance(vec3f(0.0, -1.0, 0.0)));
            let dome = luminance(env.dome_light().mean);
            let lit = albedo * dome;
            assert!(ground > 0.7 * lit && ground < 1.3 * lit, "{name}: the ground {ground} under a dome of {dome} (albedo {albedo})");
        }
    }

    /// D7 (L:261), the review's Important 1: the ground is lit by the deck
    /// with or without a weather block. It was so only where a Weather was
    /// built, so a deck closed by the Clouds coverage, or by the volume under
    /// Clear, kept the clear sky's sun-lit ground, and the first 0.01 of Fog
    /// then dropped it at once to the deck's light, about 5 times darker (the
    /// Overcast preset 10 degrees down: 0.462 with fog 0, 0.0836 with fog
    /// 0.01). Each deck with fog 0 against fog 0.01, 10 to 90 degrees down:
    /// the ground with fog 0.01 is the ground with fog 0 seen through that
    /// fog's veil: the few metres of fog down to the ground, whose own light
    /// brightens a ground as dark as the Overcast preset's by 1.4 % 10
    /// degrees down, and by no more than 2 % anywhere here.
    #[test]
    fn a_closed_deck_lights_the_ground_with_or_without_weather() {
        let mut coverage = sky_params(None);
        coverage.sky.clouds.coverage = 1.0;
        let mut volume = sky_params(Some("clear"));
        volume.sky.clouds.coverage = 1.0;
        volume.sky.volumetric = Some(crate::hdri::VolumetricParams::default());
        let decks = [("the Overcast preset", crate::hdri::presets::preset("Overcast").unwrap()), ("Clouds coverage 1", coverage), ("a closed volume under Clear", volume)];
        for (name, p) in decks {
            let fogged = |fog: f32| {
                let mut p = p.clone();
                if fog > 0.0 {
                    p.sky.weather.get_or_insert_with(WeatherParams::default).fog = fog;
                }
                Env::new(&p)
            };
            let (dry, misty) = (fogged(0.0), fogged(0.01));
            let fog = misty.weather.as_ref().expect("fog 0.01 is a weather");
            for el in [-10.0f32, -30.0, -60.0, -90.0] {
                for az in [10.0f32, 100.0, 190.0, 280.0] {
                    let d = dir_from_az_el(az, el);
                    let (a, b) = (dry.radiance(d), misty.radiance(d));
                    let veiled = fog.veil(d, a, misty.weather_sun, misty.dome);
                    let (a, b, veiled) = (luminance(a), luminance(b), luminance(veiled));
                    assert!(
                        (b / veiled - 1.0).abs() < 1.0e-4 && (b / a - 1.0).abs() < 0.02,
                        "{name} at ({az}, {el}): the ground {a} with fog 0 is {veiled} through fog 0.01, which draws {b}"
                    );
                }
            }
        }
    }

    /// D7 (L:246): the wet ground mirrors the bolt as it mirrors the sky. Rain
    /// with the Lightning tick against the same rain without it: under the
    /// mirror image of the channel the ground gains far more than beside it,
    /// where only the flash's broad glow can differ.
    #[test]
    fn the_wet_ground_mirrors_the_bolt() {
        let mut p = sky_params(Some("rain"));
        let plain = Env::new(&p);
        p.sky.weather.as_mut().unwrap().lightning = true;
        let lit = Env::new(&p);
        let bolt = lit.weather.as_ref().unwrap().bolt.as_ref().unwrap();
        // A point of the main channel half way down, and its mirror image in the ground.
        let a = bolt.segments[BOLT_STEPS / 2].a;
        let on = dir_from_az_el((bolt.azimuth + a[0] / a[1].cos()).to_degrees(), a[1].to_degrees());
        let mirror = |d: Vec3f| vec3f(d.x, -d.y, d.z);
        let gain = |d: Vec3f| luminance(lit.radiance(mirror(d))) - luminance(plain.radiance(mirror(d)));
        let beside = crate::hdri::rotate_y(on, 5.0);
        assert!(gain(on) > 10.0 * gain(beside).abs().max(1.0e-6), "the mirror gains {} under the channel, {} beside it", gain(on), gain(beside));
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
