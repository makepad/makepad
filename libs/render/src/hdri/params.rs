//! Parameters of the procedural HDRI generator.
//!
//! One serialisable struct is the in-app state, the preset file format and
//! the AI tool format at once. Every field has a documented range, and
//! [`HdriParams::clamp`] enforces the ranges and reports what it changed, so
//! a hand-edited file or an AI patch can never feed NaN or an unknown choice
//! into the model.
//!
//! micro-serde rules this file follows:
//! - There is no default attribute: a missing field fails to load unless its
//!   type is `Option<..>`. Every field added after phase 1a is therefore an
//!   `Option`, so files saved by 1a keep loading.
//! - Rust enums serialise as `{"Variant":[]}`, which is awkward to write by
//!   hand or from a model, so choices are lowercase strings with typed
//!   accessors (`HdriParams::mode`, `LightParams::shape`, ...).
//! - NaN and infinity serialise as `null` and then fail to load: clamp
//!   before saving.
//! - No `pub(crate)` on these structs or their fields; it breaks the derive.

use makepad_draw::makepad_micro_serde::*;
use std::collections::{HashMap, HashSet};

/// Most lights a map holds (16 since phase 1b).
pub const LIGHT_LIMIT: usize = 16;
/// The name `LightParams::default()` gives a light, and the one an app shows
/// for a light whose name is empty.
pub const DEFAULT_LIGHT_NAME: &str = "Light";
/// The name a shadow caster gets where none is given (`LightParams::new_caster`,
/// a caster a patch adds: `merge_json`), and the one an app shows for a caster
/// whose name is empty.
pub const DEFAULT_CASTER_NAME: &str = "Caster";
/// The params format this build reads and writes; clamp() pins `version` to it.
pub const PARAMS_VERSION: u32 = 1;
/// Accepted values of `HdriParams::mode`; the first is where unknown strings fall back to.
pub const MODE_NAMES: &[&str] = &["sky", "studio"];
/// Accepted values of `SunParams::mode`; the first is the fallback.
pub const SUN_MODE_NAMES: &[&str] = &["time", "manual"];
/// Accepted values of `LightParams::shape`; the first is the fallback. Phase 1b
/// added polygon, stroke and scrim, which the shape core draws (hdri/shape.rs).
pub const LIGHT_SHAPE_NAMES: &[&str] = &["rect", "disc", "ring", "polygon", "stroke", "scrim"];
/// Accepted values of `LightParams::blend`; the first is the fallback.
pub const BLEND_NAMES: &[&str] = &["add", "multiply"];
/// Accepted values of `LightParams::kind`; the first is the fallback (redesign §5):
/// a light, or a shadow caster (a pattern layer that darkens what is drawn
/// before it and adds no light).
pub const LIGHT_KIND_NAMES: &[&str] = &["light", "caster"];
/// Accepted values of `WeatherParams::kind`; the first is the fallback. The
/// kinds follow apps/weather's SkyKind classes (clear, overcast, rain, snow,
/// fog, thunder as "storm"), plus "hazy".
pub const WEATHER_KIND_NAMES: &[&str] = &["clear", "hazy", "overcast", "fog", "rain", "storm", "snow"];
/// Accepted values of `GoboParams::kind`; the first is the fallback.
pub const GOBO_KIND_NAMES: &[&str] = &["window", "blinds", "leaves", "fern", "grid", "image"];

/// Accepted values of `LightImageParams::fit`; the first is the fallback.
pub const IMAGE_FIT_NAMES: &[&str] = &["stretch", "fit", "fill"];
/// Longest image key kept, in characters: a path, with room to spare. A
/// light's picture (`LightImageParams::key`), a gobo's (`GoboParams::image`)
/// and the base image (`BaseImageParams::key`).
const IMAGE_KEY_LIMIT: usize = 1024;

/// Longest light name kept, in characters; a list row has no room for more.
const LIGHT_NAME_LIMIT: usize = 64;

/// The colour temperatures a light takes, kelvin: its `kelvin` and a stroke's
/// `end_kelvin`, the range `studio::kelvin_to_rgb`'s fit of the blackbody
/// colours holds over. clamp() holds both to it, and the colour and the CRI
/// readout both read the light as clamp() holds it (`LightParams::clamped`),
/// so they read one Kelvin for any settings (L:104).
pub const KELVIN_RANGE: (f32, f32) = (1000.0, 20_000.0);
/// A light's Kelvin by default, and where clamp() puts a NaN one.
pub const KELVIN_DEFAULT: f32 = 6500.0;
/// Most points a polygon's outline or a stroke keeps; clamp() drops the rest,
/// and the point editor adds no more.
pub const POINT_LIMIT: usize = 64;
/// Most width multipliers a stroke's profile keeps; clamp() drops the rest.
pub const PROFILE_LIMIT: usize = 32;
/// What clamp() keeps of `CasterParams::strength` (1 blocks fully). The panel's
/// slider and the assistant's schema read the caster ranges from here (D17).
pub const CASTER_STRENGTH_RANGE: (f32, f32) = (0.0, 1.0);
/// What clamp() keeps of `CasterParams::softness`.
pub const CASTER_SOFTNESS_RANGE: (f32, f32) = (0.0, 1.0);
/// What clamp() keeps of `CasterParams::bloom_threshold_ev`: EV of the map's
/// own radiance, as the map shows it (the sun's disc is about 2^18).
pub const CASTER_BLOOM_THRESHOLD_EV_RANGE: (f32, f32) = (-10.0, 20.0);
/// What clamp() keeps of `CasterParams::bloom_strength` (0 is bloom off).
pub const CASTER_BLOOM_STRENGTH_RANGE: (f32, f32) = (0.0, 1.0);
/// What clamp() keeps of `CasterParams::bloom_length_deg`, in degrees.
pub const CASTER_BLOOM_LENGTH_RANGE: (f32, f32) = (0.0, 45.0);
/// What clamp() keeps of `MilkyWayParams::strength` (1: the real sky's band). The inspector's sliders and the
/// assistant's schema read the night's ranges from here (D17).
pub const MILKY_WAY_STRENGTH_RANGE: (f32, f32) = (0.0, 4.0);
/// What clamp() keeps of `MilkyWayParams::tilt_deg`, degrees.
pub const MILKY_WAY_TILT_RANGE: (f32, f32) = (-90.0, 90.0);
/// What clamp() keeps of `MilkyWayParams::core` (1 real).
pub const MILKY_WAY_CORE_RANGE: (f32, f32) = (0.0, 4.0);
/// What clamp() keeps of `MilkyWayParams::dust` (1 real).
pub const MILKY_WAY_DUST_RANGE: (f32, f32) = (0.0, 2.0);
/// What clamp() keeps of `NebulaParams::amount`.
pub const NEBULA_AMOUNT_RANGE: (f32, f32) = (0.0, 1.0);
/// What clamp() keeps of `NebulaParams::size` (x).
pub const NEBULA_SIZE_RANGE: (f32, f32) = (0.25, 4.0);
/// What clamp() keeps of `NebulaParams::brightness` (x).
pub const NEBULA_BRIGHTNESS_RANGE: (f32, f32) = (0.0, 10.0);
/// What clamp() keeps of `NebulaParams::realism`.
pub const NEBULA_REALISM_RANGE: (f32, f32) = (0.0, 1.0);
/// What clamp() keeps of `AuroraParams::strength` (the green line's brightest column / 50 kR: IBC I to IV).
pub const AURORA_STRENGTH_RANGE: (f32, f32) = (0.02, 20.0);
/// What clamp() keeps of each line of `AuroraParams::mix`.
pub const AURORA_MIX_RANGE: (f32, f32) = (0.0, 2.0);
/// What clamp() keeps of `AuroraParams::height_deg`, degrees.
pub const AURORA_HEIGHT_RANGE: (f32, f32) = (1.0, 75.0);
/// What clamp() keeps of `AuroraParams::width_deg`, degrees.
pub const AURORA_WIDTH_RANGE: (f32, f32) = (10.0, 180.0);
/// What clamp() keeps of `AuroraParams::folds`.
pub const AURORA_FOLDS_RANGE: (f32, f32) = (0.0, 1.0);
/// What clamp() keeps of `AuroraParams::rays`.
pub const AURORA_RAYS_RANGE: (f32, f32) = (0.0, 1.0);
/// Accepted values of `AuroraParams::color`; the first is the fallback.
pub const AURORA_COLOR_NAMES: &[&str] = &["classic", "magenta tops", "storm", "custom"];
/// The colour presets' line amounts (green 557.7 nm, red 630.0 nm, nitrogen: 427.8 nm plus the first positive
/// bands), each x 50 kR on the brightest lines of sight at Strength 1: classic, magenta tops, storm (plan hdri-7 §4,
/// refs §5.1; Task 4 pins their colours by height). Classic's red and nitrogen are small enough that its lower edge
/// and body are green and teal, with at most a trace of violet at the top (with 0.06 and 0.2 its nitrogen fringe at
/// 103.75 km and its red top at 186 to 189 km outshone the green; with the first positive bands' 600 nm group in the
/// nitrogen, 0.1 outshone it at 103.75 km). Magenta tops' red, 0.25, is 11 kR face-on at the default Strength 3,
/// about 3.2 G0 over 210 km with the bands' blue (refs: 10 to 15 kR, 2.4 to 3.6 G0): faint beside the green, as the
/// owner's photographs show it.
pub const AURORA_PRESET_MIX: [[f32; 3]; 3] = [[1.0, 0.04, 0.05], [1.0, 0.25, 0.2], [2.0, 0.4, 2.0]];

/// clamp()'s rule for one number: NaN is `fallback`, anything else
/// (infinities too) is held into `lo..=hi`.
fn hold(value: f32, (lo, hi): (f32, f32), fallback: f32) -> f32 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(lo, hi)
    }
}

/// `clamped()` on every settings block that has a range: the sky's sun,
/// atmosphere, sun disc, deck, night, volume and weather, the studio's
/// backdrop, a base image, and each light with its polygon, stroke, scrim,
/// picture, gobo, aim, wear and caster block. A block's ranges, its defaults and its rule
/// for NaN are written once, in its `clamp_fields` (D17): no layer holds a
/// field on its own. Each layer's reader takes its block through
/// `clamped()`: the atmosphere, the sun's direction, the deck, the night,
/// the volume, the weather, the base image, the studio's backdrop, and a
/// light once where its drawing is built (`Studio`'s prepared light,
/// `LightFx::new`, `ShapedLight::new`), its colour and its CRI readout
/// (`light_color`, `light_white`, `cri::light_spectrum`), its picture, gobo,
/// aim and wear (`LightImage::new`, `Gobo::new`, `Aim::new`,
/// `Imperfections::new`). What they hand a held
/// light or number to reads it as it is (`studio::held_light_color`, the
/// shape helpers `half_extents`, `unit_to_dir`, `dir_to_unit`, `stroke_box`,
/// `light_fx::face_axes`, which the panel calls with its held lights).
/// `Env` clamps its copy of the settings first, so in a map the copy is the
/// block as it was; the panel, the assistant and a host that call a reader
/// directly get clamp()'s reading too. What keeps a guard of its own is a
/// function handed a number rather than a block (`kelvin_to_rgb`,
/// `tint_rgb`, `planck`, `led`, `gobo::penumbra`, `Foliage::leaves`,
/// `StrokeParams::around`, the edge softness `Aim::new` and the latitude and
/// solar hour `NightSky::new` are given, sky.rs's `noaa_solar_position`): it
/// holds its own domain, whoever calls it.
macro_rules! clamped_copy {
    ($($block:ty),* $(,)?) => {
        $(impl $block {
            /// A copy of the block as clamp() leaves it (`clamped_copy!`).
            pub fn clamped(&self) -> $block {
                let mut held = self.clone();
                held.clamp_fields("", &mut Fixes::default());
                held
            }
        })*
    };
}

clamped_copy!(
    SunParams,
    AtmosphereParams,
    SunDiscParams,
    CloudParams,
    NightParams,
    MilkyWayParams,
    NebulaParams,
    AuroraParams,
    VolumetricParams,
    WeatherParams,
    StudioParams,
    LightParams,
    PolygonParams,
    StrokeParams,
    ScrimParams,
    LightImageParams,
    GoboParams,
    AimParams,
    ImperfectionParams,
    CasterParams,
    BaseImageParams,
    SkylineParams,
);

/// Everything that defines one environment map.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct HdriParams {
    /// Always PARAMS_VERSION (1).
    pub version: u32,
    /// "sky" | "studio": the base layer under the lights.
    pub mode: String,
    /// -10..=10. Scales the whole map by 2^intensity_ev.
    pub intensity_ev: f32,
    /// -180..=180, wrapped. Turns the content about +Y, positive
    /// counter-clockwise seen from above (the sign of `EnvMap::procedural`
    /// and the engine's `Ibl.rotation_deg`): content at azimuth a appears at
    /// a - rotation_deg, so +90 moves a light at north (-Z) to west (-X).
    pub rotation_deg: f32,
    /// Seed of every procedural layer (clouds, stars).
    pub seed: u32,
    pub sky: SkyParams,
    pub studio: StudioParams,
    /// At most LIGHT_LIMIT, lights and shadow casters (`LightParams::kind`)
    /// together. An overlay over the base layer in both modes, evaluated in
    /// list order: a later entry draws over the earlier ones.
    pub lights: Vec<LightParams>,
    /// A loaded equirect as the base under the lights (1b): the whole base, or
    /// only its part below the horizon with the generated sky above. None = off.
    pub base_image: Option<BaseImageParams>,
    /// The night skyline as a layer of the sky (`hdri/skyline.rs`): a city
    /// strip the app renders and hands over in `ImageSet::skyline`, laid over
    /// the base under the lights. None = no skyline (presets from before it).
    pub skyline: Option<SkylineParams>,
}

/// The skyline layer's settings. The engine reads `on` and `turn_deg`; the
/// app renders the strip from the rest (makepad-skyline), so the engine never
/// depends on the skyline's own crate.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct SkylineParams {
    pub on: bool,
    pub seed: u32,
    /// Rotates the skyline round the viewer, degrees, -180..180.
    pub turn_deg: f32,
    /// Share of the city's lights kept on by day, 0..1.
    pub lights_by_day: f32,
    /// The skyline's own settings by their Rust keys (makepad_skyline::PARAMS[i].key),
    /// e.g. "ct_d" -> 0.6. Missing keys take the skyline's defaults; the app
    /// owns their ranges (the engine never reads them).
    pub settings: HashMap<String, f64>,
}

/// A loaded equirect (a photographed panorama, a render) as the base layer
/// under the lights. `hdri/composite.rs` draws it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct BaseImageParams {
    /// A key of the caller's `ImageSet`. "" or a key the set lacks: no base
    /// image. The model reads no files; the app's keys are the paths of the
    /// files the person picked. Up to 1024 characters.
    pub key: String,
    /// -180..=180, wrapped. Turns the image about +Y with the map rotation's
    /// sign: content at azimuth a shows at a - yaw_deg.
    pub yaw_deg: f32,
    /// -10..=10 stops on the image.
    pub ev: f32,
    /// Cut the image's own sun out of the map (its cone filled with the sky
    /// around it). `Env::sun` reports that sun either way.
    pub remove_sun: bool,
    /// Sky replacement: above the mask's line the mode's own base shows (the
    /// generated sky), below it the image, its exposure matched to the new sky.
    /// Off: the image is the whole base.
    pub replace_sky: bool,
    /// -45..=45 degrees of elevation: the mask's line.
    pub horizon_deg: f32,
    /// 0..=45 degrees: the width of the soft hand-over, centred on the line.
    pub feather_deg: f32,
    /// 0..=1: how much the image's own look decides what counts as sky above
    /// the line. 0: elevation only. 1: only bright, blue or neutral texels are
    /// replaced, so trees and roofs standing against the sky stay.
    pub sky_key: f32,
}

impl Default for BaseImageParams {
    fn default() -> Self {
        BaseImageParams {
            key: String::new(),
            yaw_deg: 0.0,
            ev: 0.0,
            remove_sun: false,
            replace_sky: false,
            horizon_deg: 0.0,
            feather_deg: 4.0,
            sky_key: 0.0,
        }
    }
}

impl BaseImageParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = BaseImageParams::default();
        if self.key.chars().count() > IMAGE_KEY_LIMIT {
            self.key = self.key.chars().take(IMAGE_KEY_LIMIT).collect();
            f.push(prefix, "key");
        }
        f.wrap180(prefix, "yaw_deg", &mut self.yaw_deg, d.yaw_deg);
        f.range(prefix, "ev", &mut self.ev, -10.0, 10.0, d.ev);
        f.range(prefix, "horizon_deg", &mut self.horizon_deg, -45.0, 45.0, d.horizon_deg);
        f.range(prefix, "feather_deg", &mut self.feather_deg, 0.0, 45.0, d.feather_deg);
        f.range(prefix, "sky_key", &mut self.sky_key, 0.0, 1.0, d.sky_key);
    }
}

/// The outdoor base layer.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct SkyParams {
    pub sun: SunParams,
    pub atmosphere: AtmosphereParams,
    pub sun_disc: SunDiscParams,
    pub clouds: CloudParams,
    pub night: NightParams,
    /// Volumetric clouds (1b): Some marches a cloud volume that stands on the
    /// 2D deck's footprint; None keeps the flat 2D deck.
    pub volumetric: Option<VolumetricParams>,
    /// Weather (1b): a kind that presets the sky's own fields when the map is
    /// built, plus fog, rain, wet ground, snow and lightning. None = fair.
    pub weather: Option<WeatherParams>,
}

/// Where the sun is: from a date, time and place (NOAA), or set by hand.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct SunParams {
    /// "time" (position from the date, hour and place) | "manual" (elevation_deg, azimuth_deg).
    pub mode: String,
    /// 1900..=2100, the range the NOAA formulas are good for.
    pub year: i32,
    /// 1..=12.
    pub month: u32,
    /// 1..=days in that month.
    pub day: u32,
    /// 0..=24, local clock time.
    pub hour: f32,
    /// -12..=14, hours east of UTC.
    pub tz_offset: f32,
    /// -90..=90, degrees north.
    pub latitude: f32,
    /// -180..=180, degrees east.
    pub longitude: f32,
    /// -90..=90 (manual mode).
    pub elevation_deg: f32,
    /// 0..360 clockwise from north, wrapped (manual mode).
    pub azimuth_deg: f32,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct AtmosphereParams {
    /// 0..=10, aerosol (Mie) density; 1 = a clear day.
    pub haze: f32,
    /// 0..=10, air (Rayleigh) density; 1 = Earth.
    pub air: f32,
    /// 0..=10, ozone absorption; 1 = Earth.
    pub ozone: f32,
    /// Linear 0..=1, the ground below the horizon.
    pub ground_color: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct SunDiscParams {
    /// 0.1..=20 degrees across (the real sun is 0.53). The disc keeps its energy as it grows.
    pub size_deg: f32,
    /// 0..=1, limb softness.
    pub softness: f32,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct CloudParams {
    /// 0..=1; 0 means no cloud layer.
    pub coverage: f32,
    /// 0..=1, edge sharpness.
    pub sharpness: f32,
    /// 0.1..=10, feature size.
    pub scale: f32,
    /// 500..=12000 metres, the height of the cloud layer.
    pub altitude_m: f32,
    /// 0..=1, high wispy cloud.
    pub cirrus: f32,
}

/// The marched cloud volume (`hdri/volumetric.rs`). `clouds.coverage`,
/// `sharpness` and `scale` still say where the clouds are and how large;
/// these say how the volume stands on that footprint.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct VolumetricParams {
    /// 0..=4, 1 = a fair-weather cumulus. Scales the extinction; 0 draws no volume.
    pub density: f32,
    /// 200..=8000 metres: the cloud base.
    pub base_m: f32,
    /// base_m + 200..=12000 metres: the highest tops.
    pub top_m: f32,
    /// 0..=1: how deep the fine detail eats into the edges.
    pub erosion: f32,
    /// 16..=96: the march's step budget toward the horizon (half of it overhead).
    pub steps: u32,
}

impl Default for VolumetricParams {
    fn default() -> Self {
        VolumetricParams { density: 1.0, base_m: 1500.0, top_m: 4000.0, erosion: 0.5, steps: 64 }
    }
}

impl VolumetricParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = VolumetricParams::default();
        f.range(prefix, "density", &mut self.density, 0.0, 4.0, d.density);
        f.range(prefix, "base_m", &mut self.base_m, 200.0, 8000.0, d.base_m);
        // After the base: the top is only valid above it. A NaN top lands a
        // default layer's thickness over the base.
        let lowest = self.base_m + 200.0;
        f.range(prefix, "top_m", &mut self.top_m, lowest, 12_000.0, (self.base_m + 2500.0).min(12_000.0));
        f.range(prefix, "erosion", &mut self.erosion, 0.0, 1.0, d.erosion);
        f.int(prefix, "steps", &mut self.steps, 16, 96);
    }
}

/// Weather over the sky (`hdri/weather.rs`). The kind is a preset over the
/// sky's existing fields, applied when the map is built and never written
/// back: it raises the haze and the cloud cover and brings its own fog, rain,
/// wet ground, snow or lightning. The numbers and ticks here only add to what
/// the kind brings; with kind "clear" they act alone.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct WeatherParams {
    /// "clear" | "hazy" | "overcast" | "fog" | "rain" | "storm" | "snow".
    pub kind: String,
    /// 0..=1, a fog veil: 0.05 whitens the horizon, 0.7 hides the sky.
    pub fog: f32,
    /// 0..=1, rain as haze with virga under the deck (in the snow kind: the snowfall's haze).
    pub rain: f32,
    /// 0..=1, wet ground below the horizon: a mirror of the sky with puddles.
    pub wet_ground: f32,
    /// Snow on the ground (albedo 0.8).
    pub snow: bool,
    /// One lightning bolt with its flash.
    pub lightning: bool,
    /// Seed of the bolt, the virga and the puddles.
    pub seed: u32,
}

impl Default for WeatherParams {
    fn default() -> Self {
        WeatherParams {
            kind: "clear".to_string(),
            fog: 0.0,
            rain: 0.0,
            wet_ground: 0.0,
            snow: false,
            lightning: false,
            seed: 1,
        }
    }
}

/// Typed view of `WeatherParams::kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeatherKind {
    Clear,
    Hazy,
    Overcast,
    Fog,
    Rain,
    Storm,
    Snow,
}

impl WeatherKind {
    /// The string stored in `WeatherParams::kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            WeatherKind::Clear => "clear",
            WeatherKind::Hazy => "hazy",
            WeatherKind::Overcast => "overcast",
            WeatherKind::Fog => "fog",
            WeatherKind::Rain => "rain",
            WeatherKind::Storm => "storm",
            WeatherKind::Snow => "snow",
        }
    }
}

impl WeatherParams {
    pub fn kind(&self) -> WeatherKind {
        if is_choice(&self.kind, "hazy") {
            WeatherKind::Hazy
        } else if is_choice(&self.kind, "overcast") {
            WeatherKind::Overcast
        } else if is_choice(&self.kind, "fog") {
            WeatherKind::Fog
        } else if is_choice(&self.kind, "rain") {
            WeatherKind::Rain
        } else if is_choice(&self.kind, "storm") {
            WeatherKind::Storm
        } else if is_choice(&self.kind, "snow") {
            WeatherKind::Snow
        } else {
            WeatherKind::Clear
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = WeatherParams::default();
        f.choice(prefix, "kind", &mut self.kind, WEATHER_KIND_NAMES);
        f.range(prefix, "fog", &mut self.fog, 0.0, 1.0, d.fog);
        f.range(prefix, "rain", &mut self.rain, 0.0, 1.0, d.rain);
        f.range(prefix, "wet_ground", &mut self.wet_ground, 0.0, 1.0, d.wet_ground);
    }
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct NightParams {
    /// 0..=1, star density.
    pub stars: f32,
    /// 0..=10.
    pub star_brightness: f32,
    pub moon: bool,
    /// -90..=90.
    pub moon_elevation_deg: f32,
    /// 0..360 clockwise from north, wrapped.
    pub moon_azimuth_deg: f32,
    /// 0.1..=10 degrees across (the real moon is 0.52).
    pub moon_size_deg: f32,
    /// 0..=10.
    pub moon_brightness: f32,
    /// Linear 0..=1, airglow colour.
    pub glow_color: [f32; 3],
    /// 0..=10.
    pub glow_strength: f32,
    /// Plan hdri-7 §2: the Milky Way. None (presets from before it) or off draws nothing.
    pub milky_way: Option<MilkyWayParams>,
    /// Plan hdri-7 §3: the nebulas. None or off draws nothing.
    pub nebulas: Option<NebulaParams>,
    /// Plan hdri-7 §4: the aurora. None or off draws nothing.
    pub aurora: Option<AuroraParams>,
}

/// Plan hdri-7 §2: the Milky Way's band. None, or Some with `on` false, draws nothing: the map is the base's.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct MilkyWayParams {
    /// Drawn when true. A block an edit made while off keeps its settings (the inspector's grey rows).
    pub on: bool,
    /// The band's brightness, 0..=4: 1 is the real sky's (the core region about 20.3 mag/arcsec², 4.4x the default glow)
    /// as the eye sees it (the nebulas' Realism 1); a long exposure (Realism 0) shows it 14x brighter.
    pub strength: f32,
    /// Turns the whole night sphere (stars, band, nebulas) about the celestial pole, degrees, wrapped to 0..360: a
    /// season. 116 puts the galactic centre on the meridian at 22 h (a late-July evening).
    pub turn_deg: f32,
    /// Tilts the whole night sphere about the observer's east-west line, degrees, -90..=90 (0: the latitude's own
    /// sky; positive raises the north).
    pub tilt_deg: f32,
    /// The bulge round the galactic centre, 0..=4, 1 real.
    pub core: f32,
    /// The dust lanes' optical depth, 0..=2, 1 real (the Great Rift about 1 to 1.5 mag deep).
    pub dust: f32,
}

/// Plan hdri-7 §3: glowing gas clouds, mostly along the Milky Way (anywhere when it is off). None or off draws nothing.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct NebulaParams {
    pub on: bool,
    /// How many of the seed's twelve candidates show, 0..=1; each fades in over its own twelfth.
    pub amount: f32,
    /// Their angular size, 0.25..=4 (x; 1: about 3 to 15 degrees across).
    pub size: f32,
    /// x the brightness the chosen Realism gives, 0..=10.
    pub brightness: f32,
    /// 0 space-art (astrophotography's colours, filaments, bright cores) .. 1 naked-eye real (faint, nearly grey, soft).
    /// It sets the Milky Way's look too (its brightness, colours and grain), whether the nebulas draw or not.
    pub realism: f32,
    /// Their layout (placement, shapes, knots), kept across Realism, Amount, Size and Brightness. Not clamped.
    pub seed: u32,
}

/// Plan hdri-7 §4: curtains of light 100 to 300 km up, as folded, rayed arcs. None or off draws nothing.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct AuroraParams {
    pub on: bool,
    /// The green line's brightest column / 50 kR, 0.02..=20 (IBC I to IV; 1: 50 kR). The default is 3: a clear arc at
    /// the night presets' Auto exposure (owner, 2026-10-24).
    pub strength: f32,
    /// The line mix, one of AURORA_COLOR_NAMES. Read through `color()`.
    pub color: String,
    /// The custom mix (read when `color` is "custom"): green 557.7, red 630.0, nitrogen (427.8 nm + N2 1PG), each
    /// x 50 kR at Strength 1, 0..=2.
    pub mix: [f32; 3],
    /// Azimuth of the arc's middle, degrees clockwise from north, wrapped to 0..360.
    pub direction_deg: f32,
    /// Elevation of the lower border at the arc's middle, degrees, 1..=75.
    pub height_deg: f32,
    /// The arc's span round the sky, degrees of azimuth, 10..=180.
    pub width_deg: f32,
    /// 0 one calm smooth arc .. 1 deep folds and a second and third arc.
    pub folds: f32,
    /// 0 no rays .. 1 strong rays reaching high.
    pub rays: f32,
    /// The folds' and rays' layout. Not clamped.
    pub seed: u32,
}

/// Typed view of `AuroraParams::color` (never serialised).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuroraColor {
    Classic,
    MagentaTops,
    Storm,
    Custom,
}

impl AuroraColor {
    /// In AURORA_COLOR_NAMES' order.
    pub const ALL: [AuroraColor; 4] = [AuroraColor::Classic, AuroraColor::MagentaTops, AuroraColor::Storm, AuroraColor::Custom];

    /// The string stored in `AuroraParams::color`.
    pub fn as_str(self) -> &'static str {
        match self {
            AuroraColor::Classic => "classic",
            AuroraColor::MagentaTops => "magenta tops",
            AuroraColor::Storm => "storm",
            AuroraColor::Custom => "custom",
        }
    }
}

/// The studio base layer: a vertical gradient backdrop.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct StudioParams {
    /// Linear 0..=10, straight up.
    pub top: [f32; 3],
    /// Linear 0..=10, at the horizon.
    pub horizon: [f32; 3],
    /// Linear 0..=10, straight down.
    pub floor: [f32; 3],
    /// 0.01..=1, how far the horizon colour spreads.
    pub horizon_softness: f32,
}

/// One layer of the map's list, placed on the sphere in its own angular
/// frame: a shaped light, or (`kind` "caster") a shadow caster.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct LightParams {
    pub enabled: bool,
    /// Up to 64 characters.
    pub name: String,
    /// "rect" | "disc" | "ring" (phase 1b adds more). A strip is a thin rect.
    pub shape: String,
    /// 0..=1: rounds a rect's or a polygon's corners.
    pub corner: f32,
    /// 0..=0.95 (ring only).
    pub inner: f32,
    /// 0..360 clockwise from north, wrapped.
    pub azimuth_deg: f32,
    /// -90..=90.
    pub elevation_deg: f32,
    /// 0.1..=170 degrees; 0.1..=120 for a polygon, a scrim and a caster (`max_span_deg`).
    pub width_deg: f32,
    /// 0.1..=170 degrees; 0.1..=120 for a polygon, a scrim and a caster (`max_span_deg`).
    pub height_deg: f32,
    /// -180..=180, wrapped.
    pub roll_deg: f32,
    /// 0..=1, edge falloff.
    pub softness: f32,
    /// 0..=1, centre falloff.
    pub hotspot: f32,
    /// -10..=20.
    pub intensity_ev: f32,
    /// 1000..=20000.
    pub kelvin: f32,
    /// -1..=1: negative is green, positive magenta.
    pub tint: f32,
    /// "add" | "multiply" (multiply darkens everything drawn before it: flags).
    pub blend: String,
    /// At most one light is the key; clamp() keeps the first. A caster never is.
    pub key: bool,
    /// Explicit linear colour, 0..=1 per channel; overrides kelvin and tint when Some.
    pub rgb: Option<[f32; 3]>,
    /// Shape "polygon" (1b): the outline. None draws the default hexagon.
    pub polygon: Option<PolygonParams>,
    /// Shape "stroke" (1b), a Lumi-curve. None, or a block with fewer than
    /// two points, draws the arch `StrokeParams::around` the light's place.
    pub stroke: Option<StrokeParams>,
    /// Shape "scrim" (1b): the source behind the cloth and the frame. None is ScrimParams::default().
    pub scrim: Option<ScrimParams>,
    /// Gels in front of the light (1b): up to `gels::GEL_LIMIT` names from
    /// `gels::GEL_NAMES`. None or an empty list is a bare light.
    pub gels: Option<Vec<String>>,
    /// A named light source from `spectrum::SOURCE_NAMES` (1b): its spectrum
    /// gives the colour, in place of `kelvin`. `tint` still applies; `rgb`
    /// still wins. None = the Kelvin blackbody. The source `led` is the
    /// parametric LED: it keeps `kelvin` and reads `source_quality`.
    pub source: Option<String>,
    /// The parametric LED's quality knob, 0..=1 (1b): 0 a cheap LED (CRI
    /// about 53), 0.5 about 80, 1 a full-spectrum one (about 95). Only the
    /// source `led` reads it. None = `spectrum::LED_QUALITY_DEFAULT`.
    pub source_quality: Option<f32>,
    /// A picture as the light's content (1b): see `LightImageParams`.
    pub image: Option<LightImageParams>,
    /// A pattern hung in front of the light (1b): a window, blinds, leaves,
    /// a fern, a grid of bars or an image mask. None: nothing.
    pub gobo: Option<GoboParams>,
    /// Which way the light's face points, and its grid and barn doors (1b). None: facing the centre, bare.
    pub aim: Option<AimParams>,
    /// The wear of the light's diffuser: wrinkles, dust and weave (1b). None: a clean face.
    pub imperfections: Option<ImperfectionParams>,
    /// `LIGHT_KIND_NAMES`: "light" (None, the default) or "caster": a shadow
    /// caster, a pattern layer that darkens what is drawn before it and adds
    /// no light (redesign §5). A caster reads its placement (azimuth,
    /// elevation, width, height, roll, enabled, name) and its pattern, the
    /// `gobo` block (None: the whole box covers); the pattern's own distance,
    /// fill_sky and project_on_backdrop are not read. clamp() keeps its shape
    /// "rect", its key false and its span within `max_span_deg`. The
    /// light-only fields (intensity, colour, source, gels, picture, blend,
    /// aim, surface, polygon, stroke, scrim) are kept but never read. An
    /// unknown kind is a light. Last in the struct, so a file saved without
    /// it is written as before.
    pub kind: Option<String>,
    /// A caster's own settings (redesign §5); None = CasterParams::default().
    /// Kept, never read, on a light; clamp() holds it whatever the kind.
    pub caster: Option<CasterParams>,
}

/// A picture shown by a light: it fills the light's box, is cut by the
/// light's shape, and is scaled to mean luminance 1 so the light's colour
/// and `intensity_ev` keep their meaning.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct LightImageParams {
    /// The picture's key in the `ImageSet` given to `Env::new_with` (the app
    /// uses the file's path), at most 1024 characters. "" or a key that is
    /// not loaded draws the plain light.
    pub key: String,
    /// "stretch" (fill the box, ignoring the picture's aspect) | "fit" (the
    /// whole picture inside the box) | "fill" (cover the box, cropping).
    pub fit: String,
    /// The picture's alpha cuts the light out; false ignores alpha.
    pub alpha_as_mask: bool,
    /// `IMAGE_PENUMBRA_RANGE`, 0..=1 (P1; None = 0, NaN is held to 0): the
    /// picture fades out over a band inside its own border, smoothly, to 0
    /// at the border. The band is `penumbra` x the picture's depth in the
    /// light's tangent plane, the scale the light's Softness takes, so 1
    /// fades right to the picture's centre. Only the border fades; the
    /// picture's content stays sharp, and a masked picture's cut-out edges
    /// are `mask_softness`'s (P2). At 0 the light draws exactly as without
    /// it.
    /// - "fit": the border is the letterboxed picture's own, where the light
    ///   is dark past it; the band is penumbra x its inradius (half its
    ///   shorter side), the same along both of its axes (their product
    ///   rounds the corners). The fade multiplies the light, as the
    ///   picture's alpha does, so nothing past the picture is lit. Along the
    ///   two sides where the picture fills the box, the light's own Softness
    ///   ramps there too and the two multiply; Softness's outer half, past
    ///   the picture, was already cut there at Penumbra 0.
    /// - "stretch" and "fill": the picture covers the whole light, so its
    ///   border is the light's own edge, where Softness already ramps.
    ///   There Penumbra widens that one ramp inward by penumbra x the
    ///   light's depth (half a rect's or a disc's shorter side, a ring's
    ///   half thickness, a polygon's inside depth, a stroke's half width),
    ///   its outer end staying where Softness puts it: the two bands add
    ///   into one smoothstep, from (Softness / 2 + Penumbra) x the depth
    ///   inside the outline to Softness / 2 x it outside, not two falloffs
    ///   multiplied, so the light's reach is unchanged and at Penumbra 0
    ///   the edge is Softness's alone. Where that ramp starts deeper than
    ///   the light's deepest point (Softness / 2 + Penumbra above 1), it
    ///   reads no deeper than that point: a long rect's deepest stretch,
    ///   round its centre along its long axis, is flat; a long disc (P2)
    ///   reads its exact distance more the deeper the ramp reaches, so it
    ///   peaks at its centre alone.
    pub penumbra: Option<f32>,
    /// `IMAGE_MASK_SOFTNESS_RANGE`, 0..=1 (P2, the owner's decision; None =
    /// 0, NaN is held to 0): with `alpha_as_mask`, the picture's own cut-out
    /// edges soften through an exact box filter of the premultiplied picture
    /// (its colour times its alpha, and its alpha), so the colour fades with
    /// its alpha and no dark fringe shows where the alpha was 0. The box is
    /// `mask_softness` x `image_light::MASK_SOFTNESS_SHARE` (5 %) of the
    /// picture's smaller side across, in the picture's own texels: 1 blurs a
    /// hard edge over 5 % of it, 0 keeps the sharp cut-out. It is the
    /// picture's own, whatever the Penumbra (which fades only the border)
    /// and the fit. The box softens detail finer than it with the edges.
    /// Nothing without `alpha_as_mask`. A preset saved with P1's
    /// `soften_mask` tick, which this replaced, loads with its edges sharp:
    /// presets load leniently, so the old field is skipped.
    pub mask_softness: Option<f32>,
}

/// What clamp() keeps of `LightImageParams::penumbra`; NaN is 0.
pub const IMAGE_PENUMBRA_RANGE: (f32, f32) = (0.0, 1.0);

/// What clamp() keeps of `LightImageParams::mask_softness`; NaN is 0.
pub const IMAGE_MASK_SOFTNESS_RANGE: (f32, f32) = (0.0, 1.0);

impl Default for LightImageParams {
    fn default() -> Self {
        LightImageParams { key: String::new(), fit: "stretch".to_string(), alpha_as_mask: false, penumbra: None, mask_softness: None }
    }
}

/// Typed view of `LightImageParams::fit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFit {
    Stretch,
    Fit,
    Fill,
}

impl ImageFit {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFit::Stretch => "stretch",
            ImageFit::Fit => "fit",
            ImageFit::Fill => "fill",
        }
    }
}

impl LightImageParams {
    pub fn fit(&self) -> ImageFit {
        if is_choice(&self.fit, "fit") {
            ImageFit::Fit
        } else if is_choice(&self.fit, "fill") {
            ImageFit::Fill
        } else {
            ImageFit::Stretch
        }
    }

    /// `penumbra`, None read as 0. As it is: a block clamp() holds is in range.
    pub fn penumbra(&self) -> f32 {
        self.penumbra.unwrap_or(0.0)
    }

    /// `mask_softness`, None read as 0. As it is: a block clamp() holds is in range.
    pub fn mask_softness(&self) -> f32 {
        self.mask_softness.unwrap_or(0.0)
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        if self.key.chars().count() > IMAGE_KEY_LIMIT {
            self.key = self.key.chars().take(IMAGE_KEY_LIMIT).collect();
            f.push(prefix, "key");
        }
        f.choice(prefix, "fit", &mut self.fit, IMAGE_FIT_NAMES);
        if let Some(penumbra) = self.penumbra.as_mut() {
            f.range(prefix, "penumbra", penumbra, IMAGE_PENUMBRA_RANGE.0, IMAGE_PENUMBRA_RANGE.1, 0.0);
        }
        if let Some(mask_softness) = self.mask_softness.as_mut() {
            f.range(prefix, "mask_softness", mask_softness, IMAGE_MASK_SOFTNESS_RANGE.0, IMAGE_MASK_SOFTNESS_RANGE.1, 0.0);
        }
    }
}

/// Widest a polygon, a scrim or a shadow caster may be, each way, in degrees.
/// clamp() holds them there and reports the change; the panel resyncs its
/// Width and Height sliders to it (A11 resyncs whenever clamp() changed
/// something). `LightParams::max_span_deg` says which cap a light takes.
pub const SHAPED_SPAN_MAX_DEG: f32 = 120.0;

/// Widest any other light (rect, disc, ring, stroke) may be, each way, in degrees (A1).
const LIGHT_SPAN_MAX_DEG: f32 = 170.0;

/// A polygon light's outline, in the light's unit box: x and y from -1 to 1,
/// y up and x toward increasing azimuth (the viewer's right on the panorama),
/// stretched to the light's width and height.
#[derive(Clone, Debug, PartialEq, DeJson)]
pub struct PolygonParams {
    /// 3..=64 corners of the regular polygon or star; its first corner points straight up.
    pub sides: u32,
    /// 0..=0.95. 0 is a regular polygon. Above 0 it becomes a star whose inner
    /// corners sit at (1 - star) of the polygon's own inner radius.
    pub star: f32,
    /// Up to 64 corners in the box, in order. Three or more replace the regular
    /// polygon; clamp() drops a list of fewer than three.
    pub points: Vec<[f32; 2]>,
}

impl Default for PolygonParams {
    fn default() -> Self {
        PolygonParams { sides: 6, star: 0.0, points: Vec::new() }
    }
}

/// Written by hand, as the derive would write it: micro-serde has SerJson for
/// slices (`impl<T> SerJson for [T]`) but not for fixed-size arrays, so a
/// derived `Vec<[f32; 2]>` field does not compile; DeJson for `[T; N]` exists.
impl SerJson for PolygonParams {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.st_pre();
        s.field(d + 1, "sides");
        self.sides.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "star");
        self.star.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "points");
        ser_points(&self.points, d + 1, s);
        s.st_post(d);
    }
}

/// A list of 2D points as a JSON array of [x, y] arrays, laid out like a Vec.
fn ser_points(points: &[[f32; 2]], d: usize, s: &mut SerJsonState) {
    s.out.push('[');
    if !points.is_empty() {
        let last = points.len() - 1;
        for (i, p) in points.iter().enumerate() {
            s.indent(d + 1);
            p[..].ser_json(d + 1, s);
            if i != last {
                s.out.push(',');
            }
        }
        s.indent(d);
    }
    s.out.push(']');
}

impl PolygonParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = PolygonParams::default();
        f.int(prefix, "sides", &mut self.sides, 3, 64);
        f.range(prefix, "star", &mut self.star, 0.0, 0.95, d.star);
        let points = unit_box_points(&self.points);
        if points != self.points {
            self.points = points;
            f.push(prefix, "points");
        }
    }
}

/// Finite points only, the first 64, each held inside the unit box. Fewer
/// than three make no outline, so then none are kept.
fn unit_box_points(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let kept: Vec<[f32; 2]> = points
        .iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .take(POINT_LIMIT)
        .map(|p| [p[0].clamp(-1.0, 1.0), p[1].clamp(-1.0, 1.0)])
        .collect();
    if kept.len() < 3 {
        Vec::new()
    } else {
        kept
    }
}

/// A pattern hung in front of a light (a gobo, or cookie), laid out in the
/// light's own rectangle: its azimuth, elevation, width, height and roll.
/// Fields a kind does not use are kept, so switching kinds loses nothing.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct GoboParams {
    /// "window" | "blinds" | "leaves" | "fern" | "grid" | "image".
    pub kind: String,
    /// 1..=16 panes across (window, grid).
    pub panes_x: u32,
    /// 1..=16 panes down (window, grid).
    pub panes_y: u32,
    /// 0..=0.5: the muntin bars' width, as a fraction of the smaller pane pitch.
    pub bar: f32,
    /// 0..=0.5: the window frame's width, as a fraction of the light's smaller half size.
    pub frame: f32,
    /// 2..=64 blind slats across the light.
    pub slats: u32,
    /// 0..=90 degrees: the slats' tilt from edge-on. 0 is open, 90 shut.
    pub tilt_deg: f32,
    /// 0.5..=1.5: a slat's width (its depth when open), in slat pitches. The
    /// pitch is the light's size over `slats`, so this also sets the gap:
    /// above 1 shut blinds overlap and pass nothing, below 1 a gap of
    /// (1 - slat_width) pitches stays open even when they are shut.
    pub slat_width: f32,
    /// Vertical slats; horizontal otherwise.
    pub vertical: bool,
    /// 0..=1: how much of the light leaves or fern fronds cover.
    pub density: f32,
    /// 0.1..=10: the size of leaves and fronds (1: a leaf about a sixth of the light's smaller side).
    pub scale: f32,
    /// Where leaves and fronds fall and how they turn.
    pub seed: u32,
    /// 0..=1: how far in front of the light the gobo hangs. 0 gives sharp
    /// edges (blinds too), 1 a penumbra about an eighth of the light's
    /// smaller half size. The blur moves light about and keeps it: a gobo
    /// passes the same share of its light at any distance (leaves and fern
    /// to within a few per cent).
    pub distance: f32,
    /// Image gobo: the name of an image the app has loaded; "" is none.
    pub image: String,
    /// The light is a window: it shows the map's own sky through the gobo
    /// instead of its colour. At intensity_ev 0 the sky shows as it is outside.
    pub fill_sky: bool,
    /// Studio mode: the gobo's shadow also falls on the backdrop, on the
    /// floor under the light (`gobo::BackdropGobo`).
    pub project_on_backdrop: bool,
}

impl Default for GoboParams {
    fn default() -> Self {
        GoboParams {
            kind: "window".to_string(),
            panes_x: 3,
            panes_y: 2,
            bar: 0.06,
            frame: 0.08,
            slats: 12,
            tilt_deg: 30.0,
            slat_width: 1.08,
            vertical: false,
            density: 0.5,
            scale: 1.0,
            seed: 1,
            distance: 0.1,
            image: String::new(),
            fill_sky: false,
            project_on_backdrop: false,
        }
    }
}

/// Typed view of `GoboParams::kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoboKind {
    /// A frame and muntin bars between panes.
    Window,
    /// Venetian blinds.
    Blinds,
    /// Scattered leaves (foliage.rs).
    Leaves,
    /// Fern fronds (foliage.rs).
    Fern,
    /// Bars between panes, without a frame.
    Grid,
    /// An image mask from the app's ImageSet.
    Image,
}

impl GoboKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GoboKind::Window => "window",
            GoboKind::Blinds => "blinds",
            GoboKind::Leaves => "leaves",
            GoboKind::Fern => "fern",
            GoboKind::Grid => "grid",
            GoboKind::Image => "image",
        }
    }
}

impl GoboParams {
    pub fn kind(&self) -> GoboKind {
        if is_choice(&self.kind, "blinds") {
            GoboKind::Blinds
        } else if is_choice(&self.kind, "leaves") {
            GoboKind::Leaves
        } else if is_choice(&self.kind, "fern") {
            GoboKind::Fern
        } else if is_choice(&self.kind, "grid") {
            GoboKind::Grid
        } else if is_choice(&self.kind, "image") {
            GoboKind::Image
        } else {
            GoboKind::Window
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = GoboParams::default();
        f.choice(prefix, "kind", &mut self.kind, GOBO_KIND_NAMES);
        f.int(prefix, "panes_x", &mut self.panes_x, 1, 16);
        f.int(prefix, "panes_y", &mut self.panes_y, 1, 16);
        f.range(prefix, "bar", &mut self.bar, 0.0, 0.5, d.bar);
        f.range(prefix, "frame", &mut self.frame, 0.0, 0.5, d.frame);
        f.int(prefix, "slats", &mut self.slats, 2, 64);
        f.range(prefix, "tilt_deg", &mut self.tilt_deg, 0.0, 90.0, d.tilt_deg);
        f.range(prefix, "slat_width", &mut self.slat_width, 0.5, 1.5, d.slat_width);
        f.range(prefix, "density", &mut self.density, 0.0, 1.0, d.density);
        f.range(prefix, "scale", &mut self.scale, 0.1, 10.0, d.scale);
        f.range(prefix, "distance", &mut self.distance, 0.0, 1.0, d.distance);
        // The same kind of key as an image light's (a path into the ImageSet),
        // so the same limit: a name cut shorter than its key is no gobo.
        if self.image.chars().count() > IMAGE_KEY_LIMIT {
            self.image = self.image.chars().take(IMAGE_KEY_LIMIT).collect();
            f.push(prefix, "image");
        }
    }
}

/// Which way a light's face points, and what hangs on it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct AimParams {
    /// -90..=90 degrees: the face turned off the direction to the centre,
    /// positive toward the viewer's right. Turned (with the pitch) within
    /// about five degrees of edge-on, the whole face fades out.
    pub yaw_deg: f32,
    /// -90..=90 degrees: the face tilted, positive upward.
    pub pitch_deg: f32,
    /// 0..=1: a honeycomb grid on the face, from none (0) to a 15 degree grid (1).
    pub grid: f32,
    /// Barn doors top, right, bottom, left as the viewer sees the light,
    /// each 0..=90: 0 folded flat (no door), 45 straight out, 90 closed.
    pub barn_doors: [f32; 4],
}

impl Default for AimParams {
    fn default() -> Self {
        AimParams { yaw_deg: 0.0, pitch_deg: 0.0, grid: 0.0, barn_doors: [0.0; 4] }
    }
}

impl AimParams {
    /// True when the aim changes nothing: facing the centre, no grid, no doors.
    pub fn is_neutral(&self) -> bool {
        self.yaw_deg == 0.0 && self.pitch_deg == 0.0 && self.grid == 0.0 && self.barn_doors.iter().all(|d| *d == 0.0)
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        f.range(prefix, "yaw_deg", &mut self.yaw_deg, -90.0, 90.0, 0.0);
        f.range(prefix, "pitch_deg", &mut self.pitch_deg, -90.0, 90.0, 0.0);
        f.range(prefix, "grid", &mut self.grid, 0.0, 1.0, 0.0);
        for (i, door) in self.barn_doors.iter_mut().enumerate() {
            f.range(prefix, &format!("barn_doors[{i}]"), door, 0.0, 90.0, 0.0);
        }
    }
}

/// The wear of a light's diffuser. The patterns vary the light around its
/// mean, which stays true, so intensity_ev keeps its meaning.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ImperfectionParams {
    /// 0..=1: folds in the cloth.
    pub wrinkle: f32,
    /// 0..=1: specks and smudges.
    pub dust: f32,
    /// 0..=1: the cloth's weave.
    pub weave: f32,
    /// Where the folds and the specks fall.
    pub seed: u32,
}

impl Default for ImperfectionParams {
    fn default() -> Self {
        ImperfectionParams { wrinkle: 0.0, dust: 0.0, weave: 0.0, seed: 1 }
    }
}

impl ImperfectionParams {
    /// True when nothing is set: the face is clean.
    pub fn is_clear(&self) -> bool {
        !(self.wrinkle > 0.0) && !(self.dust > 0.0) && !(self.weave > 0.0)
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        f.range(prefix, "wrinkle", &mut self.wrinkle, 0.0, 1.0, 0.0);
        f.range(prefix, "dust", &mut self.dust, 0.0, 1.0, 0.0);
        f.range(prefix, "weave", &mut self.weave, 0.0, 1.0, 0.0);
    }
}

/// A shadow caster's own settings (redesign §5), on an entry of
/// `HdriParams::lights` whose `kind` is "caster". The caster multiplies what
/// is drawn before it (everything listed before it: the base layer, the base
/// image, the lights before it) by its transmission `1 - s*m + s*m*tint`,
/// where `m` is its pattern's coverage after softness and bloom and `s` its
/// strength, so it never adds light; what is listed after it is untouched.
/// The fields are plain because the block itself is the Option; a field
/// added later must be an `Option`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct CasterParams {
    /// `CASTER_STRENGTH_RANGE`, 0..=1 (default 1): 1 blocks fully where the
    /// pattern covers, 0 is no caster.
    pub strength: f32,
    /// Linear, 0..=1 per channel (default black): what the caster lets
    /// through where it covers. Black blocks every colour; a colour lets that
    /// colour through (glass, foliage). Never above 1, so it never adds light.
    pub tint: [f32; 3],
    /// `CASTER_SOFTNESS_RANGE`, 0..=1 (default 0): blurs the whole caster (out
    /// of focus) through the exact box filter the image gobos use, so the
    /// light it lets through stays right. 0 is the sharp pattern.
    pub softness: f32,
    /// `CASTER_BLOOM_THRESHOLD_EV_RANGE`, -10..=20 (default 10): what behind
    /// the caster counts as bright: luminance above 2^bloom_threshold_ev in
    /// the map's own radiance, as the map shows it (after its intensity_ev).
    /// The sun's disc is about 2^18, a softbox at EV 3 is 8.
    pub bloom_threshold_ev: f32,
    /// `CASTER_BLOOM_STRENGTH_RANGE`, 0..=1 (default 0 = bloom off): how much
    /// that bright light eats into the caster's silhouette.
    pub bloom_strength: f32,
    /// `CASTER_BLOOM_LENGTH_RANGE`, 0..=45 degrees (default 5): how far it reaches in.
    pub bloom_length_deg: f32,
}

impl Default for CasterParams {
    fn default() -> Self {
        CasterParams { strength: 1.0, tint: [0.0; 3], softness: 0.0, bloom_threshold_ev: 10.0, bloom_strength: 0.0, bloom_length_deg: 5.0 }
    }
}

impl CasterParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = CasterParams::default();
        f.range(prefix, "strength", &mut self.strength, CASTER_STRENGTH_RANGE.0, CASTER_STRENGTH_RANGE.1, d.strength);
        f.color(prefix, "tint", &mut self.tint, 1.0, d.tint);
        f.range(prefix, "softness", &mut self.softness, CASTER_SOFTNESS_RANGE.0, CASTER_SOFTNESS_RANGE.1, d.softness);
        f.range(
            prefix,
            "bloom_threshold_ev",
            &mut self.bloom_threshold_ev,
            CASTER_BLOOM_THRESHOLD_EV_RANGE.0,
            CASTER_BLOOM_THRESHOLD_EV_RANGE.1,
            d.bloom_threshold_ev,
        );
        f.range(prefix, "bloom_strength", &mut self.bloom_strength, CASTER_BLOOM_STRENGTH_RANGE.0, CASTER_BLOOM_STRENGTH_RANGE.1, d.bloom_strength);
        f.range(prefix, "bloom_length_deg", &mut self.bloom_length_deg, CASTER_BLOOM_LENGTH_RANGE.0, CASTER_BLOOM_LENGTH_RANGE.1, d.bloom_length_deg);
    }
}

/// The largest `SkylineParams::seed` clamp() keeps.
pub const SKYLINE_SEED_MAX: u32 = 9_999_999;

impl SkylineParams {
    /// The turn wraps, the share and the seed are held; a setting that is not
    /// finite is dropped (the skyline's default takes its place), reported as
    /// `settings.<key>`. The settings' own ranges are the app's.
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        f.wrap180(prefix, "turn_deg", &mut self.turn_deg, 0.0);
        f.range(prefix, "lights_by_day", &mut self.lights_by_day, 0.0, 1.0, 0.0);
        f.int(prefix, "seed", &mut self.seed, 0, SKYLINE_SEED_MAX);
        // Sorted, so the report comes out the same every time.
        let mut dropped: Vec<String> = self.settings.iter().filter(|(_, value)| !value.is_finite()).map(|(key, _)| key.clone()).collect();
        dropped.sort();
        for key in dropped {
            self.settings.remove(&key);
            f.push(prefix, &format!("settings.{key}"));
        }
    }
}

/// A stroke light (a Lumi-curve): a smooth curve through directions on the sky.
#[derive(Clone, Debug, PartialEq, DeJson)]
pub struct StrokeParams {
    /// Up to 64 (azimuth, elevation) directions in degrees, in the map's own
    /// frame, that the curve passes through. Azimuth wraps into 0..360,
    /// elevation is -90..=90. Fewer than two draw the arch over the light's
    /// place instead (`shaped::stroke_of`).
    pub points: Vec<[f32; 2]>,
    /// 0..=2: 0 straight lines between the points, 1 a natural curve, 2 a loose one.
    pub curviness: f32,
    /// 0.1..=45 degrees, the full width.
    pub width_deg: f32,
    /// Joins the last point back to the first.
    pub closed: bool,
    /// Up to 32 width multipliers 0..=4, spread evenly along the stroke;
    /// empty is 1 all along. The brightness follows min(multiplier, 1).
    pub profile: Vec<f32>,
    /// 1000..=20000. A Kelvin ramp: the colour temperature slides from the
    /// light's `kelvin` at the first point to this at the last, evenly in
    /// mired, at the light's tint and brightness. A light with an explicit
    /// `rgb`, or a named lamp (any `source` but `led`), has a colour but no
    /// temperature to slide from: it blends from that colour to this
    /// temperature's. None is one colour all along.
    pub end_kelvin: Option<f32>,
    /// Linear 0..=1 per channel. A colour ramp: the light's colour at the
    /// first point blends to this one at the last, at the light's level.
    /// Wins over `end_kelvin` when both are set.
    pub end_rgb: Option<[f32; 3]>,
}

impl Default for StrokeParams {
    /// No points: the arch over the light's place is drawn until some are
    /// given, so a block made by a width slider or by a partial patch does
    /// not move the stroke away from its light.
    fn default() -> Self {
        StrokeParams {
            points: Vec::new(),
            curviness: 1.0,
            width_deg: 2.0,
            closed: false,
            profile: Vec::new(),
            end_kelvin: None,
            end_rgb: None,
        }
    }
}

/// An azimuth folded into 0..360.
fn wrap_azimuth(azimuth_deg: f32) -> f32 {
    let wrapped = azimuth_deg.rem_euclid(360.0);
    // rem_euclid rounds up to exactly 360 for tiny negative inputs.
    if wrapped >= 360.0 {
        0.0
    } else {
        wrapped
    }
}

impl StrokeParams {
    /// The default stroke with a gentle arch of three points centred on
    /// (azimuth, elevation) and `span_deg` wide: what a stroke light draws
    /// until it has points of its own.
    pub fn around(azimuth_deg: f32, elevation_deg: f32, span_deg: f32) -> StrokeParams {
        let az = if azimuth_deg.is_finite() { azimuth_deg } else { 0.0 };
        let el = if elevation_deg.is_finite() { elevation_deg } else { 30.0 };
        let half = if span_deg.is_finite() { (0.5 * span_deg).clamp(1.0, 80.0) } else { 15.0 };
        let lift = 0.2 * half;
        StrokeParams {
            points: vec![
                [wrap_azimuth(az - half), (el - lift).clamp(-90.0, 90.0)],
                [wrap_azimuth(az), (el + lift).clamp(-90.0, 90.0)],
                [wrap_azimuth(az + half), (el - lift).clamp(-90.0, 90.0)],
            ],
            ..StrokeParams::default()
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = StrokeParams::default();
        let points: Vec<[f32; 2]> = self
            .points
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .take(POINT_LIMIT)
            .map(|p| [wrap_azimuth(p[0]), p[1].clamp(-90.0, 90.0)])
            .collect();
        if points != self.points {
            self.points = points;
            f.push(prefix, "points");
        }
        f.range(prefix, "curviness", &mut self.curviness, 0.0, 2.0, d.curviness);
        f.range(prefix, "width_deg", &mut self.width_deg, 0.1, 45.0, d.width_deg);
        let profile: Vec<f32> = self
            .profile
            .iter()
            .take(PROFILE_LIMIT)
            .map(|v| if v.is_finite() { v.clamp(0.0, 4.0) } else { 1.0 })
            .collect();
        // NaN never equals itself, so a profile holding one is replaced too.
        if profile != self.profile {
            self.profile = profile;
            f.push(prefix, "profile");
        }
        if let Some(kelvin) = self.end_kelvin.as_mut() {
            f.range(prefix, "end_kelvin", kelvin, KELVIN_RANGE.0, KELVIN_RANGE.1, KELVIN_DEFAULT);
        }
        if let Some(rgb) = self.end_rgb.as_mut() {
            f.color(prefix, "end_rgb", rgb, 1.0, [1.0; 3]);
        }
    }
}

/// Written by hand for the `Vec<[f32; 2]>` field, as for PolygonParams. The
/// two optional ends are written only when set, the way the derive writes an
/// `Option` field, so a stroke without a ramp stays as short as before.
impl SerJson for StrokeParams {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.st_pre();
        s.field(d + 1, "points");
        ser_points(&self.points, d + 1, s);
        s.conl();
        s.field(d + 1, "curviness");
        self.curviness.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "width_deg");
        self.width_deg.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "closed");
        self.closed.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "profile");
        self.profile.ser_json(d + 1, s);
        if let Some(kelvin) = &self.end_kelvin {
            s.conl();
            s.field(d + 1, "end_kelvin");
            kelvin.ser_json(d + 1, s);
        }
        if let Some(rgb) = &self.end_rgb {
            s.conl();
            s.field(d + 1, "end_rgb");
            rgb[..].ser_json(d + 1, s);
        }
        s.st_post(d);
    }
}

/// A scrim light: a rect of diffusion cloth lit from behind.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ScrimParams {
    /// 0.05..=1: the spread of the glow the light behind the cloth makes, as a
    /// fraction of the half size (a Gaussian's sigma). 0.05 is a tight hot
    /// spot, 1 nearly even.
    pub source_size: f32,
    /// 0..=0.2: a dark frame inside the outline, as a fraction of the shorter half size.
    pub frame: f32,
}

impl Default for ScrimParams {
    fn default() -> Self {
        ScrimParams { source_size: 0.5, frame: 0.0 }
    }
}

impl ScrimParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = ScrimParams::default();
        f.range(prefix, "source_size", &mut self.source_size, 0.05, 1.0, d.source_size);
        f.range(prefix, "frame", &mut self.frame, 0.0, 0.2, d.frame);
    }
}

/// Typed view of `HdriParams::mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sky,
    Studio,
}

/// Typed view of `SunParams::mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SunMode {
    Time,
    Manual,
}

/// Typed view of `LightParams::shape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightShape {
    Rect,
    Disc,
    Ring,
    /// A regular polygon, a star, or up to 64 points (shaped.rs).
    Polygon,
    /// A Lumi-curve: a stroke through (azimuth, elevation) points (shaped.rs).
    Stroke,
    /// A rect of diffusion cloth lit from behind (shaped.rs).
    Scrim,
}

/// Typed view of `LightParams::blend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Add,
    Multiply,
}

/// Typed view of `LightParams::kind` (redesign §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// A light: it adds its light over what is drawn before it (a Multiply
    /// light, a flag, darkens it instead).
    Light,
    /// A shadow caster: its pattern multiplies what is drawn before it by its
    /// transmission, and it never adds light.
    Caster,
}

impl Mode {
    /// The string stored in `HdriParams::mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Sky => "sky",
            Mode::Studio => "studio",
        }
    }
}

impl SunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SunMode::Time => "time",
            SunMode::Manual => "manual",
        }
    }
}

impl LightShape {
    pub fn as_str(self) -> &'static str {
        match self {
            LightShape::Rect => "rect",
            LightShape::Disc => "disc",
            LightShape::Ring => "ring",
            LightShape::Polygon => "polygon",
            LightShape::Stroke => "stroke",
            LightShape::Scrim => "scrim",
        }
    }
}

impl Blend {
    pub fn as_str(self) -> &'static str {
        match self {
            Blend::Add => "add",
            Blend::Multiply => "multiply",
        }
    }
}

impl LightKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LightKind::Light => "light",
            LightKind::Caster => "caster",
        }
    }
}

impl Default for HdriParams {
    fn default() -> Self {
        HdriParams {
            version: PARAMS_VERSION,
            mode: "sky".to_string(),
            intensity_ev: 0.0,
            rotation_deg: 0.0,
            seed: 1,
            sky: SkyParams::default(),
            studio: StudioParams::default(),
            lights: Vec::new(),
            base_image: None,
            skyline: None,
        }
    }
}

impl Default for SunParams {
    fn default() -> Self {
        SunParams {
            mode: "time".to_string(),
            year: 2026,
            month: 6,
            day: 21,
            hour: 13.0,
            tz_offset: 0.0,
            latitude: 45.0,
            longitude: 0.0,
            elevation_deg: 45.0,
            azimuth_deg: 180.0,
        }
    }
}

impl Default for AtmosphereParams {
    fn default() -> Self {
        AtmosphereParams { haze: 1.0, air: 1.0, ozone: 1.0, ground_color: [0.18, 0.17, 0.15] }
    }
}

impl Default for SunDiscParams {
    fn default() -> Self {
        SunDiscParams { size_deg: 0.53, softness: 0.2, visible: true }
    }
}

impl Default for CloudParams {
    fn default() -> Self {
        CloudParams { coverage: 0.0, sharpness: 0.5, scale: 1.0, altitude_m: 2000.0, cirrus: 0.0 }
    }
}

impl Default for NightParams {
    fn default() -> Self {
        NightParams {
            stars: 0.5,
            star_brightness: 1.0,
            moon: true,
            moon_elevation_deg: 30.0,
            moon_azimuth_deg: 135.0,
            moon_size_deg: 0.52,
            moon_brightness: 1.0,
            glow_color: [0.02, 0.03, 0.06],
            glow_strength: 1.0,
            milky_way: None,
            nebulas: None,
            aurora: None,
        }
    }
}

impl Default for MilkyWayParams {
    fn default() -> Self {
        MilkyWayParams { on: true, strength: 1.0, turn_deg: 116.0, tilt_deg: 0.0, core: 1.0, dust: 1.0 }
    }
}

impl Default for NebulaParams {
    fn default() -> Self {
        NebulaParams { on: true, amount: 0.5, size: 1.0, brightness: 1.0, realism: 0.0, seed: 7 }
    }
}

impl Default for AuroraParams {
    fn default() -> Self {
        AuroraParams {
            on: true,
            strength: 3.0,
            color: "classic".to_string(),
            mix: AURORA_PRESET_MIX[0],
            direction_deg: 0.0,
            height_deg: 15.0,
            width_deg: 100.0,
            folds: 0.4,
            rays: 0.5,
            seed: 7,
        }
    }
}

impl Default for StudioParams {
    fn default() -> Self {
        StudioParams { top: [0.05; 3], horizon: [0.2; 3], floor: [0.1; 3], horizon_softness: 0.2 }
    }
}

impl Default for LightParams {
    fn default() -> Self {
        LightParams {
            enabled: true,
            name: DEFAULT_LIGHT_NAME.to_string(),
            shape: "rect".to_string(),
            corner: 0.2,
            inner: 0.5,
            azimuth_deg: 0.0,
            elevation_deg: 30.0,
            width_deg: 30.0,
            height_deg: 20.0,
            roll_deg: 0.0,
            softness: 0.2,
            hotspot: 0.0,
            intensity_ev: 3.0,
            kelvin: KELVIN_DEFAULT,
            tint: 0.0,
            blend: "add".to_string(),
            key: false,
            rgb: None,
            polygon: None,
            stroke: None,
            scrim: None,
            gels: None,
            source: None,
            source_quality: None,
            image: None,
            gobo: None,
            aim: None,
            imperfections: None,
            kind: None,
            caster: None,
        }
    }
}

/// Case-insensitive, whitespace-tolerant choice test. The accessors use it so
/// they read loose spellings correctly even before clamp() normalises them.
fn is_choice(value: &str, name: &str) -> bool {
    value.trim().eq_ignore_ascii_case(name)
}

impl HdriParams {
    pub fn mode(&self) -> Mode {
        if is_choice(&self.mode, "studio") {
            Mode::Studio
        } else {
            Mode::Sky
        }
    }

    /// Sanitises NaN/∞, clamps every field into its documented range, maps unknown
    /// enum strings to the default ("sky", "time", "rect", "add", "light"), truncates
    /// lights to LIGHT_LIMIT (shadow casters count), and keeps at most one key light
    /// (the first; a caster never is one: `LightParams::clamp_fields` clears its key).
    /// Returns the dotted paths it changed, e.g. "sky.clouds.coverage", "lights[2].shape".
    ///
    /// NaN becomes the field's default, infinities the nearest bound. Angles
    /// wrap rather than clamp: azimuths into [0, 360), rotation and roll into
    /// [-180, 180]. Choice strings are trimmed and lowercased; a spelling fix
    /// counts as a change.
    pub fn clamp(&mut self) -> Vec<String> {
        let defaults = HdriParams::default();
        let mut f = Fixes::default();
        if self.version != PARAMS_VERSION {
            self.version = PARAMS_VERSION;
            f.push("", "version");
        }
        f.choice("", "mode", &mut self.mode, MODE_NAMES);
        f.range("", "intensity_ev", &mut self.intensity_ev, -10.0, 10.0, defaults.intensity_ev);
        f.wrap180("", "rotation_deg", &mut self.rotation_deg, defaults.rotation_deg);
        self.sky.clamp_fields("sky", &mut f);
        self.studio.clamp_fields("studio", &mut f);
        if self.lights.len() > LIGHT_LIMIT {
            self.lights.truncate(LIGHT_LIMIT);
            f.push("", "lights");
        }
        let mut have_key = false;
        for (i, light) in self.lights.iter_mut().enumerate() {
            let prefix = format!("lights[{i}]");
            light.clamp_fields(&prefix, &mut f);
            // The engine takes one key light; the first one claimed wins.
            if light.key {
                if have_key {
                    light.key = false;
                    f.push(&prefix, "key");
                }
                have_key = true;
            }
        }
        if let Some(base) = self.base_image.as_mut() {
            base.clamp_fields("base_image", &mut f);
        }
        if let Some(skyline) = self.skyline.as_mut() {
            skyline.clamp_fields("skyline", &mut f);
        }
        f.changed
    }

    /// Pretty JSON: the preset file format.
    pub fn to_json(&self) -> String {
        self.serialize_json_pretty()
    }

    /// Loads JSON leniently (keys this build does not know are skipped, so
    /// files from newer builds load), then clamps.
    pub fn from_json(json: &str) -> Result<HdriParams, String> {
        let mut params = HdriParams::deserialize_json_lenient(json).map_err(|e| e.to_string())?;
        params.clamp();
        Ok(params)
    }

    /// Deep-merges a partial JSON object into a copy of self: objects merge key by key,
    /// arrays and scalars replace. Unknown keys are dropped and reported as "ignored: <path>".
    /// The merged copy is then clamped; clamped paths are reported too.
    /// "lights" given as an array replaces the whole list; a light object missing fields
    /// is completed from LightParams::default().
    ///
    /// A value of the wrong JSON kind (a string for a number, a scalar for an
    /// object) is an error naming its path, and so is a list of another length
    /// where the type fixes it (a colour, the barn doors, a point's [x, y]:
    /// `fixed_length`). Numbers aimed at integer fields are
    /// rounded (a rounding counts as a clamp). `null` clears an optional field
    /// such as a light's `rgb`; on a required field it is an error.
    pub fn merge_json(&self, patch: &str) -> Result<(HdriParams, Vec<String>), String> {
        let parsed = JsonValue::deserialize_json_strict(patch)
            .map_err(|e| format!("the patch is not valid JSON: {e}"))?;
        let patch_fields = match &parsed {
            JsonValue::Object(fields) => fields,
            _ => return Err("the patch must be a JSON object, e.g. {\"intensity_ev\": 1}".to_string()),
        };
        // Start from a clamped copy: an unclamped NaN would serialise as null
        // and the merged tree would not load back.
        let mut base = self.clone();
        let mut report = base.clamp();
        let mut tree = json_tree(&base)?;
        match &mut tree {
            JsonValue::Object(root) => merge_object(root, patch_fields, "", &mut report)?,
            _ => return Err("internal: the settings did not serialise as an object".to_string()),
        }
        let mut merged = HdriParams::deserialize_json_lenient(&tree.serialize_json()).map_err(|e| {
            format!(
                "the patched settings do not fit ({}); null only clears optional fields such as a light's rgb",
                e.msg
            )
        })?;
        // The typed struct keeps every key it knows, so a patch key missing
        // from its round trip is unknown: report it instead of dropping it silently.
        collect_ignored(&json_tree(&merged)?, patch_fields, "", &mut report);
        // Every light of a patched list starts from the default light, whose
        // name is "Light": a caster the patch names nothing is called what
        // "+ Caster" calls one (plan 4's final review M9). Here and not in
        // clamp(), which also holds a name typed in the panel: a caster typed
        // "Light", or "Lighthouse" on its way there, keeps what was typed.
        if let Some(JsonValue::Array(items)) = patch_fields.get("lights") {
            for (light, item) in merged.lights.iter_mut().zip(items) {
                let named = item.as_object().is_some_and(|fields| fields.contains_key("name"));
                if !named && light.is_caster() {
                    light.name = DEFAULT_CASTER_NAME.to_string();
                }
            }
        }
        report.extend(merged.clamp());
        // Report each path once, at its first mention.
        let mut seen = HashSet::new();
        report.retain(|path| seen.insert(path.clone()));
        Ok((merged, report))
    }
}

impl SkyParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        self.sun.clamp_fields(&join_path(prefix, "sun"), f);
        self.atmosphere.clamp_fields(&join_path(prefix, "atmosphere"), f);
        self.sun_disc.clamp_fields(&join_path(prefix, "sun_disc"), f);
        self.clouds.clamp_fields(&join_path(prefix, "clouds"), f);
        self.night.clamp_fields(&join_path(prefix, "night"), f);
        if let Some(volumetric) = self.volumetric.as_mut() {
            volumetric.clamp_fields(&join_path(prefix, "volumetric"), f);
        }
        if let Some(weather) = self.weather.as_mut() {
            weather.clamp_fields(&join_path(prefix, "weather"), f);
        }
    }
}

impl SunParams {
    pub fn mode(&self) -> SunMode {
        if is_choice(&self.mode, "manual") {
            SunMode::Manual
        } else {
            SunMode::Time
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = SunParams::default();
        f.choice(prefix, "mode", &mut self.mode, SUN_MODE_NAMES);
        f.int(prefix, "year", &mut self.year, 1900, 2100);
        f.int(prefix, "month", &mut self.month, 1, 12);
        // After the month: a day is only valid for its month (and leap year).
        let days = crate::sky::days_in_month(self.year, self.month as u8) as u32;
        f.int(prefix, "day", &mut self.day, 1, days);
        f.range(prefix, "hour", &mut self.hour, 0.0, 24.0, d.hour);
        f.range(prefix, "tz_offset", &mut self.tz_offset, -12.0, 14.0, d.tz_offset);
        f.range(prefix, "latitude", &mut self.latitude, -90.0, 90.0, d.latitude);
        f.range(prefix, "longitude", &mut self.longitude, -180.0, 180.0, d.longitude);
        f.range(prefix, "elevation_deg", &mut self.elevation_deg, -90.0, 90.0, d.elevation_deg);
        f.wrap360(prefix, "azimuth_deg", &mut self.azimuth_deg, d.azimuth_deg);
    }
}

impl AtmosphereParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = AtmosphereParams::default();
        f.range(prefix, "haze", &mut self.haze, 0.0, 10.0, d.haze);
        f.range(prefix, "air", &mut self.air, 0.0, 10.0, d.air);
        f.range(prefix, "ozone", &mut self.ozone, 0.0, 10.0, d.ozone);
        f.color(prefix, "ground_color", &mut self.ground_color, 1.0, d.ground_color);
    }
}

impl SunDiscParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = SunDiscParams::default();
        f.range(prefix, "size_deg", &mut self.size_deg, 0.1, 20.0, d.size_deg);
        f.range(prefix, "softness", &mut self.softness, 0.0, 1.0, d.softness);
    }
}

impl CloudParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = CloudParams::default();
        f.range(prefix, "coverage", &mut self.coverage, 0.0, 1.0, d.coverage);
        f.range(prefix, "sharpness", &mut self.sharpness, 0.0, 1.0, d.sharpness);
        f.range(prefix, "scale", &mut self.scale, 0.1, 10.0, d.scale);
        f.range(prefix, "altitude_m", &mut self.altitude_m, 500.0, 12_000.0, d.altitude_m);
        f.range(prefix, "cirrus", &mut self.cirrus, 0.0, 1.0, d.cirrus);
    }
}

impl NightParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = NightParams::default();
        f.range(prefix, "stars", &mut self.stars, 0.0, 1.0, d.stars);
        f.range(prefix, "star_brightness", &mut self.star_brightness, 0.0, 10.0, d.star_brightness);
        f.range(prefix, "moon_elevation_deg", &mut self.moon_elevation_deg, -90.0, 90.0, d.moon_elevation_deg);
        f.wrap360(prefix, "moon_azimuth_deg", &mut self.moon_azimuth_deg, d.moon_azimuth_deg);
        f.range(prefix, "moon_size_deg", &mut self.moon_size_deg, 0.1, 10.0, d.moon_size_deg);
        f.range(prefix, "moon_brightness", &mut self.moon_brightness, 0.0, 10.0, d.moon_brightness);
        f.color(prefix, "glow_color", &mut self.glow_color, 1.0, d.glow_color);
        f.range(prefix, "glow_strength", &mut self.glow_strength, 0.0, 10.0, d.glow_strength);
        if let Some(b) = self.milky_way.as_mut() {
            b.clamp_fields(&join_path(prefix, "milky_way"), f);
        }
        if let Some(b) = self.nebulas.as_mut() {
            b.clamp_fields(&join_path(prefix, "nebulas"), f);
        }
        if let Some(b) = self.aurora.as_mut() {
            b.clamp_fields(&join_path(prefix, "aurora"), f);
        }
    }

    /// The Milky Way's block when it draws: Some and on.
    pub fn milky_way_drawn(&self) -> Option<&MilkyWayParams> {
        self.milky_way.as_ref().filter(|b| b.on)
    }

    /// The nebulas' block when it draws: Some and on.
    pub fn nebulas_drawn(&self) -> Option<&NebulaParams> {
        self.nebulas.as_ref().filter(|b| b.on)
    }

    /// The aurora's block when it draws: Some and on.
    pub fn aurora_drawn(&self) -> Option<&AuroraParams> {
        self.aurora.as_ref().filter(|b| b.on)
    }
}

impl MilkyWayParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = MilkyWayParams::default();
        f.range(prefix, "strength", &mut self.strength, MILKY_WAY_STRENGTH_RANGE.0, MILKY_WAY_STRENGTH_RANGE.1, d.strength);
        f.wrap360(prefix, "turn_deg", &mut self.turn_deg, d.turn_deg);
        f.range(prefix, "tilt_deg", &mut self.tilt_deg, MILKY_WAY_TILT_RANGE.0, MILKY_WAY_TILT_RANGE.1, d.tilt_deg);
        f.range(prefix, "core", &mut self.core, MILKY_WAY_CORE_RANGE.0, MILKY_WAY_CORE_RANGE.1, d.core);
        f.range(prefix, "dust", &mut self.dust, MILKY_WAY_DUST_RANGE.0, MILKY_WAY_DUST_RANGE.1, d.dust);
    }
}

impl NebulaParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = NebulaParams::default();
        f.range(prefix, "amount", &mut self.amount, NEBULA_AMOUNT_RANGE.0, NEBULA_AMOUNT_RANGE.1, d.amount);
        f.range(prefix, "size", &mut self.size, NEBULA_SIZE_RANGE.0, NEBULA_SIZE_RANGE.1, d.size);
        f.range(prefix, "brightness", &mut self.brightness, NEBULA_BRIGHTNESS_RANGE.0, NEBULA_BRIGHTNESS_RANGE.1, d.brightness);
        f.range(prefix, "realism", &mut self.realism, NEBULA_REALISM_RANGE.0, NEBULA_REALISM_RANGE.1, d.realism);
    }
}

impl AuroraParams {
    /// `color` as clamp() holds it (`is_choice`: case and space forgiving); anything else is Classic.
    pub fn color(&self) -> AuroraColor {
        if is_choice(&self.color, "magenta tops") {
            AuroraColor::MagentaTops
        } else if is_choice(&self.color, "storm") {
            AuroraColor::Storm
        } else if is_choice(&self.color, "custom") {
            AuroraColor::Custom
        } else {
            AuroraColor::Classic
        }
    }

    /// The line amounts the layer draws (green, red, nitrogen): the preset's, or `mix` for Custom.
    pub fn line_mix(&self) -> [f32; 3] {
        match self.color() {
            AuroraColor::Classic => AURORA_PRESET_MIX[0],
            AuroraColor::MagentaTops => AURORA_PRESET_MIX[1],
            AuroraColor::Storm => AURORA_PRESET_MIX[2],
            AuroraColor::Custom => self.mix,
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = AuroraParams::default();
        f.range(prefix, "strength", &mut self.strength, AURORA_STRENGTH_RANGE.0, AURORA_STRENGTH_RANGE.1, d.strength);
        f.choice(prefix, "color", &mut self.color, AURORA_COLOR_NAMES);
        // `color` holds each channel to 0..=hi and reports the field once; the lines' floor is 0 as AURORA_MIX_RANGE says.
        f.color(prefix, "mix", &mut self.mix, AURORA_MIX_RANGE.1, d.mix);
        f.wrap360(prefix, "direction_deg", &mut self.direction_deg, d.direction_deg);
        f.range(prefix, "height_deg", &mut self.height_deg, AURORA_HEIGHT_RANGE.0, AURORA_HEIGHT_RANGE.1, d.height_deg);
        f.range(prefix, "width_deg", &mut self.width_deg, AURORA_WIDTH_RANGE.0, AURORA_WIDTH_RANGE.1, d.width_deg);
        f.range(prefix, "folds", &mut self.folds, AURORA_FOLDS_RANGE.0, AURORA_FOLDS_RANGE.1, d.folds);
        f.range(prefix, "rays", &mut self.rays, AURORA_RAYS_RANGE.0, AURORA_RAYS_RANGE.1, d.rays);
    }
}

impl StudioParams {
    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = StudioParams::default();
        f.color(prefix, "top", &mut self.top, 10.0, d.top);
        f.color(prefix, "horizon", &mut self.horizon, 10.0, d.horizon);
        f.color(prefix, "floor", &mut self.floor, 10.0, d.floor);
        f.range(prefix, "horizon_softness", &mut self.horizon_softness, 0.01, 1.0, d.horizon_softness);
    }
}

impl LightParams {
    pub fn shape(&self) -> LightShape {
        if is_choice(&self.shape, "disc") {
            LightShape::Disc
        } else if is_choice(&self.shape, "ring") {
            LightShape::Ring
        } else if is_choice(&self.shape, "polygon") {
            LightShape::Polygon
        } else if is_choice(&self.shape, "stroke") {
            LightShape::Stroke
        } else if is_choice(&self.shape, "scrim") {
            LightShape::Scrim
        } else {
            LightShape::Rect
        }
    }

    pub fn blend(&self) -> Blend {
        if is_choice(&self.blend, "multiply") {
            Blend::Multiply
        } else {
            Blend::Add
        }
    }

    /// Light or shadow caster. None, and a kind that is not a listed name,
    /// read as a light; loose spellings read right before clamp() fixes them.
    pub fn kind(&self) -> LightKind {
        match self.kind.as_deref() {
            Some(kind) if is_choice(kind, "caster") => LightKind::Caster,
            _ => LightKind::Light,
        }
    }

    /// True for a shadow caster (`kind` "caster").
    pub fn is_caster(&self) -> bool {
        self.kind() == LightKind::Caster
    }

    /// The caster block as clamp() holds it, or the defaults when there is none.
    pub fn caster_params(&self) -> CasterParams {
        self.caster.as_ref().map(CasterParams::clamped).unwrap_or_default()
    }

    /// The widest `width_deg` / `height_deg` clamp() keeps: SHAPED_SPAN_MAX_DEG
    /// (120) for a polygon, a scrim and a caster, which are drawn in the
    /// light's gnomonic box (it stretches toward its edges), 170 otherwise.
    /// clamp_fields and the gizmo (R7) read it.
    pub fn max_span_deg(&self) -> f32 {
        if self.is_caster() || matches!(self.shape(), LightShape::Polygon | LightShape::Scrim) {
            SHAPED_SPAN_MAX_DEG
        } else {
            LIGHT_SPAN_MAX_DEG
        }
    }

    /// A new caster as "+ Caster" adds it (the app names and places it, R6):
    /// a 60 x 45 degree box of leaves at azimuth 0, 15 degrees up, sharp,
    /// blocking fully. It is also a Multiply flag at EV 0, so one that loses
    /// its kind (a patch that leaves it out, an older build) adds no light.
    /// Already clamped.
    pub fn new_caster() -> LightParams {
        LightParams {
            name: DEFAULT_CASTER_NAME.to_string(),
            shape: LightShape::Rect.as_str().to_string(),
            azimuth_deg: 0.0,
            elevation_deg: 15.0,
            width_deg: 60.0,
            height_deg: 45.0,
            roll_deg: 0.0,
            softness: 0.0,
            intensity_ev: 0.0,
            blend: Blend::Multiply.as_str().to_string(),
            gobo: Some(GoboParams { kind: GoboKind::Leaves.as_str().to_string(), distance: 0.0, ..GoboParams::default() }),
            kind: Some(LightKind::Caster.as_str().to_string()),
            caster: Some(CasterParams::default()),
            ..LightParams::default()
        }
    }

    fn clamp_fields(&mut self, prefix: &str, f: &mut Fixes) {
        let d = LightParams::default();
        // A shadow caster (redesign §5) reads only its placement and its
        // pattern, laid in its rectangle: it is always a rect, never the key
        // (below, after `blend`), and spans at most max_span_deg (below).
        // `kind()` reads loose spellings, so this holds before `kind` itself
        // is fixed at the end.
        let caster_kind = self.is_caster();
        if self.name.chars().count() > LIGHT_NAME_LIMIT {
            self.name = self.name.chars().take(LIGHT_NAME_LIMIT).collect();
            f.push(prefix, "name");
        }
        if caster_kind && self.shape() != LightShape::Rect {
            self.shape = LightShape::Rect.as_str().to_string();
            f.push(prefix, "shape");
        }
        f.choice(prefix, "shape", &mut self.shape, LIGHT_SHAPE_NAMES);
        f.range(prefix, "corner", &mut self.corner, 0.0, 1.0, d.corner);
        f.range(prefix, "inner", &mut self.inner, 0.0, 0.95, d.inner);
        f.wrap360(prefix, "azimuth_deg", &mut self.azimuth_deg, d.azimuth_deg);
        f.range(prefix, "elevation_deg", &mut self.elevation_deg, -90.0, 90.0, d.elevation_deg);
        f.range(prefix, "width_deg", &mut self.width_deg, 0.1, LIGHT_SPAN_MAX_DEG, d.width_deg);
        f.range(prefix, "height_deg", &mut self.height_deg, 0.1, LIGHT_SPAN_MAX_DEG, d.height_deg);
        f.wrap180(prefix, "roll_deg", &mut self.roll_deg, d.roll_deg);
        f.range(prefix, "softness", &mut self.softness, 0.0, 1.0, d.softness);
        f.range(prefix, "hotspot", &mut self.hotspot, 0.0, 1.0, d.hotspot);
        f.range(prefix, "intensity_ev", &mut self.intensity_ev, -10.0, 20.0, d.intensity_ev);
        f.range(prefix, "kelvin", &mut self.kelvin, KELVIN_RANGE.0, KELVIN_RANGE.1, d.kelvin);
        f.range(prefix, "tint", &mut self.tint, -1.0, 1.0, d.tint);
        f.choice(prefix, "blend", &mut self.blend, BLEND_NAMES);
        if caster_kind && self.key {
            // A caster adds no light, so it never is the key. Cleared here
            // (and so in clamped()), it never takes clamp()'s single key slot.
            self.key = false;
            f.push(prefix, "key");
        }
        if let Some(rgb) = self.rgb.as_mut() {
            f.color(prefix, "rgb", rgb, 1.0, [1.0; 3]);
        }
        if let Some(polygon) = self.polygon.as_mut() {
            polygon.clamp_fields(&join_path(prefix, "polygon"), f);
        }
        if let Some(stroke) = self.stroke.as_mut() {
            stroke.clamp_fields(&join_path(prefix, "stroke"), f);
        }
        if let Some(scrim) = self.scrim.as_mut() {
            scrim.clamp_fields(&join_path(prefix, "scrim"), f);
        }
        // A polygon, a scrim or a caster is drawn in the light's gnomonic
        // box, which stretches toward its edges: past about 120 degrees
        // across a shape distorts (the spec's limit for one shaped light). A
        // stroke may be longer, and rect, disc and ring keep A1's 170 above.
        let span = self.max_span_deg();
        if span < LIGHT_SPAN_MAX_DEG {
            f.range(prefix, "width_deg", &mut self.width_deg, 0.1, span, d.width_deg);
            f.range(prefix, "height_deg", &mut self.height_deg, 0.1, span, d.height_deg);
        }
        if let Some(gels) = self.gels.as_mut() {
            // Known names only, spelled as the table spells them, at most GEL_LIMIT.
            let clean = super::gels::clean_names(gels);
            if clean != *gels {
                *gels = clean;
                f.push(prefix, "gels");
            }
        }
        if let Some(name) = self.source.take() {
            // A known source, spelled as the table spells it. An unknown one
            // hands the colour back to the Kelvin.
            self.source = super::spectrum::source(&name).map(|row| row.name.to_string());
            if self.source.as_deref() != Some(name.as_str()) {
                f.push(prefix, "source");
            }
        }
        if let Some(quality) = self.source_quality.as_mut() {
            f.range(prefix, "source_quality", quality, 0.0, 1.0, super::spectrum::LED_QUALITY_DEFAULT);
        }
        if let Some(image) = self.image.as_mut() {
            image.clamp_fields(&join_path(prefix, "image"), f);
        }
        if let Some(gobo) = self.gobo.as_mut() {
            gobo.clamp_fields(&join_path(prefix, "gobo"), f);
        }
        if let Some(aim) = self.aim.as_mut() {
            aim.clamp_fields(&join_path(prefix, "aim"), f);
        }
        if let Some(imperfections) = self.imperfections.as_mut() {
            imperfections.clamp_fields(&join_path(prefix, "imperfections"), f);
        }
        if let Some(kind) = self.kind.as_mut() {
            f.choice(prefix, "kind", kind, LIGHT_KIND_NAMES);
        }
        if let Some(block) = self.caster.as_mut() {
            // Held whatever the kind: a light keeps a block it does not read.
            block.clamp_fields(&join_path(prefix, "caster"), f);
        }
    }
}

/// The dotted paths clamp() changed, in visiting order.
#[derive(Default)]
struct Fixes {
    changed: Vec<String>,
}

impl Fixes {
    fn push(&mut self, prefix: &str, field: &str) {
        self.changed.push(join_path(prefix, field));
    }

    /// NaN becomes the field's default; everything else (infinities included) is clamped.
    fn range(&mut self, prefix: &str, field: &str, value: &mut f32, lo: f32, hi: f32, fallback: f32) {
        let old = *value;
        let new = hold(old, (lo, hi), fallback);
        if old.is_nan() || new != old {
            *value = new;
            self.push(prefix, field);
        }
    }

    /// Azimuths wrap into [0, 360) instead of clamping: 370 means 10.
    fn wrap360(&mut self, prefix: &str, field: &str, value: &mut f32, fallback: f32) {
        let old = *value;
        if (0.0..360.0).contains(&old) {
            return;
        }
        *value = if old.is_finite() {
            let wrapped = old.rem_euclid(360.0);
            // rem_euclid rounds up to exactly 360 for tiny negative inputs.
            if wrapped >= 360.0 { 0.0 } else { wrapped }
        } else {
            fallback
        };
        self.push(prefix, field);
    }

    /// Yaw and roll wrap into [-180, 180]; both ends are kept as given.
    fn wrap180(&mut self, prefix: &str, field: &str, value: &mut f32, fallback: f32) {
        let old = *value;
        if (-180.0..=180.0).contains(&old) {
            return;
        }
        *value = if old.is_finite() { (old + 180.0).rem_euclid(360.0) - 180.0 } else { fallback };
        self.push(prefix, field);
    }

    fn int<T: PartialOrd + Copy>(&mut self, prefix: &str, field: &str, value: &mut T, lo: T, hi: T) {
        let old = *value;
        let new = if old < lo {
            lo
        } else if old > hi {
            hi
        } else {
            old
        };
        if new != old {
            *value = new;
            self.push(prefix, field);
        }
    }

    /// A colour: each channel from 0 to `hi` like range(), reported once.
    fn color(&mut self, prefix: &str, field: &str, value: &mut [f32; 3], hi: f32, fallback: [f32; 3]) {
        let mut changed = false;
        for (channel, default) in value.iter_mut().zip(fallback) {
            let old = *channel;
            let new = if old.is_nan() { default } else { old.clamp(0.0, hi) };
            if old.is_nan() || new != old {
                *channel = new;
                changed = true;
            }
        }
        if changed {
            self.push(prefix, field);
        }
    }

    /// Trims and lowercases a known choice; anything else falls back to the first choice.
    fn choice(&mut self, prefix: &str, field: &str, value: &mut String, choices: &[&str]) {
        let normal = value.trim().to_ascii_lowercase();
        let new = if choices.contains(&normal.as_str()) { normal } else { choices[0].to_string() };
        if *value != new {
            *value = new;
            self.push(prefix, field);
        }
    }
}

/// `prefix.field`; the root has no prefix.
fn join_path(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_string()
    } else {
        format!("{prefix}.{field}")
    }
}

/// The path with every list index blanked (`lights[3].rgb` becomes
/// `lights[].rgb`): the form template_for() and integer_range() match on.
fn schema_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut in_index = false;
    for ch in path.chars() {
        match ch {
            '[' => {
                in_index = true;
                out.push('[');
            }
            ']' => {
                in_index = false;
                out.push(']');
            }
            _ if in_index => {}
            _ => out.push(ch),
        }
    }
    out
}

/// A value as a JSON tree, through its own serialiser.
fn json_tree<T: SerJson>(value: &T) -> Result<JsonValue, String> {
    JsonValue::deserialize_json(&value.serialize_json()).map_err(|e| format!("internal: {e}"))
}

/// The complete object a patch starts from when it creates something that is
/// not there yet. Every light in a patched "lights" list starts from the
/// default light, so a patch names only the fields it changes; so does a
/// light's optional shape block.
fn template_for(schema: &str) -> Option<JsonValue> {
    match schema {
        "lights[]" => json_tree(&LightParams::default()).ok(),
        "lights[].polygon" => json_tree(&PolygonParams::default()).ok(),
        "lights[].stroke" => json_tree(&StrokeParams::default()).ok(),
        "lights[].scrim" => json_tree(&ScrimParams::default()).ok(),
        "lights[].image" => json_tree(&LightImageParams::default()).ok(),
        "lights[].gobo" => json_tree(&GoboParams::default()).ok(),
        "lights[].aim" => json_tree(&AimParams::default()).ok(),
        "lights[].imperfections" => json_tree(&ImperfectionParams::default()).ok(),
        "lights[].caster" => json_tree(&CasterParams::default()).ok(),
        "base_image" => json_tree(&BaseImageParams::default()).ok(),
        "sky.volumetric" => json_tree(&VolumetricParams::default()).ok(),
        "sky.weather" => json_tree(&WeatherParams::default()).ok(),
        "sky.night.milky_way" => json_tree(&MilkyWayParams::default()).ok(),
        "sky.night.nebulas" => json_tree(&NebulaParams::default()).ok(),
        "sky.night.aurora" => json_tree(&AuroraParams::default()).ok(),
        _ => None,
    }
}

/// Integer fields, with the range their Rust type holds. micro-serde refuses
/// `3.6` or `-1` for a `u32`, so numbers aimed at these are rounded into range
/// before the typed parse.
fn integer_range(schema: &str) -> Option<(f64, f64)> {
    match schema {
        "version" | "seed" | "sky.sun.month" | "sky.sun.day" | "lights[].polygon.sides" => Some((0.0, u32::MAX as f64)),
        "sky.sun.year" => Some((i32::MIN as f64, i32::MAX as f64)),
        "lights[].gobo.panes_x" | "lights[].gobo.panes_y" | "lights[].gobo.slats" | "lights[].gobo.seed" => Some((0.0, u32::MAX as f64)),
        "lights[].imperfections.seed" => Some((0.0, u32::MAX as f64)),
        "sky.volumetric.steps" => Some((0.0, u32::MAX as f64)),
        "sky.weather.seed" => Some((0.0, u32::MAX as f64)),
        "sky.night.nebulas.seed" | "sky.night.aurora.seed" => Some((0.0, u32::MAX as f64)),
        _ => None,
    }
}

/// Lists of numbers whose length their Rust type fixes (`[f32; N]`), with
/// that length: the colours, the barn doors and each [x, y] of a point list.
/// micro-serde refuses another length with a token at a position in the
/// merged text, so `check_length` names the field instead (M13).
fn fixed_length(schema: &str) -> Option<usize> {
    match schema {
        "sky.atmosphere.ground_color" | "sky.night.glow_color" | "studio.top" | "studio.horizon" | "studio.floor" => Some(3),
        "lights[].rgb" | "lights[].stroke.end_rgb" | "lights[].caster.tint" => Some(3),
        "lights[].aim.barn_doors" => Some(4),
        "sky.night.aurora.mix" => Some(3),
        "lights[].polygon.points[]" | "lights[].stroke.points[]" => Some(2),
        _ => None,
    }
}

/// A value given for a fixed-length list (`fixed_length`) must be a list of
/// that many numbers, and so must each item of a point list; the error names
/// the field, or the item. Anything else passes through.
fn check_length(value: &JsonValue, schema: &str, path: &str) -> Result<(), String> {
    if let Some(n) = fixed_length(schema) {
        return match value.as_array() {
            Some(items) if items.len() == n && items.iter().all(|item| item.is_number()) => Ok(()),
            _ => Err(format!("{path} must be a list of {n} numbers")),
        };
    }
    let item_schema = format!("{schema}[]");
    if let (Some(items), Some(_)) = (value.as_array(), fixed_length(&item_schema)) {
        for (i, item) in items.iter().enumerate() {
            check_length(item, &item_schema, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

/// Keys in sorted order, so reports come out the same every time.
fn sorted_keys(map: &HashMap<String, JsonValue>) -> Vec<&String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    keys
}

fn kind_name(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Object(_) => "an object",
        JsonValue::Array(items) if items.first().is_some_and(|item| item.is_number()) => "a list of numbers",
        JsonValue::Array(_) => "a list",
        JsonValue::String(_) => "a string",
        JsonValue::Bool(_) => "true or false",
        JsonValue::Null => "null",
        other if other.is_number() => "a number",
        _ => "a value",
    }
}

/// A patch value must have the JSON kind of the value it replaces, so the
/// error names the field instead of a position in an internal string.
fn check_kind(existing: &JsonValue, value: &JsonValue, path: &str) -> Result<(), String> {
    let fits = match existing {
        JsonValue::Object(_) => value.is_object(),
        JsonValue::Array(items) => match value.as_array() {
            // A colour stays a list of numbers.
            Some(new_items) => {
                !items.first().is_some_and(|item| item.is_number())
                    || new_items.iter().all(|item| item.is_number())
            }
            None => false,
        },
        JsonValue::String(_) => value.is_string(),
        JsonValue::Bool(_) => value.as_bool().is_some(),
        other if other.is_number() => value.is_number(),
        _ => true,
    };
    if fits {
        Ok(())
    } else {
        Err(format!("{path} must be {}", kind_name(existing)))
    }
}

/// Rounds a number aimed at an integer field into that field's type, and
/// reports the path when that changed it. Any other value passes through.
fn coerce_number(value: &JsonValue, schema: &str, path: &str, report: &mut Vec<String>) -> JsonValue {
    match (integer_range(schema), value.as_f64()) {
        (Some((lo, hi)), Some(number)) => {
            let fixed = number.round().clamp(lo, hi);
            if fixed != number {
                report.push(path.to_string());
            }
            if lo >= 0.0 {
                JsonValue::U64(fixed as u64)
            } else {
                JsonValue::I64(fixed as i64)
            }
        }
        _ => value.clone(),
    }
}

/// Merges `patch` into `base` key by key: objects recurse, everything else replaces.
fn merge_object(
    base: &mut HashMap<String, JsonValue>,
    patch: &HashMap<String, JsonValue>,
    prefix: &str,
    report: &mut Vec<String>,
) -> Result<(), String> {
    for key in sorted_keys(patch) {
        let value = &patch[key];
        let path = join_path(prefix, key);
        let schema = schema_path(&path);
        // A list whose items have a template (the lights) is replaced whole,
        // and each item starts from the template.
        if let (Some(items), Some(template)) = (value.as_array(), template_for(&format!("{schema}[]"))) {
            let mut list = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let item_path = format!("{path}[{i}]");
                let fields = item.as_object().ok_or_else(|| format!("{item_path} must be an object"))?;
                let mut merged = template.clone();
                if let JsonValue::Object(target) = &mut merged {
                    merge_object(target, fields, &item_path, report)?;
                }
                list.push(merged);
            }
            base.insert(key.clone(), JsonValue::Array(list));
            continue;
        }
        match base.get_mut(key.as_str()) {
            Some(existing) if !existing.is_null() && !value.is_null() => match (existing, value) {
                (JsonValue::Object(target), JsonValue::Object(fields)) => {
                    merge_object(target, fields, &path, report)?;
                }
                (existing, value) => {
                    check_kind(existing, value, &path)?;
                    check_length(value, &schema, &path)?;
                    *existing = coerce_number(value, &schema, &path, report);
                }
            },
            _ => {
                // Not in the current settings: an optional field that is off,
                // or an unknown key (merge_json's typed round trip tells them
                // apart). A null clears an optional field.
                let fresh = match (value, template_for(&schema)) {
                    (JsonValue::Object(fields), Some(mut template)) => {
                        if let JsonValue::Object(target) = &mut template {
                            merge_object(target, fields, &path, report)?;
                        }
                        template
                    }
                    _ => {
                        if !value.is_null() {
                            check_length(value, &schema, &path)?;
                        }
                        coerce_number(value, &schema, &path, report)
                    }
                };
                base.insert(key.clone(), fresh);
            }
        }
    }
    Ok(())
}

/// Reports every patch key that the typed round trip (`kept`) dropped.
fn collect_ignored(kept: &JsonValue, patch: &HashMap<String, JsonValue>, prefix: &str, report: &mut Vec<String>) {
    for key in sorted_keys(patch) {
        let value = &patch[key];
        let path = join_path(prefix, key);
        match (kept.get(key), value) {
            (None, _) => {
                // A null on an absent key is either an optional field that is
                // already off or an unknown key; neither changes anything.
                if !value.is_null() {
                    report.push(format!("ignored: {path}"));
                }
            }
            (Some(kept_value), JsonValue::Object(fields)) if kept_value.is_object() => {
                collect_ignored(kept_value, fields, &path, report);
            }
            (Some(JsonValue::Array(kept_items)), JsonValue::Array(items)) => {
                for (i, (kept_item, item)) in kept_items.iter().zip(items).enumerate() {
                    if let (JsonValue::Object(_), JsonValue::Object(fields)) = (kept_item, item) {
                        collect_ignored(kept_item, fields, &format!("{path}[{i}]"), report);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A studio set-up with a key light and a coloured multiply light.
    fn studio_params() -> HdriParams {
        let mut p = HdriParams {
            mode: "studio".to_string(),
            intensity_ev: 1.5,
            rotation_deg: -12.5,
            seed: 42,
            ..Default::default()
        };
        p.studio.top = [0.3, 0.3, 0.35];
        p.lights.push(LightParams {
            name: "Key".to_string(),
            key: true,
            azimuth_deg: 37.5,
            elevation_deg: 20.0,
            kelvin: 4500.0,
            ..LightParams::default()
        });
        p.lights.push(LightParams {
            name: "Flag".to_string(),
            shape: "ring".to_string(),
            blend: "multiply".to_string(),
            rgb: Some([0.9, 0.8, 0.7]),
            ..LightParams::default()
        });
        p
    }

    /// Every f32 field (colours included) set to `v`, with one light.
    fn all_numbers(v: f32) -> HdriParams {
        let mut p = HdriParams {
            intensity_ev: v,
            rotation_deg: v,
            ..Default::default()
        };
        let sun = &mut p.sky.sun;
        sun.hour = v;
        sun.tz_offset = v;
        sun.latitude = v;
        sun.longitude = v;
        sun.elevation_deg = v;
        sun.azimuth_deg = v;
        let atmosphere = &mut p.sky.atmosphere;
        atmosphere.haze = v;
        atmosphere.air = v;
        atmosphere.ozone = v;
        atmosphere.ground_color = [v; 3];
        p.sky.sun_disc.size_deg = v;
        p.sky.sun_disc.softness = v;
        let clouds = &mut p.sky.clouds;
        clouds.coverage = v;
        clouds.sharpness = v;
        clouds.scale = v;
        clouds.altitude_m = v;
        clouds.cirrus = v;
        let night = &mut p.sky.night;
        night.stars = v;
        night.star_brightness = v;
        night.moon_elevation_deg = v;
        night.moon_azimuth_deg = v;
        night.moon_size_deg = v;
        night.moon_brightness = v;
        night.glow_color = [v; 3];
        night.glow_strength = v;
        let studio = &mut p.studio;
        studio.top = [v; 3];
        studio.horizon = [v; 3];
        studio.floor = [v; 3];
        studio.horizon_softness = v;
        p.lights.push(LightParams {
            corner: v,
            inner: v,
            azimuth_deg: v,
            elevation_deg: v,
            width_deg: v,
            height_deg: v,
            roll_deg: v,
            softness: v,
            hotspot: v,
            intensity_ev: v,
            kelvin: v,
            tint: v,
            rgb: Some([v; 3]),
            ..LightParams::default()
        });
        p
    }

    #[test]
    fn defaults_and_a_studio_setup_pass_clamp_unchanged() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams::default());
        let before = p.clone();
        assert_eq!(p.clamp(), Vec::<String>::new());
        assert_eq!(p, before);
        let mut s = studio_params();
        assert!(s.clamp().is_empty());
        assert_eq!(s.mode(), Mode::Studio);
        assert_eq!(s.lights[1].shape(), LightShape::Ring);
        assert_eq!(s.lights[1].blend(), Blend::Multiply);
        assert_eq!(HdriParams::default().mode(), Mode::Sky);
        assert_eq!(HdriParams::default().sky.sun.mode(), SunMode::Time);
    }

    #[test]
    fn every_numeric_field_is_bounded_and_reported() {
        let mut p = all_numbers(1.0e9);
        p.sky.sun.year = 99_999;
        p.sky.sun.month = 99;
        p.sky.sun.day = 99;
        let changed = p.clamp();
        let want = [
            "intensity_ev", "rotation_deg",
            "sky.sun.year", "sky.sun.month", "sky.sun.day", "sky.sun.hour", "sky.sun.tz_offset",
            "sky.sun.latitude", "sky.sun.longitude", "sky.sun.elevation_deg", "sky.sun.azimuth_deg",
            "sky.atmosphere.haze", "sky.atmosphere.air", "sky.atmosphere.ozone", "sky.atmosphere.ground_color",
            "sky.sun_disc.size_deg", "sky.sun_disc.softness",
            "sky.clouds.coverage", "sky.clouds.sharpness", "sky.clouds.scale", "sky.clouds.altitude_m",
            "sky.clouds.cirrus",
            "sky.night.stars", "sky.night.star_brightness", "sky.night.moon_elevation_deg",
            "sky.night.moon_azimuth_deg", "sky.night.moon_size_deg", "sky.night.moon_brightness",
            "sky.night.glow_color", "sky.night.glow_strength",
            "studio.top", "studio.horizon", "studio.floor", "studio.horizon_softness",
            "lights[0].corner", "lights[0].inner", "lights[0].azimuth_deg", "lights[0].elevation_deg",
            "lights[0].width_deg", "lights[0].height_deg", "lights[0].roll_deg", "lights[0].softness",
            "lights[0].hotspot", "lights[0].intensity_ev", "lights[0].kelvin", "lights[0].tint",
            "lights[0].rgb",
        ];
        assert_eq!(changed, want);
        assert_eq!(p.intensity_ev, 10.0);
        assert_eq!((p.sky.sun.year, p.sky.sun.month, p.sky.sun.day), (2100, 12, 31));
        assert_eq!(p.sky.clouds.altitude_m, 12_000.0);
        assert_eq!(p.studio.top, [10.0; 3]);
        assert_eq!(p.sky.night.glow_color, [1.0; 3]);
        assert_eq!(p.lights[0].kelvin, 20_000.0);
        assert_eq!(p.lights[0].inner, 0.95);
        assert_eq!(p.lights[0].rgb, Some([1.0; 3]));
        assert!((0.0..360.0).contains(&p.sky.sun.azimuth_deg));
        assert!((-180.0..=180.0).contains(&p.rotation_deg));
        assert!(p.clamp().is_empty(), "clamping twice changes nothing");
    }

    #[test]
    fn nan_becomes_the_default_and_infinity_the_bound() {
        let mut p = all_numbers(f32::NAN);
        let changed = p.clamp();
        // Every float field was NaN; the three integer date fields were not touched.
        assert_eq!(changed.len(), 44, "{changed:?}");
        assert_eq!(p.sky, SkyParams::default());
        assert_eq!(p.studio, StudioParams::default());
        assert_eq!((p.intensity_ev, p.rotation_deg), (0.0, 0.0));
        assert_eq!(p.lights[0], LightParams { rgb: Some([1.0; 3]), ..LightParams::default() });
        // Clean params serialise without null, so they load again.
        let json = p.to_json();
        assert!(!json.contains("null"), "{json}");
        assert_eq!(HdriParams::from_json(&json), Ok(p.clone()));

        let mut q = all_numbers(f32::NEG_INFINITY);
        q.clamp();
        assert_eq!(q.intensity_ev, -10.0);
        assert_eq!(q.sky.clouds.altitude_m, 500.0);
        assert_eq!(q.sky.sun_disc.size_deg, 0.1);
        assert_eq!(q.lights[0].width_deg, 0.1);
        assert_eq!(q.studio.top, [0.0; 3]);
        // An infinite angle has no direction: it falls back to the default.
        assert_eq!(q.sky.sun.azimuth_deg, SunParams::default().azimuth_deg);
        assert_eq!(q.rotation_deg, 0.0);
    }

    #[test]
    fn clamp_wraps_angles_instead_of_pinning_them() {
        let mut p = HdriParams { rotation_deg: 190.0, ..Default::default() };
        p.sky.sun.azimuth_deg = 370.0;
        p.sky.night.moon_azimuth_deg = -90.0;
        p.lights.push(LightParams { azimuth_deg: -30.0, roll_deg: 200.0, ..LightParams::default() });
        let changed = p.clamp();
        assert_eq!(p.rotation_deg, -170.0);
        assert_eq!(p.sky.sun.azimuth_deg, 10.0);
        assert_eq!(p.sky.night.moon_azimuth_deg, 270.0);
        assert_eq!(p.lights[0].azimuth_deg, 330.0);
        assert_eq!(p.lights[0].roll_deg, -160.0);
        assert_eq!(
            changed,
            ["rotation_deg", "sky.sun.azimuth_deg", "sky.night.moon_azimuth_deg", "lights[0].azimuth_deg", "lights[0].roll_deg"]
        );
        // The ends of the documented ranges stay put.
        let mut edge = HdriParams { rotation_deg: 180.0, ..Default::default() };
        edge.lights.push(LightParams { roll_deg: -180.0, azimuth_deg: 0.0, ..LightParams::default() });
        assert!(edge.clamp().is_empty());
    }

    #[test]
    fn clamp_keeps_the_date_real() {
        let date = |year: i32, month: u32, day: u32| {
            let mut p = HdriParams::default();
            p.sky.sun.year = year;
            p.sky.sun.month = month;
            p.sky.sun.day = day;
            let changed = p.clamp();
            ((p.sky.sun.year, p.sky.sun.month, p.sky.sun.day), changed)
        };
        assert_eq!(date(2026, 6, 31).0, (2026, 6, 30));
        assert_eq!(date(2026, 2, 29).0, (2026, 2, 28));
        assert_eq!(date(2028, 2, 29), ((2028, 2, 29), Vec::<String>::new()));
        assert_eq!(date(2026, 13, 5).0, (2026, 12, 5));
        assert_eq!(date(2026, 0, 0), ((2026, 1, 1), vec!["sky.sun.month".to_string(), "sky.sun.day".to_string()]));
        assert_eq!(date(1500, 6, 21).0, (1900, 6, 21));
    }

    #[test]
    fn unknown_choices_fall_back_and_case_is_forgiven() {
        let mut p = HdriParams { mode: " Studio".to_string(), ..Default::default() };
        p.sky.sun.mode = "sundial".to_string();
        p.lights.push(LightParams { shape: "hexagon".to_string(), blend: "Multiply".to_string(), ..LightParams::default() });
        p.lights.push(LightParams { shape: "DISC".to_string(), ..LightParams::default() });
        // The accessors already read the loose spellings before clamp().
        assert_eq!(p.mode(), Mode::Studio);
        assert_eq!(p.lights[1].shape(), LightShape::Disc);
        let changed = p.clamp();
        assert_eq!((p.mode.as_str(), p.sky.sun.mode.as_str()), ("studio", "time"));
        assert_eq!((p.lights[0].shape.as_str(), p.lights[0].blend.as_str()), ("rect", "multiply"));
        assert_eq!(p.lights[1].shape, "disc");
        assert_eq!(changed, ["mode", "sky.sun.mode", "lights[0].shape", "lights[0].blend", "lights[1].shape"]);
        assert_eq!(p.lights[0].blend(), Blend::Multiply);
        assert_eq!(Mode::Studio.as_str(), MODE_NAMES[1]);
        assert_eq!(SunMode::Manual.as_str(), "manual");
        assert_eq!(LightShape::Ring.as_str(), "ring");
        assert_eq!(Blend::Add.as_str(), BLEND_NAMES[0]);
    }

    #[test]
    fn every_choice_name_round_trips_through_its_accessor() {
        // Each listed name reads back through its typed accessor as a variant whose
        // as_str is that name, and clamp() keeps it as it is. An unknown string reads
        // as the list's first name, and every variant is listed. A name added to a
        // list but not to its accessor (or the other way round) fails here.
        let light = |shape: &str, blend: &str| HdriParams {
            lights: vec![LightParams { shape: shape.to_string(), blend: blend.to_string(), ..LightParams::default() }],
            ..Default::default()
        };
        let mut seen = Vec::new();
        for name in MODE_NAMES {
            let mut p = HdriParams { mode: name.to_string(), ..Default::default() };
            seen.push(p.mode().as_str());
            assert!(p.clamp().is_empty() && p.mode == *name, "mode {name}");
        }
        assert_eq!(seen, MODE_NAMES);
        seen.clear();
        for name in SUN_MODE_NAMES {
            let mut p = HdriParams::default();
            p.sky.sun.mode = name.to_string();
            seen.push(p.sky.sun.mode().as_str());
            assert!(p.clamp().is_empty() && p.sky.sun.mode == *name, "sun mode {name}");
        }
        assert_eq!(seen, SUN_MODE_NAMES);
        seen.clear();
        for name in LIGHT_SHAPE_NAMES {
            let mut p = light(name, BLEND_NAMES[0]);
            seen.push(p.lights[0].shape().as_str());
            assert!(p.clamp().is_empty() && p.lights[0].shape == *name, "shape {name}");
        }
        assert_eq!(seen, LIGHT_SHAPE_NAMES);
        seen.clear();
        for name in BLEND_NAMES {
            let mut p = light(LIGHT_SHAPE_NAMES[0], name);
            seen.push(p.lights[0].blend().as_str());
            assert!(p.clamp().is_empty() && p.lights[0].blend == *name, "blend {name}");
        }
        assert_eq!(seen, BLEND_NAMES);

        let unknown = light("?", "?");
        assert_eq!(HdriParams { mode: "?".to_string(), ..Default::default() }.mode().as_str(), MODE_NAMES[0]);
        let sun = SunParams { mode: "?".to_string(), ..Default::default() };
        assert_eq!(sun.mode().as_str(), SUN_MODE_NAMES[0]);
        assert_eq!(unknown.lights[0].shape().as_str(), LIGHT_SHAPE_NAMES[0]);
        assert_eq!(unknown.lights[0].blend().as_str(), BLEND_NAMES[0]);

        // Every variant has its name listed. The matches stop compiling when a
        // variant is added, until it is added to the arrays beside them too.
        for m in [Mode::Sky, Mode::Studio] {
            match m {
                Mode::Sky | Mode::Studio => assert!(MODE_NAMES.contains(&m.as_str()), "{m:?}"),
            }
        }
        for m in [SunMode::Time, SunMode::Manual] {
            match m {
                SunMode::Time | SunMode::Manual => assert!(SUN_MODE_NAMES.contains(&m.as_str()), "{m:?}"),
            }
        }
        for s in [LightShape::Rect, LightShape::Disc, LightShape::Ring, LightShape::Polygon, LightShape::Stroke, LightShape::Scrim] {
            match s {
                LightShape::Rect | LightShape::Disc | LightShape::Ring | LightShape::Polygon | LightShape::Stroke | LightShape::Scrim => {
                    assert!(LIGHT_SHAPE_NAMES.contains(&s.as_str()), "{s:?}")
                }
            }
        }
        for b in [Blend::Add, Blend::Multiply] {
            match b {
                Blend::Add | Blend::Multiply => assert!(BLEND_NAMES.contains(&b.as_str()), "{b:?}"),
            }
        }
    }

    #[test]
    fn lights_are_capped_and_only_the_first_key_survives() {
        let mut p = HdriParams::default();
        for i in 0..(LIGHT_LIMIT + 2) {
            p.lights.push(LightParams { name: format!("L{i}"), key: i == 0 || i == 2, ..LightParams::default() });
        }
        p.lights[1].name = "x".repeat(100);
        let changed = p.clamp();
        assert_eq!(p.lights.len(), LIGHT_LIMIT);
        assert_eq!(p.lights[7].name, "L7", "the list is cut at the end");
        assert!(p.lights[0].key && !p.lights[2].key);
        assert_eq!(p.lights.iter().filter(|l| l.key).count(), 1);
        assert_eq!(p.lights[1].name, "x".repeat(64));
        assert_eq!(changed, ["lights", "lights[1].name", "lights[2].key"]);
    }

    #[test]
    fn json_round_trips_and_tolerates_newer_files() {
        let p = studio_params();
        let json = p.to_json();
        assert!(json.contains('\n'), "preset files are pretty-printed");
        assert_eq!(json.matches("\"rgb\"").count(), 1, "Some(rgb) is written, None is left out");
        assert_eq!(HdriParams::from_json(&json), Ok(p.clone()));
        // A file from a newer build with a field this build does not know still loads.
        let newer = json.replacen('{', "{\"future_field\": {\"x\": [1, 2]},", 1);
        assert_eq!(HdriParams::from_json(&newer), Ok(p.clone()));
        // Loading clamps.
        let mut wild = HdriParams::default();
        wild.sky.clouds.coverage = 5.0;
        assert_eq!(HdriParams::from_json(&wild.to_json()).unwrap().sky.clouds.coverage, 1.0);
        // Missing fields and broken JSON are errors, not silent defaults.
        assert!(HdriParams::from_json("{\"version\": 1}").is_err());
        assert!(HdriParams::from_json("{\"mode\": ").is_err());
    }

    #[test]
    fn merge_changes_only_what_the_patch_names() {
        let base = studio_params();
        let (merged, report) = base
            .merge_json(r#"{"sky": {"clouds": {"coverage": 0.4}}, "studio": {"top": [1, 1, 1]}}"#)
            .unwrap();
        let mut want = base.clone();
        want.sky.clouds.coverage = 0.4;
        want.studio.top = [1.0; 3];
        assert_eq!(merged, want);
        assert!(report.is_empty(), "{report:?}");
        let (same, report) = base.merge_json("{}").unwrap();
        assert_eq!(same, base);
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn merge_reports_unknown_keys() {
        let base = HdriParams::default();
        let (merged, report) = base
            .merge_json(r#"{"colour": "red", "sky": {"clouds": {"fluffiness": 1, "cirrus": 0.5}}}"#)
            .unwrap();
        assert_eq!(merged.sky.clouds.cirrus, 0.5);
        assert_eq!(report, vec!["ignored: colour", "ignored: sky.clouds.fluffiness"]);
        let mut want = base.clone();
        want.sky.clouds.cirrus = 0.5;
        assert_eq!(merged, want, "unknown keys change nothing");
    }

    #[test]
    fn merge_clamps_and_rounds_integers() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"intensity_ev": 99, "seed": -5, "sky": {"sun": {"month": 3.6, "year": 2030.0}}}"#)
            .unwrap();
        assert_eq!(merged.intensity_ev, 10.0);
        assert_eq!(merged.seed, 0);
        assert_eq!(merged.sky.sun.month, 4);
        assert_eq!(merged.sky.sun.year, 2030);
        for path in ["intensity_ev", "seed", "sky.sun.month"] {
            assert!(report.iter().any(|r| r == path), "{path} missing from {report:?}");
        }
        assert!(!report.iter().any(|r| r == "sky.sun.year"), "2030.0 is already a whole year: {report:?}");
    }

    #[test]
    fn merge_completes_new_lights_from_the_default() {
        let base = studio_params();
        let (merged, report) = base
            .merge_json(r#"{"lights": [{"shape": "disc", "azimuth_deg": 90, "rgb": [1, 0.5, 0.25]}]}"#)
            .unwrap();
        assert_eq!(
            merged.lights,
            vec![LightParams {
                shape: "disc".to_string(),
                azimuth_deg: 90.0,
                rgb: Some([1.0, 0.5, 0.25]),
                ..LightParams::default()
            }]
        );
        assert!(report.is_empty(), "{report:?}");
        let (cleared, _) = base.merge_json(r#"{"lights": []}"#).unwrap();
        assert!(cleared.lights.is_empty(), "an array replaces the whole list");
        let (unknown, report) = base.merge_json(r#"{"lights": [{"colour": "red"}]}"#).unwrap();
        assert_eq!(unknown.lights, vec![LightParams::default()]);
        assert_eq!(report, vec!["ignored: lights[0].colour"]);
    }

    #[test]
    fn merge_refuses_malformed_patches() {
        let base = HdriParams::default();
        assert!(base.merge_json("[1, 2]").is_err(), "the patch must be an object");
        assert!(base.merge_json("{\"sky\": ").is_err(), "broken JSON");
        let err = base.merge_json(r#"{"sky": {"clouds": {"coverage": "lots"}}}"#).unwrap_err();
        assert!(err.contains("sky.clouds.coverage"), "{err}");
        let err = base.merge_json(r#"{"lights": [3]}"#).unwrap_err();
        assert!(err.contains("lights[0]"), "{err}");
        let err = base.merge_json(r#"{"sky": {"atmosphere": {"ground_color": ["a", "b", "c"]}}}"#).unwrap_err();
        assert!(err.contains("ground_color"), "{err}");
        assert!(base.merge_json(r#"{"intensity_ev": null}"#).is_err(), "null only clears optional fields");
    }

    /// Every list of numbers in `value` (each item of a list of lists too), by path.
    fn number_lists(value: &JsonValue, path: &str, out: &mut Vec<String>) {
        match value {
            JsonValue::Object(fields) => {
                for (key, field) in fields {
                    number_lists(field, &join_path(path, key), out);
                }
            }
            JsonValue::Array(items) if items.first().is_some_and(|item| item.is_number()) => out.push(path.to_string()),
            JsonValue::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    number_lists(item, &format!("{path}[{i}]"), out);
                }
            }
            _ => {}
        }
    }

    /// The value at `path` ("lights[0].stroke.points[1]") in `tree`.
    fn value_at<'a>(tree: &'a mut JsonValue, path: &str) -> &'a mut JsonValue {
        path.split('.').fold(tree, |node, part| match part.split_once('[') {
            Some((key, index)) => {
                let i: usize = index.trim_end_matches(']').parse().unwrap();
                &mut node.get_mut(key).unwrap().as_array_mut().unwrap()[i]
            }
            None => node.get_mut(part).unwrap(),
        })
    }

    /// M13: a list whose length its type fixes (a colour, a caster's tint,
    /// the barn doors, a point) given another length is refused naming its field, not by the
    /// typed parse's token at a position in an internal string. Every list of
    /// numbers a full map holds is tried one longer and one shorter, so a
    /// fixed-size field added without its length fails here; a list of any
    /// length (a stroke's profile) merges.
    #[test]
    fn a_list_of_the_wrong_length_is_refused_naming_its_field() {
        let mut full = HdriParams::default();
        full.sky.night.aurora = Some(AuroraParams::default());
        full.lights.push(LightParams {
            rgb: Some([0.5, 0.4, 0.3]),
            polygon: Some(PolygonParams { points: vec![[0.0, 1.0], [-1.0, -1.0], [1.0, -1.0]], ..Default::default() }),
            stroke: Some(StrokeParams {
                points: vec![[10.0, 20.0], [40.0, 30.0]],
                profile: vec![1.0, 0.5],
                end_kelvin: Some(3000.0),
                end_rgb: Some([1.0, 0.5, 0.2]),
                ..Default::default()
            }),
            aim: Some(AimParams { barn_doors: [10.0, 0.0, 0.0, 0.0], ..Default::default() }),
            caster: Some(CasterParams { tint: [0.2, 0.3, 0.1], ..CasterParams::default() }),
            ..LightParams::default()
        });
        let tree = json_tree(&full).unwrap();
        let mut lists = Vec::new();
        number_lists(&tree, "", &mut lists);
        lists.sort();
        assert!(lists.len() >= 12, "{lists:?}");
        for path in &lists {
            for longer in [true, false] {
                let mut patch = tree.clone();
                let list = value_at(&mut patch, path).as_array_mut().unwrap();
                if longer {
                    list.push(JsonValue::F64(0.5));
                } else {
                    list.pop();
                }
                match full.merge_json(&patch.serialize_json()) {
                    Ok(_) => assert_eq!(schema_path(path), "lights[].stroke.profile", "{path} took a list of another length"),
                    Err(err) => assert!(err.starts_with(&format!("{path} must be a list of ")), "{path}: {err}"),
                }
            }
        }
        // The fields a model most often gets wrong, on a fresh light too.
        for (patch, says) in [
            (r#"{"lights": [{"aim": {"barn_doors": [90, 0]}}]}"#, "lights[0].aim.barn_doors must be a list of 4 numbers"),
            (r#"{"lights": [{"rgb": [1, 0.5]}]}"#, "lights[0].rgb must be a list of 3 numbers"),
            (r#"{"lights": [{"rgb": "red"}]}"#, "lights[0].rgb must be a list of 3 numbers"),
            (r#"{"lights": [{"polygon": {"points": [[0, 1], [1, -1, 0], [-1, -1]]}}]}"#, "lights[0].polygon.points[1] must be a list of 2 numbers"),
            (r#"{"lights": [{"stroke": {"points": [10, 20, 30, 40]}}]}"#, "lights[0].stroke.points[0] must be a list of 2 numbers"),
            (r#"{"lights": [{"stroke": {"end_rgb": [1, 1]}}]}"#, "lights[0].stroke.end_rgb must be a list of 3 numbers"),
            (r#"{"studio": {"top": [1, 1, 1, 1]}}"#, "studio.top must be a list of 3 numbers"),
        ] {
            let err = HdriParams::default().merge_json(patch).unwrap_err();
            assert_eq!(err, says, "{patch}");
        }
        // A list of the right length, and null on an optional one, still merge.
        let (merged, _) = HdriParams::default().merge_json(r#"{"lights": [{"rgb": [1, 0.5, 0.25], "aim": {"barn_doors": [90, 0, 0, 0]}}]}"#).unwrap();
        assert_eq!(merged.lights[0].rgb, Some([1.0, 0.5, 0.25]));
        assert!(full.merge_json(r#"{"lights": [{"rgb": null}]}"#).is_ok());
    }
}

#[cfg(test)]
mod shape_param_tests {
    use super::*;

    #[test]
    fn a_map_holds_sixteen_lights() {
        assert_eq!(LIGHT_LIMIT, 16);
        let mut p = HdriParams::default();
        p.lights = vec![LightParams::default(); 20];
        let changed = p.clamp();
        assert_eq!(p.lights.len(), 16);
        assert_eq!(changed, ["lights"]);
    }

    #[test]
    fn the_shaped_light_names_are_choices() {
        assert_eq!(LIGHT_SHAPE_NAMES, &["rect", "disc", "ring", "polygon", "stroke", "scrim"]);
        for (name, shape) in [("polygon", LightShape::Polygon), ("stroke", LightShape::Stroke), ("scrim", LightShape::Scrim)] {
            let mut p = HdriParams::default();
            p.lights.push(LightParams { shape: name.to_ascii_uppercase(), ..LightParams::default() });
            assert_eq!(p.lights[0].shape(), shape, "the accessor reads a loose spelling");
            let changed = p.clamp();
            assert_eq!(p.lights[0].shape, name);
            assert_eq!(changed, ["lights[0].shape"], "only the spelling was fixed");
            assert_eq!(shape.as_str(), name);
        }
    }

    #[test]
    fn polygon_params_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            shape: "polygon".to_string(),
            polygon: Some(PolygonParams { sides: 1, star: 2.0, points: vec![[f32::NAN, 0.0], [5.0, 0.0], [0.0, 1.0], [-1.0, -1.0]] }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        let poly = p.lights[0].polygon.clone().unwrap();
        assert_eq!(poly.sides, 3);
        assert_eq!(poly.star, 0.95);
        assert_eq!(poly.points, vec![[1.0, 0.0], [0.0, 1.0], [-1.0, -1.0]], "NaN dropped, the rest held in the box");
        assert_eq!(changed, ["lights[0].polygon.sides", "lights[0].polygon.star", "lights[0].polygon.points"]);
        // Fewer than three points make no outline: they go, and the regular polygon shows.
        p.lights[0].polygon = Some(PolygonParams { sides: 5, star: 0.0, points: vec![[0.0, 0.0], [1.0, 1.0]] });
        assert_eq!(p.clamp(), ["lights[0].polygon.points"]);
        assert!(p.lights[0].polygon.as_ref().unwrap().points.is_empty());
        // JSON: pretty and compact both load back, and a missing polygon stays None.
        p.lights[0].polygon = Some(poly.clone());
        assert_eq!(HdriParams::from_json(&p.to_json()).unwrap(), p);
        assert_eq!(PolygonParams::deserialize_json(&poly.serialize_json()).unwrap(), poly);
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("polygon"), "an absent polygon is not written");
        assert_eq!(HdriParams::from_json(&plain.to_json()).unwrap().lights[0].polygon, None);
    }

    #[test]
    fn a_partial_polygon_patch_is_completed_from_the_defaults() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"shape": "polygon", "polygon": {"sides": 5.4, "star": 0.5}}]}"#)
            .unwrap();
        assert_eq!(merged.lights[0].polygon, Some(PolygonParams { sides: 5, star: 0.5, points: Vec::new() }));
        assert_eq!(report, ["lights[0].polygon.sides"], "5.4 was rounded to a whole number of sides");
    }

    #[test]
    fn polygons_and_scrims_stay_under_120_degrees_but_strokes_do_not() {
        let mut p = HdriParams::default();
        for shape in ["polygon", "scrim", "stroke", "rect"] {
            p.lights.push(LightParams { shape: shape.to_string(), width_deg: 150.0, height_deg: 130.0, ..LightParams::default() });
        }
        let changed = p.clamp();
        let span = |i: usize| (p.lights[i].width_deg, p.lights[i].height_deg);
        assert_eq!(span(0), (SHAPED_SPAN_MAX_DEG, SHAPED_SPAN_MAX_DEG), "a polygon lives in its gnomonic box");
        assert_eq!(span(1), (SHAPED_SPAN_MAX_DEG, SHAPED_SPAN_MAX_DEG), "so does a scrim");
        // A stroke may be longer (its points place it), and A3's rect keeps A1's 170.
        assert_eq!(span(2), (150.0, 130.0));
        assert_eq!(span(3), (150.0, 130.0));
        assert_eq!(changed, ["lights[0].width_deg", "lights[0].height_deg", "lights[1].width_deg", "lights[1].height_deg"]);
    }

    #[test]
    fn stroke_params_clamp_and_round_trip() {
        let stroke = StrokeParams {
            points: (0..70).map(|i| [400.0 + i as f32, 100.0]).collect(),
            curviness: 5.0,
            width_deg: 0.0,
            closed: true,
            profile: (0..40).map(|i| if i == 0 { f32::NAN } else { -1.0 }).collect(),
            end_kelvin: Some(500.0),
            end_rgb: Some([2.0, f32::NAN, -1.0]),
        };
        let mut p = HdriParams::default();
        p.lights.push(LightParams { shape: "stroke".to_string(), stroke: Some(stroke), ..LightParams::default() });
        let changed = p.clamp();
        let s = p.lights[0].stroke.clone().unwrap();
        assert_eq!(s.points.len(), 64);
        assert_eq!(s.points[0], [40.0, 90.0], "azimuth wrapped, elevation held");
        assert_eq!((s.curviness, s.width_deg), (2.0, 0.1));
        assert_eq!(s.profile.len(), 32);
        assert_eq!((s.profile[0], s.profile[1]), (1.0, 0.0), "NaN becomes 1, negatives 0");
        assert_eq!(s.end_kelvin, Some(1000.0));
        assert_eq!(s.end_rgb, Some([1.0, 1.0, 0.0]), "each channel into 0..1, NaN to 1");
        assert_eq!(
            changed,
            [
                "lights[0].stroke.points",
                "lights[0].stroke.curviness",
                "lights[0].stroke.width_deg",
                "lights[0].stroke.profile",
                "lights[0].stroke.end_kelvin",
                "lights[0].stroke.end_rgb"
            ]
        );
        assert_eq!(HdriParams::from_json(&p.to_json()).unwrap(), p);
        assert_eq!(StrokeParams::deserialize_json(&s.serialize_json()).unwrap(), s);
        // The default block has no points (the arch over the light is drawn) and
        // no ramp: it writes neither end, and loads back as it was.
        let plain = StrokeParams::default();
        assert!(plain.points.is_empty());
        assert!(!plain.serialize_json().contains("end_"), "{}", plain.serialize_json());
        assert_eq!(StrokeParams::deserialize_json(&plain.serialize_json()).unwrap(), plain);
        let arch = StrokeParams::around(350.0, 85.0, 40.0);
        assert_eq!(arch.points.len(), 3);
        assert!(arch.points.iter().all(|q| (0.0..360.0).contains(&q[0]) && q[1] <= 90.0), "{:?}", arch.points);
        assert_eq!(StrokeParams { points: Vec::new(), ..arch }, plain, "the arch is the default plus its points");
        // A partial patch makes a block without points, so the stroke stays over its light.
        let (merged, _) = HdriParams::default()
            .merge_json(r#"{"lights": [{"shape": "stroke", "stroke": {"width_deg": 5, "end_kelvin": 3000}}]}"#)
            .unwrap();
        let got = merged.lights[0].stroke.clone().unwrap();
        assert_eq!((got.width_deg, got.end_kelvin, got.end_rgb), (5.0, Some(3000.0), None));
        assert!(got.points.is_empty(), "no points were given: the rest is the default block");
    }

    #[test]
    fn scrim_params_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            shape: "scrim".to_string(),
            scrim: Some(ScrimParams { source_size: 0.0, frame: f32::NAN }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        assert_eq!(p.lights[0].scrim, Some(ScrimParams { source_size: 0.05, frame: 0.0 }));
        assert_eq!(changed, ["lights[0].scrim.source_size", "lights[0].scrim.frame"]);
        assert_eq!(HdriParams::from_json(&p.to_json()).unwrap(), p);
        let (merged, _) = HdriParams::default()
            .merge_json(r#"{"lights": [{"shape": "scrim", "scrim": {"frame": 0.1}}]}"#)
            .unwrap();
        assert_eq!(merged.lights[0].scrim, Some(ScrimParams { source_size: 0.5, frame: 0.1 }));
    }
}

#[cfg(test)]
mod source_param_tests {
    use super::*;

    #[test]
    fn gel_lists_are_cleaned_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            gels: Some(vec![" CTO_Half".to_string(), "fog".to_string(), "nd_0.3".to_string(), "red".to_string()]),
            ..LightParams::default()
        });
        assert_eq!(p.clamp(), ["lights[0].gels"]);
        assert_eq!(p.lights[0].gels, Some(vec!["cto_half".to_string(), "nd_0.3".to_string()]));
        assert!(p.clamp().is_empty(), "a clean list stays as it is");
        // An empty list is a bare light and is left alone, so a patch can clear the gels.
        p.lights[0].gels = Some(Vec::new());
        assert!(p.clamp().is_empty());
        p.lights[0].gels = Some(vec!["amber".to_string()]);
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        // A light without gels writes no key, so 1a files and 1b files look alike.
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("gels"));
        assert_eq!(HdriParams::from_json(&plain.to_json()).unwrap().lights[0].gels, None);
    }

    #[test]
    fn a_patch_sets_and_clears_gels() {
        let base = HdriParams::default();
        let (merged, report) = base.merge_json(r#"{"lights": [{"gels": ["ctb_quarter", "Blue", "pink"]}]}"#).unwrap();
        assert_eq!(merged.lights[0].gels, Some(vec!["ctb_quarter".to_string(), "blue".to_string()]));
        assert_eq!(report, ["lights[0].gels"], "the spelling fix and the dropped third gel are reported once");
        let (cleared, report) = merged.merge_json(r#"{"lights": [{"gels": []}]}"#).unwrap();
        assert_eq!(cleared.lights[0].gels, Some(Vec::new()));
        assert!(report.is_empty(), "{report:?}");
        let err = base.merge_json(r#"{"lights": [{"gels": "red"}]}"#).unwrap_err();
        assert!(err.contains("do not fit"), "{err}");
    }

    #[test]
    fn sources_are_known_names_or_nothing() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams { source: Some(" Sodium_Low_Pressure".to_string()), ..LightParams::default() });
        p.lights.push(LightParams { source: Some("candle".to_string()), ..LightParams::default() });
        p.lights.push(LightParams { source: Some("halogen".to_string()), ..LightParams::default() });
        // The parametric LED's knob is held to 0..1; NaN is the default quality.
        p.lights.push(LightParams { source: Some("led".to_string()), source_quality: Some(1.5), ..LightParams::default() });
        p.lights.push(LightParams { source: Some("led".to_string()), source_quality: Some(f32::NAN), ..LightParams::default() });
        assert_eq!(p.clamp(), ["lights[0].source", "lights[1].source", "lights[3].source_quality", "lights[4].source_quality"]);
        assert_eq!(p.lights[0].source.as_deref(), Some("sodium_low_pressure"));
        assert_eq!(p.lights[1].source, None, "an unknown source hands the colour back to the Kelvin");
        assert_eq!(p.lights[2].source.as_deref(), Some("halogen"));
        assert_eq!(p.lights[3].source_quality, Some(1.0));
        assert_eq!(p.lights[4].source_quality, Some(crate::hdri::spectrum::LED_QUALITY_DEFAULT));
        assert!(p.clamp().is_empty());
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("source"));
    }

    #[test]
    fn a_patch_sets_and_clears_the_source() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"source": "led_warm_cri95", "tint": 0.1}]}"#)
            .unwrap();
        assert_eq!(merged.lights[0].source.as_deref(), Some("led_warm_cri95"));
        assert!(report.is_empty(), "{report:?}");
        // A lights array replaces the list, so leaving the key out clears it; so does null.
        let (cleared, _) = merged.merge_json(r#"{"lights": [{"source": null}]}"#).unwrap();
        assert_eq!(cleared.lights[0].source, None);
        let (unknown, report) = merged.merge_json(r#"{"lights": [{"source": "candle"}]}"#).unwrap();
        assert_eq!(unknown.lights[0].source, None);
        assert_eq!(report, ["lights[0].source"]);
        // The parametric LED with its knob.
        let (knob, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"source": "led", "kelvin": 3000.0, "source_quality": 0.9}]}"#)
            .unwrap();
        assert!(report.is_empty(), "{report:?}");
        assert_eq!((knob.lights[0].source.as_deref(), knob.lights[0].source_quality), (Some("led"), Some(0.9)));
    }

    #[test]
    fn image_blocks_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            image: Some(LightImageParams { key: "C:\\pics\\window.png".to_string(), fit: " FILL".to_string(), alpha_as_mask: true, ..LightImageParams::default() }),
            ..LightParams::default()
        });
        assert_eq!(p.lights[0].image.as_ref().unwrap().fit(), ImageFit::Fill, "the accessor reads a loose spelling");
        assert_eq!(p.clamp(), ["lights[0].image.fit"]);
        let image = p.lights[0].image.clone().unwrap();
        assert_eq!((image.key.as_str(), image.fit.as_str(), image.alpha_as_mask), ("C:\\pics\\window.png", "fill", true));
        // A key is a path: backslashes and spaces survive the file format.
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        p.lights[0].image.as_mut().unwrap().fit = "tile".to_string();
        p.lights[0].image.as_mut().unwrap().key = "k".repeat(2000);
        assert_eq!(p.clamp(), ["lights[0].image.key", "lights[0].image.fit"]);
        assert_eq!(p.lights[0].image.as_ref().unwrap().fit, "stretch");
        assert_eq!(p.lights[0].image.as_ref().unwrap().key.len(), 1024);
        assert_eq!(ImageFit::Fit.as_str(), IMAGE_FIT_NAMES[1]);
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("image"));
    }

    /// P1, P2: Penumbra and Mask softness are Options, so a preset saved
    /// before them loads (with neither), and clamp() holds them to
    /// `IMAGE_PENUMBRA_RANGE` and `IMAGE_MASK_SOFTNESS_RANGE` (NaN to 0, the
    /// default) and reports it, as it does every other field.
    #[test]
    fn a_pictures_penumbra_and_mask_softness_are_held_and_old_presets_load() {
        let mut p = HdriParams::default();
        for value in [-0.5, 1.5, f32::NAN, f32::INFINITY, 0.3, 0.0, 1.0] {
            let image = LightImageParams { key: "a.png".to_string(), penumbra: Some(value), mask_softness: Some(value), ..LightImageParams::default() };
            p.lights.push(LightParams { image: Some(image), ..LightParams::default() });
        }
        let held: Vec<String> = (0..4).flat_map(|i| [format!("lights[{i}].image.penumbra"), format!("lights[{i}].image.mask_softness")]).collect();
        assert_eq!(p.clamp(), held);
        let want = [Some(0.0), Some(1.0), Some(0.0), Some(1.0), Some(0.3), Some(0.0), Some(1.0)];
        let penumbras: Vec<Option<f32>> = p.lights.iter().map(|l| l.image.as_ref().unwrap().penumbra).collect();
        let softnesses: Vec<Option<f32>> = p.lights.iter().map(|l| l.image.as_ref().unwrap().mask_softness).collect();
        assert_eq!((penumbras.as_slice(), softnesses.as_slice()), (&want[..], &want[..]));
        assert!(p.clamp().is_empty(), "held values stay");
        assert_eq!((IMAGE_PENUMBRA_RANGE, IMAGE_MASK_SOFTNESS_RANGE), ((0.0, 1.0), (0.0, 1.0)));
        // Saved and loaded with the settings, as the other picture fields are.
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        // A file from before P1: its image blocks have neither field.
        let mut tree = JsonValue::deserialize_json(&p.to_json()).unwrap();
        for light in tree.get_mut("lights").and_then(JsonValue::as_array_mut).unwrap() {
            let Some(JsonValue::Object(image)) = light.get_mut("image") else { panic!("an image block") };
            assert!(image.remove("penumbra").is_some() && image.remove("mask_softness").is_some());
        }
        let old = tree.serialize_json();
        assert!(!old.contains("penumbra") && !old.contains("mask_softness"));
        let mut loaded = HdriParams::from_json(&old).expect("a preset saved before P1 loads");
        assert!(loaded.clamp().is_empty());
        for light in &loaded.lights {
            let image = light.image.as_ref().unwrap();
            assert_eq!((image.penumbra, image.mask_softness, image.penumbra(), image.mask_softness()), (None, None, 0.0, 0.0));
        }
        // A patch names them as it names the other picture fields.
        let (merged, report) = HdriParams::default().merge_json(r#"{"lights": [{"image": {"key": "a.png", "penumbra": 7, "mask_softness": 3}}]}"#).unwrap();
        let image = merged.lights[0].image.as_ref().unwrap();
        assert_eq!((image.penumbra, image.mask_softness), (Some(1.0), Some(1.0)));
        assert_eq!(report, ["lights[0].image.penumbra", "lights[0].image.mask_softness"]);
    }

    /// P2: a preset saved with P1's Soften mask edges tick (`soften_mask`,
    /// a bool, only between P1 and P2) still loads: `from_json` reads
    /// leniently, so the field this build no longer has is skipped and the
    /// picture's cut-out edges load sharp (Mask softness None), the rest of
    /// the block as it was; saved again it is gone. A patch that still names
    /// it is told so ("ignored"), as for any unknown key.
    #[test]
    fn a_preset_saved_with_p1s_soften_mask_still_loads() {
        // A preset as P1 saved it: its picture's block carries the tick.
        let image = LightImageParams { key: "logo.png".to_string(), fit: "fit".to_string(), alpha_as_mask: true, penumbra: Some(0.3), mask_softness: None };
        let saved = HdriParams { lights: vec![LightParams { name: "Logo".to_string(), image: Some(image), ..LightParams::default() }], ..HdriParams::default() };
        let mut tree = JsonValue::deserialize_json(&saved.to_json()).unwrap();
        let Some(JsonValue::Object(block)) = tree.get_mut("lights").and_then(JsonValue::as_array_mut).unwrap()[0].get_mut("image") else { panic!("an image block") };
        block.remove("mask_softness");
        block.insert("soften_mask".to_string(), JsonValue::Bool(true));
        let p1 = tree.serialize_json();
        assert!(p1.contains(r#""soften_mask":true"#) && !p1.contains("mask_softness\""), "{p1}");
        let loaded = HdriParams::from_json(&p1).expect("a P1 preset with soften_mask loads");
        let image = loaded.lights[0].image.as_ref().expect("its picture");
        assert_eq!((image.key.as_str(), image.fit.as_str(), image.alpha_as_mask, image.penumbra), ("logo.png", "fit", true, Some(0.3)));
        assert_eq!((image.mask_softness, image.mask_softness()), (None, 0.0));
        assert_eq!(loaded.lights[0].name, "Logo");
        assert!(!loaded.to_json().contains("soften_mask"), "never written back");
        let (merged, report) = HdriParams::default().merge_json(r#"{"lights": [{"image": {"key": "a.png", "soften_mask": true}}]}"#).unwrap();
        assert_eq!(merged.lights[0].image.as_ref().unwrap().mask_softness, None);
        assert_eq!(report, ["ignored: lights[0].image.soften_mask"]);
    }

    #[test]
    fn a_partial_image_patch_is_completed_from_the_defaults() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"image": {"key": "a.png"}}]}"#)
            .unwrap();
        assert_eq!(merged.lights[0].image, Some(LightImageParams { key: "a.png".to_string(), ..LightImageParams::default() }));
        assert!(report.is_empty(), "{report:?}");
        let (merged, report) = merged.merge_json(r#"{"lights": [{"image": {"key": "a.png", "fit": "fit", "zoom": 2}}]}"#).unwrap();
        assert_eq!(merged.lights[0].image.as_ref().unwrap().fit(), ImageFit::Fit);
        assert_eq!(report, ["ignored: lights[0].image.zoom"]);
    }
}

#[cfg(test)]
mod fx_param_tests {
    use super::*;

    #[test]
    fn clamp_fixes_gobo_fields() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            gobo: Some(GoboParams { kind: "portal".to_string(), panes_x: 99, bar: f32::NAN, tilt_deg: 400.0, slat_width: 3.0, ..GoboParams::default() }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        let g = p.lights[0].gobo.as_ref().expect("the gobo stays");
        assert_eq!(g.kind, "window", "an unknown kind falls back to the first");
        assert_eq!(g.panes_x, 16);
        assert_eq!(g.bar, GoboParams::default().bar, "NaN becomes the default");
        assert_eq!(g.tilt_deg, 90.0);
        assert_eq!(g.slat_width, 1.5);
        assert_eq!(
            changed,
            ["lights[0].gobo.kind", "lights[0].gobo.panes_x", "lights[0].gobo.bar", "lights[0].gobo.tilt_deg", "lights[0].gobo.slat_width"]
        );
        // The typed accessor reads a loose spelling before clamp() fixes it.
        let loose = GoboParams { kind: " Blinds".to_string(), ..GoboParams::default() };
        assert_eq!(loose.kind(), GoboKind::Blinds);
        for (name, kind) in GOBO_KIND_NAMES.iter().zip([GoboKind::Window, GoboKind::Blinds, GoboKind::Leaves, GoboKind::Fern, GoboKind::Grid, GoboKind::Image]) {
            assert_eq!(GoboParams { kind: name.to_string(), ..GoboParams::default() }.kind(), kind);
            assert_eq!(kind.as_str(), *name);
        }
    }

    #[test]
    fn a_partial_gobo_patch_is_completed_from_the_defaults() {
        let patch = r#"{"lights": [{"name": "Window", "gobo": {"kind": "blinds", "slats": 20.4}}]}"#;
        let (merged, report) = HdriParams::default().merge_json(patch).expect("the patch merges");
        let g = merged.lights[0].gobo.as_ref().expect("a gobo");
        assert_eq!(g.kind, "blinds");
        assert_eq!(g.slats, 20);
        assert_eq!(g.panes_x, GoboParams::default().panes_x, "a field left out takes its default");
        assert_eq!(report, ["lights[0].gobo.slats"], "20.4 was rounded to a whole number of slats");
        // null takes the gobo away again.
        let (cleared, _) = merged.merge_json(r#"{"lights": [{"gobo": null}]}"#).expect("null merges");
        assert_eq!(cleared.lights[0].gobo, None);
    }

    #[test]
    fn a_gobo_round_trips_through_json() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            gobo: Some(GoboParams { kind: "blinds".to_string(), slats: 20, vertical: true, fill_sky: true, ..GoboParams::default() }),
            ..LightParams::default()
        });
        assert_eq!(HdriParams::from_json(&p.to_json()).expect("loads"), p);
        let plain = HdriParams { lights: vec![LightParams::default()], ..HdriParams::default() };
        assert!(!plain.to_json().contains("gobo"), "an absent gobo is not written");
        assert_eq!(HdriParams::from_json(&plain.to_json()).expect("loads").lights[0].gobo, None);
    }

    #[test]
    fn a_gobo_picture_name_is_kept_as_long_as_a_light_pictures_key() {
        // Both are keys of the same ImageSet (a path as text): a name cut
        // shorter than the key the picture was loaded under is no gobo.
        let mut p = HdriParams::default();
        let long = format!("C:\\{}\\mask.png", "d".repeat(990));
        p.lights.push(LightParams { gobo: Some(GoboParams { kind: "image".to_string(), image: long.clone(), ..GoboParams::default() }), ..LightParams::default() });
        assert!(p.clamp().is_empty());
        assert_eq!(p.lights[0].gobo.as_ref().unwrap().image, long);
        p.lights[0].gobo.as_mut().unwrap().image = "k".repeat(2000);
        assert_eq!(p.clamp(), ["lights[0].gobo.image"]);
        assert_eq!(p.lights[0].gobo.as_ref().unwrap().image.len(), IMAGE_KEY_LIMIT);
    }

    #[test]
    fn clamp_fixes_aim_fields() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            aim: Some(AimParams { yaw_deg: 200.0, grid: -1.0, barn_doors: [0.0, 120.0, f32::NAN, 45.0], ..AimParams::default() }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        let a = p.lights[0].aim.clone().expect("the aim stays");
        assert_eq!((a.yaw_deg, a.grid, a.barn_doors), (90.0, 0.0, [0.0, 90.0, 0.0, 45.0]));
        assert_eq!(changed, ["lights[0].aim.yaw_deg", "lights[0].aim.grid", "lights[0].aim.barn_doors[1]", "lights[0].aim.barn_doors[2]"]);
        assert!(!a.is_neutral());
        assert!(AimParams::default().is_neutral());
    }

    #[test]
    fn a_partial_aim_patch_is_completed_and_round_trips() {
        let (merged, report) = HdriParams::default().merge_json(r#"{"lights": [{"aim": {"grid": 0.5, "barn_doors": [90, 0, 0, 0]}}]}"#).expect("merges");
        assert_eq!(merged.lights[0].aim, Some(AimParams { grid: 0.5, barn_doors: [90.0, 0.0, 0.0, 0.0], ..AimParams::default() }));
        assert!(report.is_empty(), "{report:?}");
        assert_eq!(HdriParams::from_json(&merged.to_json()).expect("loads"), merged);
        // Four doors, no fewer: a short list does not fit.
        assert!(HdriParams::default().merge_json(r#"{"lights": [{"aim": {"barn_doors": [90, 0]}}]}"#).is_err());
    }

    #[test]
    fn imperfection_params_clamp_merge_and_round_trip() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams {
            imperfections: Some(ImperfectionParams { wrinkle: 3.0, dust: f32::NAN, weave: -1.0, seed: 4 }),
            ..LightParams::default()
        });
        let changed = p.clamp();
        let im = p.lights[0].imperfections.clone().expect("it stays");
        assert_eq!((im.wrinkle, im.dust, im.weave, im.seed), (1.0, 0.0, 0.0, 4));
        assert_eq!(changed, ["lights[0].imperfections.wrinkle", "lights[0].imperfections.dust", "lights[0].imperfections.weave"]);
        assert!(!im.is_clear() && ImperfectionParams::default().is_clear());
        let (merged, report) = HdriParams::default().merge_json(r#"{"lights": [{"imperfections": {"dust": 0.3, "seed": 8.2}}]}"#).expect("merges");
        assert_eq!(merged.lights[0].imperfections, Some(ImperfectionParams { dust: 0.3, seed: 8, ..ImperfectionParams::default() }));
        assert_eq!(report, ["lights[0].imperfections.seed"], "8.2 was rounded");
        assert_eq!(HdriParams::from_json(&merged.to_json()).expect("loads"), merged);
    }
}

/// Redesign §5 (plan R1): an entry of `HdriParams::lights` is a light or a
/// shadow caster (`kind`), and a caster carries its own block (`caster`).
#[cfg(test)]
mod layer_param_tests {
    use super::*;

    /// A default light with this `kind`.
    fn with_kind(kind: Option<&str>) -> LightParams {
        LightParams { kind: kind.map(str::to_string), ..LightParams::default() }
    }

    /// The entry as clamp() leaves it, alone in a map, and what clamp() reported.
    fn clamp_one(light: LightParams) -> (LightParams, Vec<String>) {
        let mut p = HdriParams { lights: vec![light], ..HdriParams::default() };
        let changed = p.clamp();
        (p.lights.remove(0), changed)
    }

    /// A studio map whose one light has every optional block older than R1,
    /// held by clamp(): what a preset saved before R1 may hold.
    fn every_older_block() -> HdriParams {
        let mut p = HdriParams { mode: "studio".to_string(), ..HdriParams::default() };
        p.lights.push(LightParams {
            key: true,
            rgb: Some([0.9, 0.8, 0.7]),
            polygon: Some(PolygonParams::default()),
            stroke: Some(StrokeParams { points: vec![[10.0, 20.0], [40.0, 30.0]], ..StrokeParams::default() }),
            scrim: Some(ScrimParams::default()),
            gels: Some(Vec::new()),
            source: Some("led".to_string()),
            source_quality: Some(0.5),
            image: Some(LightImageParams { key: "a.png".to_string(), penumbra: Some(0.2), mask_softness: Some(0.4), ..LightImageParams::default() }),
            gobo: Some(GoboParams { kind: "leaves".to_string(), ..GoboParams::default() }),
            aim: Some(AimParams::default()),
            imperfections: Some(ImperfectionParams::default()),
            ..LightParams::default()
        });
        p.clamp();
        p
    }

    #[test]
    fn a_layer_kind_reads_loose_spellings_and_falls_back_to_light() {
        assert_eq!(LIGHT_KIND_NAMES, &["light", "caster"]);
        for (kind, want) in [
            (None, LightKind::Light),
            (Some("light"), LightKind::Light),
            (Some("caster"), LightKind::Caster),
            (Some(" Caster "), LightKind::Caster),
            (Some("CASTER"), LightKind::Caster),
            (Some(""), LightKind::Light),
            (Some("shadow"), LightKind::Light),
        ] {
            let light = with_kind(kind);
            assert_eq!(light.kind(), want, "{kind:?}");
            assert_eq!(light.is_caster(), want == LightKind::Caster, "{kind:?}");
        }
        // Each listed name reads back as the variant spelled that way, and
        // clamp() keeps it as it is.
        let mut seen = Vec::new();
        for name in LIGHT_KIND_NAMES {
            let (held, changed) = clamp_one(with_kind(Some(*name)));
            seen.push(held.kind().as_str());
            assert!(changed.is_empty(), "kind {name}: {changed:?}");
            assert_eq!(held.kind.as_deref(), Some(*name));
        }
        assert_eq!(seen, LIGHT_KIND_NAMES);
        // A loose spelling is fixed once, and reported.
        let (held, changed) = clamp_one(with_kind(Some(" Caster")));
        assert_eq!(changed, ["lights[0].kind"]);
        assert_eq!(held.kind.as_deref(), Some("caster"));
        // Every variant has its name listed: the match stops compiling when a
        // variant is added, until it is listed too.
        for k in [LightKind::Light, LightKind::Caster] {
            match k {
                LightKind::Light | LightKind::Caster => assert!(LIGHT_KIND_NAMES.contains(&k.as_str()), "{k:?}"),
            }
        }
    }

    #[test]
    fn clamp_holds_every_caster_field_and_reports_it() {
        assert_eq!(CASTER_STRENGTH_RANGE, (0.0, 1.0));
        assert_eq!(CASTER_SOFTNESS_RANGE, (0.0, 1.0));
        assert_eq!(CASTER_BLOOM_THRESHOLD_EV_RANGE, (-10.0, 20.0));
        assert_eq!(CASTER_BLOOM_STRENGTH_RANGE, (0.0, 1.0));
        assert_eq!(CASTER_BLOOM_LENGTH_RANGE, (0.0, 45.0));
        assert_eq!(
            CasterParams::default(),
            CasterParams { strength: 1.0, tint: [0.0; 3], softness: 0.0, bloom_threshold_ev: 10.0, bloom_strength: 0.0, bloom_length_deg: 5.0 }
        );
        let all = [
            "lights[0].caster.strength",
            "lights[0].caster.tint",
            "lights[0].caster.softness",
            "lights[0].caster.bloom_threshold_ev",
            "lights[0].caster.bloom_strength",
            "lights[0].caster.bloom_length_deg",
        ];
        let held = |block: CasterParams| {
            let (light, changed) = clamp_one(LightParams { caster: Some(block), ..LightParams::new_caster() });
            (light.caster.expect("the block stays"), changed)
        };
        // Above every range: each field to its top, reported in field order.
        let (high, changed) = held(CasterParams { strength: 2.0, tint: [2.0, 0.5, 1.5], softness: 1.5, bloom_threshold_ev: 50.0, bloom_strength: 3.0, bloom_length_deg: 90.0 });
        assert_eq!(high, CasterParams { strength: 1.0, tint: [1.0, 0.5, 1.0], softness: 1.0, bloom_threshold_ev: 20.0, bloom_strength: 1.0, bloom_length_deg: 45.0 });
        assert_eq!(changed, all);
        // Below: each field to its bottom.
        let (low, changed) = held(CasterParams { strength: -1.0, tint: [-1.0, 0.5, -0.2], softness: -0.5, bloom_threshold_ev: -50.0, bloom_strength: -1.0, bloom_length_deg: -5.0 });
        assert_eq!(low, CasterParams { strength: 0.0, tint: [0.0, 0.5, 0.0], softness: 0.0, bloom_threshold_ev: -10.0, bloom_strength: 0.0, bloom_length_deg: 0.0 });
        assert_eq!(changed, all);
        // NaN: each field to its default (a tint channel to black).
        let nan = f32::NAN;
        let (fallback, changed) = held(CasterParams { strength: nan, tint: [nan, 0.5, nan], softness: nan, bloom_threshold_ev: nan, bloom_strength: nan, bloom_length_deg: nan });
        assert_eq!(fallback, CasterParams { tint: [0.0, 0.5, 0.0], ..CasterParams::default() });
        assert_eq!(changed, all);
        // Infinities: the nearest bound.
        let (inf, changed) = held(CasterParams {
            strength: f32::INFINITY,
            tint: [f32::INFINITY; 3],
            softness: f32::NEG_INFINITY,
            bloom_threshold_ev: f32::INFINITY,
            bloom_strength: f32::NEG_INFINITY,
            bloom_length_deg: f32::INFINITY,
        });
        assert_eq!(inf, CasterParams { strength: 1.0, tint: [1.0; 3], softness: 0.0, bloom_threshold_ev: 20.0, bloom_strength: 0.0, bloom_length_deg: 45.0 });
        assert_eq!(changed, all);
        // The ends of each range stay put, and clamped() holds what clamp() holds.
        for block in [high, low] {
            assert_eq!(held(block.clone()), (block.clone(), Vec::<String>::new()));
            assert_eq!(block.clamped(), block);
        }
        let wild = CasterParams { strength: 2.0, bloom_length_deg: nan, ..CasterParams::default() };
        assert_eq!(wild.clamped(), CasterParams { strength: 1.0, ..CasterParams::default() });
    }

    #[test]
    fn an_unknown_kind_is_a_light_and_is_reported() {
        let raw = LightParams { kind: Some("flag".to_string()), shape: "disc".to_string(), key: true, width_deg: 150.0, ..LightParams::default() };
        assert_eq!(raw.kind(), LightKind::Light, "read as a light before clamp()");
        let (held, changed) = clamp_one(raw);
        assert_eq!(changed, ["lights[0].kind"]);
        assert_eq!(held.kind.as_deref(), Some("light"), "it falls back to the first name");
        assert_eq!((held.shape.as_str(), held.key, held.width_deg), ("disc", true, 150.0), "and keeps what a light keeps");
        // A patch that names a kind of its own makes a light too, and says so.
        let (merged, report) = HdriParams::default().merge_json(r#"{"lights": [{"kind": "tree"}]}"#).expect("the patch merges");
        assert_eq!(merged.lights[0].kind.as_deref(), Some("light"));
        assert_eq!(report, ["lights[0].kind"]);
    }

    #[test]
    fn a_caster_is_always_a_rect_and_never_the_key() {
        // Every other shape becomes a rect, reported once (a loose or an
        // unknown spelling too): a caster's pattern lies in its rectangle.
        for shape in ["disc", "ring", "polygon", "stroke", "scrim", " Disc", "hexagon"] {
            let (held, changed) = clamp_one(LightParams { shape: shape.to_string(), ..LightParams::new_caster() });
            assert_eq!(changed, ["lights[0].shape"], "{shape:?}");
            assert_eq!((held.shape.as_str(), held.shape()), ("rect", LightShape::Rect), "{shape:?}");
        }
        // A caster claimed as the key is not (it adds no light), and a light
        // listed after it keeps the key: the caster never took the one slot.
        let mut p = HdriParams::default();
        p.lights.push(LightParams { key: true, ..LightParams::new_caster() });
        p.lights.push(LightParams { name: "Key".to_string(), key: true, ..LightParams::default() });
        assert_eq!(p.clamp(), ["lights[0].key"]);
        assert!(!p.lights[0].key && p.lights[1].key, "a caster never is the key");
        // clamped() holds both rules, as every reader takes a light through it.
        let held = LightParams { shape: "ring".to_string(), key: true, ..LightParams::new_caster() }.clamped();
        assert_eq!((held.shape.as_str(), held.key), ("rect", false));
        // A light keeps its shape and its key.
        let light = LightParams { shape: "ring".to_string(), key: true, ..LightParams::default() };
        assert_eq!(light.clamped(), light);
    }

    #[test]
    fn a_caster_spans_at_most_120_degrees() {
        let mut p = HdriParams::default();
        p.lights.push(LightParams { width_deg: 150.0, height_deg: 130.0, ..LightParams::new_caster() });
        p.lights.push(LightParams { width_deg: 150.0, height_deg: 130.0, ..LightParams::default() });
        assert_eq!(p.clamp(), ["lights[0].width_deg", "lights[0].height_deg"]);
        assert_eq!((p.lights[0].width_deg, p.lights[0].height_deg), (SHAPED_SPAN_MAX_DEG, SHAPED_SPAN_MAX_DEG), "a caster lives in its gnomonic box");
        assert_eq!((p.lights[1].width_deg, p.lights[1].height_deg), (150.0, 130.0), "a rect light keeps A1's 170");
        // max_span_deg() says what clamp() keeps, for every shape and for a caster.
        for (shape, light_span) in [("rect", 170.0), ("disc", 170.0), ("ring", 170.0), ("polygon", 120.0), ("stroke", 170.0), ("scrim", 120.0)] {
            let light = LightParams { shape: shape.to_string(), ..LightParams::default() };
            let caster = LightParams { shape: shape.to_string(), ..LightParams::new_caster() };
            assert_eq!(light.max_span_deg(), light_span, "a {shape} light");
            assert_eq!(caster.max_span_deg(), SHAPED_SPAN_MAX_DEG, "a caster that says {shape}");
            for l in [light, caster] {
                let wide = LightParams { width_deg: 1000.0, height_deg: 1000.0, ..l.clone() }.clamped();
                assert_eq!((wide.width_deg, wide.height_deg), (l.max_span_deg(), l.max_span_deg()), "{shape}, caster {}", l.is_caster());
            }
        }
    }

    #[test]
    fn a_light_keeps_a_caster_block_it_does_not_read() {
        let block = CasterParams { strength: 3.0, softness: 0.4, ..CasterParams::default() };
        let raw = LightParams { shape: "disc".to_string(), key: true, width_deg: 150.0, caster: Some(block), ..LightParams::default() };
        assert_eq!(raw.kind(), LightKind::Light);
        assert_eq!(raw.caster_params().strength, 1.0, "the accessor reads the block as clamp() holds it");
        assert_eq!(raw.caster.as_ref().unwrap().strength, 3.0, "and leaves the block itself as it is");
        let mut p = HdriParams { lights: vec![raw], ..HdriParams::default() };
        assert_eq!(p.clamp(), ["lights[0].caster.strength"], "held, whatever the kind");
        let held = &p.lights[0];
        assert_eq!(held.caster, Some(CasterParams { strength: 1.0, softness: 0.4, ..CasterParams::default() }));
        assert_eq!((held.shape.as_str(), held.key, held.width_deg), ("disc", true, 150.0), "a light keeps its shape, its key and its span");
        assert!(!held.is_caster());
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()), "and saves the block");
        // With no block, the accessor gives the defaults.
        assert_eq!(LightParams::default().caster_params(), CasterParams::default());
    }

    /// Redesign §5: `kind` and `caster` are Options after every older field,
    /// so a preset saved before them (no such keys) loads as lights with no
    /// caster block, and one saved now is written as before: a light with
    /// neither writes neither key (the derive writes struct order and leaves
    /// a None out; Step 1's probe compares the text itself byte for byte).
    #[test]
    fn old_presets_load_unchanged() {
        use super::super::presets::{preset, PRESET_NAMES};
        let mut sets: Vec<(String, HdriParams)> = PRESET_NAMES.iter().map(|n| (n.to_string(), preset(n).unwrap())).collect();
        sets.push(("every older block".to_string(), every_older_block()));
        for (name, p) in &sets {
            // Saved now: no light writes `kind` or `caster`.
            let tree = JsonValue::deserialize_json(&p.to_json()).unwrap();
            for (i, light) in tree.get("lights").and_then(JsonValue::as_array).unwrap().iter().enumerate() {
                let fields = light.as_object().unwrap_or_else(|| panic!("{name}: lights[{i}] is an object"));
                assert!(!fields.contains_key("kind") && !fields.contains_key("caster"), "{name}: lights[{i}] writes {:?}", sorted_keys(fields));
            }
            // A file from before R1: the same settings with both keys taken
            // out of every light (written here first, then removed).
            let mut marked = p.clone();
            for light in &mut marked.lights {
                light.kind = Some("light".to_string());
                light.caster = Some(CasterParams::default());
            }
            let mut tree = JsonValue::deserialize_json(&marked.to_json()).unwrap();
            for light in tree.get_mut("lights").and_then(JsonValue::as_array_mut).unwrap() {
                let fields = light.as_object_mut().unwrap();
                assert!(fields.remove("kind").is_some() && fields.remove("caster").is_some(), "{name}");
            }
            let old = tree.serialize_json();
            assert!(!old.contains("\"caster\""), "{name}");
            let loaded = HdriParams::from_json(&old).unwrap_or_else(|e| panic!("{name}: a preset saved before R1 loads: {e}"));
            assert_eq!(&loaded, p, "{name}");
            assert!(loaded.lights.iter().all(|l| l.kind.is_none() && l.caster.is_none() && l.kind() == LightKind::Light), "{name}");
            assert_eq!(loaded.to_json(), p.to_json(), "{name}: saved again as it was");
            // The skyline is an Option after every older field: a preset without it
            // loads without one, and one saved now writes no such key.
            assert!(p.skyline.is_none() && loaded.skyline.is_none(), "{name}");
            assert!(!old.contains("\"skyline\"") && !p.to_json().contains("\"skyline\""), "{name}");
            // Plan hdri-7: the night's blocks are Options after every older field: none loads, none is written.
            assert!(p.sky.night.milky_way.is_none() && p.sky.night.nebulas.is_none() && p.sky.night.aurora.is_none(), "{name}");
            for key in ["\"milky_way\"", "\"nebulas\"", "\"aurora\""] {
                assert!(!old.contains(key) && !p.to_json().contains(key), "{name}: {key}");
            }
        }
        // A skyline block round-trips, its settings included.
        let mut p = every_older_block();
        p.mode = "sky".to_string();
        let settings: HashMap<String, f64> = [("ct_d".to_string(), 0.6), ("lt_n".to_string(), 3.0)].into_iter().collect();
        p.skyline = Some(SkylineParams { on: true, seed: 42, turn_deg: -35.5, lights_by_day: 0.25, settings });
        assert!(p.clone().clamp().is_empty());
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()), "presets save the skyline");
        assert_eq!(HdriParams::deserialize_json(&p.serialize_json()).unwrap(), p);
        let off = HdriParams { skyline: Some(SkylineParams { on: false, settings: HashMap::new(), ..p.skyline.clone().unwrap() }), ..p.clone() };
        assert_eq!(HdriParams::from_json(&off.to_json()), Ok(off.clone()), "an empty settings map too");
    }

    #[test]
    fn clamp_holds_the_skyline_and_reports_it() {
        let mut settings: HashMap<String, f64> = HashMap::new();
        settings.insert("ct_d".to_string(), 0.6);
        settings.insert("zz".to_string(), f64::NAN);
        settings.insert("aa".to_string(), f64::INFINITY);
        let mut p = HdriParams { skyline: Some(SkylineParams { on: true, seed: 20_000_000, turn_deg: 270.0, lights_by_day: 1.5, settings }), ..HdriParams::default() };
        let changed = p.clamp();
        assert_eq!(changed, ["skyline.turn_deg", "skyline.lights_by_day", "skyline.seed", "skyline.settings.aa", "skyline.settings.zz"]);
        let held = p.skyline.as_ref().unwrap();
        assert_eq!((held.turn_deg, held.lights_by_day, held.seed), (-90.0, 1.0, SKYLINE_SEED_MAX));
        assert_eq!(held.settings.len(), 1);
        assert_eq!(held.settings.get("ct_d"), Some(&0.6));
        // NaN takes the field's default; the ends stay put; clamped() holds what clamp() holds.
        let nan = SkylineParams { on: false, seed: 0, turn_deg: f32::NAN, lights_by_day: f32::NAN, settings: HashMap::new() };
        assert_eq!(nan.clamped(), SkylineParams { turn_deg: 0.0, lights_by_day: 0.0, ..nan.clone() });
        let ends = SkylineParams { on: true, seed: SKYLINE_SEED_MAX, turn_deg: 180.0, lights_by_day: 0.0, settings: HashMap::new() };
        assert_eq!(ends.clamped(), ends);
        assert!(HdriParams { skyline: Some(ends), ..HdriParams::default() }.clamp().is_empty());
    }

    #[test]
    fn a_caster_round_trips_through_json() {
        let mut p = HdriParams { mode: "studio".to_string(), ..HdriParams::default() };
        p.lights.push(LightParams { name: "Key".to_string(), key: true, ..LightParams::default() });
        p.lights.push(LightParams::new_caster());
        p.lights.push(LightParams {
            name: "Glass".to_string(),
            gobo: Some(GoboParams { kind: "image".to_string(), image: "C:\\masks\\tree.png".to_string(), ..GoboParams::default() }),
            caster: Some(CasterParams { strength: 0.7, tint: [0.2, 0.5, 0.1], softness: 0.3, bloom_threshold_ev: 12.5, bloom_strength: 0.6, bloom_length_deg: 8.0 }),
            ..LightParams::new_caster()
        });
        assert!(p.clone().clamp().is_empty());
        let json = p.to_json();
        assert_eq!(HdriParams::from_json(&json), Ok(p.clone()), "presets save casters");
        let tree = JsonValue::deserialize_json(&json).unwrap();
        let lights = tree.get("lights").and_then(JsonValue::as_array).unwrap();
        let kinds: Vec<Option<&str>> = lights.iter().map(|l| l.get("kind").and_then(JsonValue::as_str)).collect();
        assert_eq!(kinds, [None, Some("caster"), Some("caster")], "a None kind is left out");
        let blocks: Vec<bool> = lights.iter().map(|l| l.get("caster").is_some()).collect();
        assert_eq!(blocks, [false, true, true], "Some(caster) is written, None is left out");
        // Compact JSON too, and the block on its own.
        assert_eq!(HdriParams::deserialize_json(&p.serialize_json()).unwrap(), p);
        let block = p.lights[2].caster.clone().unwrap();
        assert_eq!(CasterParams::deserialize_json(&block.serialize_json()).unwrap(), block);
    }

    #[test]
    fn a_partial_caster_patch_is_completed_from_the_defaults() {
        let patch = r#"{"lights": [{"name": "Tree", "kind": "caster", "caster": {"strength": 0.5, "tint": [0.1, 0.3, 0.1]}}]}"#;
        let (merged, report) = HdriParams::default().merge_json(patch).expect("the patch merges");
        let tree = &merged.lights[0];
        assert!(tree.is_caster());
        assert_eq!(tree.caster, Some(CasterParams { strength: 0.5, tint: [0.1, 0.3, 0.1], ..CasterParams::default() }), "a field left out takes its default");
        assert!(report.is_empty(), "{report:?}");
        // Held and reported as every other block is; an unknown key is named.
        let (held, report) = merged
            .merge_json(r#"{"lights": [{"kind": "caster", "caster": {"bloom_length_deg": 90, "blur": 1}}]}"#)
            .expect("the patch merges");
        assert_eq!(held.lights[0].caster, Some(CasterParams { bloom_length_deg: 45.0, ..CasterParams::default() }));
        assert_eq!(report, ["ignored: lights[0].caster.blur", "lights[0].caster.bloom_length_deg"]);
        // null takes the block away again: the caster reads the defaults.
        let (cleared, _) = merged.merge_json(r#"{"lights": [{"kind": "caster", "caster": null}]}"#).expect("null merges");
        assert_eq!(cleared.lights[0].caster, None);
        assert_eq!(cleared.lights[0].caster_params(), CasterParams::default());
    }

    /// Plan 4's final review M9: a caster a patch adds without a name is
    /// called "Caster", as "+ Caster" calls one, not the default light's
    /// "Light". A name the patch gives is kept, and so is a light's. A name
    /// typed in the panel ("Light", or "Lighthouse" on its way) never goes
    /// through the merge, so clamp() leaves it as it is.
    #[test]
    fn a_caster_a_patch_adds_without_a_name_is_called_caster() {
        let patch = r#"{"lights": [{"kind": "caster"}, {"kind": "Caster ", "name": "Tree"}, {"kind": "caster", "name": ""}, {}, {"kind": "light"}]}"#;
        let (merged, report) = HdriParams::default().merge_json(patch).expect("the patch merges");
        let names: Vec<&str> = merged.lights.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, [DEFAULT_CASTER_NAME, "Tree", "", DEFAULT_LIGHT_NAME, DEFAULT_LIGHT_NAME]);
        assert_eq!(report, ["lights[1].kind"], "the name is no clamp: only the kind's spelling is reported");
        assert_eq!(LightParams::new_caster().name, DEFAULT_CASTER_NAME);
        assert_eq!(LightParams::default().name, DEFAULT_LIGHT_NAME);
        // clamp() keeps a caster's name, whatever it says.
        let mut p = HdriParams::default();
        p.lights.push(LightParams { name: DEFAULT_LIGHT_NAME.to_string(), ..LightParams::new_caster() });
        assert!(p.clamp().is_empty());
        assert_eq!(p.lights[0].name, DEFAULT_LIGHT_NAME);
    }

    #[test]
    fn a_caster_tint_of_the_wrong_length_is_refused_by_its_field() {
        for (patch, says) in [
            (r#"{"lights": [{"kind": "caster", "caster": {"tint": [0.2, 0.3]}}]}"#, "lights[0].caster.tint must be a list of 3 numbers"),
            (r#"{"lights": [{"kind": "caster", "caster": {"tint": [0.2, 0.3, 0.4, 0.5]}}]}"#, "lights[0].caster.tint must be a list of 3 numbers"),
            (r#"{"lights": [{"kind": "caster", "caster": {"tint": "green"}}]}"#, "lights[0].caster.tint must be a list of numbers"),
        ] {
            assert_eq!(HdriParams::default().merge_json(patch).unwrap_err(), says, "{patch}");
        }
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"lights": [{"kind": "caster", "caster": {"tint": [0.2, 0.3, 0.4]}}]}"#)
            .expect("three numbers merge");
        assert_eq!(merged.lights[0].caster_params().tint, [0.2, 0.3, 0.4]);
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn casters_count_toward_the_sixteen_layers() {
        let mut p = HdriParams::default();
        for i in 0..20 {
            let mut layer = if i % 2 == 0 { LightParams::new_caster() } else { LightParams::default() };
            layer.name = format!("L{i}");
            p.lights.push(layer);
        }
        assert_eq!(p.clamp(), ["lights"]);
        assert_eq!(p.lights.len(), LIGHT_LIMIT, "16 layers in all, lights and casters together");
        assert_eq!(p.lights.iter().filter(|l| l.is_caster()).count(), 8);
        assert_eq!(p.lights[15].name, "L15", "the list is cut at the end (the outliner's top)");
        assert!(p.lights[14].is_caster() && !p.lights[15].is_caster());
    }

    #[test]
    fn a_new_caster_is_already_clamped() {
        let c = LightParams::new_caster();
        assert_eq!(c.clamped(), c);
        let mut p = HdriParams { mode: "studio".to_string(), lights: vec![c.clone()], ..HdriParams::default() };
        assert!(p.clamp().is_empty(), "clamp() reports nothing");
        assert_eq!(p.lights[0], c);
        // What "+ Caster" adds (the app names and places it, R6).
        assert_eq!((c.kind(), c.name.as_str(), c.shape(), c.enabled, c.key), (LightKind::Caster, "Caster", LightShape::Rect, true, false));
        assert_eq!((c.azimuth_deg, c.elevation_deg, c.width_deg, c.height_deg, c.roll_deg, c.softness), (0.0, 15.0, 60.0, 45.0, 0.0, 0.0));
        assert_eq!(c.gobo, Some(GoboParams { kind: "leaves".to_string(), distance: 0.0, ..GoboParams::default() }));
        assert_eq!(c.caster, Some(CasterParams::default()));
        // So that one which loses its kind is harmless: a Multiply flag at EV 0.
        assert_eq!((c.blend(), c.intensity_ev), (Blend::Multiply, 0.0));
        let lost = LightParams { kind: None, ..c.clone() };
        assert!(!lost.is_caster() && lost.blend() == Blend::Multiply);
    }
}

#[cfg(test)]
mod base_image_param_tests {
    use super::*;

    #[test]
    fn base_image_params_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        assert_eq!(p.base_image, None);
        assert!(!p.to_json().contains("base_image"), "an absent base image is not written");
        p.base_image = Some(BaseImageParams {
            key: "C:/maps/field.hdr".to_string(),
            yaw_deg: 190.0,
            ev: 99.0,
            remove_sun: true,
            replace_sky: true,
            horizon_deg: -80.0,
            feather_deg: f32::NAN,
            sky_key: 3.0,
        });
        let changed = p.clamp();
        assert_eq!(
            changed,
            ["base_image.yaw_deg", "base_image.ev", "base_image.horizon_deg", "base_image.feather_deg", "base_image.sky_key"]
        );
        let base = p.base_image.clone().unwrap();
        assert_eq!(base.key, "C:/maps/field.hdr");
        assert_eq!(
            (base.yaw_deg, base.ev, base.horizon_deg, base.feather_deg, base.sky_key),
            (-170.0, 10.0, -45.0, BaseImageParams::default().feather_deg, 1.0)
        );
        assert!(base.remove_sun && base.replace_sky);
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        assert!(p.clamp().is_empty(), "clamping twice changes nothing");
        // A key longer than any path is cut, and that is reported.
        p.base_image.as_mut().unwrap().key = "k".repeat(2000);
        assert_eq!(p.clamp(), ["base_image.key"]);
        assert_eq!(p.base_image.as_ref().unwrap().key.len(), 1024);
    }

    #[test]
    fn a_partial_base_image_patch_is_completed_and_null_removes_it() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"base_image": {"key": "field.hdr", "replace_sky": true}}"#)
            .unwrap();
        assert_eq!(
            merged.base_image,
            Some(BaseImageParams { key: "field.hdr".to_string(), replace_sky: true, ..BaseImageParams::default() })
        );
        assert!(report.is_empty(), "{report:?}");
        let (cleared, _) = merged.merge_json(r#"{"base_image": null}"#).unwrap();
        assert_eq!(cleared.base_image, None);
        let (same, report) = merged.merge_json(r#"{"base_image": {"flavour": 1}}"#).unwrap();
        assert_eq!(same, merged);
        assert_eq!(report, ["ignored: base_image.flavour"]);
    }
}

#[cfg(test)]
mod volumetric_param_tests {
    use super::*;

    #[test]
    fn volumetric_params_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        assert_eq!(p.sky.volumetric, None);
        assert!(!p.to_json().contains("volumetric"), "the 2D layer is the default and writes nothing");
        p.sky.volumetric = Some(VolumetricParams { density: 9.0, base_m: 50.0, top_m: 100.0, erosion: f32::NAN, steps: 500 });
        let changed = p.clamp();
        assert_eq!(
            changed,
            [
                "sky.volumetric.density",
                "sky.volumetric.base_m",
                "sky.volumetric.top_m",
                "sky.volumetric.erosion",
                "sky.volumetric.steps"
            ]
        );
        let v = p.sky.volumetric.clone().unwrap();
        assert_eq!(
            (v.density, v.base_m, v.top_m, v.erosion, v.steps),
            (4.0, 200.0, 400.0, VolumetricParams::default().erosion, 96)
        );
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        assert!(p.clamp().is_empty(), "clamping twice changes nothing");
        // The top clears the base by 200 m wherever the base is dragged.
        p.sky.volumetric = Some(VolumetricParams { base_m: 8000.0, top_m: 3000.0, ..VolumetricParams::default() });
        assert_eq!(p.clamp(), ["sky.volumetric.top_m"]);
        assert_eq!(p.sky.volumetric.as_ref().unwrap().top_m, 8200.0);
        // The defaults are inside their own ranges.
        p.sky.volumetric = Some(VolumetricParams::default());
        assert!(p.clamp().is_empty());
    }

    #[test]
    fn a_partial_volumetric_patch_is_completed_and_rounds_its_steps() {
        let (merged, report) = HdriParams::default()
            .merge_json(r#"{"sky": {"volumetric": {"density": 2, "steps": 40.4}}}"#)
            .unwrap();
        assert_eq!(merged.sky.volumetric, Some(VolumetricParams { density: 2.0, steps: 40, ..VolumetricParams::default() }));
        assert_eq!(report, ["sky.volumetric.steps"], "40.4 was rounded to a whole number of steps");
        let (flat, _) = merged.merge_json(r#"{"sky": {"volumetric": null}}"#).unwrap();
        assert_eq!(flat.sky.volumetric, None);
    }
}

#[cfg(test)]
mod weather_param_tests {
    use super::*;

    #[test]
    fn every_weather_kind_round_trips() {
        assert_eq!(WEATHER_KIND_NAMES.len(), 7);
        for (i, name) in WEATHER_KIND_NAMES.iter().enumerate() {
            let mut p = HdriParams::default();
            p.sky.weather = Some(WeatherParams {
                kind: name.to_string(),
                fog: 0.25,
                rain: 0.5,
                wet_ground: 0.75,
                snow: i % 2 == 0,
                lightning: i % 3 == 0,
                seed: 7 + i as u32,
            });
            assert!(p.clamp().is_empty(), "{name} is a documented kind");
            assert_eq!(p.sky.weather.as_ref().unwrap().kind().as_str(), *name);
            assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()), "{name}");
        }
        assert_eq!(WeatherParams::default().kind(), WeatherKind::Clear);
        assert_eq!(WeatherKind::Storm.as_str(), "storm");
        assert!(!HdriParams::default().to_json().contains("weather"), "no weather writes nothing");
    }

    #[test]
    fn weather_params_clamp_and_forgive_spelling() {
        let mut p = HdriParams::default();
        p.sky.weather = Some(WeatherParams {
            kind: " Storm".to_string(),
            fog: 4.0,
            rain: -1.0,
            wet_ground: f32::NAN,
            ..WeatherParams::default()
        });
        assert_eq!(p.sky.weather.as_ref().unwrap().kind(), WeatherKind::Storm, "the accessor reads loose spellings");
        let changed = p.clamp();
        assert_eq!(changed, ["sky.weather.kind", "sky.weather.fog", "sky.weather.rain", "sky.weather.wet_ground"]);
        let w = p.sky.weather.clone().unwrap();
        assert_eq!((w.kind.as_str(), w.fog, w.rain, w.wet_ground), ("storm", 1.0, 0.0, 0.0));
        // An unknown kind falls back to clear.
        p.sky.weather.as_mut().unwrap().kind = "monsoon".to_string();
        assert_eq!(p.clamp(), ["sky.weather.kind"]);
        assert_eq!(p.sky.weather.as_ref().unwrap().kind, "clear");
    }

    #[test]
    fn a_kind_alone_is_a_complete_weather_patch() {
        let (merged, report) = HdriParams::default().merge_json(r#"{"sky": {"weather": {"kind": "rain"}}}"#).unwrap();
        assert_eq!(merged.sky.weather, Some(WeatherParams { kind: "rain".to_string(), ..WeatherParams::default() }));
        assert!(report.is_empty(), "{report:?}");
        let (seeded, report) = merged.merge_json(r#"{"sky": {"weather": {"seed": 12.4}}}"#).unwrap();
        assert_eq!(seeded.sky.weather.as_ref().unwrap().seed, 12);
        assert_eq!(seeded.sky.weather.as_ref().unwrap().kind, "rain", "the rest of the block stays");
        assert_eq!(report, ["sky.weather.seed"]);
        let (off, _) = seeded.merge_json(r#"{"sky": {"weather": null}}"#).unwrap();
        assert_eq!(off.sky.weather, None);
    }
}

#[cfg(test)]
mod night_param_tests {
    use super::*;

    /// Plan hdri-7 §1: the three blocks are Options after every older field; a block's defaults sit inside its
    /// ranges, clamp() holds every field and reports it once (a seed is not clamped), and the blocks round-trip.
    #[test]
    fn the_night_blocks_clamp_and_round_trip() {
        let mut p = HdriParams::default();
        assert!(p.sky.night.milky_way.is_none() && p.sky.night.nebulas.is_none() && p.sky.night.aurora.is_none());
        p.sky.night.milky_way = Some(MilkyWayParams::default());
        p.sky.night.nebulas = Some(NebulaParams::default());
        p.sky.night.aurora = Some(AuroraParams::default());
        assert!(p.clamp().is_empty(), "the defaults are inside their own ranges");
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
        // owner, 2026-10-24: at Strength 1 the default arc was a faint smear at the night presets' Auto exposure
        assert_eq!(AuroraParams::default().strength, 3.0, "the aurora's default Strength");
        p.sky.night.milky_way = Some(MilkyWayParams { on: true, strength: f32::INFINITY, turn_deg: 370.0, tilt_deg: f32::NAN, core: -1.0, dust: 3.0 });
        p.sky.night.nebulas = Some(NebulaParams { on: true, amount: 2.0, size: 0.0, brightness: f32::NEG_INFINITY, realism: f32::NAN, seed: u32::MAX });
        p.sky.night.aurora = Some(AuroraParams {
            on: true,
            strength: 0.0,
            color: " Magenta Tops ".to_string(),
            mix: [f32::NAN, 3.0, -1.0],
            direction_deg: -90.0,
            height_deg: 90.0,
            width_deg: 5.0,
            folds: 2.0,
            rays: f32::NAN,
            seed: 0,
        });
        let changed = p.clamp();
        assert_eq!(
            changed,
            [
                "sky.night.milky_way.strength", "sky.night.milky_way.turn_deg", "sky.night.milky_way.tilt_deg", "sky.night.milky_way.core",
                "sky.night.milky_way.dust", "sky.night.nebulas.amount", "sky.night.nebulas.size", "sky.night.nebulas.brightness",
                "sky.night.nebulas.realism", "sky.night.aurora.strength", "sky.night.aurora.color", "sky.night.aurora.mix",
                "sky.night.aurora.direction_deg", "sky.night.aurora.height_deg", "sky.night.aurora.width_deg", "sky.night.aurora.folds",
                "sky.night.aurora.rays",
            ]
        );
        let mw = p.sky.night.milky_way.clone().unwrap();
        assert_eq!((mw.strength, mw.turn_deg, mw.tilt_deg, mw.core, mw.dust), (4.0, 10.0, 0.0, 0.0, 2.0));
        let neb = p.sky.night.nebulas.clone().unwrap();
        assert_eq!((neb.amount, neb.size, neb.brightness, neb.realism, neb.seed), (1.0, 0.25, 0.0, 0.0, u32::MAX));
        let aur = p.sky.night.aurora.clone().unwrap();
        assert_eq!(
            (aur.strength, aur.color.as_str(), aur.mix, aur.direction_deg, aur.height_deg, aur.width_deg, aur.folds, aur.rays),
            (0.02, "magenta tops", [AURORA_PRESET_MIX[0][0], 2.0, 0.0], 270.0, 75.0, 10.0, 1.0, 0.5)
        );
        assert!(p.clamp().is_empty(), "clamping twice changes nothing");
        assert_eq!(HdriParams::from_json(&p.to_json()), Ok(p.clone()));
    }

    /// Every range the inspector and the assistant state is what clamp() keeps (D17); the two azimuths wrap.
    #[test]
    fn the_night_ranges_are_what_clamp_keeps() {
        fn full() -> HdriParams {
            let mut p = HdriParams::default();
            p.sky.night.milky_way = Some(MilkyWayParams::default());
            p.sky.night.nebulas = Some(NebulaParams::default());
            p.sky.night.aurora = Some(AuroraParams::default());
            p
        }
        type At = fn(&mut HdriParams) -> &mut f32;
        let fields: [(&str, (f32, f32), At); 15] = [
            ("milky_way.strength", MILKY_WAY_STRENGTH_RANGE, |p| &mut p.sky.night.milky_way.as_mut().unwrap().strength),
            ("milky_way.tilt_deg", MILKY_WAY_TILT_RANGE, |p| &mut p.sky.night.milky_way.as_mut().unwrap().tilt_deg),
            ("milky_way.core", MILKY_WAY_CORE_RANGE, |p| &mut p.sky.night.milky_way.as_mut().unwrap().core),
            ("milky_way.dust", MILKY_WAY_DUST_RANGE, |p| &mut p.sky.night.milky_way.as_mut().unwrap().dust),
            ("nebulas.amount", NEBULA_AMOUNT_RANGE, |p| &mut p.sky.night.nebulas.as_mut().unwrap().amount),
            ("nebulas.size", NEBULA_SIZE_RANGE, |p| &mut p.sky.night.nebulas.as_mut().unwrap().size),
            ("nebulas.brightness", NEBULA_BRIGHTNESS_RANGE, |p| &mut p.sky.night.nebulas.as_mut().unwrap().brightness),
            ("nebulas.realism", NEBULA_REALISM_RANGE, |p| &mut p.sky.night.nebulas.as_mut().unwrap().realism),
            ("aurora.strength", AURORA_STRENGTH_RANGE, |p| &mut p.sky.night.aurora.as_mut().unwrap().strength),
            ("aurora.mix[0]", AURORA_MIX_RANGE, |p| &mut p.sky.night.aurora.as_mut().unwrap().mix[0]),
            ("aurora.mix[2]", AURORA_MIX_RANGE, |p| &mut p.sky.night.aurora.as_mut().unwrap().mix[2]),
            ("aurora.height_deg", AURORA_HEIGHT_RANGE, |p| &mut p.sky.night.aurora.as_mut().unwrap().height_deg),
            ("aurora.width_deg", AURORA_WIDTH_RANGE, |p| &mut p.sky.night.aurora.as_mut().unwrap().width_deg),
            ("aurora.folds", AURORA_FOLDS_RANGE, |p| &mut p.sky.night.aurora.as_mut().unwrap().folds),
            ("aurora.rays", AURORA_RAYS_RANGE, |p| &mut p.sky.night.aurora.as_mut().unwrap().rays),
        ];
        for (name, (lo, hi), at) in fields {
            for (given, want) in [(-1.0e9f32, lo), (1.0e9, hi)] {
                let mut p = full();
                *at(&mut p) = given;
                p.clamp();
                assert_eq!(*at(&mut p), want, "{name}: {given}");
            }
        }
        let mut p = full();
        p.sky.night.milky_way.as_mut().unwrap().turn_deg = 360.0;
        p.sky.night.aurora.as_mut().unwrap().direction_deg = -10.0;
        p.clamp();
        assert_eq!((p.sky.night.milky_way.unwrap().turn_deg, p.sky.night.aurora.unwrap().direction_deg), (0.0, 350.0));
    }

    /// A partial patch on an absent block (the assistant's set_settings) is completed from the block's defaults, a
    /// fractional seed is rounded, a mix of another length is refused naming it, and null clears the block.
    #[test]
    fn a_partial_night_patch_is_completed_and_rounds_its_seed() {
        let (merged, report) = HdriParams::default().merge_json(r#"{"sky": {"night": {"aurora": {"strength": 0.5, "seed": 3.6}}}}"#).unwrap();
        assert_eq!(merged.sky.night.aurora, Some(AuroraParams { strength: 0.5, seed: 4, ..AuroraParams::default() }));
        assert_eq!(report, ["sky.night.aurora.seed"], "3.6 was rounded to a whole seed");
        let (merged, _) = merged.merge_json(r#"{"sky": {"night": {"nebulas": {"realism": 1}, "milky_way": {"dust": 0.5}}}}"#).unwrap();
        assert_eq!(merged.sky.night.nebulas, Some(NebulaParams { realism: 1.0, ..NebulaParams::default() }));
        assert_eq!(merged.sky.night.milky_way, Some(MilkyWayParams { dust: 0.5, ..MilkyWayParams::default() }));
        let err = merged.merge_json(r#"{"sky": {"night": {"aurora": {"mix": [1, 2]}}}}"#).unwrap_err();
        assert!(err.starts_with("sky.night.aurora.mix must be a list of 3 numbers"), "{err}");
        let (cleared, _) = merged.merge_json(r#"{"sky": {"night": {"aurora": null}}}"#).unwrap();
        assert_eq!(cleared.sky.night.aurora, None);
    }

    /// The aurora's colour is a choice string with a typed accessor: every name round-trips, loose spellings read,
    /// anything else is the first; the line mix is the preset's or the custom one.
    #[test]
    fn every_aurora_colour_round_trips_through_its_accessor() {
        assert_eq!(AURORA_COLOR_NAMES, &["classic", "magenta tops", "storm", "custom"]);
        for (i, name) in AURORA_COLOR_NAMES.iter().enumerate() {
            let mut p = HdriParams::default();
            p.sky.night.aurora = Some(AuroraParams { color: name.to_string(), ..AuroraParams::default() });
            assert!(p.clamp().is_empty(), "{name}");
            let a = p.sky.night.aurora.unwrap();
            assert_eq!((a.color(), a.color().as_str()), (AuroraColor::ALL[i], *name));
        }
        assert_eq!(AuroraParams { color: " STORM ".to_string(), ..AuroraParams::default() }.color(), AuroraColor::Storm);
        assert_eq!(AuroraParams { color: "pink".to_string(), ..AuroraParams::default() }.color(), AuroraColor::Classic);
        // Every variant has its name listed: the match stops compiling when a variant is added.
        for c in AuroraColor::ALL {
            match c {
                AuroraColor::Classic | AuroraColor::MagentaTops | AuroraColor::Storm | AuroraColor::Custom => assert!(AURORA_COLOR_NAMES.contains(&c.as_str())),
            }
        }
        let custom = AuroraParams { color: "custom".to_string(), mix: [0.3, 0.4, 0.5], ..AuroraParams::default() };
        assert_eq!(custom.line_mix(), [0.3, 0.4, 0.5]);
        assert_eq!(AuroraParams { mix: [0.3, 0.4, 0.5], ..AuroraParams::default() }.line_mix(), AURORA_PRESET_MIX[0], "classic ignores the mix");
        assert_eq!(AuroraParams { color: "magenta tops".into(), ..AuroraParams::default() }.line_mix(), AURORA_PRESET_MIX[1]);
        assert_eq!(AuroraParams { color: "storm".into(), ..AuroraParams::default() }.line_mix(), AURORA_PRESET_MIX[2]);
        assert_eq!(AuroraParams::default().mix, AURORA_PRESET_MIX[0]);
    }

    /// A block draws when it is there and on; a block that exists draws by default (a template, `full()`).
    #[test]
    fn a_night_block_draws_when_it_is_there_and_on() {
        let mut n = NightParams::default();
        assert!(n.milky_way_drawn().is_none() && n.nebulas_drawn().is_none() && n.aurora_drawn().is_none());
        n.milky_way = Some(MilkyWayParams { on: false, ..MilkyWayParams::default() });
        n.nebulas = Some(NebulaParams { on: false, ..NebulaParams::default() });
        n.aurora = Some(AuroraParams { on: false, ..AuroraParams::default() });
        assert!(n.milky_way_drawn().is_none() && n.nebulas_drawn().is_none() && n.aurora_drawn().is_none(), "off draws nothing");
        n.milky_way.as_mut().unwrap().on = true;
        n.nebulas.as_mut().unwrap().on = true;
        n.aurora.as_mut().unwrap().on = true;
        assert!(n.milky_way_drawn().is_some() && n.nebulas_drawn().is_some() && n.aurora_drawn().is_some());
        assert!(MilkyWayParams::default().on && NebulaParams::default().on && AuroraParams::default().on);
    }
}

/// D17 (L:31, :71, :104, :132, :231): every reader of a settings block reads
/// the block as clamp() holds it, so a block's ranges, its defaults and its
/// rule for NaN are written once, in its `clamp_fields`. `Env` clamps its own
/// copy first, so inside a map this changes nothing; the panel, the assistant
/// and a host call the readers directly, and they get clamp()'s reading too.
#[cfg(test)]
mod reader_tests {
    use super::*;
    use super::super::aim::Aim;
    use super::super::atmosphere::{sun_direction, Atmosphere};
    use super::super::clouds::CloudLayer;
    use super::super::composite::BaseImage;
    use super::super::cri::light_spectrum;
    use super::super::gobo::Gobo;
    use super::super::imperfections::Imperfections;
    use super::super::light_fx::{face_axes, LightFx};
    use super::super::night::NightSky;
    use super::super::shaped::{polygon_unit_points, stroke_of, ShapedLight};
    use super::super::studio::{light_color, light_white, Studio};
    use super::super::weather::effects;
    use super::super::{dir_from_az_el, ImageSet};
    use makepad_draw::*;
    use makepad_render_material::ibl::EnvMap;

    /// The light as clamp() leaves it.
    fn held(light: &LightParams) -> LightParams {
        let mut p = HdriParams { lights: vec![light.clone()], ..HdriParams::default() };
        p.clamp();
        p.lights.remove(0)
    }

    /// Directions over the whole sphere, every 7.5 degrees of azimuth and elevation.
    fn directions() -> Vec<Vec3f> {
        let mut out = Vec::new();
        for i in 0..48 {
            for j in 0..23 {
                out.push(dir_from_az_el(7.5 * i as f32, -82.5 + 7.5 * j as f32));
            }
        }
        out
    }

    /// L:104: the render and the CRI readout read one Kelvin, clamp()'s.
    #[test]
    fn a_lights_colour_and_its_readout_read_one_kelvin() {
        for kelvin in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 500.0, 30_000.0, 3200.0] {
            let raw = LightParams { kelvin, ..LightParams::default() };
            let clamped = held(&raw);
            assert_eq!(light_color(&raw), light_color(&clamped), "the colour of a {kelvin} K light");
            assert_eq!(light_spectrum(&raw), light_spectrum(&clamped), "the readout of a {kelvin} K light");
        }
    }

    /// L:31, :71: a stroke's, a scrim's and a polygon's drawing.
    #[test]
    fn a_shaped_light_draws_as_clamp_holds_it() {
        let arc: Vec<[f32; 2]> = (0..70).map(|i| [370.0 + 2.0 * i as f32, 10.0 + (i % 7) as f32]).collect();
        let stroke = |s: StrokeParams| LightParams { shape: "stroke".to_string(), stroke: Some(s), ..LightParams::default() };
        let scrim = |s: ScrimParams| LightParams { shape: "scrim".to_string(), width_deg: 60.0, height_deg: 40.0, scrim: Some(s), ..LightParams::default() };
        let polygon = |p: PolygonParams| LightParams { shape: "polygon".to_string(), width_deg: 50.0, height_deg: 40.0, polygon: Some(p), ..LightParams::default() };
        let lights = [
            stroke(StrokeParams {
                points: vec![[350.0, 10.0], [f32::NAN, 5.0], [20.0, 120.0], [40.0, 20.0]],
                curviness: -3.0,
                width_deg: f32::INFINITY,
                profile: (0..40).map(|i| [f32::NAN, 9.0, -1.0, 0.5][i % 4]).collect(),
                end_kelvin: Some(f32::NAN),
                ..StrokeParams::default()
            }),
            stroke(StrokeParams { points: arc, curviness: f32::NAN, width_deg: f32::NAN, end_kelvin: Some(f32::INFINITY), ..StrokeParams::default() }),
            stroke(StrokeParams { points: vec![[10.0, 5.0]], width_deg: 300.0, end_rgb: Some([2.0, f32::NAN, -1.0]), ..StrokeParams::default() }),
            scrim(ScrimParams { source_size: f32::INFINITY, frame: 0.5 }),
            scrim(ScrimParams { source_size: 0.0, frame: f32::NAN }),
            polygon(PolygonParams { sides: 100, star: f32::NAN, points: vec![[f32::NAN, 0.0], [5.0, 0.0], [0.0, 1.0], [-1.0, -1.0]] }),
            polygon(PolygonParams { sides: 1, star: 2.0, points: Vec::new() }),
        ];
        for (i, raw) in lights.iter().enumerate() {
            let clamped = held(raw);
            assert_eq!(stroke_of(raw), stroke_of(&clamped), "light {i}: the stroke it draws");
            if let Some(p) = &raw.polygon {
                assert_eq!(polygon_unit_points(p), polygon_unit_points(clamped.polygon.as_ref().unwrap()), "light {i}: the outline");
            }
            let (a, b) = (ShapedLight::new(raw), ShapedLight::new(&clamped));
            assert_eq!(a.is_some(), b.is_some(), "light {i} builds as its clamped copy does");
            let (Some(a), Some(b)) = (a, b) else { continue };
            let (ca, cb) = (light_color(raw), light_color(&clamped));
            for d in directions() {
                let (sa, sb) = (a.sample(d), b.sample(d));
                assert!(sa == sb, "light {i} at {d:?}: {sa:?} against {sb:?}");
                let (pa, pb) = (a.compose(d, Vec3f::all(0.25), ca), b.compose(d, Vec3f::all(0.25), cb));
                assert!(pa == pb, "light {i} at {d:?}: {pa:?} against {pb:?}");
            }
        }
    }

    /// L:132: a gobo's pattern.
    #[test]
    fn a_gobo_passes_as_clamp_holds_it() {
        let bad = |kind: &str| GoboParams {
            kind: kind.to_string(),
            panes_x: 0,
            panes_y: 40,
            bar: f32::NAN,
            frame: f32::INFINITY,
            slats: 100,
            tilt_deg: -10.0,
            slat_width: f32::NAN,
            density: f32::NAN,
            scale: f32::INFINITY,
            distance: 2.0,
            ..GoboParams::default()
        };
        let images = ImageSet::default();
        for kind in ["window", "blinds", "leaves", "fern", "grid"] {
            for raw in [bad(kind), GoboParams { tilt_deg: f32::NAN, distance: f32::NAN, slats: 0, ..bad(kind) }] {
                let light = held(&LightParams { gobo: Some(raw.clone()), ..LightParams::default() });
                let clamped = light.gobo.unwrap();
                let (a, b) = (Gobo::new(&raw, vec2f(0.3, 0.2), &images).unwrap(), Gobo::new(&clamped, vec2f(0.3, 0.2), &images).unwrap());
                for i in 0..41 {
                    for j in 0..41 {
                        let q = vec2f(-0.33 + 0.0165 * i as f32, -0.22 + 0.011 * j as f32);
                        assert_eq!(a.transmission(q), b.transmission(q), "{kind} at {q:?}");
                    }
                }
            }
        }
    }

    /// `a` and `b` are the same three numbers, bit for bit (NaN equals NaN).
    #[track_caller]
    fn same(a: Vec3f, b: Vec3f, what: std::fmt::Arguments) {
        let bits = |c: Vec3f| [c.x, c.y, c.z].map(f32::to_bits);
        assert!(bits(a) == bits(b), "{what}: {a:?} against {b:?}");
    }

    /// A light of every kind with its own numbers out of range or NaN, each
    /// where clamp() reads it otherwise than a reader holding it by hand
    /// did: a NaN width, softness, corner, inner radius, level or tint is the
    /// default, an infinite height or hotspot the top of the range, a
    /// polygon or a scrim at most SHAPED_SPAN_MAX_DEG wide, an RGB 0..1 with
    /// NaN as 1, a knob LED's NaN Kelvin 6500 K and its infinite quality 1.
    fn wild_lights() -> Vec<LightParams> {
        let wild = LightParams {
            key: true,
            width_deg: f32::NAN,
            height_deg: f32::INFINITY,
            softness: f32::NAN,
            corner: f32::NAN,
            hotspot: f32::INFINITY,
            inner: f32::NAN,
            intensity_ev: f32::NAN,
            tint: f32::NAN,
            ..LightParams::default()
        };
        let boxed = |shape: &str| LightParams { shape: shape.to_string(), width_deg: 150.0, height_deg: 130.0, ..wild.clone() };
        vec![
            wild.clone(),
            LightParams { shape: "disc".to_string(), ..wild.clone() },
            LightParams { shape: "ring".to_string(), width_deg: 60.0, ..wild.clone() },
            LightParams { rgb: Some([f32::NAN, 2.0, -1.0]), intensity_ev: f32::NEG_INFINITY, width_deg: 40.0, ..wild.clone() },
            LightParams { polygon: Some(PolygonParams::default()), ..boxed("polygon") },
            LightParams { scrim: Some(ScrimParams::default()), ..boxed("scrim") },
            LightParams {
                shape: "stroke".to_string(),
                stroke: Some(StrokeParams { points: vec![[10.0, 5.0], [40.0, 25.0], [70.0, 5.0]], end_kelvin: Some(9000.0), ..StrokeParams::default() }),
                ..wild.clone()
            },
            LightParams { source: Some("led".to_string()), kelvin: f32::NAN, source_quality: Some(f32::INFINITY), width_deg: 40.0, ..wild.clone() },
            LightParams { source: Some("sodium_high_pressure".to_string()), width_deg: 40.0, ..wild.clone() },
        ]
    }

    /// D17 (H8's fix round 1): a light's own numbers, its box, softness,
    /// corner, hotspot, inner radius, level, tint, RGB and source, are read
    /// as clamp() holds them by its colour, its CRI readout, the studio's
    /// drawing, what hangs in front of it and the shape core's drawing, where
    /// each of them held them by hand before.
    #[test]
    fn a_lights_own_numbers_read_as_clamp_holds_them() {
        let images = ImageSet::default();
        for (i, raw) in wild_lights().iter().enumerate() {
            let clamped = held(raw);
            let shape = &raw.shape;
            same(light_color(raw), light_color(&clamped), format_args!("light {i} ({shape}): its colour"));
            same(light_white(raw), light_white(&clamped), format_args!("light {i} ({shape}): its white"));
            assert_eq!(light_spectrum(raw).map(|s| s.map(f32::to_bits)), light_spectrum(&clamped).map(|s| s.map(f32::to_bits)), "light {i} ({shape}): its CRI readout's spectrum");
            let (sa, sb) = (Studio::new(&StudioParams::default(), std::slice::from_ref(raw)), Studio::new(&StudioParams::default(), std::slice::from_ref(&clamped)));
            let (fa, fb) = (LightFx::new(raw, &images), LightFx::new(&clamped, &images));
            let (a, b) = (ShapedLight::new(raw), ShapedLight::new(&clamped));
            assert_eq!(a.is_some(), b.is_some(), "light {i} ({shape}) builds as its clamped copy does");
            for d in directions() {
                let (pa, pb) = (sa.apply_lights(d, Vec3f::all(0.25)), sb.apply_lights(d, Vec3f::all(0.25)));
                same(pa, pb, format_args!("light {i} ({shape}): the studio's drawing at {d:?}"));
                let (xa, xb) = (fa.apply(d, Vec3f::all(0.25), vec3f(2.0, 1.5, 1.0)), fb.apply(d, Vec3f::all(0.25), vec3f(2.0, 1.5, 1.0)));
                same(xa, xb, format_args!("light {i} ({shape}): what passes in front of it at {d:?}"));
                if let (Some(a), Some(b)) = (&a, &b) {
                    let (ma, mb) = (a.sample(d), b.sample(d));
                    assert_eq!((ma.mask.to_bits(), ma.emit.to_bits()), (mb.mask.to_bits(), mb.emit.to_bits()), "light {i} ({shape}): the shape at {d:?}, {ma:?} against {mb:?}");
                }
            }
        }
    }

    /// D17 (H8's fix round 1): what hangs on a light reads the light, its aim
    /// and its wear as clamp() holds them: the softness of the doors'
    /// shadows, the level a window filled with sky shows the sky at, the
    /// face's yaw, pitch, grid and doors, and the wear's three amounts.
    /// `Aim::new` and `Imperfections::new` held the aim's and the wear's
    /// fields by hand with clamp()'s own ranges and NaN rule, so for these
    /// two blocks raw and held read alike either way (the tie is
    /// `the_aim_and_the_wear_take_every_value_clamp_keeps`); the light's own
    /// softness and level did not.
    #[test]
    fn a_lights_aim_and_wear_read_as_clamp_holds_them() {
        let wide = AimParams { yaw_deg: 130.0, pitch_deg: f32::NEG_INFINITY, grid: 3.0, barn_doors: [-5.0, 200.0, f32::NAN, 40.0] };
        let mild = AimParams { yaw_deg: 9.0, pitch_deg: f32::NAN, grid: 0.2, barn_doors: [25.0, f32::NAN, -10.0, 140.0] };
        let worn = ImperfectionParams { wrinkle: 3.0, dust: f32::NAN, weave: 1.5, seed: 4 };
        let window = GoboParams { kind: "window".to_string(), fill_sky: true, ..GoboParams::default() };
        let base = LightParams { width_deg: 40.0, height_deg: 30.0, softness: f32::NAN, intensity_ev: f32::NAN, ..LightParams::default() };
        let lights = [
            LightParams { aim: Some(mild.clone()), imperfections: Some(worn.clone()), ..base.clone() },
            LightParams { aim: Some(wide.clone()), ..base.clone() },
            LightParams { gobo: Some(window), aim: Some(mild.clone()), ..base.clone() },
            LightParams { shape: "scrim".to_string(), width_deg: 150.0, aim: Some(mild.clone()), imperfections: Some(worn.clone()), ..base.clone() },
        ];
        let images = ImageSet::default();
        for (i, raw) in lights.iter().enumerate() {
            let clamped = held(raw);
            let (a, b) = (LightFx::new(raw, &images), LightFx::new(&clamped, &images));
            let (before, after, sky) = (Vec3f::all(0.25), vec3f(2.0, 1.5, 1.0), vec3f(0.3, 0.5, 0.9));
            for d in directions() {
                same(a.apply(d, before, after), b.apply(d, before, after), format_args!("light {i}: what passes its extras at {d:?}"));
                same(a.window(d, 0.5, before, sky), b.window(d, 0.5, before, sky), format_args!("light {i}: its window at {d:?}"));
            }
        }
        // The two blocks' own readers, handed the face as `LightFx` hands it.
        let (frame, sx, half) = face_axes(&held(&base));
        for (j, raw) in [wide, mild].iter().enumerate() {
            let clamped = held(&LightParams { aim: Some(raw.clone()), ..base.clone() }).aim.unwrap();
            let make = |a: &AimParams| Aim::new(a, frame.center, frame.right * sx, frame.up, half, 0.2);
            let (a, b) = (make(raw), make(&clamped));
            assert_eq!(format!("{a:?}"), format!("{b:?}"), "aim {j}");
            let (Some(a), Some(b)) = (a, b) else { continue };
            for d in directions() {
                for q in [vec2f(0.0, 0.0), vec2f(0.3, -0.2), vec2f(-0.35, 0.25)] {
                    assert_eq!(a.transmission(d, q).to_bits(), b.transmission(d, q).to_bits(), "aim {j} at {d:?}, {q:?}");
                }
            }
        }
        let clamped = held(&LightParams { imperfections: Some(worn.clone()), ..base.clone() }).imperfections.unwrap();
        let (a, b) = (Imperfections::new(&worn, half).unwrap(), Imperfections::new(&clamped, half).unwrap());
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "the wear");
        for u in 0..21 {
            for v in 0..21 {
                let q = vec2f(-half.x + 0.1 * half.x * u as f32, -half.y + 0.1 * half.y * v as f32);
                assert_eq!(a.factor(q).to_bits(), b.factor(q).to_bits(), "the wear at {q:?}");
            }
        }
    }

    /// D17 (H8's fix round 1, the review's spec issue): the aim's and the
    /// wear's readers take every value clamp() keeps. At each end of each
    /// field's range, as clamp() is probed to hold it (what it keeps of -1e9
    /// and 1e9), a step inside reads otherwise than the end. A reader that
    /// held a field inside a range of its own, as `Aim::new` and
    /// `Imperfections::new` did with clamp()'s ranges written again, reads
    /// the end and the step alike once `clamp_fields` alone is widened, so
    /// this ties the two to it the way `ranges.rs` ties the panel's sliders.
    #[test]
    fn the_aim_and_the_wear_take_every_value_clamp_keeps() {
        let base = LightParams { width_deg: 40.0, height_deg: 30.0, ..LightParams::default() };
        let (frame, sx, half) = face_axes(&held(&base));
        // clamp()'s range of a field of `light`, written through `field`.
        let range_of = |field: &dyn Fn(&mut LightParams) -> &mut f32| {
            let mut light = LightParams { aim: Some(AimParams::default()), imperfections: Some(ImperfectionParams::default()), ..base.clone() };
            let mut ends = [-1.0e9f32, 1.0e9].map(|v| {
                *field(&mut light) = v;
                *field(&mut held(&light))
            });
            ends.sort_by(f32::total_cmp);
            ends
        };
        let ends_and_steps = |[lo, hi]: [f32; 2]| {
            let step = 1.0e-3 * (hi - lo);
            [(lo, lo + step), (hi, hi - step)]
        };
        type AimField = fn(&mut AimParams) -> &mut f32;
        let aim_fields: [(&str, AimField); 7] = [
            ("yaw_deg", |a| &mut a.yaw_deg),
            ("pitch_deg", |a| &mut a.pitch_deg),
            ("grid", |a| &mut a.grid),
            ("barn_doors[0]", |a| &mut a.barn_doors[0]),
            ("barn_doors[1]", |a| &mut a.barn_doors[1]),
            ("barn_doors[2]", |a| &mut a.barn_doors[2]),
            ("barn_doors[3]", |a| &mut a.barn_doors[3]),
        ];
        let mid = AimParams { yaw_deg: 10.0, pitch_deg: 5.0, grid: 0.3, barn_doors: [20.0; 4] };
        for (name, field) in aim_fields {
            let range = range_of(&|l: &mut LightParams| field(l.aim.as_mut().unwrap()));
            let aim = |v: f32| {
                let mut a = mid.clone();
                *field(&mut a) = v;
                format!("{:?}", Aim::new(&a, frame.center, frame.right * sx, frame.up, half, 0.2))
            };
            for (end, step) in ends_and_steps(range) {
                assert_ne!(aim(end), aim(step), "Aim::new reads {name} {end} as {step}: it holds the field inside clamp()'s {range:?}");
            }
        }
        type WearField = fn(&mut ImperfectionParams) -> &mut f32;
        let wear_fields: [(&str, WearField); 3] = [("wrinkle", |p| &mut p.wrinkle), ("dust", |p| &mut p.dust), ("weave", |p| &mut p.weave)];
        let mid = ImperfectionParams { wrinkle: 0.4, dust: 0.3, weave: 0.2, seed: 5 };
        for (name, field) in wear_fields {
            let range = range_of(&|l: &mut LightParams| field(l.imperfections.as_mut().unwrap()));
            let wear = |v: f32| {
                let mut p = mid.clone();
                *field(&mut p) = v;
                format!("{:?}", Imperfections::new(&p, half))
            };
            for (end, step) in ends_and_steps(range) {
                assert_ne!(wear(end), wear(step), "Imperfections::new reads {name} {end} as {step}: it holds the field inside clamp()'s {range:?}");
            }
        }
    }

    /// D17 (H8's fix round 1): plan 1a's sky layers read their blocks as
    /// clamp() holds them: the atmosphere and the sun disc, the sun's date,
    /// the night sky and the studio's backdrop. The night takes the
    /// latitude and the solar hour as numbers, held (`Env` passes them so).
    #[test]
    fn the_sky_layers_and_the_backdrop_read_as_clamp_holds_them() {
        let mut raw = HdriParams::default();
        raw.sky.atmosphere = AtmosphereParams { haze: f32::INFINITY, air: f32::NAN, ozone: -1.0, ground_color: [f32::NAN, 2.0, -1.0] };
        raw.sky.sun_disc = SunDiscParams { size_deg: f32::INFINITY, softness: f32::NEG_INFINITY, ..SunDiscParams::default() };
        raw.sky.sun.month = 2;
        raw.sky.sun.day = 31;
        raw.sky.sun.hour = f32::NAN;
        raw.sky.night = NightParams {
            stars: f32::INFINITY,
            star_brightness: f32::NAN,
            moon: true,
            moon_elevation_deg: f32::NAN,
            moon_azimuth_deg: 200.0,
            moon_size_deg: f32::INFINITY,
            moon_brightness: f32::NAN,
            glow_color: [f32::NAN, 0.5, -1.0],
            glow_strength: f32::INFINITY,
            milky_way: Some(MilkyWayParams { on: true, strength: f32::INFINITY, turn_deg: f32::NAN, tilt_deg: 200.0, core: f32::NAN, dust: -1.0 }),
            nebulas: Some(NebulaParams { on: true, amount: f32::NAN, size: f32::INFINITY, brightness: -1.0, realism: 2.0, seed: 3 }),
            aurora: Some(AuroraParams { on: true, strength: f32::INFINITY, color: "Storm ".to_string(), mix: [f32::NAN; 3], direction_deg: f32::NAN, height_deg: 500.0, width_deg: f32::NEG_INFINITY, folds: f32::NAN, rays: 7.0, seed: 1 }),
        };
        raw.studio = StudioParams { top: [f32::NAN, 20.0, -1.0], horizon: [0.5, f32::INFINITY, 0.2], floor: [0.1; 3], horizon_softness: f32::NAN };
        let mut clamped = raw.clone();
        clamped.clamp();
        let (sa, sb) = (sun_direction(&raw.sky.sun), sun_direction(&clamped.sky.sun));
        same(sa, sb, format_args!("the sun of February 31 at hour {}", raw.sky.sun.hour));
        let sun = dir_from_az_el(150.0, 25.0);
        let atmosphere = |p: &HdriParams| Atmosphere::new(sun, &p.sky.atmosphere, &p.sky.sun_disc);
        let (aa, ab) = (atmosphere(&raw), atmosphere(&clamped));
        let night = |p: &HdriParams| NightSky::new(&p.sky.night, 7, dir_from_az_el(200.0, -30.0), 22.0, clamped.sky.sun.latitude);
        let (na, nb) = (night(&raw), night(&clamped));
        let (ba, bb) = (Studio::new(&raw.studio, &[]), Studio::new(&clamped.studio, &[]));
        for d in directions() {
            same(aa.sky(d), ab.sky(d), format_args!("the sky at {d:?}"));
            same(aa.sun_disc(d), ab.sun_disc(d), format_args!("the sun's disc at {d:?}"));
            for (what, a, b) in [("stars", na.stars(d), nb.stars(d)), ("moon", na.moon(d), nb.moon(d)), ("glow", na.glow(d), nb.glow(d)), ("deep sky", na.deep_sky(d, 0.0), nb.deep_sky(d, 0.0)), ("aurora", na.aurora(d, 0.0), nb.aurora(d, 0.0))] {
                same(a, b, format_args!("the night's {what} at {d:?}"));
            }
            same(ba.backdrop(d), bb.backdrop(d), format_args!("the backdrop at {d:?}"));
        }
    }

    /// L:231: the cloud deck, a base image and the weather.
    #[test]
    fn the_sky_blocks_read_as_clamp_holds_them() {
        let mut raw = HdriParams::default();
        raw.sky.clouds = CloudParams { coverage: f32::INFINITY, sharpness: f32::NAN, scale: f32::NAN, altitude_m: f32::INFINITY, cirrus: -2.0 };
        raw.sky.weather = Some(WeatherParams { fog: f32::INFINITY, rain: f32::NAN, wet_ground: 3.0, ..WeatherParams::default() });
        raw.base_image = Some(BaseImageParams {
            key: "pano".to_string(),
            yaw_deg: f32::NAN,
            ev: f32::INFINITY,
            horizon_deg: 90.0,
            feather_deg: f32::NAN,
            sky_key: f32::INFINITY,
            replace_sky: true,
            ..BaseImageParams::default()
        });
        let mut clamped = raw.clone();
        clamped.clamp();
        let sun = vec3f(0.3, 0.5, -0.8).normalize();
        let (a, b) = (CloudLayer::new(&raw.sky.clouds, 7, sun).unwrap(), CloudLayer::new(&clamped.sky.clouds, 7, sun).unwrap());
        let mut images = ImageSet::default();
        images.insert("pano", EnvMap::from_fn(64, |d| [0.4 + 0.5 * d[1].max(0.0), 0.3, 0.6 + 0.2 * d[0]]));
        let base = |p: &HdriParams| BaseImage::new(p.base_image.as_ref().unwrap(), &images, Some(2.0)).unwrap();
        let (ia, ib) = (base(&raw), base(&clamped));
        for d in directions() {
            assert_eq!(format!("{:?}", a.sample(d)), format!("{:?}", b.sample(d)), "the deck at {d:?}");
            assert_eq!(ia.compose(d, Vec3f::all(0.5)), ib.compose(d, Vec3f::all(0.5)), "the base image at {d:?}");
        }
        assert_eq!(effects(raw.sky.weather.as_ref().unwrap()), effects(clamped.sky.weather.as_ref().unwrap()));
    }
}
