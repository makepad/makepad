//! Procedural HDRI generator: outdoor skies (sun, atmosphere, clouds, moon,
//! stars) and studio lighting (a backdrop plus shaped lights) as linear
//! Rec.709 HDR equirect maps, in the engine's own [`EnvMap`].
//!
//! [`Env::new`] does all the work that depends only on the parameters, and
//! [`Env::radiance`] then answers "what arrives from this direction" for
//! any direction. The app preview, file export and the engine's dome all
//! sample that one function, so they cannot disagree.
//!
//! Conventions (pinned by the tests at the bottom of this file):
//! - Y is up, +X is east and -Z is north.
//! - Azimuths are degrees clockwise from north (0 = -Z, 90 = +X), as in NOAA
//!   and `ibl::dir_deg`.
//! - Equirect: `ibl`'s, and only `ibl`'s: `u = 0.5 + atan2(x, -z) / 2pi`,
//!   `v = acos(y) / pi`. -Z (north) sits at the image centre, +X at
//!   u = 0.75, row 0 is the zenith and texels are evaluated at their
//!   centres. `EnvMap::from_fn`, `prefilter`, `sh9`, `DrawEnvBackground`
//!   and the `mat_ibl_*` shader functions all assume this, so a baked map
//!   plugs into every one of them unchanged. Files keep the DCC convention
//!   (+X at the centre); `image::roll_quarter` converts at the file
//!   boundary and nowhere else.
//! - `rotation_deg` turns the content about +Y, positive counter-clockwise
//!   seen from above, exactly as `EnvMap::procedural` and `Ibl.rotation_deg`
//!   do: +90 moves a light at -Z to -X ([`rotate_y`]).
//! - `ibl` works in `[f32; 3]`; this module works in `Vec3f` and converts at
//!   the boundary with [`arr`] and [`vec`].
//!
//! Design: local/agent_state/hdri/design.md (local notes, not under version
//! control).

// `!(x > 0.0)` and its kin are written negated on purpose throughout this module and its
// children: a NaN fails the comparison, so it takes the guard's branch. Clippy would have
// them as `partial_cmp`, which says the same in more words.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use makepad_draw::*;
use makepad_render_material::ibl::{self, EnvMap};

pub mod params;
pub mod presets;
pub mod noise;
pub mod studio;
pub mod atmosphere;
pub mod clouds;
pub mod night;
pub mod image;
pub mod export;
pub mod shape;
pub mod shaped;
pub mod envmap;
pub mod prepare;
pub mod gels;
pub mod spectrum;
pub mod cri;
pub mod image_light;
pub mod gobo;
pub mod light_fx;
pub mod caster;
pub mod foliage;
pub mod aim;
pub mod imperfections;
pub mod composite;
pub mod volumetric;
pub mod weather;
pub mod skyline;
mod milky_way;
mod lazy_grid;
mod nebulas;
mod aurora;

pub use params::*;
pub use image_light::ImageSet;
// hdri::bake_env_map, load_env_map, detect_sun, remove_sun, as the spec names them.
pub use envmap::*;

use std::cell::Cell;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The strongest radiance a bake writes: far above any real sun, but finite,
/// so a layer bug cannot put infinity into an export or the SH projection.
const MAX_RADIANCE: f32 = 1.0e30;

/// The width a shadow caster's bloom raster is tied to when an `Env` is not told
/// the width it is baked at ([`Env::new_for_width`]): the released preview's and
/// the engine's bake width, and the end-to-end key test's.
pub const CASTER_REFERENCE_WIDTH: usize = 1024;

/// Sub-samples per axis for a texel that touches a hot spot.
const HOT_SAMPLES: usize = 8;
/// A feature is refined while its radius is under this many texels; a wider one is resolved by the grid itself.
const HOT_MAX_TEXELS: f32 = 6.0;

/// `ibl` works in `[f32; 3]`; the layers work in `Vec3f`. These two
/// functions are the whole boundary between them.
pub fn arr(v: Vec3f) -> [f32; 3] {
    [v.x, v.y, v.z]
}

pub fn vec(a: [f32; 3]) -> Vec3f {
    vec3f(a[0], a[1], a[2])
}

/// The environment's key light is the engine's own type (makepad-scene),
/// so a baked or detected sun goes straight into `World.environment.sun`.
pub use makepad_scene::EnvSun;

/// One environment, ready to sample: every layer is built once in `new`, so
/// `radiance` is cheap enough to call per texel from many threads (the struct
/// stays `Send + Sync`; A1's test pins that).
pub struct Env {
    /// Clamped copy of the parameters the layers were built from.
    params: HdriParams,
    /// 2^intensity_ev, applied last so every layer keeps its own units.
    scale: f32,
    /// The studio backdrop (Studio mode) and the light overlay (both modes).
    studio: studio::Studio,
    /// The clear sky; Some exactly in Sky mode. The three Sky layers (atmosphere,
    /// clouds, night) are all built in the map's own, unrotated frame.
    atmo: Option<atmosphere::Atmosphere>,
    /// Cloud deck and cirrus; Some in Sky mode when there is any cover.
    clouds: Option<clouds::CloudLayer>,
    /// Stars, moon and glow; Some exactly in Sky mode.
    night: Option<night::NightSky>,
    /// Light on the clouds, gathered once per map: the direct sun, and the ambient
    /// of the day sky plus the night (glow and moonlit air).
    cloud_sun: Vec3f,
    cloud_ambient: Vec3f,
    /// The sky a window filled with sky (GoboParams::fill_sky) looks out at
    /// in Studio mode: a Sky-mode Env of the same parameters with no lights,
    /// no rotation and no intensity. None in Sky mode (the base layer is the
    /// sky there) and when no light asks for it, so every other map costs
    /// what it did before.
    window_sky: Option<Box<Env>>,
    /// The shadow a light's gobo throws onto the studio backdrop (Studio
    /// mode only: a sky has no floor for it to fall on).
    backdrop_gobo: Option<gobo::BackdropGobo>,
    /// A loaded image as the base, or as its part below the horizon (1b).
    /// None when the params name none or the caller's ImageSet lacks it.
    base_image: Option<composite::BaseImage>,
    /// The marched cloud volume (1b); Some in Sky mode when `sky.volumetric`
    /// is set and there is coverage and density, and no image is the whole
    /// base. With the tick on `clouds` is the cirrus veil alone (D8), so a
    /// volume of no density draws no clouds.
    volumetric: Option<volumetric::VolumetricClouds>,
    /// The weather drawn over the dry sky (1b); None in fair weather and
    /// under an image that is the whole base (D6).
    weather: Option<weather::Weather>,
    /// The sun's light under the clouds, for the fog and the ground's gain:
    /// its irradiance at the ground times the cover along the sun.
    weather_sun: Vec3f,
    /// The dome's light under the clouds, for the rain haze and the fog: the
    /// dry sky's mean radiance as this map draws it (`dome_light`). Measured
    /// with `ground_gain`; zero where that is not measured.
    dome: Vec3f,
    /// What the sky's cover does to the light on the ground, per channel: the
    /// light this map's sky puts on it (the dome as drawn, the sun and the
    /// moon through the cover) over the light the clear sky lit it with
    /// (D7). Measured wherever clouds, a volume or a weather stand over the
    /// generated ground, whether or not a weather is built (the Overcast
    /// preset closes its deck with the Clouds alone); exactly 1 when nothing
    /// covers the sky, and 1 where it is not measured (Studio mode, a fair sky
    /// with no clouds, an image that is the whole base).
    ground_gain: Vec3f,
    /// The skyline's strip and its turn (degrees), laid over the base in
    /// `base_at`: Some exactly in Sky mode with `params.skyline` on and a band
    /// in the images ([`Self::has_skyline`]).
    skyline: Option<(std::sync::Arc<skyline::SkylineBand>, f32)>,
    /// The skyline's lights by region (`skyline::BandPeaks`), for a caster's
    /// bloom (`base_peak`): built once, with the band.
    city_peaks: Option<skyline::BandPeaks>,
    /// The ground the strip runs on into below it (`skyline::BandSkirt`),
    /// built once, with the band; Some exactly with `skyline`.
    city_skirt: Option<skyline::BandSkirt>,
    /// The width the casters' bloom was rastered for (`new_for_width`), so
    /// [`Self::with_skyline`] rasters it again as the build did.
    bloom_width: usize,
    /// The key ([`Self::key`]), measured on the engine's first ask and kept
    /// (D15, M5): the app's preview bakes never ask, and a held drag paid the
    /// key's integrals on every bake.
    measured_key: OnceLock<Option<(KeyTerm, EnvSun)>>,
}

/// What one bake hands each of its lookups: the cloud march it reads, when it
/// has one, and the angular size of its texels (radians; 0 for a lookup along
/// one exact direction), over which a line thinner than a texel is averaged
/// (the lightning's channel, D7). `bare` leaves out the sky's points, the
/// sun's and the moon's discs and the stars, which a few looks round the
/// horizon cannot sample (`Env::skyline_light`'s ring, as `dome_light`).
/// `unveiled` leaves out the weather's veil (its rain haze, virga and fog)
/// and the bolt it stands in front of: the base behind them, which the
/// skyline's strip is lit and grounded by and the map then veils once
/// (`base_at`, `skyline::over_veiled`). Without a weather it changes nothing.
/// `even_ground` reads a wet ground with its puddles at their mean share
/// (`Weather::ground_even`): the strip's ground ring and the skirt's hold, a
/// few looks that would otherwise land on a puddle or between them. The map's
/// own ground keeps its puddles. A dry ground is unchanged.
/// `damp_ground` reads a wet ground without its puddles (`Weather::ground_damp`):
/// the strip's own ground, the ground between the puddles. It takes precedence
/// over `even_ground`.
/// `water_ground` reads a wet ground all under water (`Weather::ground_water`):
/// the floor's puddles where they lie over the strip's floor (`Env::under_band`).
/// `no_aurora` leaves the aurora out (plan hdri-7 D12: the strip's aurora rings are the sea's rings less the same
/// rings without it).
#[derive(Clone, Copy, Default)]
struct Sampling<'a> {
    cache: Option<&'a volumetric::CloudCache>,
    texel: f32,
    bare: bool,
    unveiled: bool,
    even_ground: bool,
    damp_ground: bool,
    water_ground: bool,
    no_aurora: bool,
}

impl<'a> Sampling<'a> {
    /// The lookups of a bake `width` wide (2:1, so a texel is as tall as it
    /// is wide at the equator).
    fn bake(width: usize, cache: Option<&'a volumetric::CloudCache>) -> Sampling<'a> {
        Sampling { cache, texel: std::f32::consts::TAU / width.max(2) as f32, bare: false, unveiled: false, even_ground: false, damp_ground: false, water_ground: false, no_aurora: false }
    }
}

impl Env {
    /// An environment that uses no pictures: `new_with` over an empty set.
    pub fn new(params: &HdriParams) -> Env {
        Env::new_with(params, &ImageSet::default())
    }

    /// Clamps a copy of `params` and builds every layer once. `images` holds
    /// the pictures the params name by key (image lights); a key that is not
    /// in it draws the plain light. Still pure: no file is opened here.
    pub fn new_with(params: &HdriParams, images: &ImageSet) -> Env {
        // Nothing cancels it, so it is always built.
        Env::new_cancellable(params, images, &|| false).expect("an Env nothing cancels is built")
    }

    /// [`Self::new_with`] for a job a newer request can overtake (the app's
    /// preview and export; a host's generated bake through
    /// `envmap::bake_generated_env_map_cancellable`: the sandbox's map job,
    /// the renderer's preset bake): the slow part of building a map, the
    /// first build of a seed's cloud volume noise (about 2.2 s in release,
    /// L:229), polls `cancelled` and stops when it says so, and the map is
    /// then None (D15).
    /// The noise is not kept, so the next build of that seed starts it again.
    /// A map that was not stopped is `new_with`'s, bit for bit; a map with
    /// nothing slow to build (no volume, or a seed already built) is built
    /// whatever `cancelled` says, and the caller checks its token after.
    /// A shadow caster's bloom is rastered at [`CASTER_REFERENCE_WIDTH`] ([`Self::new_for_width`]).
    pub fn new_cancellable(params: &HdriParams, images: &ImageSet, cancelled: &dyn Fn() -> bool) -> Option<Env> {
        Env::new_for_width(params, images, CASTER_REFERENCE_WIDTH, cancelled)
    }

    /// [`Self::new_cancellable`] for a map baked `width` wide. A shadow caster's
    /// bloom (redesign §5) is rastered at that width's texel, coarsened to a
    /// sixth of its bloom length (a finer share at a wider map) and to at most
    /// width / 12 cells a side (and `caster::CASTER_RASTER_MAX_SIDE`), the width
    /// held to at least [`CASTER_REFERENCE_WIDTH`] (D6): the held 512 preview
    /// reads the released 1024 one's raster, and an export gets a finer one; a
    /// bright disc behind it (the sun, the moon) is sampled alike at every width. A soft caster's
    /// raster has cells no finer than that width's texel (M5). `new`,
    /// `new_with` and `new_cancellable` are this at [`CASTER_REFERENCE_WIDTH`].
    /// Every lookup of the Env (both bakes at any width, the keyless copy, the
    /// key, `disc_share`) reads those rasters, so the key carries the light the
    /// map draws and the copy lacks exactly it (D1, N3). The pre-pass polls
    /// `cancelled` too. Without a caster that is soft or whose bloom is on,
    /// `width` changes nothing: the Env is `new_cancellable`'s, bit for bit.
    /// The casters' rasters are built on this thread; [`Self::new_for_width_par`]
    /// spreads their rows.
    pub fn new_for_width(params: &HdriParams, images: &ImageSet, width: usize, cancelled: &dyn Fn() -> bool) -> Option<Env> {
        Env::new_for_width_par(params, images, width, cancelled, &caster::serial_rows)
    }

    /// [`Self::new_for_width`] with the shadow casters' rasters (softness and
    /// bloom) spread by `run` as [`Self::bake_par`]'s rows are: `run(n, row)`
    /// calls `row(0..n)`, each once (`pool.fan_out` inside a Heavy job, never
    /// on the UI thread). The Env is `new_for_width`'s, bit for bit. A runner
    /// that skips rows (its job overtaken) leaves the map None, or a caster's
    /// softness raster empty: the caller drops a map its token has cancelled.
    pub fn new_for_width_par(params: &HdriParams, images: &ImageSet, width: usize, cancelled: &dyn Fn() -> bool, run: &dyn Fn(usize, &(dyn Fn(usize) + Sync))) -> Option<Env> {
        let mut params = params.clone();
        params.clamp();
        let scale = 2.0f32.powf(params.intensity_ev);
        // In Sky mode the lights hang against the sky itself (D12: a window
        // filled with sky draws its frame and bars there).
        let studio = studio::Studio::new_with(&params.studio, &params.lights, images, params.mode() == Mode::Sky, width, run);
        // An image that is the whole base hides the generated sky: its weather
        // and its cloud volume are not drawn, so they are not built (D6).
        let whole_base = composite::whole_base(&params, images);
        let mut volumetric = None;
        let mut weather = None;
        let mut weather_sun = Vec3f::default();
        let (atmo, clouds, night, cloud_sun, cloud_ambient) = match params.mode() {
            Mode::Sky => {
                // Weather kinds are presets over the sky's own fields (1b): the
                // layers are built from the weathered copy, the stored params
                // stay the user's.
                let weathered = weather::weathered_sky(&params.sky);
                let sky = &weathered;
                let sun_dir = atmosphere::sun_direction(&sky.sun);
                let atmo = atmosphere::Atmosphere::new(sun_dir, &sky.atmosphere, &sky.sun_disc);
                // Volumetric mode (1b): the marched volume replaces the 2D deck
                // and stands on its footprint; the cirrus veil above it stays
                // the 2D one. The tick chooses the mode (D8): a volume of no
                // density draws no clouds, and the flat deck does not come back.
                if !whole_base {
                    volumetric = match sky.volumetric.as_ref() {
                        Some(vol) => volumetric::VolumetricClouds::build(&sky.clouds, vol, params.seed, atmo.sun_dir(), cancelled).ok()?,
                        None => None,
                    };
                }
                let clouds = if sky.volumetric.is_some() {
                    clouds::CloudLayer::new(&CloudParams { coverage: 0.0, ..sky.clouds.clone() }, params.seed, atmo.sun_dir())
                } else {
                    clouds::CloudLayer::new(&sky.clouds, params.seed, atmo.sun_dir())
                };
                let night = night::NightSky::new_par(&sky.night, params.seed, atmo.sun_dir(), night::star_hours(&sky.sun), sky.sun.latitude, run)?;
                let cloud_sun = atmo.sun_irradiance();
                let cloud_ambient = atmo.ambient() + night.ambient();
                // The weather itself works its floors out from the user's block.
                if !whole_base {
                    weather = weather::Weather::new(&params.sky, atmo.sun_dir());
                }
                // The sun under the clouds, as the key is dimmed in `sun`: the 2D
                // layer's UNFADED cover (1a's fix round; a 2 degree sun behind a
                // full overcast is hidden, not 80 % through) and the volume.
                let sun_cover = clouds.as_ref().map_or(1.0, |layer| 1.0 - layer.cover_toward(atmo.sun_dir()))
                    * volumetric.as_ref().map_or(1.0, |volume| volume.transmittance_toward(atmo.sun_dir()));
                weather_sun = cloud_sun * sun_cover;
                (Some(atmo), clouds, Some(night), cloud_sun, cloud_ambient)
            }
            Mode::Studio => (None, None, None, Vec3f::default(), Vec3f::default()),
        };
        // A window filled with sky looks out at this map's own sky settings.
        // In Studio mode that sky is a second, lightless Sky-mode Env, built
        // only when a light asks for it (a shadow caster never does). Its
        // lights are cleared, so it never nests further, and it is made from
        // the parameters alone: new_with stays pure.
        let window_sky = if params.mode() == Mode::Studio
            && params.lights.iter().any(|l| l.enabled && !l.is_caster() && l.blend() == Blend::Add && l.gobo.as_ref().is_some_and(|g| g.fill_sky))
        {
            let mut outside = params.clone();
            outside.mode = "sky".to_string();
            outside.lights.clear();
            outside.intensity_ev = 0.0;
            outside.rotation_deg = 0.0;
            // The window looks out at the generated sky, not at a base image,
            // and a studio draws no skyline.
            outside.base_image = None;
            outside.skyline = None;
            Some(Box::new(Env::new_cancellable(&outside, images, cancelled)?))
        } else {
            None
        };
        let backdrop_gobo = if params.mode() == Mode::Studio { gobo::BackdropGobo::new(&params.lights, images) } else { None };
        // The skyline is a layer of the sky: drawn in Sky mode, when its block
        // is on and the app has handed its strip over.
        let skyline = images.skyline.as_ref().and_then(|band| Env::skyline_of(&params, band));
        let mut env = Env {
            params,
            scale,
            studio,
            atmo,
            clouds,
            night,
            cloud_sun,
            cloud_ambient,
            window_sky,
            backdrop_gobo,
            volumetric,
            weather,
            weather_sun,
            dome: Vec3f::default(),
            ground_gain: Vec3f::all(1.0),
            base_image: None,
            city_peaks: skyline.as_ref().map(|(band, _)| band.peaks()),
            // built once the ground it is held to is (`skirt_for`, below)
            city_skirt: None,
            skyline,
            bloom_width: width,
            measured_key: OnceLock::new(),
        };
        // The weather's veil and the ground are lit by the dome as this map
        // draws it, so the dome is measured once its layers are in place (and
        // before a base image meters the base, the veil included): wherever
        // anything covers the sky over the generated ground, a weather or not
        // (D7). Under an image that is the whole base no ground is drawn.
        let covered = env.weather.is_some() || env.clouds.is_some() || env.volumetric.is_some();
        if covered && !whole_base {
            let dome = env.dome_light();
            env.dome = dome.mean;
            env.ground_gain = env.measure_ground_gain(&dome);
        }
        if let Some(base) = env.params.base_image.clone() {
            // Sky replacement matches the image's exposure to the base it is
            // composited over, so the base is metered first (base_image is
            // still None here, so it meters the mode's own base).
            let sky_irradiance = if base.replace_sky { Some(env.base_irradiance()) } else { None };
            env.base_image = composite::BaseImage::new(&base, images, sky_irradiance);
        }
        if let Some((band, turn)) = env.skyline.clone() {
            env.city_skirt = Some(env.skirt_for(&band, turn));
        }
        // A caster's bloom reads the finished layers behind it, so it comes
        // last; a newer request stops it as it stops the cloud noise (R5).
        if !env.build_caster_bloom(width, cancelled, run) {
            return None;
        }
        // The key is not measured here: the engine's first `sun` does that,
        // once (D15).
        Some(env)
    }

    pub fn params(&self) -> &HdriParams {
        &self.params
    }

    /// True when this map draws the skyline: Sky mode, `params.skyline` on,
    /// and a band in the images it was built with.
    pub fn has_skyline(&self) -> bool {
        self.skyline.is_some()
    }

    /// Where the night's features are (`night::NightFeatures`), in the map's own frame (the display shows azimuth a at
    /// a - rotation_deg); None in Studio mode. Plan hdri-7.
    pub fn night_features(&self) -> Option<night::NightFeatures> {
        self.night.as_ref().map(|night| night.features())
    }

    /// What the skyline draws at `d` (premultiplied; its skirt below it), or
    /// None without a skyline.
    fn city_at(&self, d: Vec3f) -> Option<[f32; 4]> {
        let ((band, turn), skirt) = (self.skyline.as_ref()?, self.city_skirt.as_ref()?);
        Some(band.sample_skirted(skirt, d, *turn))
    }

    /// `city_at` with the distance (m) of what the texel shows and the light in it (`SkylineBand::sample_skirted_lit`).
    /// The distance is None where the texel has no coverage (the glow, a prop's light over the sky) or the band no
    /// distance plane, and where the plane reads 0 (a distance the handoff made safe), which no surface the strip draws
    /// stands at. The light is None where there is none or the band has no light plane. The colour is `city_at`'s.
    /// Where the strip gives way to this map's floor (`gives_way`) it is read on either side of the horizon apart
    /// (`SkylineBand::sample_skirted_lit_parted`): it leaves its flat ground clear from the horizon down, so a look just
    /// over the horizon, and the puddle that mirrors it, sees the near rise standing there, not the clear under it and
    /// the sky behind (plan hdri-6 fix wave 1, round 3).
    fn city_far_at(&self, d: Vec3f) -> Option<([f32; 4], Option<f32>, Option<skyline::TexelLight>)> {
        let ((band, turn), skirt) = (self.skyline.as_ref()?, self.city_skirt.as_ref()?);
        let (s, far, light) = if self.gives_way() { band.sample_skirted_lit_parted(skirt, d, *turn) } else { band.sample_skirted_lit(skirt, d, *turn) };
        Some((s, far.filter(|&far| far > 0.0), light))
    }

    /// Whether the skyline's strip gives way to this map's floor: under standing water (the puddles' share above 0,
    /// with a sky: what `skyline_light` hands as `wet`) the app's strip leaves the puddles' mean share of its flat
    /// ground clear below the horizon, at every depression (makepad_skyline's `SkyLight::floor_share` and
    /// `under_floor`, plan hdri-6 fix wave 1, the owner's D1), and the map lays its puddles there (`under_band`). False
    /// without a skyline, a weather, a sky or a wet ground.
    fn gives_way(&self) -> bool {
        self.skyline.is_some() && self.atmo.is_some() && self.weather.as_ref().is_some_and(|weather| weather.puddle_share() > 0.0)
    }

    /// How the map lays strip texel `s` along `d` over what is behind it: the scale of the texel (its colour, its
    /// coverage and its light) and that base behind it. Where the strip gives way to the floor (`gives_way`, below the
    /// horizon) it has left the puddles' mean share m of its floor clear (`SkyLight::floor_share`): the share of the
    /// texel left clear past what the band leaves below it (its skirt's fade), m of the texel's floor. The strip cannot
    /// see the puddles; the map can: there the floor keeps its puddles as its own field lays them along `d` (p,
    /// `Weather::puddle_cover`), the strip its own ground between them. So the texel's floor is the strip's ground by 1 -
    /// p (the texel scaled as one colour from its kept 1 - m) and the floor's water by p (`Sampling::water_ground`: a
    /// puddle's bottom and what it mirrors), and below the band's share the floor as it is: the puddles keep their shapes
    /// over the band and run on into the floor's below it, and between them the strip's ground, never the floor's dry
    /// ground, which by the horizon is its haze (fix wave 1, the owner's D1). A clear share the strip's floor does not
    /// explain (more than m, or more than the ground it kept would leave) shows the floor as it is. Elsewhere (1,
    /// `base_under_skyline`), the base not looked up behind an opaque texel.
    fn under_band(&self, d: Vec3f, s: [f32; 4], without: Option<KeyTerm>, at: Sampling) -> (f32, Vec3f) {
        let a = s[3];
        let base = |at: Sampling| if a < 1.0 { self.base_under_skyline(d, without, at) } else { Vec3f::default() };
        let (Some((band, _)), Some(weather)) = (&self.skyline, &self.weather) else { return (1.0, base(at)) };
        if !(d.y < 0.0 && a > 0.0 && self.gives_way()) {
            return (1.0, base(at));
        }
        let (share, m) = (band.share_at(d), weather.puddle_share().min(1.0));
        let left = share - a;
        if !(left > 0.0) {
            return (1.0, base(at));
        }
        // the texel's floor: what the strip left over m, at most all of it and no more than the ground it kept
        let kept = if m < 1.0 { a / ((1.0 - m) * share) } else { f32::INFINITY };
        let floor = (left / (m * share)).min(1.0).min(kept);
        let p = weather.puddle_cover(d, weather::Puddles::Field);
        // the strip's ground over the texel's floor by 1 - p, the floor's water by p, the floor as it is the rest
        let k = (a + (m - p) * floor * share) / a;
        let (water, rest) = (p * floor * share, 1.0 - a * k - p * floor * share);
        let behind = 1.0 - a * k;
        if !(behind > 0.0) {
            return (k, Vec3f::default());
        }
        let mut c = Vec3f::default();
        if water > 0.0 {
            c += self.base_under_skyline(d, without, Sampling { water_ground: true, ..at }) * water;
        }
        if rest > 1e-6 {
            c += self.base_under_skyline(d, without, at) * rest;
        }
        (k, c * (1.0 / behind))
    }

    /// The skirt this map draws under `band` (turned by `turn` degrees): its bottom row blurred
    /// (`SkylineBand::skirt`), held at every azimuth to the darkest of this map's own base (the
    /// generated ground, a photograph's) at the band's bottom, half way down the skirt and at its
    /// end, so the strip never runs on brighter than the ground it fades into (the owner's ruling:
    /// no bright patch under the sea by day). The base is the skyline's own lookup without it, before
    /// the weather's veil: the skirt is veiled with the band (`base_at`), and the veil is affine and
    /// grey (one transmittance for all channels), so a skirt no brighter than the unveiled ground is,
    /// veiled, no brighter than the ground the map draws. A wet ground is held to its mean
    /// (`Weather::ground_even`), which is smooth where the puddles change finer than these looks.
    fn skirt_for(&self, band: &skyline::SkylineBand, turn: f32) -> skyline::BandSkirt {
        let at = Sampling { bare: true, unveiled: true, even_ground: true, ..Sampling::default() };
        let ground = |u: f32, el: f32| {
            let d = vec(ibl::equirect_uv_to_dir([(u + turn / 360.0).rem_euclid(1.0), 0.5 - el / 180.0]));
            let c = self.base_under_skyline(d, None, at);
            0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
        };
        let bottom = band.bottom_deg;
        // The ground is smooth round the turn: looked up at SKIRT_HOLD_SAMPLES azimuths and read between them, not
        // at each of the band's columns (three lookups a column cost a held bake about 4 ms, plan hdri-5 Task 8).
        let n = skyline::SKIRT_HOLD_SAMPLES;
        let bound: Vec<f32> = (0..n)
            .map(|i| {
                let u = i as f32 / n as f32;
                [bottom, bottom - 0.5 * skyline::SKIRT_DEG, bottom - skyline::SKIRT_DEG].into_iter().map(|el| ground(u, el)).fold(f32::INFINITY, f32::min)
            })
            .collect();
        band.skirt().held_to(|u| {
            let x = u.rem_euclid(1.0) * n as f32;
            let i = x.floor() as usize % n;
            // the darker of the two samples either side, so the hold never lets a column past its ground
            bound[i].min(bound[(i + 1) % n])
        })
    }

    /// The band and its turn, by the rule the ImageSet path keeps: Sky mode
    /// and `params.skyline` on (`params` clamped).
    fn skyline_of(params: &HdriParams, band: &std::sync::Arc<skyline::SkylineBand>) -> Option<(std::sync::Arc<skyline::SkylineBand>, f32)> {
        match (params.mode(), params.skyline.as_ref()) {
            (Mode::Sky, Some(block)) if block.on => Some((band.clone(), block.turn_deg)),
            _ => None,
        }
    }

    /// This map with the skyline's strip laid over its base, as if `band`
    /// had been in the ImageSet it was built with: the map, its key and its
    /// casters' bloom are that Env's, bit for bit. The app's two phases: build
    /// the map without the strip, hand the strip [`Self::skyline_light`],
    /// render it, attach it here. Only what reads the band is done again: its
    /// lights by region, the casters' bloom (at the width the map was built
    /// for) and the key, which is measured again on the next ask. The turn is
    /// the params' (Sky mode with the block on; otherwise the map is returned
    /// unchanged, as the ImageSet path draws no skyline then). The casters'
    /// rows are rastered on this thread; [`Self::with_skyline_par`] spreads them.
    pub fn with_skyline(self, band: std::sync::Arc<skyline::SkylineBand>) -> Env {
        self.with_skyline_par(band, &|| false, &caster::serial_rows).expect("a bloom nothing cancels is built")
    }

    /// [`Self::with_skyline`] with the casters' bloom rows spread by `run`
    /// (as [`Self::new_for_width_par`]), polling `cancelled`: None when it
    /// stopped (or the runner skipped a row), and the map is then dropped.
    pub fn with_skyline_par(mut self, band: std::sync::Arc<skyline::SkylineBand>, cancelled: &dyn Fn() -> bool, run: &dyn Fn(usize, &(dyn Fn(usize) + Sync))) -> Option<Env> {
        let Some(skyline) = Env::skyline_of(&self.params, &band) else {
            return Some(self);
        };
        self.city_peaks = Some(skyline.0.peaks());
        self.city_skirt = Some(self.skirt_for(&skyline.0, skyline.1));
        self.skyline = Some(skyline);
        // The bloom read the base without the city: it is rastered again from
        // nothing, in list order, as the build rastered it.
        self.studio.clear_caster_blooms();
        if !self.build_caster_bloom(self.bloom_width, cancelled, run) {
            return None;
        }
        self.measured_key = OnceLock::new();
        Some(self)
    }

    /// The sky's light as the skyline needs it (`makepad_skyline::SkyLight`,
    /// field by field, the app adding `lights_by_day` and `design_gain`), taken
    /// from this map itself, so the city follows the sky the map draws: in the
    /// strip's frame (turned by the skyline block's turn_deg; 0 without a
    /// block), map units before intensity.
    /// - The rings: the base as the map draws it under the skyline (clouds,
    ///   volume, the weathered sky and its wet ground, a base image) at
    ///   u = (i + 0.5) / RING, at 1 and 15 degrees, and the ground ring below
    ///   the horizon, without its points (the discs and the stars, whose light
    ///   the ring's few looks cannot sample; the sun's reaches the strip as
    ///   `sun_rgb`), and before the weather's veil and its bolt: the map veils
    ///   the strip in front of it (`base_at`), so a texel the strip hazes to
    ///   the ring is, veiled once, the map's own horizon (`sea_low`,
    ///   `sea_high`: the sky the strip's sea mirrors, as the map shows it,
    ///   through its weather; without one the rings). A wet ground is read
    ///   with its puddles at their mean share (`Weather::ground_even`, the
    ///   owner's ruling): the ground ring's 64 looks would otherwise land on a
    ///   puddle or between them, and the strip's near ground would be striped.
    /// - `wet` and `ground_damp`: the puddles' mean share of the map's ground
    ///   (`Weather::puddle_share`, 0 on a dry ground or without a sky) and the
    ///   ground ring read with no puddles (`Weather::ground_damp`; the ground
    ///   ring itself when `wet` is 0): under a wet ground the strip keeps its
    ///   own ground, the damp ring (its near rise, its land, its lakes'
    ///   bottoms), and below the horizon, its flat ground at every depression,
    ///   leaves the puddles' share clear (`SkyLight::floor_share` over a
    ///   host's ground; plan hdri-6 §5, fix wave 1), where this map lays its
    ///   puddles, their water and what they mirror (`under_band`), so nothing
    ///   is mirrored twice.
    /// - `aurora`: the aurora's share of `sea_low` and `sea_high` (the same rings read without it, subtracted), only
    ///   while it draws: what the strip's water mirrors of it at night (plan hdri-7 D12).
    /// - `sun_rgb`: the sun's irradiance through the cover the key is dimmed
    ///   by (`cover_toward`), times the key's own fade over the horizon, / pi:
    ///   so under a closed deck the strip is as unlit by the sun as the key.
    /// - `ambient_rgb`: the rest of the map's own white ground's light (its
    ///   ground gain included), so `sun_rgb * sin(el) + ambient_rgb` is the
    ///   white ground straight down, before the veil as the rings are. Never
    ///   under 0.
    /// In Studio mode the rings are the backdrop and the sun and the ambient
    /// are 0; under an image that is the whole base the rings are the image
    /// and the sun and the ambient the generated sky's (the image is not metered).
    pub fn skyline_light(&self) -> skyline::SkylineLight {
        use skyline::RING;
        let turn = self.params.skyline.as_ref().map_or(0.0, |block| block.turn_deg);
        let at = Sampling { bare: true, unveiled: true, even_ground: true, ..Sampling::default() };
        let ring_seen = |el_deg: f32, at: Sampling| -> [[f32; 3]; RING] {
            let mut ring = [[0.0; 3]; RING];
            for (i, entry) in ring.iter_mut().enumerate() {
                let u = ((i as f32 + 0.5) / RING as f32 + turn / 360.0).rem_euclid(1.0);
                let d = vec(ibl::equirect_uv_to_dir([u, 0.5 - el_deg / 180.0]));
                *entry = arr(self.base_under_skyline(d, None, at));
            }
            ring
        };
        let ring = |el_deg: f32| ring_seen(el_deg, at);
        let (ring_low, ring_high, ground_ring) = (ring(skyline::RING_LOW_DEG), ring(skyline::RING_HIGH_DEG), ring(-skyline::GROUND_RING_DEG));
        // The sky the strip's sea mirrors: as the map shows it, through its weather (the light a far sea mirrors has
        // come through the rain and fog to the water); without a weather the rings themselves.
        let (sea_low, sea_high) = match self.weather {
            Some(_) => {
                let seen = Sampling { bare: true, ..Sampling::default() };
                (ring_seen(skyline::RING_LOW_DEG, seen), ring_seen(skyline::RING_HIGH_DEG, seen))
            }
            None => (ring_low, ring_high),
        };
        // The strip's wet ground (plan hdri-6 §5): the puddles' share and the ground between them; the map's floor holds the water.
        let wet = match &self.weather { Some(weather) if self.atmo.is_some() => weather.puddle_share(), _ => 0.0 };
        let ground_damp = if wet > 0.0 { ring_seen(-skyline::GROUND_RING_DEG, Sampling { damp_ground: true, ..at }) } else { ground_ring };
        // The aurora as the map shows it at the sea's two rings, for the strip's water to mirror at night (plan
        // hdri-7 D12): the sea's rings less the same rings without it, so the cover and the veil in front are the map's.
        let sea_at = match self.weather {
            Some(_) => Sampling { bare: true, ..Sampling::default() },
            None => at,
        };
        let aurora = self.night.as_ref().filter(|night| night.has_aurora()).map(|_| {
            let without = Sampling { no_aurora: true, ..sea_at };
            let less = |with: &[[f32; 3]; RING], el_deg: f32| -> [[f32; 3]; RING] {
                let bare = ring_seen(el_deg, without);
                std::array::from_fn(|i| std::array::from_fn(|c| (with[i][c] - bare[i][c]).max(0.0)))
            };
            (less(&sea_low, skyline::RING_LOW_DEG), less(&sea_high, skyline::RING_HIGH_DEG))
        });
        let Some(atmo) = &self.atmo else {
            return skyline::SkylineLight { sun_u: 0.0, sun_el_deg: -90.0, sun_rgb: [0.0; 3], ambient_rgb: [0.0; 3], ring_low, ring_high, ground_ring, sea_low, sea_high, wet, ground_damp, aurora };
        };
        let pi = std::f32::consts::PI;
        let sun = atmo.sun_dir();
        let elevation = sun.y.clamp(-1.0, 1.0).asin();
        let outer = atmo.sun_outer_radius();
        let visible = smoothstep(-outer, outer, elevation);
        let sun_rgb = if visible > 0.0 { atmo.sun_irradiance() * (self.cover_toward(sun) * visible / pi) } else { Vec3f::default() };
        // The white ground as the map lights it: the atmosphere's (the sky and
        // the sun over a level surface) and the night's, times the ground gain.
        let rise = sun.y.max(0.0);
        let night = self.night.as_ref().map_or(Vec3f::default(), |night| night.ground(vec3f(0.0, -1.0, 0.0), Vec3f::all(1.0)));
        let white = ((atmo.sky_irradiance() + atmo.sun_irradiance() * rise) * (1.0 / pi) + night) * self.ground_gain;
        let ambient = white - sun_rgb * rise;
        let ambient = vec3f(ambient.x.max(0.0), ambient.y.max(0.0), ambient.z.max(0.0));
        skyline::SkylineLight {
            sun_u: (ibl::dir_to_equirect_uv(arr(sun))[0] - turn / 360.0).rem_euclid(1.0),
            sun_el_deg: elevation.to_degrees(),
            sun_rgb: arr(sun_rgb),
            ambient_rgb: arr(ambient),
            ring_low,
            ring_high,
            ground_ring,
            sea_low,
            sea_high,
            wet,
            ground_damp,
            aurora,
        }
    }

    /// 2^intensity_ev, the factor every lookup ends with (for the tests; the
    /// layers read the field).
    #[cfg(test)]
    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    /// Linear radiance arriving from `dir` (unit, world space, as displayed:
    /// rotation already applied). Order: undo the rotation, base layer (Sky:
    /// atmosphere + disc + stars + moon + glow + night ground, then the clouds
    /// over it, then the weather; Studio: backdrop, under any gobo shadow cast
    /// onto it; or a base image, the whole base or below its mask, the
    /// weather's veil over it there too), the skyline over the base (Sky mode,
    /// the veil in front of it too), lights overlay, x 2^intensity_ev.
    pub fn radiance(&self, dir: Vec3f) -> Vec3f {
        self.radiance_with(dir, None, Sampling::default())
    }

    /// [`Self::radiance`] with the key's own term left out (`Some`): the key
    /// light skipped, the sun's disc not drawn, or the share of the moon's disc
    /// the key carries taken off it. Everything else (the sky around a disc,
    /// the light it puts on the clouds, every other light) is drawn as before,
    /// so this is the map without exactly its key's light (N3). `None` is
    /// `radiance`, bit for bit. The base is the mode's own, a base image, or
    /// both through the image's mask. `at` is what a bake hands its lookups:
    /// its cloud march, when it has one, and its texel size.
    fn radiance_with(&self, dir: Vec3f, without: Option<KeyTerm>, at: Sampling) -> Vec3f {
        // The layers live in the map's own frame; turning the map by
        // rotation_deg (ibl's sign) is turning the lookup the other way.
        let d = rotate_y(dir, -self.params.rotation_deg);
        let base = self.base_at(d, without, at);
        // The map without its key (N3) leaves the key light out here.
        self.lights_over(d, base, without == Some(KeyTerm::Light), at) * self.scale
    }

    /// The base under the lights along `d`, in the map's own frame: the
    /// mode's own base layer, a base image, or both through the image's mask;
    /// then the skyline over all of it, the weather's veil in front of the
    /// city as it is in front of the sky, over the distance to what the strip
    /// shows there and without the rain's shafts, which hang behind it
    /// (`city_veil`, `skyline::over_veiled`): the strip's coverage veiled over
    /// its own distance, the base behind it as the map veils it; its lights
    /// shine through (`city_lit`, plan hdri-6 R4). The city hides what stands
    /// behind it, a storm's bolt included. Where the strip gives way to a wet
    /// floor (`under_band`), the floor's puddles lie over the strip's floor as
    /// the map's own field lays them, the strip's ground between them. Without
    /// a skyline the base is drawn exactly as before.
    fn base_at(&self, d: Vec3f, without: Option<KeyTerm>, at: Sampling) -> Vec3f {
        // The distance and the light are read only where a weather veils the strip by them.
        let read = match &self.weather {
            Some(_) => self.city_far_at(d),
            None => self.city_at(d).map(|s| (s, None, None)),
        };
        let Some((s, far, light)) = read else {
            return self.base_under_skyline(d, without, at);
        };
        if s == [0.0; 4] {
            return self.base_under_skyline(d, without, at);
        }
        // An opaque texel hides the base outright (`over_veiled`), so the base
        // is not looked up behind it: a held bake's ranges, city and ground.
        // Where the strip gives way to a wet floor the texel is scaled to the
        // floor's puddles there (`under_band`).
        let (k, c) = self.under_band(d, s, without, at);
        let (s, light) = if k == 1.0 { (s, light) } else { (s.map(|c| c * k), light.map(|l| skyline::TexelLight { lum: l.lum * k, ..l })) };
        match &self.weather {
            Some(weather) => {
                // (what the strip's water mirrors is veiled up the mirror, `mirrored`)
                let m = self.mirrored(d, far);
                let veil = |x| self.strip_veil(weather, d, far, m, x);
                match self.city_lit(weather, d, m, s, light) {
                    None => skyline::over_veiled(s, c, &veil),
                    Some((surface, lit)) => skyline::over_veiled(surface, c, &veil) + lit,
                }
            }
            None => skyline::over_veiled(s, c, &|x| x),
        }
    }

    /// How much of the strip texel along `d`, `far` m off, shows what the strip's water mirrors (`SkylineBand::mirrored`,
    /// plan hdri-6 fix wave 2): the light of what stands above the horizon, run down to the water and on to the eye over
    /// `far`. Its veil and its lights' run up the mirror, (d.x, -d.y, d.z), as a puddle's (`over_city`), not along `d`,
    /// whose path the weather ends on its floor short of `far` (`strip_veil`, `city_lit`). 0 without a distance, above
    /// the horizon, and for a band handed without the strip's eye.
    fn mirrored(&self, d: Vec3f, far: Option<f32>) -> f32 {
        match (&self.skyline, far) {
            (Some((band, _)), Some(far)) => band.mirrored(d, far),
            _ => 0.0,
        }
    }

    /// `city_veil` for a strip texel along `d`, `m` of it what the strip's water mirrors (`mirrored`): that share veiled
    /// up the mirror, (d.x, -d.y, d.z), over `far`, the rest along `d`. Both are affine in `x`, and so is the blend. At
    /// m 0, `city_veil` along `d` itself.
    fn strip_veil(&self, weather: &weather::Weather, d: Vec3f, far: Option<f32>, m: f32, x: Vec3f) -> Vec3f {
        if !(m > 0.0) {
            return self.city_veil(weather, d, far, x);
        }
        let up = self.city_veil(weather, vec3f(d.x, -d.y, d.z), far, x);
        if m >= 1.0 {
            return up;
        }
        let down = self.city_veil(weather, d, far, x);
        down + (up - down) * m
    }

    /// `seen`, the map's sky along `v` above the horizon (veiled, its bolt in), with the skyline's strip laid over it where
    /// the strip covers `v`, veiled as `base_at` veils it, over the strip's distance there: what a wet ground mirrors (the
    /// light runs from the city to the puddle and on to the eye, about the city's distance). Without a skyline, `seen`
    /// itself.
    /// Where the texel has a distance, `own` gets the strip's own light in it, `veil_D(s) - (1 - a) veil_D(0)` (its
    /// coverage veiled over its distance), and the coverage `a`: the floor does not veil that light again (`base_layer`).
    /// Its lights are seen as the weather shows them (`city_lit`, plan hdri-6 R4) and go into `own` too, a light without
    /// coverage (the glow, a lamp over the sky) with a = 0, so it is veiled once, over the distance it burns at.
    /// The strip is seen only through `city_at`, which is None until the skirt is built, so the dome, the ground gain and
    /// the skirt's hold never see the mirrored city, and a map built with the band and one it is attached to agree.
    fn over_city(&self, v: Vec3f, seen: Vec3f, weather: &weather::Weather, own: &Cell<Option<(Vec3f, f32)>>) -> Vec3f {
        match self.city_far_at(v) {
            Some((s, far, light)) if s != [0.0; 4] => {
                let veil = |x| self.city_veil(weather, v, far, x);
                // the texel's surface, and its light as the weather shows it on top (none: the texel all surface, + 0 exactly)
                let split = self.city_lit(weather, v, 0.0, s, light);
                let (surface, lit) = split.unwrap_or((s, Vec3f::default()));
                let behind = 1.0 - s[3];
                let covered = far.map(|_| {
                    let c = veil(vec3f(surface[0], surface[1], surface[2]));
                    (if behind > 0.0 { c - veil(Vec3f::default()) * behind } else { c }, s[3])
                });
                match (covered, split.is_some()) {
                    (Some((c, a)), _) => own.set(Some((c + lit, a))),
                    (None, true) => own.set(Some((lit, 0.0))),
                    (None, false) => {}
                }
                skyline::over_veiled(surface, seen, &veil) + lit
            }
            _ => seen,
        }
    }

    /// Texel `s` along `d` split into its surface and its light as the weather shows the light (plan hdri-6 R4: the
    /// city's lights shine through rain and fog): the surface (`TexelLight::split`) for `city_veil` to veil, and the
    /// light by `Weather::light_through` over the distance it burns at, the forward-scattered share kept; `m` of it, the
    /// share the strip's water mirrors (`mirrored`), up the mirror. None where the texel has no light, the band no light
    /// plane, or the weather veils nothing (a weather with rain and fog at 0 draws as before, bit for bit).
    fn city_lit(&self, weather: &weather::Weather, d: Vec3f, m: f32, s: [f32; 4], light: Option<skyline::TexelLight>) -> Option<([f32; 4], Vec3f)> {
        let light = light.filter(|_| weather.veils())?;
        let (surface, lit) = light.split(s);
        let (down, up) = (|| weather.light_through(d, light.far), || weather.light_through(vec3f(d.x, -d.y, d.z), light.far));
        let through = if !(m > 0.0) { down() } else if m >= 1.0 { up() } else { let a = down(); a + (up() - a) * m };
        Some((surface, lit * through))
    }

    /// The veil the skyline's strip gets along `d` (`base_at`, `over_city`): the rain haze and the fog over the distance
    /// `far` (m) to what the strip shows there (plan hdri-6 R1: the city 1-3 km off stays visible under rain, the far
    /// ranges veiled more; never more than the whole path, `Weather::veil_within`), and not the rain's shafts, which hang
    /// far off behind the city (§5b). A texel without a distance (None: no coverage, the glow; a band without the
    /// plane) is veiled over the whole path, as before. The base behind the strip keeps the whole veil and its shafts
    /// (`over_veiled` lays the strip over the base as the map veils it), and so does the key's cover.
    fn city_veil(&self, weather: &weather::Weather, d: Vec3f, far: Option<f32>, x: Vec3f) -> Vec3f {
        match far {
            Some(far) => weather.veil_within(d, x, self.weather_sun, self.dome, far),
            None => weather.veil_without_virga(d, x, self.weather_sun, self.dome),
        }
    }

    /// [`Self::base_at`] without the skyline.
    fn base_under_skyline(&self, d: Vec3f, without: Option<KeyTerm>, at: Sampling) -> Vec3f {
        match &self.base_image {
            // An image that is the whole base: the sky under it is never seen, so it is not drawn.
            Some(image) if !image.replaces_sky() => image.radiance(d),
            // Sky replacement: the image below its mask, the mode's own base
            // above. The weather's rain haze and fog lie between the eye and
            // the photograph too, so its ground meets the veiled sky without a
            // seam (D6); the bolt stands behind the photograph's line.
            Some(image) => {
                let sky = self.base_layer(d, without, at);
                match &self.weather {
                    // (unveiled, the sky is too: the veil is affine, so veiling the mix later is mixing the veiled two)
                    Some(weather) if !at.unveiled => image.compose_veiled(d, sky, &|photo| weather.veil(d, photo, self.weather_sun, self.dome)),
                    _ => image.compose(d, sky),
                }
            }
            None => self.base_layer(d, without, at),
        }
    }

    /// The lights drawn over `base` along `d`, in the map's own frame. A light
    /// filled with sky (GoboParams::fill_sky) is a window: it shows the sky
    /// along the same direction ([`Self::window_sky_at`]), sampled as `at`
    /// samples this map. `without_key` leaves the key light out (N3).
    fn lights_over(&self, d: Vec3f, base: Vec3f, without_key: bool, at: Sampling) -> Vec3f {
        let sky = |w: Vec3f| self.window_sky_at(w, base, at);
        self.studio.apply_lights_sky(d, base, &sky, without_key)
    }

    /// What the map draws at `d` (map frame, the rotation already taken out)
    /// from everything before studio entry `entry`, times 2^intensity_ev: what
    /// a caster at `entry` stands in front of (its bloom's pre-pass). That is
    /// the base (a base image, the sky with its discs, clouds and weather, or
    /// the backdrop under its gobo shadow) and the lights and casters listed
    /// before it, their bloom included. `at` samples it as a bake of the
    /// pre-pass's width would, without a cloud cache.
    fn content_before(&self, d: Vec3f, entry: usize, at: Sampling) -> Vec3f {
        let base = self.base_at(d, None, at);
        let sky = |w: Vec3f| self.window_sky_at(w, base, at);
        self.studio.apply_lights_until(d, base, &sky, entry) * self.scale
    }

    /// The casters' bloom (redesign §5, decision 11). It is built once per map,
    /// after every layer the casters stand in front of, for each caster with
    /// bloom on, in list order, so a caster sees the bloom of the casters
    /// before it. What the map draws behind the caster (`content_before`) and
    /// its own coverage are rastered in its frame for a map `width` wide, the
    /// sun's and the moon's discs sampled on grids of their own (D6), what the
    /// lights before it may reach as finely as those lights ask
    /// (`Studio::bright_boxes`), and the base's fine detail where it has any
    /// (`base_peak`), the rows spread by `run` and the samples held to one
    /// budget for all of them (M5). A caster that
    /// nothing bright stands behind keeps no raster: it stays exactly the flag
    /// it draws as. False when `cancelled` stopped it (or the runner skipped a
    /// row).
    fn build_caster_bloom(&mut self, width: usize, cancelled: &dyn Fn() -> bool, run: caster::RowRunner) -> bool {
        let entries = self.studio.bloom_casters();
        if entries.is_empty() {
            return true;
        }
        // The discs, turned into the map's own frame, as `studio_key` turns them.
        let hot: Vec<(Vec3f, f32)> = self
            .hot_spots()
            .into_iter()
            .map(|(dir, radius)| (rotate_y(dir, -self.params.rotation_deg), radius))
            .collect();
        let at = Sampling::bake(width, None);
        // M5: the casters share one budget of samples (`caster::BLOOM_CELL_BUDGET`), so many
        // blooming casters cost no more than one may.
        let casters = entries.len();
        let fine_base = self.base_image.is_some() || (self.atmo.is_some() && self.weather.is_some()) || self.has_skyline();
        for entry in entries {
            let built = {
                let env: &Env = self;
                let Some(caster) = env.studio.caster(entry) else { continue };
                let behind = |d: Vec3f| env.content_before(d, entry, at);
                let sharp = |d: Vec3f| env.studio.caster_sharp(entry, d);
                let bright = env.studio.bright_boxes(entry);
                let peak = |d: Vec3f, radius: f32| env.base_peak(d, radius);
                let peak: Option<&(dyn Fn(Vec3f, f32) -> Option<(f32, f32)> + Sync)> = if fine_base { Some(&peak) } else { None };
                caster::CasterBloom::build(caster, width, casters, &hot, &bright, peak, &behind, &sharp, cancelled, run)
            };
            let Some(bloom) = built else { return false };
            if bloom.thins_anything() {
                self.studio.set_caster_bloom(entry, bloom);
            }
        }
        true
    }

    /// What a caster's bloom must look for in the base within `radius`
    /// (radians) of `d` (map frame), detail a look at a raster cell's corners
    /// may miss (fix round 3 of S3's review): the most luminance it may draw
    /// there, times 2^intensity_ev, and the spacing that detail asks for
    /// (radians). A base image's (`BaseImage::peak`, under or beside the sky it
    /// replaces), a storm's bolt (`Weather::fine_step_near`, no bound) and
    /// the skyline's lights there (the band's `BandPeaks`, built with the
    /// band; under a wet ground also those over the mirrored direction, which
    /// the puddles show): a city light is far finer than a cell. None where the base
    /// has no such detail: the sky's discs are sampled apart (`hot_spots`),
    /// and the rest of a generated sky and the backdrop are smooth at a
    /// cell's scale.
    fn base_peak(&self, d: Vec3f, radius: f32) -> Option<(f32, f32)> {
        let image = self.base_image.as_ref().map(|image| image.peak(d, radius)).map(|(lum, step)| (lum * self.scale, step));
        let bolt = self.weather.as_ref().filter(|_| self.atmo.is_some()).and_then(|weather| weather.fine_step_near(d, radius)).map(|step| (f32::INFINITY, step));
        let city = match (&self.skyline, &self.city_peaks) {
            (Some((_, turn)), Some(peaks)) => {
                let direct = peaks.near(d, *turn, radius);
                // a wet ground mirrors the city: its lights show below the horizon at the mirrored elevation
                let mirrored = self.weather.as_ref().filter(|w| w.mirrors() && d.y < 0.0).and_then(|_| peaks.near(vec3f(d.x, -d.y, d.z), *turn, radius));
                [direct, mirrored].into_iter().flatten().reduce(|a, b| (a.0.max(b.0), a.1.min(b.1))).map(|(most, step)| (most * self.scale, step))
            }
            _ => None,
        };
        [image, bolt, city].into_iter().flatten().reduce(|a, b| (a.0.max(b.0), a.1.min(b.1)))
    }

    /// The sky a window shows along `d` (map frame) over `base`, the base
    /// there: in Sky mode the base itself; in Studio mode the lightless
    /// Sky-mode copy works it out, and only for the directions such a window
    /// covers. A window shows the sky along its texel's own directions, so the
    /// copy is read with this lookup's texel (a bake's bolt is averaged over
    /// it there too, D7); it marches its own volume, the cache in `at` being
    /// this map's.
    fn window_sky_at(&self, d: Vec3f, base: Vec3f, at: Sampling) -> Vec3f {
        match &self.window_sky {
            Some(outside) => outside.radiance_with(d, None, Sampling { cache: None, ..at }),
            None => base,
        }
    }

    /// The spacing the key's integral over a window filled with sky must
    /// sample the sky it lets in at within `radius` (radians) of `d` (map
    /// frame), where that sky draws a line thinner than the integral's own
    /// samples: a storm's bolt (`Weather::fine_step_near`). None where it is
    /// smooth at their spacing. The sky behind a studio is the lightless copy's.
    fn window_sky_step_near(&self, d: Vec3f, radius: f32) -> Option<f32> {
        match &self.window_sky {
            Some(outside) => outside.window_sky_step_near(d, radius),
            None => self.weather.as_ref().and_then(|weather| weather.fine_step_near(d, radius)),
        }
    }

    /// The mode's own base layer under the lights, in the map's own frame: the sky
    /// (the dry sky, its ground lit by this map's own sky, then the weather over it)
    /// or the studio backdrop, under any gobo shadow cast onto it. `without` leaves
    /// the key's disc out (plan 2's N3); `at` is what the bake hands its lookups (its
    /// cloud march, its texels).
    fn base_layer(&self, d: Vec3f, without: Option<KeyTerm>, at: Sampling) -> Vec3f {
        match &self.atmo {
            Some(atmo) => match &self.weather {
                // Below the horizon the ground lit by this map's own sky (D7): a
                // deck the Clouds or the volume close lights it with no weather too.
                None => weather::relit_ground(d, self.sky_layers(d, atmo, without, at), self.ground_gain),
                Some(weather) => {
                    let dry = |v: Vec3f| self.sky_layers(v, atmo, without, at);
                    // Below the horizon: the ground lit by this map's own sky (D7),
                    // and the wet ground's mirror of the sky above it.
                    // The mirror shows the sky as the weather shows it: through the
                    // rain and fog, the bolt included (D7). It reflects the whole sky,
                    // the key's disc included, also in the map without its key: the
                    // reflection is light from below, which the engine's directional
                    // light does not carry (plan 2's N3 takes out exactly what the key
                    // carries, no more).
                    // The strip's own light in the mirror and its coverage, where it has a distance (`over_city`).
                    let city = Cell::new(None);
                    let seen = |v: Vec3f| {
                        let sky = weather.veil(v, self.sky_layers(v, atmo, None, at), self.weather_sun, self.dome)
                            + weather.lightning(v, at.texel);
                        // The skyline stands over the sky it mirrors: a puddle shows the city where it stands above the
                        // horizon, veiled as the map veils it there (plan hdri-6 §5).
                        self.over_city(v, sky, weather, &city)
                    };
                    let puddles = if at.damp_ground {
                        weather::Puddles::Dry
                    } else if at.even_ground {
                        weather::Puddles::Even
                    } else if at.water_ground {
                        weather::Puddles::Full
                    } else {
                        weather::Puddles::Field
                    };
                    let c = match puddles {
                        weather::Puddles::Dry => weather.ground_damp(d, dry(d), self.ground_gain, &seen),
                        weather::Puddles::Even => weather.ground_even(d, dry(d), self.ground_gain, &seen),
                        weather::Puddles::Full => weather.ground_water(d, dry(d), self.ground_gain, &seen),
                        weather::Puddles::Field => weather.ground(d, dry(d), self.ground_gain, &seen),
                    };
                    // The base behind the veil, for the skyline's strip, which the map veils once (`base_at`).
                    if at.unveiled {
                        return c;
                    }
                    // Rain haze, its virga and fog between the eye and all of it, lit
                    // by the dome above them; then the bolt and its flash, which stand
                    // inside the rain.
                    let veiled = match city.get().filter(|_| weather.veils()) {
                        // (a weather that veils nothing passes `c` itself, bit for bit)
                        None => weather.veil(d, c, self.weather_sun, self.dome),
                        // The mirror shows the strip, veiled over its distance (plan hdri-6 R1: the light runs city ->
                        // puddle -> eye, about the city's distance, and is veiled once). The share of the view the mirror
                        // takes there (`mirror_share`, k, of it the strip's coverage a) runs to the strip, not along the
                        // floor: its light `own` is not veiled again, and the floor's in-scatter is left off that share.
                        // The rest of the texel (the puddle's bottom, the damp ground, the sky beside the strip's
                        // coverage) keeps the floor's veil. Where the floor's veil is thin this is the old sum.
                        Some((own, a)) => {
                            let k = weather.mirror_share(d, puddles);
                            weather.veil(d, c - own * k, self.weather_sun, self.dome) + own * k
                                - weather.veil(d, Vec3f::default(), self.weather_sun, self.dome) * (a * k)
                        }
                    };
                    veiled + weather.lightning(d, at.texel)
                }
            },
            None => {
                let backdrop = self.studio.backdrop(d);
                // A gobo cast from its light darkens the floor and the walls under it.
                match &self.backdrop_gobo {
                    Some(cast) => backdrop * cast.shade(d),
                    None => backdrop,
                }
            }
        }
    }

    /// The dry sky: atmosphere + disc + stars + moon + glow + night ground, then the
    /// clouds over it (the discs under the unfaded cover), then the marched volume.
    /// Under weather the clouds keep only the light the kind's deck passes, and the
    /// night ground and the volume's bases have the weathered ground colour.
    /// `without` leaves the key's disc out (plan 2's N3: no sun disc under
    /// `KeyTerm::Sun`, the moon's key share off under `KeyTerm::Moon`).
    /// `at` is what the lookup samples with: `points` is `!at.bare` (no discs,
    /// no stars: a mean over a few hundred directions cannot sample them,
    /// `dome_light`), the cloud march is `at.cache`, and the night's layers
    /// band-limit to `at.texel` (plan hdri-7 D10).
    fn sky_layers(&self, d: Vec3f, atmo: &atmosphere::Atmosphere, without: Option<KeyTerm>, at: Sampling) -> Vec3f {
        let points = !at.bare;
        // The ground under the sky, as the weather leaves it (snow is white).
        let g = self
            .weather
            .as_ref()
            .map_or(self.params.sky.atmosphere.ground_color, |weather| weather.ground_color());
        let ground_color = vec3f(g[0], g[1], g[2]);
        let mut c = atmo.sky(d);
        // The sun's and the moon's discs are blocked by the unfaded
        // cover, like their keys in `sun`; the rest of the sky by the
        // visible deck, which thins into the haze at the horizon.
        let mut discs = if without == Some(KeyTerm::Sun) || !points { Vec3f::default() } else { atmo.sun_disc(d) };
        if let Some(night) = &self.night {
            let stars = if points { night.deep_sky(d, at.texel) } else { Vec3f::default() };
            c += stars + night.glow(d) + night.ground(d, ground_color);
            // The aurora (plan hdri-7 §4): diffuse light behind the clouds, outside the points, so it lights the dome
            // and the strip's rings; band-limited to the lookup's texel, or to the rings' spacing for a bare lookup.
            if night.has_aurora() && !at.no_aurora {
                c += night.aurora(d, if at.bare { aurora::AURORA_BARE_TEXEL } else { at.texel });
            }
            if points {
                discs += match without {
                    Some(KeyTerm::Moon(share)) => night.moon(d) * (1.0 - share),
                    _ => night.moon(d),
                };
            }
        }
        // 1 in fair weather, so nothing changes there bit for bit.
        let gain = self.weather.as_ref().map_or(1.0, |weather| weather.cloud_gain());
        // Clouds are in front of everything in the sky: the stars, the
        // moon and the sun's disc. In volumetric mode this layer is the
        // cirrus veil alone.
        if let Some(layer) = &self.clouds {
            let s = layer.sample(d);
            if s.alpha > 0.0 {
                let lit = layer.shade(&s, self.cloud_sun, self.cloud_ambient);
                c = c * (1.0 - s.alpha) + lit * (s.alpha * gain);
            }
            // Only a texel on a disc pays for the second cloud lookup.
            if discs.max_elem() > 0.0 {
                discs *= 1.0 - layer.cover_toward(d);
            }
        }
        // The discs, under the 2D layer's unfaded cover, join the sky behind
        // the volume: the marched volume is the lower layer, so it goes over
        // the veil and the discs.
        c += discs;
        if let Some(volume) = &self.volumetric {
            if d.y > 0.0 {
                let m = match at.cache {
                    Some(cache) => cache.sample(volume, d),
                    None => volume.march(d),
                };
                if m.alpha > 0.0 {
                    // The ground under the layer, lit by the same dome (as the
                    // clouds see it, greyed by their cover), lights the bases.
                    let ground = ground_color * volume.sky_light(self.cloud_ambient);
                    c = c * (1.0 - m.alpha) + volume.shade(&m, self.cloud_sun, self.cloud_ambient, ground) * gain;
                }
            }
        }
        c
    }

    /// The light the weather's veil gets from the sky above it (B17's fix
    /// round): the solid-angle mean of the dry sky over the upper hemisphere,
    /// as this map draws it, without the discs and the stars (the sun reaches
    /// the veil on its own, through the cover, as `weather_sun`). On a fair
    /// sky this is the open dome's mean (`cloud_ambient`) to within the
    /// sampling; under a deck it is the light the deck passes down, its sunlit
    /// undersides and the kind's cloud light included, so the fog and the rain
    /// haze are lit as the clouds above them are. Eight rows equally spaced in
    /// the sine of the elevation (equal solid angles) of 32 directions each,
    /// every other row turned by half a column; a volume is marched along each.
    /// The same directions give the light the dome puts on the ground (D7),
    /// and the light the open sky put there (its clouds left out), so the two
    /// differ by the cover alone.
    fn dome_light(&self) -> Dome {
        const ROWS: usize = DOME_ROWS;
        const COLUMNS: usize = DOME_COLUMNS;
        let Some(atmo) = &self.atmo else {
            return Dome::default();
        };
        let mut dome = Dome::default();
        // Each direction stands for an equal share of the hemisphere's 2 pi.
        let share = std::f32::consts::TAU / (ROWS * COLUMNS) as f32;
        for j in 0..ROWS {
            for i in 0..COLUMNS {
                let (d, sine) = dome_look(j, i);
                let drawn = self.sky_layers(d, atmo, None, Sampling { bare: true, ..Sampling::default() });
                // What sky_layers draws with no clouds, and without the points.
                let open = atmo.sky(d) + self.night.as_ref().map_or(Vec3f::default(), |night| night.glow(d));
                // The aurora is in what the open sky puts on the ground too, so the cover's gain stays a cover ratio
                // (plan hdri-7 D9; the ground takes the aurora's light through `NightSky::ground_irradiance`).
                let open = match &self.night {
                    Some(night) if night.has_aurora() => open + night.aurora(d, aurora::AURORA_BARE_TEXEL),
                    _ => open,
                };
                dome.mean += drawn;
                dome.irradiance += drawn * (sine * share);
                dome.open += open * (sine * share);
            }
        }
        dome.mean *= 1.0 / (ROWS * COLUMNS) as f32;
        dome
    }

    /// The cover's gain on the ground's light (D7): what this map's sky puts
    /// on a level surface (the dome as drawn, `dome_light`, and the sun and
    /// the moon through the clouds) over what the clear sky put there, which
    /// the atmosphere and the night lit their ground with. So the ground under
    /// a closed deck is lit by the deck's light, not by the clear sky's sun,
    /// and a sky nothing covers leaves it exactly as it was. Per channel; 1
    /// where the clear sky put no light.
    fn measure_ground_gain(&self, dome: &Dome) -> Vec3f {
        let Some(atmo) = &self.atmo else {
            return Vec3f::all(1.0);
        };
        let rise = atmo.sun_dir().y.max(0.0);
        let (moon, moon_through) = match &self.night {
            Some(night) => {
                let through = night.moon_disc().map_or(1.0, |(dir, _)| {
                    self.clouds.as_ref().map_or(1.0, |layer| 1.0 - layer.cover_toward(dir))
                        * self.volumetric.as_ref().map_or(1.0, |volume| volume.transmittance_toward(dir))
                });
                (night.moon_ground_irradiance(), through)
            }
            None => (Vec3f::default(), 1.0),
        };
        let open = dome.open + atmo.sun_irradiance() * rise + moon;
        let lit = dome.irradiance + self.weather_sun * rise + moon * moon_through;
        let gain = |lit: f32, open: f32| if open > 0.0 && lit.is_finite() { lit / open } else { 1.0 };
        vec3f(gain(lit.x, open.x), gain(lit.y, open.y), gain(lit.z, open.z))
    }

    /// The luminance a level surface receives from the mode's own base (its
    /// upper hemisphere), metered the way a base image is: a 64 x 32 bake of
    /// the base layer, the sun's and moon's texels area-averaged as in any
    /// bake, read by `composite::irradiance_above`.
    fn base_irradiance(&self) -> f32 {
        let at = Sampling::bake(composite::SKY_METER_WIDTH, None);
        let layer = |d: Vec3f| if d.y > 0.0 { self.base_layer(d, None, at) } else { Vec3f::default() };
        let mut map = bake_with(composite::SKY_METER_WIDTH, &layer);
        // hot_spots() are in world space; the base layer lives in the map's own frame.
        let hot: Vec<(Vec3f, f32)> = self
            .hot_spots()
            .into_iter()
            .map(|(dir, radius)| (rotate_y(dir, -self.params.rotation_deg), radius))
            .collect();
        refine_hot_spots(&mut map, &layer, &hot);
        composite::irradiance_above(&map, 0.0)
    }

    /// Key light for the engine, in world space, already x 2^intensity_ev
    /// (every layer's key goes through `key_to_world`, which turns it with the
    /// map and applies the intensity). In order: the key light (both modes);
    /// the sun of an image that is the whole base; Sky mode only: the sun while
    /// any of its disc is above the horizon, else the moon once the sun is 6
    /// degrees down and it is risen, else None. Every key carries exactly the
    /// light the map draws of it (the plan owner's D1): the sun and the moon
    /// are dimmed by the cloud cover along them (unfaded: a full overcast
    /// hides a 2 degree sun too), by the weather's rain, its shafts and fog,
    /// and by what stands in front of their discs in the map (the lights over
    /// the sky, a base image's line: `disc_share`), as an image's own sun is.
    ///
    /// The sun and the moon fade instead of switching: the key is a continuous
    /// function of the hour, so a host that samples it finely (a slider, a day
    /// cycle read every frame) passes sunset without a jump. The sun's key is its
    /// radiance x the part of its disc that shows, `smoothstep(-outer, +outer,
    /// elevation)` (outer: the disc's outer limb, soft edge included), so it is
    /// half on the horizon and `Some` until the whole disc is down; the moon's
    /// arrives over a sun elevation of -6 to -8 degrees, `smoothstep(6, 8, -sun
    /// elevation)`, so between the two there is a stretch of twilight with no key
    /// at all.
    ///
    /// The fade is narrow, though: the sun crosses it in about two minutes of
    /// sun time. A host that takes the key from a coarser grid steps by the grid,
    /// not by the fade: the sandbox's day cycle re-bakes a map on the quarter
    /// hour, at the world's latitude on its fixed game date, and every 7.5
    /// minutes while the sun is low, and the step of its key is bounded at 20 %
    /// of the 16:00 key per bake (the plan owner's K8). Under the default clear
    /// sky the quarter hour alone would step up to 25.5 % at the equator and
    /// 30 % at 35 S; on the halved grid the largest step is 8.2 % at 52 N and
    /// at most 15.9 % from 70 N to 45 S, and the sandbox's env_map tests pin it.
    ///
    /// The key is measured from the map's drawing on the first call (a key with
    /// a grid, a picture or a window takes tens of milliseconds), and later calls
    /// and [`Self::bake_keyless_par`] read that one key (D15, M5): a host that
    /// never asks, as the app's preview bakes do not, never pays for it.
    pub fn sun(&self) -> Option<EnvSun> {
        self.key().map(|(_, key)| key)
    }

    /// [`Self::sun`] with the term of the map it is: the choice is made once,
    /// on the first call (`measure_key`), so the key the engine lights with and
    /// the light a lookup without it leaves out are always the same one.
    fn key(&self) -> Option<(KeyTerm, EnvSun)> {
        *self.measured_key.get_or_init(|| self.measure_key())
    }

    /// The choice and the measurement behind [`Self::key`].
    fn measure_key(&self) -> Option<(KeyTerm, EnvSun)> {
        if let Some(key) = self.studio_key() {
            return Some((KeyTerm::Light, self.key_to_world(key)));
        }
        // An image that is the whole base brings its own sun, or none: the
        // generated sky is not drawn under it. (With sky replacement the image
        // sits below the horizon and the generated sun stays the key.) The
        // lights in front of the photographed sun dim it as they dim it in
        // the map, weighed by the photograph itself.
        if let Some(image) = &self.base_image {
            if !image.replaces_sky() {
                return image.sun().map(|sun| {
                    let radiance = sun.radiance * self.disc_share(&sun, &|d| image.radiance(d));
                    (KeyTerm::Image, self.key_to_world(EnvSun { radiance, ..sun }))
                });
            }
        }
        // Studio mode has no sky and no sun.
        let atmo = self.atmo.as_ref()?;
        // The same cover that hides the discs in the map dims the key: the 2D
        // layer's unfaded cover (in volumetric mode the cirrus veil's), the
        // marched volume, and the weather's rain, its shafts and fog. What
        // stands in front of the disc after that, the lights and a base
        // image's mask, is `disc_share`.
        let cover = |dir: Vec3f| self.cover_toward(dir);
        let sun = atmo.sun_dir();
        let elevation = sun.y.clamp(-1.0, 1.0).asin();
        let outer = atmo.sun_outer_radius();
        let visible = smoothstep(-outer, outer, elevation);
        if visible > 0.0 {
            // The cone's mean radiance, so radiance x cone solid angle is the
            // sun's irradiance at the ground whatever the disc's size and limb.
            // A disc under a degree wide: a surface facing it gets all of its
            // emission (facing 1), and its outer limb, soft edge included, is
            // the cone that holds it.
            let key = EnvSun {
                dir: sun,
                radiance: atmo.sun_cone_radiance() * (cover(sun) * visible),
                cos_radius: atmo.sun_cos_radius(),
                facing: 1.0,
                // The two cosines are separately rounded: the min keeps a tiny soft
                // edge from putting the covering cone's above the cone's own.
                cos_cover: outer.cos().min(atmo.sun_cos_radius()),
            };
            let radiance = key.radiance * self.disc_share(&key, &|d| atmo.sun_disc(d));
            return Some((KeyTerm::Sun, self.key_to_world(EnvSun { radiance, ..key })));
        }
        // After dark the risen moon takes over, once the sun is 6 degrees down.
        let night = self.night.as_ref()?;
        let moon = night.moon_key()?;
        let arrived = smoothstep(6.0, 8.0, -elevation.to_degrees());
        if !(arrived > 0.0) {
            return None;
        }
        // The map draws the whole disc while the key carries `arrived` of it:
        // that share is the key's own light.
        let radiance = moon.radiance * (cover(moon.dir) * arrived) * self.disc_share(&moon, &|d| night.moon(d));
        Some((KeyTerm::Moon(arrived), self.key_to_world(EnvSun { radiance, ..moon })))
    }

    /// What the sky's cover passes along `dir` (map frame), as the key and
    /// the strip's sun are dimmed: the 2D layer's unfaded cover (in
    /// volumetric mode the cirrus veil's), the marched volume, and the
    /// weather's rain, its shafts and fog.
    fn cover_toward(&self, dir: Vec3f) -> f32 {
        self.clouds.as_ref().map_or(1.0, |layer| 1.0 - layer.cover_toward(dir))
            * self.volumetric.as_ref().map_or(1.0, |volume| volume.transmittance_toward(dir))
            * self.weather.as_ref().map_or(1.0, |weather| weather.transmittance_toward(dir))
    }

    /// The studio's key light in the map's own frame, as the map draws it
    /// (D1, `Studio::key`). A window filled with sky as the key carries the
    /// sky it lets in, so it is measured here, against that sky: the base in
    /// Sky mode, the lightless copy behind a studio, with its discs (map
    /// frame) and the lines thinner than its samples (a storm's bolt), which
    /// it samples finely where they run, reading the bolt as a bake of fine
    /// texels draws it (the bake's own channel, `weather::CHANNEL_FOOTPRINT`).
    fn studio_key(&self) -> Option<EnvSun> {
        if !self.studio.key_is_window() {
            return self.studio.key();
        }
        let hot: Vec<(Vec3f, f32)> = self
            .hot_spots()
            .into_iter()
            .map(|(dir, radius)| (rotate_y(dir, -self.params.rotation_deg), radius))
            .collect();
        let fine_texels = Sampling { texel: weather::CHANNEL_FOOTPRINT, ..Sampling::default() };
        let sky = |d: Vec3f| self.window_sky_at(d, self.base_at(d, None, fine_texels), fine_texels);
        let fine = |d: Vec3f, radius: f32| self.window_sky_step_near(d, radius);
        self.studio.measured_window_key(&sky, &hot, &fine)
    }

    /// What stands in front of a disc in the map leaves of it, per channel
    /// (D1): the lights drawn over the sky (flags and gels, a scrim's cloth, a
    /// window's glass at its own exposure and, in Sky mode, its frame and
    /// bars, which hide what is behind them: `LightFx::against_sky`, D12)
    /// and, under sky replacement, the base image below its line. `key` is
    /// the disc's key in the map's own frame, and `disc(d)` the disc as the
    /// map draws it along `d`, which weighs each direction: the soft limb,
    /// and a hard edge across the disc, count as they show. Each is affine in
    /// the base it is drawn over, so what the map makes of a white base less
    /// what it makes of a black one is the share that reaches the eye, the
    /// lights' own light cancelling. 1, exactly, when nothing stands in front
    /// of any disc.
    ///
    /// A midpoint grid of DISC_GRID x DISC_GRID points over the tangent plane
    /// round the disc's covering cone, as `studio::key_emission` takes a key.
    fn disc_share(&self, key: &EnvSun, disc: &dyn Fn(Vec3f) -> Vec3f) -> Vec3f {
        const DISC_GRID: usize = 64;
        let replaced = self.base_image.as_ref().filter(|image| image.replaces_sky());
        if !self.studio.has_lights() && replaced.is_none() && !self.has_skyline() {
            return Vec3f::all(1.0);
        }
        #[cfg(test)]
        probe::count(probe::Work::Key);
        let through = |d: Vec3f, v: Vec3f| {
            let mut base = match replaced {
                Some(image) => image.compose(d, v),
                None => v,
            };
            // The skyline hides what of the disc its alpha covers; its own
            // light is the city's, never the sun's.
            if let Some(s) = self.city_at(d) {
                base *= 1.0 - s[3];
            }
            self.lights_over(d, base, false, Sampling::default())
        };
        let (azimuth_deg, elevation_deg) = az_el_from_dir(key.dir);
        let frame = studio::light_frame(azimuth_deg, elevation_deg, 0.0);
        let c = key.cos_cover.min(key.cos_radius).clamp(0.05, 1.0);
        let reach = (1.0 - c * c).sqrt() / c;
        let cell = 2.0 * reach / DISC_GRID as f32;
        let (mut drawn, mut passed) = (Vec3f::default(), Vec3f::default());
        for j in 0..DISC_GRID {
            let y = -reach + (j as f32 + 0.5) * cell;
            for i in 0..DISC_GRID {
                let x = -reach + (i as f32 + 0.5) * cell;
                let d = (frame.center + frame.right * x + frame.up * y).normalize();
                let w = disc(d) * (1.0 + x * x + y * y).powf(-1.5);
                if !(w.max_elem() > 0.0) {
                    continue;
                }
                drawn += w;
                passed += w * (through(d, Vec3f::all(1.0)) - through(d, Vec3f::default()));
            }
        }
        let share = |p: f32, w: f32| if w > 0.0 { (p / w).max(0.0) } else { 1.0 };
        vec3f(share(passed.x, drawn.x), share(passed.y, drawn.y), share(passed.z, drawn.z))
    }

    /// World-space sun direction in Sky mode (also below the horizon), None
    /// in Studio mode. This is the environment's own sun the engine's "is it
    /// day?" switches read (`Environment.daylight_sun`), whatever the key in
    /// [`Self::sun`] is: the sun itself, the moon, or none at twilight. None
    /// too under an image that is the whole base (1b): the generated sky is not
    /// shown, and a photograph reports no sun of its own, as a loaded file does.
    pub fn sun_dir(&self) -> Option<Vec3f> {
        if self.base_image.as_ref().is_some_and(|image| !image.replaces_sky()) {
            return None;
        }
        self.atmo
            .as_ref()
            .map(|atmo| rotate_y(atmo.sun_dir(), self.params.rotation_deg))
    }

    /// Map frame to world frame, with the map's intensity applied. The cones
    /// and the facing share are angles about the key's own centre, so only the
    /// direction turns.
    fn key_to_world(&self, key: EnvSun) -> EnvSun {
        EnvSun {
            dir: rotate_y(key.dir, self.params.rotation_deg),
            radiance: key.radiance * self.scale,
            ..key
        }
    }

    /// The sun's and the moon's discs, in world space, for the bake's
    /// refinement: below 4K both are smaller than or comparable to a texel, so
    /// their texels are area-averaged (A1's `refine_hot_spots`).
    /// In Studio mode the discs of the sky behind the windows count too, and
    /// over wet ground the discs' reflections. A base image's own sun, where
    /// the map draws it, is as small (D6); under an image that is the whole
    /// base the generated discs are not drawn, so they are not refined.
    fn hot_spots(&self) -> Vec<(Vec3f, f32)> {
        let mut hot = Vec::new();
        let whole = self.base_image.as_ref().is_some_and(|image| !image.replaces_sky());
        if let Some(atmo) = self.atmo.as_ref().filter(|_| !whole) {
            if self.params.sky.sun_disc.visible && atmo.sun_dir().y > -0.1 {
                hot.push((rotate_y(atmo.sun_dir(), self.params.rotation_deg), atmo.sun_outer_radius()));
            }
        }
        if let Some((dir, radius)) = self.night.as_ref().filter(|_| !whole).and_then(|night| night.moon_disc()) {
            if dir.y > -0.1 {
                hot.push((rotate_y(dir, self.params.rotation_deg), radius));
            }
        }
        if let Some(outside) = &self.window_sky {
            // The copy has no rotation, so its world frame is this map's own frame.
            for (dir, radius) in outside.hot_spots() {
                hot.push((rotate_y(dir, self.params.rotation_deg), radius));
            }
        }
        // Wet ground mirrors the discs: their reflections are as small as they
        // are, so their texels are area-averaged too. (Mirroring in the ground
        // plane commutes with the map's rotation about +Y.)
        if self.weather.as_ref().is_some_and(|weather| weather.mirrors()) {
            let mirrored: Vec<(Vec3f, f32)> = hot.iter().map(|&(dir, radius)| (vec3f(dir.x, -dir.y, dir.z), radius)).collect();
            hot.extend(mirrored);
        }
        if let Some((dir, radius)) = self.base_image.as_ref().and_then(|image| image.drawn_sun()) {
            hot.push((rotate_y(dir, self.params.rotation_deg), radius));
        }
        hot
    }

    /// Serial bake of the whole map (tests, small previews): `EnvMap::from_fn`
    /// over `radiance`, so the map is in ibl's convention by construction;
    /// then the texels on the hot spots (the sun's and moon's discs) are
    /// replaced by their area average. A volumetric sky wider than
    /// `volumetric::MARCH_MAX_WIDTH` marches its clouds at that width and
    /// reads them bilinearly.
    pub fn bake(&self, width: usize) -> EnvMap {
        self.bake_marched(width, volumetric::MARCH_MAX_WIDTH)
    }

    /// Row-parallel bake. `run(n, f)` must call f(i) for every i in 0..n
    /// (`pool.fan_out(Lane::Heavy, n, f)` inside a submitted Heavy job; on the
    /// UI thread run it serially, as `render_kernels::task_pool_executor` does).
    /// A skipped row (cancelled) stays black. Equals `bake` bit for bit.
    pub fn bake_par(&self, width: usize, run: impl FnOnce(usize, &(dyn Fn(usize) + Sync))) -> EnvMap {
        self.bake_par_marched(width, volumetric::MARCH_MAX_WIDTH, run)
    }

    /// A row-parallel bake whose texels are each sampled `jitter` texels off
    /// their centre ([x along a row, y down the map], a sub-texel offset, each
    /// within +-0.5), handed to `sink(y, row)` row by row instead of kept: for
    /// a caller that sums jittered bakes into one image (antialiasing: the hard
    /// edges of a skyline, a beacon, an aurora's fold) without holding a
    /// second map. The texel footprint the lookups take ([`Sampling`]) is the
    /// bake's own at `width`. The texels on the hot spots (the sun's and the
    /// moon's discs) are their texel's area average, as a bake makes them,
    /// which no jitter moves. `run(n, f)` spreads the rows as for
    /// [`Self::bake_par`]; a row it skips is never handed over. With jitter
    /// [0, 0] each row is that row of [`Self::bake_par`]'s map bit for bit.
    pub fn bake_rows_jittered_par(
        &self,
        width: usize,
        jitter: [f32; 2],
        run: impl FnOnce(usize, &(dyn Fn(usize) + Sync)),
        sink: &(dyn Fn(usize, &[[f32; 4]]) + Sync),
    ) {
        let cache = self.cloud_cache(width, volumetric::MARCH_MAX_WIDTH);
        let at = Sampling::bake(width, cache.as_ref());
        let radiance = |d: Vec3f| self.radiance_with(d, None, at);
        // bake_par_with's size rounding.
        let width = width.max(2);
        let height = (width / 2).max(1);
        let hot = hot_reaches(&self.hot_spots(), width, height);
        let [jx, jy] = jitter.map(|j| if j.is_finite() { j.clamp(-0.5, 0.5) } else { 0.0 });
        let bake_row = |y: usize| {
            if y >= height {
                return;
            }
            // bake_par_with's texel-centre arithmetic plus the jitter (x + 0.5 + 0 is x + 0.5 exactly), through the
            // same functions, so an unjittered row matches the bake's bits.
            let v = (y as f32 + 0.5 + jy) / height as f32;
            let row: Vec<[f32; 4]> = (0..width)
                .map(|x| {
                    if hot.iter().any(|spot| spot.covers(x, y, width, height)) {
                        return hot_texel(&radiance, x, y, width, height);
                    }
                    let d = ibl::equirect_uv_to_dir([(x as f32 + 0.5 + jx) / width as f32, v]);
                    let c = texel(radiance(vec(d)));
                    [c[0], c[1], c[2], 1.0]
                })
                .collect();
            sink(y, &row);
        };
        run(height, &bake_row);
    }

    /// The cloud march a bake of `width` reads: None when it marches every
    /// texel itself (no volume, or a map no wider than `march_max`).
    fn cloud_cache(&self, width: usize, march_max: usize) -> Option<volumetric::CloudCache> {
        if self.volumetric.is_some() && width.max(2) > march_max {
            Some(volumetric::CloudCache::new(march_max))
        } else {
            None
        }
    }

    /// `bake` with the march's width cap as a parameter, so a test can
    /// exercise the upsampling at a tiny size.
    fn bake_marched(&self, width: usize, march_max: usize) -> EnvMap {
        let cache = self.cloud_cache(width, march_max);
        let at = Sampling::bake(width, cache.as_ref());
        let radiance = |d: Vec3f| self.radiance_with(d, None, at);
        let mut map = bake_with(width, &radiance);
        refine_hot_spots(&mut map, &radiance, &self.hot_spots());
        map
    }

    /// `bake_par` with the march's width cap as a parameter. The cache's rows
    /// are marched by whichever baking thread first needs them, each exactly
    /// once and by the same function, so the result still equals the serial
    /// bake bit for bit.
    fn bake_par_marched(
        &self,
        width: usize,
        march_max: usize,
        run: impl FnOnce(usize, &(dyn Fn(usize) + Sync)),
    ) -> EnvMap {
        let cache = self.cloud_cache(width, march_max);
        let at = Sampling::bake(width, cache.as_ref());
        let radiance = |d: Vec3f| self.radiance_with(d, None, at);
        // Count the rows the runner really baked: a cancelled bake keeps its
        // skipped rows black, so it gets no refinement either.
        let baked = AtomicUsize::new(0);
        let mut map = bake_par_with(width, &radiance, |rows, row| {
            run(rows, &|y: usize| {
                row(y);
                baked.fetch_add(1, Ordering::Relaxed);
            })
        });
        if baked.load(Ordering::Relaxed) >= map.height {
            refine_hot_spots(&mut map, &radiance, &self.hot_spots());
        }
        map
    }

    /// The map without exactly its key's own light (N3), from `map`, this
    /// env's bake ([`Self::bake`] / [`Self::bake_par`] at any width): the key
    /// light skipped, the sun's disc hidden, or the share of the moon's disc
    /// the key carries taken off. The engine's directional light carries that
    /// light, so the lighting it takes from the map (the SH, the specular
    /// atlas, the meter, the fog band) is made from this copy, while the dome
    /// shows `map`. Every other light stays as the map draws it: a wide studio
    /// key's neighbours, the moon beside a sun, the sky's own glow around the
    /// disc. `None` when the map has no key, the key is a base image's sun (a
    /// file's sun: the renderer fills its cone), or `map` is not a whole map.
    ///
    /// Sampled as the map is, so the two differ by the key's light and nothing
    /// else: a bake through the lookup without the key whose texels on and
    /// around the discs (the key's own included) are area-averaged as the
    /// map's are. Only the texels the key can reach are evaluated again (its
    /// covering cone, plus the refinement's reach around a disc), the rows
    /// spread by `run` as for [`Self::bake_par`], then the discs' texels are
    /// refined again; outside the cone the key draws nothing, so the map's
    /// texels are the answer there. A cancelled `run` skips rows, as a
    /// cancelled bake does; the caller drops the copy with the bake. A
    /// volumetric sky reads the same cloud march as the map's bake
    /// ([`Self::bake_keyless_marched`]).
    pub fn bake_keyless_par(&self, map: &EnvMap, run: impl FnOnce(usize, &(dyn Fn(usize) + Sync))) -> Option<EnvMap> {
        self.bake_keyless_marched(map, volumetric::MARCH_MAX_WIDTH, run)
    }

    /// `bake_keyless_par` with the march's width cap as a parameter: the
    /// texels the key can reach are evaluated again through the lookup without
    /// it, reading a cloud cache built as the map's bake built it (the same
    /// rows, marched by the same function), then the discs' texels are refined
    /// as the map's are; outside the key's reach the map's texels are the answer.
    fn bake_keyless_marched(&self, map: &EnvMap, march_max: usize, run: impl FnOnce(usize, &(dyn Fn(usize) + Sync))) -> Option<EnvMap> {
        let (term, key) = self.key()?;
        // A base image's sun is a file's sun: what lies behind it in the
        // photograph is unknown, so there is no map without it to bake; the
        // renderer fills its covering cone with the sky around it (N3's rule
        // for a sun found in a file).
        if term == KeyTerm::Image {
            return None;
        }
        let (w, h) = (map.width, map.height);
        if w < 2 || h < 1 || map.data.len() != w * h {
            return None;
        }
        let without = Some(term);
        let cache = self.cloud_cache(w, march_max);
        let at = Sampling::bake(w, cache.as_ref());
        let radiance = |d: Vec3f| self.radiance_with(d, without, at);
        // The key's reach: its covering cone (a disc's outer limb, a light's
        // reach box) and two texels past it, which holds the refinement's 1.5
        // around a disc.
        let step = std::f32::consts::PI / h as f32;
        let cone = key.cos_cover.min(key.cos_radius).clamp(-1.0, 1.0).acos();
        let reach = (cone + 2.0 * step).min(std::f32::consts::PI);
        let mut keyless = map.clone();
        rebake_cone(&mut keyless, key.dir.normalize(), reach, &radiance, run);
        refine_hot_spots(&mut keyless, &radiance, &self.hot_spots());
        Some(keyless)
    }
}

#[cfg(test)]
impl Env {
    /// A whole bake through the lookup without the key, made the way `bake`
    /// makes the map (every texel, then the discs' texels refined): what
    /// `bake_keyless_par`, which evaluates only the key's reach again, must
    /// equal.
    fn bake_without_key(&self, width: usize) -> Option<EnvMap> {
        let (term, _) = self.key()?;
        if term == KeyTerm::Image {
            return None;
        }
        let cache = self.cloud_cache(width, volumetric::MARCH_MAX_WIDTH);
        let at = Sampling::bake(width, cache.as_ref());
        let radiance = |d: Vec3f| self.radiance_with(d, Some(term), at);
        let mut map = bake_with(width, &radiance);
        refine_hot_spots(&mut map, &radiance, &self.hot_spots());
        Some(map)
    }
}

/// The dome as `Env::dome_light` measures it (zero in Studio mode).
#[derive(Clone, Copy, Default)]
struct Dome {
    /// The dry sky's mean radiance over the upper hemisphere: what lights the
    /// weather's veil.
    mean: Vec3f,
    /// The light it puts on a level surface.
    irradiance: Vec3f,
    /// The light the open sky (its clouds left out) put there.
    open: Vec3f,
}

/// The term of the map that is its key ([`Env::sun`]), for a lookup without
/// it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum KeyTerm {
    /// The studio key light (in either mode).
    Light,
    /// The sun's disc.
    Sun,
    /// The moon's disc, of which the key carries this share (its fade-in
    /// after dusk): that share is the key's light, the rest stays the sky's.
    Moon(f32),
    /// The sun found in an image that is the whole base (1b, B15): a file's
    /// sun, whose covering cone the renderer fills; no keyless bake.
    Image,
}

/// The dome's looks (`Env::dome_light`): DOME_ROWS rows equally spaced in the
/// sine of the elevation (equal solid angles) of DOME_COLUMNS directions each,
/// every other row turned by half a column. The night sums the aurora's light
/// over the same looks (`NightSky::integrate_aurora`, Task 4c), so the ground
/// under a deck gains just what the deck's light gains (plan hdri-7 D9).
pub(super) const DOME_ROWS: usize = 8;
pub(super) const DOME_COLUMNS: usize = 32;

/// Look `i` of the dome's row `j`, and the row's sine of the elevation.
pub(super) fn dome_look(j: usize, i: usize) -> (Vec3f, f32) {
    let sine = (j as f32 + 0.5) / DOME_ROWS as f32;
    let elevation = sine.asin().to_degrees();
    let turn = if j % 2 == 0 { 0.5 } else { 0.0 };
    let azimuth = (i as f32 + turn) * 360.0 / DOME_COLUMNS as f32;
    (dir_from_az_el(azimuth, elevation), sine)
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

/// One baked channel made safe for every consumer: NaN becomes 0, a negative
/// becomes 0 (radiance is never negative) and the rest stays finite. Layer
/// tests that need the raw values call `Env::radiance` directly.
fn clean_radiance(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, MAX_RADIANCE)
    }
}

/// A texel's RGB from a radiance: the array crossing plus the clean-up. Both
/// bakes go through this one function, which is what keeps them bit-identical.
fn texel(radiance: Vec3f) -> [f32; 3] {
    [clean_radiance(radiance.x), clean_radiance(radiance.y), clean_radiance(radiance.z)]
}

/// The serial bake behind `Env::bake`, over any radiance function.
fn bake_with(width: usize, radiance: &dyn Fn(Vec3f) -> Vec3f) -> EnvMap {
    EnvMap::from_fn(width, |d| texel(radiance(vec(d))))
}

/// The parallel bake behind `Env::bake_par`, over any radiance function.
fn bake_par_with(
    width: usize,
    radiance: &(dyn Fn(Vec3f) -> Vec3f + Sync),
    run: impl FnOnce(usize, &(dyn Fn(usize) + Sync)),
) -> EnvMap {
    // from_fn's size rounding, so the two bakes agree on every request.
    let width = width.max(2);
    let height = (width / 2).max(1);
    // Rows a cancelled run never reaches stay black, with the alpha every
    // texel carries.
    let mut data = vec![[0.0, 0.0, 0.0, 1.0]; width * height];
    {
        // One lock per row. Each row is written by one call, so the locks
        // never contend; they only let disjoint rows cross threads safely.
        let rows: Vec<Mutex<&mut [[f32; 4]]>> = data.chunks_exact_mut(width).map(Mutex::new).collect();
        let bake_row = |y: usize| {
            if let Some(Ok(mut row)) = rows.get(y).map(|row| row.lock()) {
                // The same texel-centre arithmetic as EnvMap::from_fn, through
                // the same function, so the bits match.
                let v = (y as f32 + 0.5) / height as f32;
                for (x, out) in row.iter_mut().enumerate() {
                    let d = ibl::equirect_uv_to_dir([(x as f32 + 0.5) / width as f32, v]);
                    let c = texel(radiance(vec(d)));
                    *out = [c[0], c[1], c[2], 1.0];
                }
            }
        };
        run(height, &bake_row);
    }
    EnvMap { width, height, data }
}

/// Evaluates again, through `radiance`, every texel of `map` whose centre lies
/// within `reach` (radians) of `centre`, with the arithmetic of the bakes
/// (texel centres, `texel`'s clean-up), so a texel comes out as a bake through
/// `radiance` would make it. The rows the cone touches are spread by `run` as
/// for [`bake_par_with`]; a row it skips keeps the map's texels.
fn rebake_cone(
    map: &mut EnvMap,
    centre: Vec3f,
    reach: f32,
    radiance: &(dyn Fn(Vec3f) -> Vec3f + Sync),
    run: impl FnOnce(usize, &(dyn Fn(usize) + Sync)),
) {
    let (width, height) = (map.width, map.height);
    let cos_reach = reach.cos();
    let near = envmap::rows_near(centre, reach, height);
    let first = near.start;
    let rows: Vec<Mutex<&mut [[f32; 4]]>> = map.data.chunks_exact_mut(width).skip(first).take(near.len()).map(Mutex::new).collect();
    let bake_row = |i: usize| {
        if let Some(Ok(mut row)) = rows.get(i).map(|row| row.lock()) {
            // bake_par_with's texel-centre arithmetic, through the same
            // functions, so the bits match a bake's.
            let v = ((first + i) as f32 + 0.5) / height as f32;
            for (x, out) in row.iter_mut().enumerate() {
                let d = ibl::equirect_uv_to_dir([(x as f32 + 0.5) / width as f32, v]);
                if vec(d).dot(centre) >= cos_reach {
                    let c = texel(radiance(vec(d)));
                    *out = [c[0], c[1], c[2], 1.0];
                }
            }
        }
    };
    run(rows.len(), &bake_row);
}

/// Re-evaluates the texels on and around each hot spot as the mean of
/// HOT_SAMPLES x HOT_SAMPLES sub-samples spread over the texel. A 0.5 degree
/// sun is smaller than a texel below 1K: at texel centres alone it lands on
/// one texel or none, and the map's sun energy swings from 0 to 2.5 times.
/// Both bakes run this same serial pass, so they still agree bit for bit.
fn refine_hot_spots(map: &mut EnvMap, radiance: &dyn Fn(Vec3f) -> Vec3f, hot: &[(Vec3f, f32)]) {
    let (w, h) = (map.width, map.height);
    if w == 0 || h == 0 || map.data.len() != w * h {
        return;
    }
    for spot in hot_reaches(hot, w, h) {
        for y in spot.rows.clone() {
            for x in 0..w {
                if spot.covers(x, y, w, h) {
                    map.data[y * w + x] = hot_texel(radiance, x, y, w, h);
                }
            }
        }
    }
}

/// A hot spot as the refinement reaches it in a `w` x `h` map (`hot_reaches`): the feature plus a texel's diagonal,
/// every texel it can touch.
struct HotReach {
    centre: Vec3f,
    cos_reach: f32,
    rows: std::ops::Range<usize>,
}

impl HotReach {
    /// The texel (x, y)'s centre lies within the reach.
    fn covers(&self, x: usize, y: usize, w: usize, h: usize) -> bool {
        if !self.rows.contains(&y) {
            return false;
        }
        let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        // (spelled as the refinement always tested it, so a NaN counts as within)
        !(vec(ibl::equirect_uv_to_dir([u, v])).dot(self.centre) < self.cos_reach)
    }
}

/// The hot spots `refine_hot_spots` refines in a `w` x `h` map, with their reach; a feature as wide as
/// HOT_MAX_TEXELS texels or wider is resolved by the grid itself and left out.
fn hot_reaches(hot: &[(Vec3f, f32)], w: usize, h: usize) -> Vec<HotReach> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    // One texel of latitude, in radians.
    let step = std::f32::consts::PI / h as f32;
    let mut reaches = Vec::new();
    for &(centre, radius) in hot {
        // The negated comparison also skips a NaN radius.
        if !centre.is_finite() || !(radius > 0.0) || radius >= HOT_MAX_TEXELS * step {
            continue;
        }
        let centre = centre.normalize();
        // The feature plus a texel's diagonal: every texel it can touch.
        let reach = radius + 1.5 * step;
        let cos_reach = reach.min(std::f32::consts::PI).cos();
        let row = ibl::dir_to_equirect_uv(arr(centre))[1] * h as f32;
        let rows = reach / step;
        let y0 = (row - rows).floor().max(0.0) as usize;
        let y1 = ((row + rows).ceil().max(0.0) as usize).min(h);
        reaches.push(HotReach { centre, cos_reach, rows: y0..y1 });
    }
    reaches
}

/// The texel (x, y) of a `w` x `h` map on a hot spot: the mean of HOT_SAMPLES x HOT_SAMPLES sub-samples spread over
/// it (a sample that is not finite counts as black), through `texel`'s clean-up.
fn hot_texel(radiance: &dyn Fn(Vec3f) -> Vec3f, x: usize, y: usize, w: usize, h: usize) -> [f32; 4] {
    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
    let mut sum = Vec3f::default();
    for j in 0..HOT_SAMPLES {
        let sv = v + ((j as f32 + 0.5) / HOT_SAMPLES as f32 - 0.5) / h as f32;
        for i in 0..HOT_SAMPLES {
            let su = u + ((i as f32 + 0.5) / HOT_SAMPLES as f32 - 0.5) / w as f32;
            let s = radiance(vec(ibl::equirect_uv_to_dir([su, sv])));
            if s.is_finite() {
                sum += s;
            }
        }
    }
    let c = texel(sum * (1.0 / (HOT_SAMPLES * HOT_SAMPLES) as f32));
    [c[0], c[1], c[2], 1.0]
}

/// Test probes (the plan owner's D15): how often this thread did each piece
/// of the slow work a held drag must not repeat, so a test can say a piece
/// ran once, or not at all. Per thread, so tests running side by side do not
/// see each other's work; an `Env` is built and a serial bake runs on the
/// calling thread.
#[cfg(test)]
pub(crate) mod probe {
    use std::cell::Cell;

    /// The pieces counted.
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Work {
        /// A key measured from the map's drawing: `Studio::measured_key`,
        /// `Studio::measured_window_key`, `Env::disc_share`.
        Key,
        /// A gobo picture's summed-area table built (`gobo::shared_pass_table`).
        PassTable,
        /// A light picture's texels walked for its mean (`image_light::PictureMeans`).
        PictureMean,
        /// A worn face's normaliser sampled (`Imperfections::mean_raw`).
        WearNorm,
        /// A light picture's softened-mask table built (`image_light::SoftTable`, P1, P2).
        SoftMask,
        /// A soft shadow caster's coverage rastered (`caster::SoftCoverage::build`, R4).
        CasterSoft,
        /// A caster's bloom raster built (`caster::CasterBloom::build`, R5).
        CasterBloom,
        /// A nebula seed's sprites made (`nebulas::sprites_for`, plan hdri-7): once per seed, never per held tick.
        NebulaSprites,
    }

    thread_local! {
        static COUNTS: [Cell<usize>; 8] = const { [const { Cell::new(0) }; 8] };
    }

    /// One more of `work` on this thread.
    pub(crate) fn count(work: Work) {
        COUNTS.with(|counts| counts[work as usize].set(counts[work as usize].get() + 1));
    }

    /// How many of `work` this thread has done.
    pub(crate) fn counted(work: Work) -> usize {
        COUNTS.with(|counts| counts[work as usize].get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn close3(a: Vec3f, b: Vec3f, eps: f32) -> bool {
        (a - b).length() <= eps
    }

    /// Direction-coded radiance: every channel in 0..1, so the bake's
    /// clean-up leaves it exactly as it is and a texel decodes back to the
    /// direction it was evaluated at.
    fn coded(d: Vec3f) -> Vec3f {
        (d + Vec3f::all(1.0)) * 0.5
    }

    /// The direction a `coded` texel was evaluated at.
    fn decoded(texel: [f32; 4]) -> Vec3f {
        vec3f(texel[0], texel[1], texel[2]) * 2.0 - Vec3f::all(1.0)
    }

    /// A deterministic spread of unit vectors (LCG, rejection-sampled from the unit ball).
    fn pseudo_random_dir(state: &mut u32) -> Vec3f {
        loop {
            let mut next = || {
                *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (*state >> 8) as f32 / 16_777_216.0 * 2.0 - 1.0
            };
            let d = vec3f(next(), next(), next());
            let length = d.length();
            if length > 0.1 && length <= 1.0 {
                return d / length;
            }
        }
    }

    /// Runs every index on four scoped threads, interleaved, as fan_out would.
    fn four_threads(n: usize, f: &(dyn Fn(usize) + Sync)) {
        std::thread::scope(|scope| {
            for t in 0..4 {
                scope.spawn(move || {
                    let mut i = t;
                    while i < n {
                        f(i);
                        i += 4;
                    }
                });
            }
        });
    }

    #[test]
    fn arr_and_vec_cross_the_boundary_losslessly() {
        let v = vec3f(0.25, -3.0, 1.0e-7);
        assert_eq!(arr(v), [0.25, -3.0, 1.0e-7]);
        assert_eq!(vec(arr(v)), v);
        assert_eq!(vec([1.0, 2.0, 3.0]), vec3f(1.0, 2.0, 3.0));
        // ibl's helpers take and return arrays; a round trip through them is
        // the whole boundary this module crosses.
        let d = vec(ibl::equirect_uv_to_dir(ibl::dir_to_equirect_uv(arr(vec3f(0.6, 0.0, -0.8)))));
        assert!(close3(d, vec3f(0.6, 0.0, -0.8), 1e-5), "{d:?}");
    }

    #[test]
    fn azimuth_and_elevation_follow_the_compass() {
        assert!(close3(dir_from_az_el(0.0, 0.0), vec3f(0.0, 0.0, -1.0), 1e-6), "azimuth 0 is north (-Z)");
        assert!(close3(dir_from_az_el(90.0, 0.0), vec3f(1.0, 0.0, 0.0), 1e-6), "azimuth 90 is east (+X)");
        assert!(close3(dir_from_az_el(180.0, 0.0), vec3f(0.0, 0.0, 1.0), 1e-6), "azimuth 180 is south (+Z)");
        assert!(close3(dir_from_az_el(270.0, 0.0), vec3f(-1.0, 0.0, 0.0), 1e-6), "azimuth 270 is west (-X)");
        assert!(dir_from_az_el(123.0, 90.0).y > 0.999_999);
        for az in (0..24).map(|i| i as f32 * 15.0) {
            for el in [-80.0f32, -45.0, -10.0, 0.0, 10.0, 45.0, 80.0] {
                let (a, e) = az_el_from_dir(dir_from_az_el(az, el));
                let da = ((a - az + 540.0).rem_euclid(360.0) - 180.0).abs();
                assert!(da < 1e-3 && close(e, el, 1e-3), "({az}, {el}) came back as ({a}, {e})");
                assert!((0.0..360.0).contains(&a));
            }
            // On the horizon, azimuth maps linearly onto ibl's u, from north
            // at the centre: u = 0.5 + az / 360 (mod 1). This is the one
            // place the compass meets the equirect convention.
            let u = ibl::dir_to_equirect_uv(arr(dir_from_az_el(az, 0.0)))[0];
            let want = (0.5 + az / 360.0).rem_euclid(1.0);
            let du = (u - want).abs().min(1.0 - (u - want).abs());
            assert!(du < 1e-5, "az {az}: u {u}, want {want}");
        }
        // The anchors of ibl's convention, through this module's helpers.
        assert!(close(ibl::dir_to_equirect_uv(arr(dir_from_az_el(0.0, 0.0)))[0], 0.5, 1e-6), "north at the centre");
        assert!(close(ibl::dir_to_equirect_uv(arr(dir_from_az_el(90.0, 0.0)))[0], 0.75, 1e-6), "+X at u = 0.75");
        assert!(close(ibl::dir_to_equirect_uv(arr(dir_from_az_el(270.0, 0.0)))[0], 0.25, 1e-6), "-X at u = 0.25");
        assert!(close(ibl::dir_to_equirect_uv(arr(dir_from_az_el(45.0, 60.0)))[1], 30.0 / 180.0, 1e-6), "v is the angle from the zenith");
        assert_eq!(az_el_from_dir(vec3f(0.0, 1.0, 0.0)), (0.0, 90.0));
        assert_eq!(az_el_from_dir(vec3f(0.0, -2.0, 0.0)), (0.0, -90.0));
        assert_eq!(az_el_from_dir(Vec3f::default()), (0.0, 0.0));
    }

    #[test]
    fn rotate_y_turns_counter_clockwise_seen_from_above() {
        let north = vec3f(0.0, 0.0, -1.0);
        let west = vec3f(-1.0, 0.0, 0.0);
        // ibl's sign (EnvMap::procedural, mat_ibl_dir): +90 takes -Z to -X.
        assert!(close3(rotate_y(north, 90.0), west, 1e-6), "north + 90 = west: {:?}", rotate_y(north, 90.0));
        assert!(close3(rotate_y(west, 90.0), vec3f(0.0, 0.0, 1.0), 1e-6), "west + 90 = south");
        assert!(close3(rotate_y(north, -90.0), vec3f(1.0, 0.0, 0.0), 1e-6), "north - 90 = east");
        assert_eq!(rotate_y(north, 0.0), north, "no turn is exact");
        let mut state = 7u32;
        for _ in 0..50 {
            let d = pseudo_random_dir(&mut state);
            let r = rotate_y(d, 37.0);
            assert!(close(r.y, d.y, 1e-6) && close(r.length(), 1.0, 1e-5), "elevation and length are kept");
            assert!(close3(rotate_y(r, -37.0), d, 1e-5), "the opposite turn undoes it");
            if d.y.abs() < 0.99 {
                // Counter-clockwise from above is a falling compass azimuth.
                let (a0, e0) = az_el_from_dir(d);
                let (a1, e1) = az_el_from_dir(r);
                let da = ((a1 - a0 + 37.0 + 540.0).rem_euclid(360.0) - 180.0).abs();
                assert!(da < 1e-2 && close(e0, e1, 1e-3), "{a0} - 37 came out as {a1}");
            }
        }
        // The same turn EnvMap::procedural applies: the preset is evaluated at
        // rotate_y(d, -rotation), so a feature at -Z lands at -X for +90.
        let sunset = EnvMap::procedural(&ibl::EnvPreset::Sunset, 64, 1.0, 90.0);
        let brightest = vec(sunset.brightest_direction());
        assert!(brightest.x < -0.9, "the sunset's sun moved from -Z to -X: {brightest:?}");
    }

    #[test]
    fn env_keeps_a_clamped_copy_and_bakes_a_clean_map() {
        let mut params = HdriParams { intensity_ev: 50.0, ..Default::default() };
        params.sky.clouds.coverage = f32::NAN;
        let env = Env::new(&params);
        assert_eq!(env.params().intensity_ev, 10.0);
        assert_eq!(env.params().sky.clouds.coverage, CloudParams::default().coverage);
        assert_eq!(params.intensity_ev, 50.0, "the caller's params are untouched");
        let map = env.bake(32);
        assert_eq!((map.width, map.height, map.data.len()), (32, 16, 32 * 16));
        assert!(map.data.iter().all(|t| t[3] == 1.0), "alpha is 1 everywhere");
        assert!(map.data.iter().all(|t| t[..3].iter().all(|v| v.is_finite() && *v >= 0.0)));
        // from_fn's size rounding is kept, so tiny requests still make a map.
        let tiny = env.bake(0);
        assert_eq!((tiny.width, tiny.height), (2, 1));
    }

    #[test]
    fn a_bake_cleans_every_channel() {
        // The one layer-free test of the clean-up: every other bake here is
        // already inside 0..1, so only this one fails if clean_radiance or
        // texel stops mapping NaN to 0, a negative to 0 and +inf to
        // MAX_RADIANCE (each channel on its own branch).
        let dirty = |_: Vec3f| vec3f(f32::NAN, -1.0, f32::INFINITY);
        let want = [0.0, 0.0, MAX_RADIANCE, 1.0];
        let serial = bake_with(16, &dirty);
        assert_eq!((serial.width, serial.height), (16, 8));
        assert!(serial.data.iter().all(|t| *t == want), "serial bake: {:?}", serial.data[0]);
        let parallel = bake_par_with(16, &dirty, four_threads);
        assert!(parallel.data.iter().all(|t| *t == want), "parallel bake: {:?}", parallel.data[0]);
        // And the minus-infinity and large-negative ends of the same branches.
        let low = |_: Vec3f| vec3f(f32::NEG_INFINITY, f32::MIN, f32::MAX);
        assert!(bake_with(8, &low).data.iter().all(|t| *t == [0.0, 0.0, MAX_RADIANCE, 1.0]));
    }

    #[test]
    fn env_can_be_shared_across_threads() {
        // bake_par hands &Env to worker threads; later layers must keep it Send + Sync.
        fn shareable<T: Send + Sync>() {}
        shareable::<Env>();
        shareable::<EnvSun>();
    }

    /// A caster's pattern only darkens (decision 8). Its light fields may
    /// still say otherwise: an add blend, a key, or a gobo asking to fill
    /// with sky and to be cast onto the backdrop. It still opens no window
    /// onto a second sky and throws no shadow onto the studio backdrop. The
    /// same entry as a light does both.
    #[test]
    fn a_caster_never_casts_on_the_backdrop_nor_opens_a_sky_window() {
        let entry = LightParams {
            blend: "add".to_string(),
            key: true,
            intensity_ev: 3.0,
            gobo: Some(GoboParams { fill_sky: true, project_on_backdrop: true, ..GoboParams::default() }),
            ..LightParams::new_caster()
        };
        let studio = |light: LightParams| HdriParams { mode: "studio".to_string(), lights: vec![light], ..HdriParams::default() };
        let as_light = Env::new(&studio(LightParams { kind: None, caster: None, ..entry.clone() }));
        assert!(as_light.window_sky.is_some() && as_light.backdrop_gobo.is_some(), "as a light it opens a window and casts");
        let as_caster = Env::new(&studio(entry.clone()));
        assert!(as_caster.window_sky.is_none(), "a caster opens no window onto the sky");
        assert!(as_caster.backdrop_gobo.is_none(), "a caster throws no shadow onto the backdrop");
        assert!(gobo::BackdropGobo::new(&[entry], &ImageSet::default()).is_none(), "nor when handed lights clamp() has not held");
    }

    /// D15 (M5): the key is measured only when the engine asks for it. The
    /// app's preview bakes never call `sun`, and a held drag of a gridded key
    /// paid 50 ms and more per bake for a key nobody read (the review: 85 to
    /// 90 ms held bakes of a key with fern, grid, doors and wear, against 9 to
    /// 10 ms with the key off). Each kind of key that takes a measurement: a
    /// studio key with everything hanging in front of it, a window filled
    /// with sky as the key, and a sun with a gel in front of its disc. A bake
    /// of either kind measures nothing; the first `sun` measures the key, and
    /// later calls and the keyless copy read that same key.
    #[test]
    fn the_key_is_measured_only_when_the_engine_asks_for_it() {
        use probe::{counted, Work};
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f);
        let studio = |light: LightParams| HdriParams { mode: "studio".to_string(), lights: vec![light], ..HdriParams::default() };
        let gridded = LightParams {
            key: true,
            azimuth_deg: 30.0,
            elevation_deg: 35.0,
            width_deg: 40.0,
            height_deg: 30.0,
            aim: Some(AimParams { yaw_deg: 20.0, pitch_deg: 0.0, grid: 0.6, barn_doors: [30.0, 0.0, 20.0, 0.0] }),
            gobo: Some(GoboParams { kind: "fern".to_string(), distance: 0.3, ..GoboParams::default() }),
            imperfections: Some(ImperfectionParams { wrinkle: 0.5, dust: 0.5, weave: 0.5, seed: 7 }),
            ..LightParams::default()
        };
        let window = LightParams {
            key: true,
            azimuth_deg: 0.0,
            elevation_deg: 20.0,
            width_deg: 40.0,
            height_deg: 30.0,
            gobo: Some(GoboParams { fill_sky: true, ..GoboParams::default() }),
            ..LightParams::default()
        };
        let mut gelled_sun = sun_at(30.0);
        gelled_sun.lights.push(LightParams {
            azimuth_deg: 200.0,
            elevation_deg: 30.0,
            width_deg: 10.0,
            height_deg: 10.0,
            blend: "multiply".to_string(),
            rgb: Some([0.3, 0.5, 1.0]),
            ..LightParams::default()
        });
        for (name, params) in [("a gridded key", studio(gridded)), ("a window as the key", studio(window)), ("a gelled sun", gelled_sun)] {
            let before = counted(Work::Key);
            let env = Env::new(&params);
            let map = env.bake(32);
            assert_eq!(env.bake_par(32, serial), map);
            assert_eq!(counted(Work::Key), before, "{name}: a bake that never asks for the key measured it");
            let key = env.sun().expect("a key");
            let measured = counted(Work::Key);
            assert!(measured > before, "{name}: the first sun() measures the key");
            assert_eq!(env.sun(), Some(key), "{name}");
            assert!(env.bake_keyless_par(&map, serial).is_some(), "{name}");
            assert_eq!(counted(Work::Key), measured, "{name}: the key is measured once");
        }
    }

    /// D15 (L:229): a map whose build a newer request cancels stops in the
    /// first build of its seed's cloud noise and is None, in Sky mode and
    /// behind a studio's window filled with sky alike (its sky is a nested
    /// map of the same seed); one that is not cancelled is `new_with`'s, to
    /// the bit. A map with nothing slow to stop is built whatever the poll
    /// says. (Seeds no other test uses: the kept noise is shared.)
    #[test]
    fn a_cancelled_map_stops_in_its_first_cloud_noise() {
        let images = ImageSet::default();
        let volumetric = |seed: u32| {
            let mut p = HdriParams { seed, ..HdriParams::default() };
            p.sky.clouds.coverage = 0.6;
            p.sky.volumetric = Some(VolumetricParams { steps: 16, ..VolumetricParams::default() });
            p
        };
        let sky = volumetric(0x4837_0230);
        assert!(Env::new_cancellable(&sky, &images, &|| true).is_none(), "cancelled in the noise");
        let built = Env::new_cancellable(&sky, &images, &|| false).expect("not cancelled");
        assert_eq!(built.bake(16), Env::new(&sky).bake(16));
        let mut studio = volumetric(0x4837_0231);
        studio.mode = "studio".to_string();
        studio.lights.push(LightParams { gobo: Some(GoboParams { fill_sky: true, ..GoboParams::default() }), ..LightParams::default() });
        assert!(Env::new_cancellable(&studio, &images, &|| true).is_none(), "cancelled in the window's sky");
        assert!(Env::new_cancellable(&HdriParams::default(), &images, &|| true).is_some(), "nothing slow to stop");
    }

    #[test]
    fn bake_evaluates_texel_centres_with_row_zero_at_the_zenith() {
        let map = bake_with(64, &coded);
        let (w, h) = (map.width, map.height);
        assert_eq!((w, h), (64, 32));
        for y in 0..h {
            for x in 0..w {
                // The very arithmetic EnvMap::from_fn uses, so this is exact.
                let d = vec(ibl::equirect_uv_to_dir([(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32]));
                let want = coded(d);
                assert_eq!(map.data[y * w + x], [want.x, want.y, want.z, 1.0], "texel ({x}, {y})");
            }
        }
        let at = |x: usize, y: usize| decoded(map.data[y * w + x]);
        assert!(at(20, 0).y > 0.99, "row 0 looks up");
        assert!(at(20, h - 1).y < -0.99, "the last row looks down");
        assert!(at(w / 2, h / 2).z < -0.99, "-Z (north) at the centre column");
        assert!(at(3 * w / 4, h / 2).x > 0.99, "+X at three quarters of the width");
        assert!(at(w / 4, h / 2).x < -0.99, "-X at a quarter of the width");
        assert!(at(0, h / 2).z > 0.99, "+Z at the seam");
    }

    #[test]
    fn the_centre_column_looks_north() {
        // The task's pin: bake(64) puts azimuth 0 (north) in the centre column.
        let uv = ibl::dir_to_equirect_uv(arr(dir_from_az_el(0.0, 0.0)));
        assert!(close(uv[0], 0.5, 1e-6) && close(uv[1], 0.5, 1e-6), "{uv:?}");
        let map = Env::new(&HdriParams::default()).bake(64);
        assert_eq!((map.width, map.height), (64, 32));
        let (x, y) = ((uv[0] * map.width as f32).floor() as usize, (uv[1] * map.height as f32).floor() as usize);
        assert_eq!((x, y), (32, 16), "the centre column of a 64-wide map");
        // With a direction-coded map the texel there faces north, and a
        // bilinear lookup at exactly u = 0.5 splits the two centre columns
        // symmetrically about -Z.
        let coded_map = bake_with(64, &coded);
        let texel = decoded(coded_map.data[y * 64 + x]);
        assert!(texel.z < -0.99 && texel.x.abs() < 0.06 && texel.y.abs() < 0.06, "{texel:?}");
        let north = decoded(coded_map.sample(arr(dir_from_az_el(0.0, 0.0))));
        assert!(close3(north, vec3f(0.0, 0.0, -1.0), 0.01), "{north:?}");
        let east = decoded(coded_map.sample(arr(dir_from_az_el(90.0, 0.0))));
        assert!(close3(east, vec3f(1.0, 0.0, 0.0), 0.01), "{east:?}");
        assert_eq!((uv[0] * 64.0).floor() as usize, 32);
        assert_eq!((ibl::dir_to_equirect_uv(arr(dir_from_az_el(90.0, 0.0)))[0] * 64.0).floor() as usize, 48);
    }

    #[test]
    fn parallel_bake_matches_the_serial_bake() {
        assert_eq!(bake_par_with(48, &coded, four_threads), bake_with(48, &coded));
        let env = Env::new(&HdriParams::default());
        assert_eq!(env.bake_par(32, four_threads), env.bake(32));
        assert_eq!(env.bake_par(0, four_threads), env.bake(0), "the same size rounding");
        // A serial runner (what the UI thread does) gives the same bits too.
        assert_eq!(bake_par_with(16, &coded, |n, f| (0..n).for_each(f)), bake_with(16, &coded));
    }

    /// The rows `Env::bake_rows_jittered_par` hands over at `jitter`, gathered into one map's texels.
    fn jittered_rows(env: &Env, width: usize, jitter: [f32; 2]) -> Vec<[f32; 4]> {
        let (w, h) = (width.max(2), (width.max(2) / 2).max(1));
        let texels = Mutex::new(vec![[-1.0f32; 4]; w * h]);
        env.bake_rows_jittered_par(width, jitter, four_threads, &|y, row| {
            assert_eq!(row.len(), w);
            texels.lock().unwrap()[y * w..(y + 1) * w].copy_from_slice(row);
        });
        texels.into_inner().unwrap()
    }

    /// Task 10 (the preview's antialiasing): without a jitter the row bake hands over the bake's rows bit for bit,
    /// the sun's disc refined as the bake refines it; a jitter moves the samples off the texel centres, and the texels
    /// on the disc stay their area average.
    #[test]
    fn an_unjittered_row_bake_is_the_bake() {
        let mut p = HdriParams { mode: "sky".to_string(), ..HdriParams::default() };
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.elevation_deg = 30.0;
        p.lights.push(LightParams::default());
        let env = Env::new(&p);
        for width in [64, 34] {
            let map = env.bake_par(width, four_threads);
            assert!(jittered_rows(&env, width, [0.0, 0.0]) == map.data, "width {width}: the unjittered rows are the bake's");
            let hot = hot_reaches(&env.hot_spots(), map.width, map.height);
            assert!(!hot.is_empty(), "width {width}: the sun's disc is refined");
            let jittered = jittered_rows(&env, width, [0.375, -0.125]);
            assert!(jittered != map.data, "width {width}: the jitter moved nothing");
            let (w, h) = (map.width, map.height);
            let mut on_disc = 0;
            for y in 0..h {
                for x in 0..w {
                    if hot.iter().any(|spot| spot.covers(x, y, w, h)) {
                        on_disc += 1;
                        assert_eq!(jittered[y * w + x], map.data[y * w + x], "width {width}: the disc's texel ({x}, {y})");
                    }
                }
            }
            assert!(on_disc > 0);
        }
        // A runner that skips rows hands over only the rows it ran.
        let handed = Mutex::new(Vec::new());
        env.bake_rows_jittered_par(16, [0.1, 0.1], |_, f| (0..8).filter(|y| y % 3 == 0).for_each(f), &|y, _| handed.lock().unwrap().push(y));
        assert_eq!(handed.into_inner().unwrap(), vec![0, 3, 6]);
    }

    #[test]
    fn rows_a_cancelled_run_skipped_stay_black() {
        let full = bake_par_with(16, &coded, |n, f| (0..n).for_each(f));
        let half = bake_par_with(16, &coded, |n, f| (0..n).filter(|i| i % 2 == 0).for_each(f));
        let w = full.width;
        for y in 0..full.height {
            let row = y * w..(y + 1) * w;
            if y % 2 == 0 {
                assert_eq!(half.data[row.clone()], full.data[row]);
            } else {
                assert!(half.data[row].iter().all(|t| *t == [0.0, 0.0, 0.0, 1.0]), "row {y} was skipped");
            }
        }
        // A runner that calls out of range is ignored, not a panic.
        let wild = bake_par_with(8, &coded, |n, f| (0..n + 5).for_each(f));
        assert_eq!(wild, bake_with(8, &coded));
    }

    #[test]
    fn hot_spots_keep_the_energy_of_a_disc_smaller_than_a_texel() {
        // A 4 degree disc of radiance 1 in a 64 x 32 map (5.6 degree texels):
        // sampled at texel centres alone it holds 0 to 2 times its energy.
        let radius = 2.0f32.to_radians();
        for az in [100.0f32, 101.4, 102.8, 104.2] {
            let centre = dir_from_az_el(az, 35.0);
            let disc = move |d: Vec3f| if d.dot(centre) >= radius.cos() { Vec3f::all(1.0) } else { Vec3f::default() };
            let mut map = bake_with(64, &disc);
            refine_hot_spots(&mut map, &disc, &[(centre, radius)]);
            let energy: f32 = map.data.iter().enumerate().map(|(i, t)| {
                let theta = ((i / map.width) as f32 + 0.5) / map.height as f32 * std::f32::consts::PI;
                t[0] * (std::f32::consts::TAU / map.width as f32) * (std::f32::consts::PI / map.height as f32) * theta.sin()
            }).sum();
            let want = std::f32::consts::TAU * (1.0 - radius.cos());
            assert!((energy / want - 1.0).abs() < 0.1, "az {az}: {energy} vs {want}");
        }
        // No hot spot, or a feature the grid resolves, leaves the map as it is.
        let plain = bake_with(64, &coded);
        let mut same = plain.clone();
        refine_hot_spots(&mut same, &coded, &[]);
        refine_hot_spots(&mut same, &coded, &[(vec3f(0.0, 1.0, 0.0), 1.0)]);
        assert_eq!(same, plain);
    }

    /// f(0..n) on every core, the results in index order: the sweeps below build
    /// an Env (and an atmosphere) per sample.
    fn sweep<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
        let next = AtomicUsize::new(0);
        let found = Mutex::new(Vec::with_capacity(n));
        let cores = std::thread::available_parallelism().map_or(4, |c| c.get()).min(16);
        std::thread::scope(|scope| {
            for _ in 0..cores {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let v = f(i);
                    found.lock().unwrap().push((i, v));
                });
            }
        });
        let mut found = found.into_inner().unwrap();
        found.sort_by_key(|(i, _)| *i);
        found.into_iter().map(|(_, v)| v).collect()
    }

    fn lum(v: Vec3f) -> f32 {
        crate::sky::luminance(v)
    }

    /// The default clear sky with the sun held at `elevation_deg` (manual mode).
    fn sun_at(elevation_deg: f32) -> HdriParams {
        let mut p = HdriParams::default();
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.elevation_deg = elevation_deg;
        p.sky.sun.azimuth_deg = 200.0;
        p
    }

    /// The sun of `sun_at(30.0)` (azimuth 200, 30 degrees up) 2 degrees under the bottom edge
    /// of a 40 x 30 degree leaves caster: its centre is 17 degrees above the sun on the sun's
    /// meridian, so its edge runs 15 degrees down from the centre, at 32. The sun shows in
    /// full beside it whatever the pattern, and its bloom, `strength` over `length_deg` above
    /// `threshold_ev`, eats into the leaves over it.
    fn sun_under_leaves(strength: f32, length_deg: f32, threshold_ev: f32) -> HdriParams {
        let mut p = sun_at(30.0);
        p.lights.push(LightParams {
            azimuth_deg: 200.0,
            elevation_deg: 47.0,
            width_deg: 40.0,
            height_deg: 30.0,
            gobo: Some(GoboParams { kind: "leaves".to_string(), density: 0.6, distance: 0.0, seed: 3, ..GoboParams::default() }),
            caster: Some(CasterParams { bloom_strength: strength, bloom_length_deg: length_deg, bloom_threshold_ev: threshold_ev, ..CasterParams::default() }),
            ..LightParams::new_caster()
        });
        p
    }

    /// The light the map draws over the leaves the sun's bloom reaches: luminance summed over
    /// a grid from 1 degree under the caster's edge to 11 degrees into it, 20 degrees either
    /// side of the sun.
    fn light_through_leaves(env: &Env) -> f64 {
        let mut sum = 0.0f64;
        for i in 0..81 {
            for j in 0..49 {
                sum += lum(env.radiance(dir_from_az_el(180.0 + 0.5 * i as f32, 31.0 + 0.25 * j as f32))) as f64;
            }
        }
        sum
    }

    /// What the sun's key would be with nothing fading it (a clear sky): the
    /// atmosphere's cone radiance, and the disc's outer limb in radians.
    fn unfaded_sun(p: &HdriParams) -> (Vec3f, f32) {
        let atmo = atmosphere::Atmosphere::new(atmosphere::sun_direction(&p.sky.sun), &p.sky.atmosphere, &p.sky.sun_disc);
        (atmo.sun_cone_radiance(), atmo.sun_outer_radius())
    }

    /// The moon's key with nothing fading it: Some only once the sun is under -6 degrees.
    fn unfaded_moon(p: &HdriParams) -> Option<EnvSun> {
        let sun = atmosphere::sun_direction(&p.sky.sun);
        night::NightSky::new(&p.sky.night, p.seed, sun, night::star_hours(&p.sky.sun), p.sky.sun.latitude).moon_key()
    }

    #[test]
    fn the_suns_key_fades_out_with_the_visible_part_of_its_disc() {
        // The default disc is 0.53 degrees across with a 0.2 soft limb: its outer limb
        // is 0.29 degrees from its centre, so the key is full 0.29 degrees up, half
        // on the horizon and gone 0.29 degrees down, along a smoothstep.
        let reference = lum(unfaded_sun(&sun_at(0.0)).0);
        let samples = sweep(91, |i| {
            let e = 0.45 - 0.01 * i as f32;
            let p = sun_at(e);
            let (full, outer) = unfaded_sun(&p);
            (e, outer.to_degrees(), lum(full), Env::new(&p).sun())
        });
        let mut last: Option<(f32, f32)> = None;
        let mut half = None;
        for &(e, outer_deg, full, key) in &samples {
            let want = smoothstep(-outer_deg, outer_deg, e);
            match key {
                None => assert!(want <= 1.0e-3 && e < 0.0, "no key at {e} deg, where {want} of the disc shows"),
                Some(key) => {
                    assert!(key.validate().is_ok(), "{e} deg: {key:?}");
                    let got = lum(key.radiance);
                    assert!((got - full * want).abs() <= 2.0e-3 * full + 1.0e-3 * reference, "{e} deg: radiance {got} vs {full} x {want}");
                    // A fade, not a switch: no step over 0.01 degrees of elevation
                    // reaches a tenth of the key on the horizon, and the key only
                    // falls as the sun sinks.
                    if let Some((prev_e, prev)) = last {
                        assert!((got - prev).abs() < 0.1 * reference, "{prev_e} -> {e} deg: {prev} -> {got}");
                        assert!(got <= prev * 1.0001, "{prev_e} -> {e} deg: the key rose from {prev} to {got}");
                    }
                    last = Some((e, got));
                    if e.abs() < 0.005 {
                        half = Some(got / full);
                    }
                }
            }
        }
        // Full above the limb, nothing below it, half on the horizon.
        let top = &samples[0];
        assert!((lum(top.3.expect("the sun is up").radiance) - top.2).abs() < 1.0e-3 * top.2, "full at {} deg", top.0);
        assert!(samples.last().unwrap().3.is_none(), "gone at {} deg", samples.last().unwrap().0);
        assert!((half.expect("a sample on the horizon") - 0.5).abs() < 0.01, "{half:?}");
    }

    #[test]
    fn the_moon_key_fades_in_as_the_sun_sinks_from_6_to_8_degrees() {
        // The sun is under -6 degrees and the moon is up; the moon's key waits for
        // -6 and takes -6 to -8 to arrive, so the key goes from nothing to the moon
        // by degrees instead of switching on.
        let reference = lum(unfaded_moon(&sun_at(-8.0)).expect("night, moon up").radiance);
        let samples = sweep(111, |i| {
            let e = -5.81 - 0.02 * i as f32;
            let p = sun_at(e);
            (e, unfaded_moon(&p), Env::new(&p).sun())
        });
        let mut last: Option<(f32, f32)> = None;
        for &(e, moon, key) in &samples {
            let want = smoothstep(6.0, 8.0, -e);
            if e > -6.0 {
                assert!(key.is_none() && moon.is_none(), "{e} deg: the twilight sky still outshines the moon, {key:?}");
                continue;
            }
            let moon = moon.expect("the moon is a key under -6 degrees");
            match key {
                None => assert!(want <= 1.0e-3, "no key at {e} deg, where the moon has {want} of its way in"),
                Some(key) => {
                    assert!(key.validate().is_ok(), "{e} deg: {key:?}");
                    assert!(key.dir.dot(moon.dir) > 0.99999, "{e} deg: the key points at the moon");
                    let got = lum(key.radiance);
                    let full = lum(moon.radiance);
                    assert!((got - full * want).abs() <= 2.0e-3 * full, "{e} deg: radiance {got} vs {full} x {want}");
                    if let Some((prev_e, prev)) = last {
                        assert!((got - prev).abs() < 0.1 * reference, "{prev_e} -> {e} deg: {prev} -> {got}");
                        assert!(got >= prev * 0.9999, "{prev_e} -> {e} deg: the key fell from {prev} to {got}");
                    }
                    last = Some((e, got));
                }
            }
        }
        // Gone at -6, the moon itself from -8 down.
        assert!(samples.iter().filter(|s| s.0 > -6.0).all(|s| s.2.is_none()));
        let (e, moon, key) = samples.last().unwrap();
        assert!((lum(key.expect("full moon").radiance) - lum(moon.unwrap().radiance)).abs() < 1.0e-3 * reference, "{e} deg");
    }

    #[test]
    fn the_key_has_no_jump_over_an_evening() {
        // 21 June at 45 N, every two minutes from 16:00 to 23:00: the sun sets about
        // 19:45, the moon (up, 30 degrees) takes over from 20:28 and has all of its
        // light by 20:44. Two minutes is as long as the sun takes to cross its own
        // limb, so a sample can step from nearly full to nearly nothing there: what
        // the sweep pins is that every sample is the sun's unfaded key times its
        // visible part, or the moon's times its fade, and that the key goes through
        // the middle of its fade at a sample, where a switch would not.
        let hours = |i: usize| 16.0 + i as f32 / 30.0;
        let samples = sweep(211, |i| {
            let mut p = HdriParams::default();
            p.sky.sun.hour = hours(i);
            let (full, outer) = unfaded_sun(&p);
            let sun = atmosphere::sun_direction(&p.sky.sun);
            let e = sun.y.asin();
            (p.sky.sun.hour, e.to_degrees(), lum(full), smoothstep(-outer, outer, e), unfaded_moon(&p), Env::new(&p).sun())
        });
        let peak = samples.iter().map(|s| lum(s.5.map_or(Vec3f::default(), |k| k.radiance))).fold(0.0, f32::max);
        let mut state = 0; // 0 sun, 1 dark, 2 moon
        let mut mid_fade = false;
        let mut last: Option<f32> = None;
        for &(hour, e, full, weight, moon, key) in &samples {
            let got = key.map_or(0.0, |k| lum(k.radiance));
            assert!(got.is_finite(), "{hour} h");
            if weight > 0.0 {
                assert_eq!(state, 0, "{hour} h: the sun is back after dusk");
                assert!(key.is_some(), "{hour} h: the sun shows");
                assert!((got - full * weight).abs() <= 2.0e-3 * full, "{hour} h ({e} deg): {got} vs {full} x {weight}");
                mid_fade |= weight > 0.05 && weight < 0.95;
            } else if let Some(moon) = moon.filter(|_| e < -6.0) {
                let fade = smoothstep(6.0, 8.0, -e);
                state = 2;
                assert_eq!(key.is_some(), fade > 0.0, "{hour} h ({e} deg)");
                assert!((got - lum(moon.radiance) * fade).abs() <= 2.0e-3 * lum(moon.radiance), "{hour} h ({e} deg): {got} vs the moon's {} x {fade}", lum(moon.radiance));
                mid_fade |= fade > 0.05 && fade < 0.95;
            } else {
                state = state.max(1);
                assert!(key.is_none(), "{hour} h ({e} deg): a key in the dark");
            }
            // No gross jump from one sample to the next, at the coarse step.
            if let Some(prev) = last {
                assert!((got - prev).abs() < 0.05 * peak, "{hour} h: {prev} -> {got}");
            }
            last = Some(got);
        }
        assert_eq!(state, 2, "the evening ends in moonlight");
        assert!(mid_fade, "a sample falls inside a fade");
        assert!(samples.iter().filter(|s| s.5.is_none()).count() > 10, "the key is out for a while between sunset and the moon");
    }

    /// M6: the sun's key carries the whole irradiance the atmosphere lets
    /// through, through the f32 cone it stores: `EnvSun::irradiance` (and the
    /// renderer's light, the same product) gives it back to the float's
    /// rounding at the 0.1 degree minimum as at the 0.53 degree default. A
    /// cancellation-free cone in the producer lit the 0.1 degree sun 6 % too
    /// dark, because every consumer takes 1 - cos from the f32 cosine.
    #[test]
    fn the_suns_key_carries_its_irradiance_through_its_f32_cone() {
        for size_deg in [0.1f32, 0.53] {
            let mut p = sun_at(60.0);
            p.sky.sun_disc.size_deg = size_deg;
            p.sky.clouds.coverage = 0.0;
            p.sky.clouds.cirrus = 0.0;
            let env = Env::new(&p);
            let key = env.sun().expect("the sun is up");
            let want = env.atmo.as_ref().expect("sky mode").sun_irradiance();
            let got = key.irradiance();
            for (g, w) in [(got.x, want.x), (got.y, want.y), (got.z, want.z)] {
                assert!((g / w - 1.0).abs() < 1.0e-6, "{size_deg} deg: {g} for {w}");
            }
        }
    }

    /// N3: while the moon fades in after dusk its key carries `arrived` of the
    /// disc's light, so the map without its key keeps the rest of the disc:
    /// the lookup takes exactly that share off the disc and nothing beside
    /// it, and the keyless bake, which evaluates only the disc's reach again,
    /// is a whole bake through that lookup.
    #[test]
    fn a_moon_on_its_way_in_leaves_the_share_its_key_carries() {
        let env = Env::new(&sun_at(-7.0));
        let (term, key) = env.key().expect("the moon is arriving");
        let KeyTerm::Moon(share) = term else { panic!("the moon is the key: {term:?}") };
        assert!((share - smoothstep(6.0, 8.0, 7.0)).abs() < 1.0e-3, "half way in: {share}");
        let d = key.dir.normalize();
        let all = env.radiance(d);
        let without = env.radiance_with(d, Some(KeyTerm::Moon(1.0)), Sampling::default());
        let kept = env.radiance_with(d, Some(term), Sampling::default());
        let disc = all - without;
        assert!(disc.y > without.y, "premise: the disc outshines the twilight behind it: {all:?} {without:?}");
        assert!(((all - kept) - disc * share).length() <= 1.0e-4 * disc.length(), "{all:?} - {kept:?} vs {share} x {disc:?}");
        let beside = rotate_y(d, 5.0);
        assert_eq!(env.radiance_with(beside, Some(term), Sampling::default()), env.radiance(beside), "nothing beside the disc changes");
        // The keyless bake against a whole bake through the same lookup.
        let map = env.bake(64);
        assert_eq!(env.bake_keyless_par(&map, |n, f| (0..n).for_each(f)), env.bake_without_key(64));
    }

    /// N1: the environment's own sun for the daylight switches is the sky's
    /// sun wherever it is: below the horizon under the moonlit preset, whose
    /// key is the moon; a studio has none.
    #[test]
    fn a_sky_reports_its_sun_below_the_horizon_and_a_studio_none() {
        let moonlit = Env::new(&presets::preset("Moonlit night").unwrap());
        let key = moonlit.sun().expect("the risen moon is the key");
        let daylight = moonlit.sun_dir().expect("a sky has its sun");
        assert!(key.dir.y > 0.4, "the moon is 30 degrees up: {key:?}");
        assert!(daylight.y < -0.4, "the sun is 30 degrees down: {daylight:?}");
        assert!(Env::new(&presets::preset("Three-point").unwrap()).sun_dir().is_none());
    }

    /// The engine's key light and the generator's are one type: a
    /// `hdri::EnvSun` goes straight into `World.environment.sun`.
    #[test]
    fn the_env_sun_is_the_scene_type() {
        fn scene(s: makepad_scene::EnvSun) -> makepad_scene::EnvSun {
            s
        }
        let s = EnvSun { dir: vec3f(0.0, 1.0, 0.0), radiance: vec3f(1.0, 1.0, 1.0), cos_radius: 0.99, facing: 1.0, cos_cover: 0.98 };
        assert_eq!(scene(s), s);
        let env = makepad_scene::Environment { sun: Some(s), ..makepad_scene::Environment::default() };
        assert!(env.validate().is_ok());
    }

    /// Redesign §5 (R5): bloom only thins a caster. Every texel with bloom is at most the map
    /// without the caster and at least the map with bloom off, for a short bloom, a long one,
    /// and one whose threshold the whole sky passes; and the bloom does thin somewhere.
    #[test]
    fn bloom_never_adds_light() {
        let open = Env::new(&sun_at(30.0)).bake(128);
        for (length_deg, threshold_ev) in [(8.0f32, 10.0f32), (20.0, 10.0), (20.0, -2.0)] {
            let on = Env::new(&sun_under_leaves(1.0, length_deg, threshold_ev)).bake(128);
            let off = Env::new(&sun_under_leaves(0.0, length_deg, threshold_ev)).bake(128);
            let mut thinner = 0;
            for ((a, b), c) in on.data.iter().zip(&off.data).zip(&open.data) {
                for k in 0..3 {
                    assert!(a[k] <= c[k] * (1.0 + 1.0e-5) + 1.0e-12, "bloom over {length_deg} deg above {threshold_ev} EV: {} is above the map without the caster, {}", a[k], c[k]);
                    assert!(a[k] >= b[k] * (1.0 - 1.0e-5) - 1.0e-12, "bloom over {length_deg} deg above {threshold_ev} EV: {} is under the map with bloom off, {}", a[k], b[k]);
                }
                if a[1] > b[1] * 1.001 + 1.0e-9 {
                    thinner += 1;
                }
            }
            assert!(thinner > 0, "bloom over {length_deg} deg above {threshold_ev} EV thins nothing");
        }
    }

    /// Redesign §5 (R5): more bloom strength or more bloom length thins more. Strength scales
    /// one raster, so the light only grows (to the paths' rounding); a longer length widens the
    /// kernel, so the light grows at every step.
    #[test]
    fn more_bloom_strength_or_length_thins_more() {
        let by_strength: Vec<f64> = [0.0f32, 0.25, 0.5, 1.0].iter().map(|&s| light_through_leaves(&Env::new(&sun_under_leaves(s, 8.0, 10.0)))).collect();
        assert!(by_strength.windows(2).all(|w| w[1] >= w[0] * (1.0 - 1.0e-6)), "by strength: {by_strength:?}");
        assert!(by_strength[3] > 1.01 * by_strength[0], "bloom 1 shows: {by_strength:?}");
        let by_length: Vec<f64> = [2.0f32, 5.0, 10.0, 20.0].iter().map(|&l| light_through_leaves(&Env::new(&sun_under_leaves(1.0, l, 10.0)))).collect();
        assert!(by_length.windows(2).all(|w| w[1] > w[0]), "by length: {by_length:?}");
    }

    /// Redesign §5 (R5): a threshold above everything behind the caster changes nothing, bit
    /// for bit: the pre-pass looks, keeps no raster, and the caster stays exactly its flag.
    /// The map is dimmed by 5 EV so nothing in it, the sun's disc included, comes near 2^20.
    #[test]
    fn a_bloom_threshold_above_everything_changes_nothing() {
        use probe::{counted, Work};
        let dim = |mut p: HdriParams| {
            p.intensity_ev = -5.0;
            p
        };
        let on = dim(sun_under_leaves(1.0, 20.0, 20.0));
        let off = dim(sun_under_leaves(0.0, 20.0, 20.0));
        let before = counted(Work::CasterBloom);
        let env = Env::new(&on);
        assert_eq!(counted(Work::CasterBloom), before + 1, "the pre-pass looked behind the caster");
        let sun = env.sun_dir().expect("a manual sun");
        assert!(lum(env.radiance(sun)) < 2.0f32.powf(20.0), "nothing in this map reaches 2^20: the sun is {}", lum(env.radiance(sun)));
        assert!(env.studio.caster(0).is_some_and(|c| c.bloom().is_none()), "nothing behind the caster is that bright, so no raster is kept");
        assert!(env.bake(64) == Env::new(&off).bake(64), "bit for bit the map with bloom off");
    }

    /// Redesign §5 (R5): bloom strength 0, or a bloom length of 0, builds nothing and changes
    /// nothing, even with a threshold the whole sky passes.
    #[test]
    fn bloom_strength_zero_changes_nothing() {
        use probe::{counted, Work};
        let plain = Env::new(&sun_under_leaves(0.0, 5.0, 10.0)).bake(64);
        for (name, p) in [("strength 0 over 45 degrees above -10 EV", sun_under_leaves(0.0, 45.0, -10.0)), ("length 0 at strength 1 above -10 EV", sun_under_leaves(1.0, 0.0, -10.0))] {
            let before = counted(Work::CasterBloom);
            let map = Env::new(&p).bake(64);
            assert_eq!(counted(Work::CasterBloom), before, "{name}: no pre-pass");
            assert!(map == plain, "{name}: bit for bit the caster without bloom");
        }
    }

    /// Redesign §5, decision 11 (R5): one raster per bloom caster per Env, built with the Env;
    /// the bakes (serial and parallel), the key and the keyless copy read it and build no
    /// other. None for bloom off, a disabled caster or one whose picture is missing.
    #[test]
    fn bloom_is_built_once_per_env_and_not_when_off() {
        use probe::{counted, Work};
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f);
        let on = sun_under_leaves(1.0, 10.0, 10.0);
        let before = counted(Work::CasterBloom);
        let env = Env::new(&on);
        assert_eq!(counted(Work::CasterBloom), before + 1, "one bloom caster, one raster");
        let map = env.bake(32);
        assert!(env.bake_par(32, serial) == map);
        let _ = env.sun().expect("the sun is the key");
        assert!(env.bake_keyless_par(&map, serial).is_some());
        assert_eq!(counted(Work::CasterBloom), before + 1, "the bakes, the key and the keyless copy read that raster");
        let mut two = on.clone();
        two.lights.push(LightParams { azimuth_deg: 160.0, ..on.lights[0].clone() });
        let before = counted(Work::CasterBloom);
        let _ = Env::new(&two);
        assert_eq!(counted(Work::CasterBloom), before + 2, "two bloom casters, two rasters");
        let mut disabled = on.clone();
        disabled.lights[0].enabled = false;
        let mut missing = on.clone();
        missing.lights[0].gobo = Some(GoboParams { kind: "image".to_string(), image: "nowhere.png".to_string(), distance: 0.0, ..GoboParams::default() });
        for (name, p) in [("bloom strength 0", sun_under_leaves(0.0, 10.0, 10.0)), ("bloom length 0", sun_under_leaves(1.0, 0.0, 10.0)), ("a disabled caster", disabled), ("a caster whose picture is missing", missing)] {
            let before = counted(Work::CasterBloom);
            let _ = Env::new(&p);
            assert_eq!(counted(Work::CasterBloom), before, "{name}: no bloom raster");
        }
    }

    /// R5: the bloom is fixed per Env, so a row-parallel bake is the serial bake, bit for bit,
    /// at the reference width and at the widths the app bakes.
    #[test]
    fn a_bake_and_a_parallel_bake_agree_with_bloom() {
        for width in [CASTER_REFERENCE_WIDTH, 512, 64] {
            let env = Env::new_for_width(&sun_under_leaves(1.0, 10.0, 10.0), &ImageSet::default(), width, &|| false).expect("nothing cancels it");
            assert!(env.studio.caster(0).is_some_and(|c| c.bloom().is_some()), "width {width}: the sun blooms");
            assert!(env.bake_par(48, four_threads) == env.bake(48), "width {width}: the parallel bake is the serial one");
        }
    }

    /// M5: an Env whose casters' rasters (softness and bloom) are spread over four threads is
    /// the one built on this thread, bit for bit, at the held and released widths; a runner
    /// that skips rows, as an overtaken job's does, builds no map.
    #[test]
    fn an_env_whose_caster_rasters_are_spread_is_the_serial_one() {
        let images = ImageSet::default();
        let mut p = sun_under_leaves(1.0, 10.0, 10.0);
        p.lights[0].caster = Some(CasterParams { softness: 0.4, ..p.lights[0].caster_params() });
        p.lights.push(LightParams { azimuth_deg: 180.0, elevation_deg: 25.0, ..p.lights[0].clone() });
        for width in [512, CASTER_REFERENCE_WIDTH] {
            let serial = Env::new_for_width(&p, &images, width, &|| false).expect("nothing cancels it");
            let spread = Env::new_for_width_par(&p, &images, width, &|| false, &four_threads).expect("nothing cancels it");
            assert!((0..2).all(|i| spread.studio.caster(i).is_some_and(|c| c.bloom().is_some())), "width {width}: both casters bloom");
            assert!(spread.bake(48) == serial.bake(48), "width {width}: the rasters spread over four threads are the serial ones");
        }
        let skipping = |n: usize, row: &(dyn Fn(usize) + Sync)| (0..n).filter(|i| i % 2 == 0).for_each(row);
        assert!(Env::new_for_width_par(&p, &images, 512, &|| false, &skipping).is_none(), "a runner that skipped rows built a map");
    }

    /// R5 (N3): the keyless copy reads the raster the map was built with, its key included.
    /// It is the whole bake without the key, and the leaves the sun's bloom thins (outside
    /// its disc) are as thin in it as in the map. Had the copy built its bloom without the
    /// sun, they would be as dark as with bloom off.
    #[test]
    fn the_keyless_copy_reads_the_same_bloom() {
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(f);
        let env = Env::new(&sun_under_leaves(1.0, 10.0, 10.0));
        assert!(matches!(env.key(), Some((KeyTerm::Sun, _))), "the sun is the key");
        let map = env.bake(64);
        assert!(env.bake_keyless_par(&map, serial) == env.bake_without_key(64), "the copy is the whole bake without the key");
        let off = Env::new(&sun_under_leaves(0.0, 10.0, 10.0));
        let mut thinned = 0;
        for i in 0..40 {
            for j in 0..24 {
                let d = dir_from_az_el(194.0 + 0.3 * i as f32, 32.0 + 0.25 * j as f32);
                let with_key = env.radiance(d);
                let without = env.radiance_with(d, Some(KeyTerm::Sun), Sampling::default());
                assert!((with_key - without).length() <= 1.0e-6 * with_key.length().max(1.0e-12), "{:?}: {with_key:?} in the map, {without:?} in the copy", az_el_from_dir(d));
                if lum(with_key) > 1.01 * lum(off.radiance(d)) {
                    thinned += 1;
                }
            }
        }
        assert!(thinned > 20, "the sun's bloom thins the leaves over it ({thinned} of 960 directions)");
    }

    /// R5, the bloom's property 4: the sun's disc (0.53 degrees) is smaller than a cell of
    /// the raster at the reference width, and it is area-averaged into it. As it moves along
    /// the caster's edge in tenths of a degree, the bright light the raster holds stays within
    /// 5 % of its mean. Sampled at the cell centres alone, it swings from none to about twice.
    #[test]
    fn a_small_sun_behind_leaves_blooms_smoothly_as_it_moves() {
        let mut energies = Vec::new();
        for i in 0..=10 {
            let mut p = sun_under_leaves(1.0, 5.0, 10.0);
            p.sky.sun.azimuth_deg = 200.0 + 0.1 * i as f32;
            let env = Env::new(&p);
            let bloom = env.studio.caster(0).and_then(|c| c.bloom()).expect("the sun blooms");
            assert!(bloom.cell() > 2.0 * 0.265f32.to_radians().tan(), "the disc is smaller than a cell: {}", bloom.cell());
            energies.push(bloom.energy());
        }
        let mean = energies.iter().sum::<f64>() / energies.len() as f64;
        assert!(mean > 0.0, "{energies:?}");
        assert!(energies.iter().all(|e| (e / mean - 1.0).abs() < 0.05), "the bright light behind the caster as the sun moves: {energies:?}");
    }

    /// R5 review round 1: a disc far from the caster's centre is found where the caster's
    /// tangent plane puts it. The sun is 39 degrees out from the centre of a 70 x 40 degree
    /// leaves caster, past its side edge, where the plane stretches the disc by about 1.6
    /// outward; only the cells its projection reaches take the fine grid. As it climbs in
    /// tenths of a degree, the bright light the raster holds stays within 5 % of its mean.
    #[test]
    fn a_small_sun_far_off_the_casters_centre_blooms_smoothly_as_it_moves() {
        let mut energies = Vec::new();
        for i in 0..=10 {
            let mut p = sun_at(30.0 + 0.1 * i as f32);
            p.lights.push(LightParams {
                azimuth_deg: 245.0,
                elevation_deg: 30.0,
                width_deg: 70.0,
                height_deg: 40.0,
                gobo: Some(GoboParams { kind: "leaves".to_string(), density: 0.6, distance: 0.0, seed: 3, ..GoboParams::default() }),
                caster: Some(CasterParams { bloom_strength: 1.0, bloom_length_deg: 10.0, ..CasterParams::default() }),
                ..LightParams::new_caster()
            });
            let env = Env::new(&p);
            let bloom = env.studio.caster(0).and_then(|c| c.bloom()).expect("the sun blooms");
            energies.push(bloom.energy());
        }
        let mean = energies.iter().sum::<f64>() / energies.len() as f64;
        assert!(mean > 0.0, "{energies:?}");
        assert!(energies.iter().all(|e| (e / mean - 1.0).abs() < 0.05), "the bright light behind the caster as the sun climbs: {energies:?}");
    }

    /// R5: the pre-pass polls `cancelled`, so a map with a bloom caster stops in it and is
    /// None; one that is not cancelled is built; a map with no bloom to build is built
    /// whatever the poll says.
    #[test]
    fn a_cancelled_bloom_pre_pass_builds_no_map() {
        let images = ImageSet::default();
        assert!(Env::new_cancellable(&sun_under_leaves(1.0, 10.0, 10.0), &images, &|| true).is_none(), "cancelled in the pre-pass");
        assert!(Env::new_cancellable(&sun_under_leaves(1.0, 10.0, 10.0), &images, &|| false).is_some());
        assert!(Env::new_cancellable(&sun_under_leaves(0.0, 10.0, 10.0), &images, &|| true).is_some(), "no bloom: nothing slow to stop");
    }

    // ---- plan hdri-7: the night's bit-identity across commits ------------------------------------------------------

    /// Where the pre-plan-7 night baselines live (`local/` is not committed; a machine without them skips).
    fn night_baseline_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/agent_state/hdri/night-baseline")
    }

    /// The maps the night baselines hold: the three night presets, the moonlit night under rain with clouds 0.5, and a
    /// studio whose one window is filled with the moonlit night's sky.
    fn night_baseline_cases() -> Vec<(&'static str, HdriParams)> {
        use super::presets::preset;
        let moonlit = preset("Moonlit night").unwrap();
        let mut rain = moonlit.clone();
        rain.sky.weather = Some(WeatherParams { kind: "rain".to_string(), ..WeatherParams::default() });
        rain.sky.clouds.coverage = 0.5;
        let window = LightParams { gobo: Some(GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, fill_sky: true, ..GoboParams::default() }), ..LightParams::default() };
        let studio = HdriParams { mode: "studio".to_string(), lights: vec![window], ..moonlit.clone() };
        vec![("moonlit", moonlit), ("starry", preset("Starry night").unwrap()), ("blue_hour", preset("Blue hour").unwrap()), ("moonlit_rain", rain), ("studio_window", studio)]
    }

    /// Everything of a map the night baselines pin, as f32s: its 128 bake; its key (a flag, then its nine numbers) and
    /// the keyless copy when there is one; the strip's light (every field the base has); rows 300..308 of its 2048 bake.
    fn night_dump(p: &HdriParams) -> Vec<f32> {
        let env = Env::new(p);
        let mut out: Vec<f32> = Vec::new();
        let map = env.bake(128);
        out.extend(map.data.iter().flatten());
        match env.sun() {
            Some(k) => {
                out.push(1.0);
                out.extend([k.dir.x, k.dir.y, k.dir.z, k.radiance.x, k.radiance.y, k.radiance.z, k.cos_radius, k.facing, k.cos_cover]);
                if let Some(keyless) = env.bake_keyless_par(&map, |n, f| (0..n).for_each(f)) {
                    out.extend(keyless.data.iter().flatten());
                }
            }
            None => out.push(0.0),
        }
        let l = env.skyline_light();
        out.extend([l.sun_u, l.sun_el_deg]);
        out.extend(l.sun_rgb.iter().chain(&l.ambient_rgb));
        for ring in [&l.ring_low, &l.ring_high, &l.ground_ring, &l.sea_low, &l.sea_high, &l.ground_damp] {
            out.extend(ring.iter().flatten());
        }
        out.push(l.wet);
        let rows = env.bake_par(2048, |_, f| (300..308).for_each(f));
        out.extend(rows.data[300 * 2048..308 * 2048].iter().flatten());
        out
    }

    /// Writes the night baselines (plan hdri-7 Task 1a, at the base, before any engine change): `HDRI_NIGHT_RECORD=1`.
    #[test]
    #[ignore = "records local/agent_state/hdri/night-baseline: HDRI_NIGHT_RECORD=1"]
    fn record_night_baseline() {
        if std::env::var_os("HDRI_NIGHT_RECORD").is_none() {
            return eprintln!("HDRI_NIGHT_RECORD not set, nothing written");
        }
        let dir = night_baseline_dir();
        std::fs::create_dir_all(&dir).unwrap();
        for (name, p) in night_baseline_cases() {
            let bytes: Vec<u8> = night_dump(&p).iter().flat_map(|x| x.to_le_bytes()).collect();
            std::fs::write(dir.join(format!("{name}.f32")), &bytes).unwrap();
            println!("night baseline {name}: {} floats", bytes.len() / 4);
        }
    }

    /// Plan hdri-7: with no night block every map is the pre-plan-7 base's, bit for bit (the baselines Task 1a recorded;
    /// skipped where they are missing).
    #[test]
    fn night_bakes_match_the_pre_plan7_baseline() {
        let dir = night_baseline_dir();
        for (name, p) in night_baseline_cases() {
            let Ok(bytes) = std::fs::read(dir.join(format!("{name}.f32"))) else {
                eprintln!("no night baseline {name}, skipped");
                continue;
            };
            let want: Vec<u32> = bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
            let got: Vec<u32> = night_dump(&p).iter().map(|x| x.to_bits()).collect();
            assert_eq!(got.len(), want.len(), "{name}: the dump's length");
            let first = got.iter().zip(&want).position(|(g, w)| g != w);
            assert!(first.is_none(), "{name}: float {} differs: {} vs {}", first.unwrap(), f32::from_bits(got[first.unwrap()]), f32::from_bits(want[first.unwrap()]));
            println!("night baseline {name}: identical");
        }
    }

    /// Plan hdri-7: for a night block set by `set(p, on)`: absent, present but off, and on while the sun is up, every
    /// bake (serial, parallel, an export width), the key, the keyless copy and the strip's light are the base's.
    fn night_block_changes_nothing(set: &dyn Fn(&mut HdriParams, bool)) {
        use super::presets::preset;
        let same = |a: &HdriParams, b: &HdriParams, what: &str| {
            let (ea, eb) = (Env::new(a), Env::new(b));
            let (ma, mb) = (ea.bake(128), eb.bake(128));
            assert!(ma == mb, "{what}: the 128 bake");
            assert!(eb.bake_par(128, four_threads) == mb, "{what}: the parallel bake");
            assert_eq!(ea.sun(), eb.sun(), "{what}: the key");
            assert!(ea.bake_keyless_par(&ma, four_threads) == eb.bake_keyless_par(&mb, four_threads), "{what}: the keyless copy");
            assert!(ea.skyline_light() == eb.skyline_light(), "{what}: the strip's light");
            assert!(ea.bake(512) == eb.bake(512), "{what}: an export width");
        };
        for name in ["Moonlit night", "Starry night"] {
            let base = preset(name).unwrap();
            let mut off = base.clone();
            set(&mut off, false);
            same(&base, &off, &format!("{name}, the block off"));
            let mut day = base.clone();
            day.sky.sun.elevation_deg = 10.0;
            let mut day_on = day.clone();
            set(&mut day_on, true);
            same(&day, &day_on, &format!("{name}, the block on by day"));
        }
    }

    /// Plan hdri-7 §5: two bake widths show a layer alike. Two things are compared, each by `blocks_agree`: the light the
    /// layer draws (what it adds with the stars off on both sides) and, when its dust takes any, the share of the stars'
    /// light it takes (1 less `NightSky::star_transmittance` at each width's texel centres). The stars themselves stay
    /// out: they are points read at texel centres, so the star field alone differs by up to 82 % a block between 1024
    /// and 2048, and the light a dust takes off them carries that (the Milky Way's: a 10.5 % block), which is the stars'
    /// sampling, not the layer's. Held against released is 512 against 1024 (`held_and_released_agree`); an export
    /// width, 1024 against 2048 (each layer's ignored `*_at_export_widths` test).
    ///
    /// The two make the whole layer only if everything that dims the stars goes through `star_transmittance`, so that is
    /// checked first, at every starlit texel centre of the `width` bake: `deep_sky` with the stars on is `deep_sky` with
    /// them off plus `stars` times `star_transmittance` (plus the stars a layer holds, which Star brightness scales too
    /// and no dust dims: the nebulas' embedded stars, `NightSky::embedded_stars`). A layer that dims the stars some other
    /// way fails there, instead of passing with its dimming unmeasured.
    fn widths_agree(base: &HdriParams, set: &dyn Fn(&mut HdriParams), width: usize, block_share: f32, total_share: f32, what: &str) {
        let lum = |t: &[f32; 4]| 0.2126 * t[0] + 0.7152 * t[1] + 0.0722 * t[2];
        let starless = |p: &HdriParams| {
            let mut p = p.clone();
            p.sky.night.star_brightness = 0.0;
            p
        };
        let mut on = base.clone();
        set(&mut on);
        let (on_dark, base_dark) = (starless(&on), starless(base));
        let added = |w: usize| {
            let (a, b) = (Env::new(&on_dark).bake_par(w, four_threads), Env::new(&base_dark).bake_par(w, four_threads));
            a.data.iter().zip(&b.data).map(|(x, y)| lum(x) - lum(y)).collect::<Vec<f32>>()
        };
        blocks_agree(&added(width), &added(2 * width), width, block_share, total_share, &format!("{what}, its light"));
        let (env, dark_env) = (Env::new(&on), Env::new(&on_dark));
        if let (Some(lit), Some(dark)) = (env.night.as_ref(), dark_env.night.as_ref()) {
            let (h, texel) = (width / 2, std::f32::consts::TAU / width as f32);
            let (mut starlit, mut dimmed) = (0usize, 0usize);
            for i in 0..width * h {
                let uv = [((i % width) as f32 + 0.5) / width as f32, ((i / width) as f32 + 0.5) / h as f32];
                let d = vec(ibl::equirect_uv_to_dir(uv));
                let stars = lit.stars(d);
                if stars.max_elem() <= 0.0 {
                    continue;
                }
                let t = lit.star_transmittance(d);
                let embedded = lit.embedded_stars(d) - dark.embedded_stars(d);
                let (got, want) = (lit.deep_sky(d, texel), dark.deep_sky(d, texel) + stars * t + embedded);
                assert!((got - want).length() <= 1.0e-5 * got.length().max(want.length()), "{what}: along {d:?} the stars are dimmed outside star_transmittance ({t}): {got:?} against {want:?}");
                starlit += 1;
                dimmed += (t < 1.0) as usize;
            }
            assert!(starlit > 0, "{what}: the premise: stars at the texel centres");
            println!("{what}: every dimming of the stars goes through star_transmittance ({dimmed} of {starlit} starlit texels dimmed)");
        }
        let taken = |w: usize| {
            let take = |d: Vec3f| vec3f(1.0 - env.night.as_ref().map_or(1.0, |n| n.star_transmittance(d)), 0.0, 0.0);
            bake_par_with(w, &take, four_threads).data.iter().map(|t| t[0]).collect::<Vec<f32>>()
        };
        let (held, released) = (taken(width), taken(2 * width));
        if held.iter().chain(&released).any(|t| *t > 0.0) {
            blocks_agree(&held, &released, width, block_share, total_share, &format!("{what}, the stars' light its dust takes"));
        } else {
            println!("{what}: it dims no star");
        }
    }

    /// `held` (`w` x `w / 2`) and `released` (twice as wide), box-averaged to `w`, agree in every 16 x 16 block holding at
    /// least a tenth of the largest block's, within `block_share`, and in total within `total_share`.
    fn blocks_agree(held: &[f32], released: &[f32], w: usize, block_share: f32, total_share: f32, what: &str) {
        let h = w / 2;
        let down: Vec<f32> = (0..h * w)
            .map(|i| {
                let (y, x) = (i / w, i % w);
                let at = |dy: usize, dx: usize| released[(2 * y + dy) * 2 * w + 2 * x + dx];
                0.25 * (at(0, 0) + at(0, 1) + at(1, 0) + at(1, 1))
            })
            .collect();
        let block = |m: &[f32], by: usize, bx: usize| (0..16).flat_map(|y| (0..16).map(move |x| (y, x))).map(|(y, x)| m[(by * 16 + y) * w + bx * 16 + x]).sum::<f32>() / 256.0;
        let blocks: Vec<(f32, f32)> = (0..h / 16).flat_map(|by| (0..w / 16).map(move |bx| (by, bx))).map(|(by, bx)| (block(held, by, bx), block(&down, by, bx))).collect();
        let most = blocks.iter().map(|(h, r)| h.abs().max(r.abs())).fold(0.0f32, f32::max);
        assert!(most > 0.0, "{what}: the premise: the layer adds something");
        let mut worst = 0.0f32;
        for (h, r) in blocks.iter().filter(|(h, r)| h.abs().max(r.abs()) >= 0.1 * most) {
            worst = worst.max((h - r).abs() / h.abs().max(r.abs()));
        }
        let (th, tr) = (held.iter().sum::<f32>(), down.iter().sum::<f32>());
        println!("{what}: {w} against {}, worst block {:.1} %, total {:.2} %", 2 * w, 100.0 * worst, 100.0 * (th - tr).abs() / tr.abs());
        assert!(worst <= block_share, "{what}: a block differs by {:.1} %", 100.0 * worst);
        assert!((th - tr).abs() <= total_share * tr.abs(), "{what}: the totals {th} and {tr}");
    }

    /// Plan hdri-7 §5: held (512) and released (1024) show a layer alike (`widths_agree` at 512).
    fn held_and_released_agree(base: &HdriParams, set: &dyn Fn(&mut HdriParams), block_share: f32, total_share: f32, what: &str) {
        widths_agree(base, set, 512, block_share, total_share, what);
    }

    /// Plan hdri-7 §5: a night layer `set` switches on sits where the stars do: what it adds along `d` (map frame) on an
    /// open sky goes through a closed deck only as the deck's coverage lets it, thick fog leaves under 1 % of it, and an
    /// opaque skyline texel hides it. A layer that `lights` the scene (the aurora) also lights the deck's underside and the
    /// fog a little: 10 % of what it adds is allowed there.
    fn a_layer_sits_behind_the_deck_the_fog_and_the_skyline(base: &HdriParams, set: &dyn Fn(&mut HdriParams), d: Vec3f, lights: bool) {
        let lum = crate::sky::luminance;
        let added = |p: &HdriParams| {
            let mut on = p.clone();
            set(&mut on);
            lum(Env::new(&on).radiance(d)) - lum(Env::new(p).radiance(d))
        };
        let open = added(base);
        assert!(open.abs() > 0.0, "the premise: the layer shows along {d:?}");
        let mut deck = base.clone();
        deck.sky.clouds.coverage = 1.0;
        deck.sky.clouds.sharpness = 1.0;
        let alpha = Env::new(&deck).clouds.as_ref().map_or(0.0, |layer| layer.sample(d).alpha);
        let through = added(&deck);
        let slack = if lights { 0.1 * open.abs() } else { 1.0e-3 * open.abs() };
        assert!(alpha > 0.9 && (through - open * (1.0 - alpha)).abs() <= slack + 1.0e-12, "a deck of coverage {alpha} passes {through} of {open}");
        let mut fog = base.clone();
        fog.sky.weather = Some(WeatherParams { kind: "fog".to_string(), fog: 1.0, ..WeatherParams::default() });
        // thick fog leaves under 1 % of a point layer; one that lights the scene also lights the fog a little
        let fog_slack = if lights { 0.1 } else { 0.01 };
        assert!(added(&fog).abs() < fog_slack * open.abs(), "thick fog lets {} of {open} through", added(&fog));
        let (w, h) = (64usize, 16usize);
        let mut images = ImageSet::default();
        images.skyline = Some(std::sync::Arc::new(skyline::SkylineBand { w, h, texels: vec![[0.5, 0.5, 0.5, 1.0]; w * h], dist: Vec::new(), light: Vec::new(), top_deg: 80.0, bottom_deg: -5.0, eye_m: 0.0 }));
        let mut city = base.clone();
        city.skyline = Some(SkylineParams { on: true, seed: 7, turn_deg: 0.0, lights_by_day: 0.0, settings: Default::default() });
        let mut city_on = city.clone();
        set(&mut city_on);
        assert_eq!(Env::new_with(&city_on, &images).radiance(d), Env::new_with(&city, &images).radiance(d), "an opaque skyline texel hides the layer");
    }

    /// Plan hdri-7 D9: a layer that rides with the stars lights nothing: the clouds' ambient, the dome, the ground's gain,
    /// the night's ground and the strip's light are what they were.
    fn a_point_layer_lights_nothing(set: &dyn Fn(&mut HdriParams)) {
        let mut p = super::presets::preset("Starry night").unwrap();
        p.sky.clouds.coverage = 0.5;
        let mut on = p.clone();
        set(&mut on);
        let (a, b) = (Env::new(&p), Env::new(&on));
        assert_eq!((a.cloud_ambient, a.dome, a.ground_gain), (b.cloud_ambient, b.dome, b.ground_gain));
        assert!(a.skyline_light() == b.skyline_light());
        let down = vec3f(0.0, -1.0, 0.0);
        assert_eq!(a.night.as_ref().unwrap().ground(down, Vec3f::all(1.0)), b.night.as_ref().unwrap().ground(down, Vec3f::all(1.0)));
    }

    /// Writes `name`.png of `p` at 1024, exposed as the app's Auto would (0.5 log2(0.18 / mean)), into
    /// HDRI_NIGHT_VISUAL_DIR (plan hdri-7's engine-side looks; nothing without it). The exposure, when written.
    fn write_look(name: &str, p: &HdriParams) -> Option<f32> {
        let Some(dir) = std::env::var_os("HDRI_NIGHT_VISUAL_DIR").map(std::path::PathBuf::from) else {
            eprintln!("HDRI_NIGHT_VISUAL_DIR not set, {name} skipped");
            return None;
        };
        std::fs::create_dir_all(&dir).unwrap();
        let map = Env::new(p).bake_par(1024, four_threads);
        let mean = image::mean_luminance(&map).max(1.0e-30);
        let ev = (0.5 * (0.18 / mean).log2()).clamp(-16.0, 16.0);
        std::fs::write(dir.join(format!("{name}.png")), export::encode_png(&map, ev).unwrap()).unwrap();
        println!("look {name}: mean {mean:.3e}, EV {ev:.2}");
        Some(ev)
    }

    /// Task 3b's views of `p`'s night, from its 4096 map at exposure `ev`, into HDRI_NIGHT_VISUAL_DIR: `name`-look (90
    /// degrees toward the galactic centre, raised 15, as Look around shows it), `name`-sky (the whole sky from below,
    /// north up and east left, the zenith in the middle) and `name`-core (the 4096 map's own texels round the centre,
    /// 1024 square).
    fn write_views(name: &str, p: &HdriParams, ev: f32) {
        let Some(dir) = std::env::var_os("HDRI_NIGHT_VISUAL_DIR").map(std::path::PathBuf::from) else { return };
        let env = Env::new(p);
        let map = env.bake_par(4096, four_threads);
        let centre = env.night_features().and_then(|f| f.galactic_centre).expect("the band draws");
        let size = 1024usize;
        let write = |what: &str, rgb: Vec<f32>| {
            let png = export::encode_face_png(&export::FaceImage { size, rgb }, ev).unwrap();
            std::fs::write(dir.join(format!("{name}-{what}.png")), png).unwrap();
        };
        let view = |look: &dyn Fn(f32, f32) -> Option<Vec3f>| {
            let mut rgb = Vec::with_capacity(size * size * 3);
            for i in 0..size * size {
                let (u, v) = (((i % size) as f32 + 0.5) / size as f32 * 2.0 - 1.0, 1.0 - ((i / size) as f32 + 0.5) / size as f32 * 2.0);
                let c = look(u, v).map_or([0.0; 4], |d| map.sample([d.x, d.y, d.z]));
                rgb.extend_from_slice(&c[..3]);
            }
            rgb
        };
        let (az, el) = az_el_from_dir(centre);
        let ahead = dir_from_az_el(az, el + 15.0);
        let right = Vec3f::cross(ahead, vec3f(0.0, 1.0, 0.0)).normalize();
        let up = Vec3f::cross(right, ahead);
        write("look", view(&|u, v| Some((ahead + right * u + up * v).normalize())));
        write("sky", view(&|u, v| {
            let r = (u * u + v * v).sqrt();
            (r <= 1.0).then(|| dir_from_az_el((-u).atan2(v).to_degrees(), 90.0 - 90.0 * r))
        }));
        let uv = ibl::dir_to_equirect_uv([centre.x, centre.y, centre.z]);
        let (cx, cy) = ((uv[0] * map.width as f32) as i64, (uv[1] * map.height as f32) as i64);
        let top = (cy - 700).clamp(0, (map.height - size) as i64);
        let mut rgb = Vec::with_capacity(size * size * 3);
        for y in 0..size as i64 {
            for x in 0..size as i64 {
                let tx = (cx - size as i64 / 2 + x).rem_euclid(map.width as i64) as usize;
                rgb.extend_from_slice(&map.data[(top + y) as usize * map.width + tx][..3]);
            }
        }
        write("core", rgb);
        println!("views {name}: the centre at azimuth {az:.1}, elevation {el:.1}");
    }

    /// The galactic centre of "Starry night" with the Milky Way at its defaults, in the map's frame (about 16 degrees
    /// up in the south at 22 h, 45 N).
    fn starry_centre() -> (HdriParams, Vec3f) {
        let base = super::presets::preset("Starry night").unwrap();
        assert_eq!(base.rotation_deg, 0.0, "the map frame is the world frame");
        let mut on = base.clone();
        on.sky.night.milky_way = Some(MilkyWayParams::default());
        let centre = Env::new(&on).night_features().expect("sky mode").galactic_centre.expect("the band draws");
        (base, centre)
    }

    #[test]
    fn the_milky_way_off_or_by_day_changes_nothing() {
        night_block_changes_nothing(&|p, on| p.sky.night.milky_way = Some(MilkyWayParams { on, ..MilkyWayParams::default() }));
    }

    #[test]
    fn the_milky_way_held_and_released_agree() {
        let base = super::presets::preset("Starry night").unwrap();
        // Turn 0: the stars stay where they are, so what the layer adds is the band and its dust
        held_and_released_agree(&base, &|p| p.sky.night.milky_way = Some(MilkyWayParams { turn_deg: 0.0, ..MilkyWayParams::default() }), 0.10, 0.03, "the Milky Way");
    }

    /// Plan hdri-7 §5 ("in exports at every width", design §7): 1024 against 2048, by the held rule. Ignored: two 2048
    /// bakes each way, a release run (`cargo test -p makepad-render --release --lib at_export_widths -- --ignored`).
    #[test]
    #[ignore = "release: 1024 against 2048"]
    fn the_milky_way_agrees_at_export_widths() {
        let base = super::presets::preset("Starry night").unwrap();
        widths_agree(&base, &|p| p.sky.night.milky_way = Some(MilkyWayParams { turn_deg: 0.0, ..MilkyWayParams::default() }), 1024, 0.10, 0.03, "the Milky Way");
    }

    #[test]
    fn the_milky_way_sits_behind_the_weather_and_the_skyline() {
        let (base, centre) = starry_centre();
        assert!(centre.y > 0.1, "the premise: the centre is up ({centre:?})");
        let d = (centre + vec3f(0.0, 0.05, 0.0)).normalize();
        a_layer_sits_behind_the_deck_the_fog_and_the_skyline(&base, &|p| p.sky.night.milky_way = Some(MilkyWayParams::default()), d, false);
    }

    #[test]
    fn the_milky_way_lights_nothing() {
        a_point_layer_lights_nothing(&|p| p.sky.night.milky_way = Some(MilkyWayParams::default()));
    }

    /// The feature is the map's: None in Studio mode; with the band, the turned centre; without, none.
    #[test]
    fn the_night_features_are_in_the_maps_frame() {
        let (base, centre) = starry_centre();
        assert_eq!(Env::new(&base).night_features(), Some(night::NightFeatures::default()));
        assert!((centre.length() - 1.0).abs() < 1.0e-5);
        assert_eq!(Env::new(&HdriParams { mode: "studio".to_string(), ..base }).night_features(), None);
    }

    /// Plan hdri-7 D11: where the unlifted twilight sky and the lifted night glow cross at the zenith (recorded, not
    /// asserted: ASTRO_FADE_DEG is the spec's, not tuned without the owner). A report, so ignored: Step 7 runs it.
    #[test]
    #[ignore = "report: the twilight crossover"]
    fn the_twilight_crossover_is_measured() {
        for el in [-6.0f32, -8.0, -10.0, -12.0, -14.0, -16.0, -18.0, -20.0] {
            let mut p = HdriParams::default();
            p.sky.sun.mode = "manual".to_string();
            p.sky.sun.elevation_deg = el;
            p.sky.night.moon = false;
            let env = Env::new(&p);
            let up = vec3f(0.0, 1.0, 0.0);
            let (sky, glow) = (crate::sky::luminance(env.atmo.as_ref().unwrap().sky(up)), crate::sky::luminance(env.night.as_ref().unwrap().glow(up)));
            println!("sun {el:6.1}: twilight sky {sky:.3e}, night glow {glow:.3e}, ratio {:.2}", sky / glow.max(1.0e-30));
        }
    }

    #[test]
    #[ignore = "writes PNGs: HDRI_NIGHT_VISUAL_DIR"]
    fn milky_way_looks() {
        let base = super::presets::preset("Starry night").unwrap();
        write_look("starry-night", &base);
        for turn in [0.0f32, 116.0] {
            let mut p = base.clone();
            p.sky.night.milky_way = Some(MilkyWayParams { turn_deg: turn, ..MilkyWayParams::default() });
            write_look(&format!("milky-way-turn{turn:.0}"), &p);
        }
        // Task 3b: the band and the nebulas at their defaults, at Realism 0 and 1
        for realism in [0.0f32, 1.0] {
            let mut p = base.clone();
            p.sky.night.milky_way = Some(MilkyWayParams::default());
            p.sky.night.nebulas = Some(NebulaParams { realism, ..NebulaParams::default() });
            let name = format!("deep-sky-r{realism:.0}");
            if let Some(ev) = write_look(&name, &p) {
                write_views(&name, &p, ev);
            }
        }
    }

    #[test]
    fn the_nebulas_off_or_by_day_change_nothing() {
        night_block_changes_nothing(&|p, on| p.sky.night.nebulas = Some(NebulaParams { on, ..NebulaParams::default() }));
    }

    /// The gas's filaments and the Realism clamp are thresholded, so a coarse level's mean moves a little: the nebulas'
    /// held-vs-released rule is 20 % a block and 10 % in total (the Milky Way's and the aurora's: 10 % and 3 %). Over the
    /// band (Task 3b), at their defaults, they hold its knots too, by the same rule.
    #[test]
    fn the_nebulas_held_and_released_agree() {
        let base = super::presets::preset("Starry night").unwrap();
        held_and_released_agree(&base, &|p| p.sky.night.nebulas = Some(NebulaParams { amount: 1.0, size: 2.0, ..NebulaParams::default() }), 0.20, 0.10, "the nebulas");
        let mut band = base.clone();
        band.sky.night.milky_way = Some(MilkyWayParams { turn_deg: 0.0, ..MilkyWayParams::default() });
        held_and_released_agree(&band, &|p| p.sky.night.nebulas = Some(NebulaParams::default()), 0.20, 0.10, "the nebulas and their knots over the band");
    }

    /// Plan hdri-7 §5 ("in exports at every width"): 1024 against 2048, by the nebulas' held rule (ignored, release).
    #[test]
    #[ignore = "release: 1024 against 2048"]
    fn the_nebulas_agree_at_export_widths() {
        let base = super::presets::preset("Starry night").unwrap();
        widths_agree(&base, &|p| p.sky.night.nebulas = Some(NebulaParams { amount: 1.0, size: 2.0, ..NebulaParams::default() }), 1024, 0.20, 0.10, "the nebulas");
        let mut band = base.clone();
        band.sky.night.milky_way = Some(MilkyWayParams { turn_deg: 0.0, ..MilkyWayParams::default() });
        widths_agree(&band, &|p| p.sky.night.nebulas = Some(NebulaParams::default()), 1024, 0.20, 0.10, "the nebulas and their knots over the band");
    }

    #[test]
    fn the_nebulas_sit_behind_the_weather_and_the_skyline() {
        let base = super::presets::preset("Starry night").unwrap();
        let set = |p: &mut HdriParams| p.sky.night.nebulas = Some(NebulaParams { amount: 1.0, size: 2.0, ..NebulaParams::default() });
        let mut on = base.clone();
        set(&mut on);
        let clouds = Env::new(&on).night_features().unwrap().nebulas;
        // the centre, well up, where the clouds add the most (a dark cloud adds only where a star stands behind it)
        let lum = |p: &HdriParams, d: Vec3f| crate::sky::luminance(Env::new(p).radiance(d));
        let (d, _) = clouds
            .iter()
            .filter(|(c, _)| c.y > 0.25 && c.y < 0.9)
            .map(|(c, _)| (*c, (lum(&on, *c) - lum(&base, *c)).abs()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("the premise: a cloud well up");
        a_layer_sits_behind_the_deck_the_fog_and_the_skyline(&base, &set, d, false);
    }

    #[test]
    fn the_nebulas_light_nothing() {
        a_point_layer_lights_nothing(&|p| p.sky.night.nebulas = Some(NebulaParams { amount: 1.0, ..NebulaParams::default() }));
    }

    /// A map with nebulas bakes the same on one thread and on four (the sprites' rows fill on whichever thread reads
    /// them first: a seed no other test uses, so this bake fills them).
    #[test]
    fn a_bake_with_nebulas_is_its_parallel_bake() {
        let mut p = super::presets::preset("Starry night").unwrap();
        p.sky.night.nebulas = Some(NebulaParams { amount: 1.0, size: 3.0, seed: 0x51ab_0004, ..NebulaParams::default() });
        let env = Env::new(&p);
        let parallel = env.bake_par(64, four_threads);
        assert!(env.bake(64) == parallel);
        p.sky.night.milky_way = Some(MilkyWayParams::default());
        p.sky.night.nebulas = Some(NebulaParams { amount: 1.0, size: 3.0, seed: 0x51ab_0005, ..NebulaParams::default() });
        p.sky.night.aurora = Some(AuroraParams { folds: 0.8, rays: 0.8, ..AuroraParams::default() });
        let env = Env::new(&p);
        let parallel = env.bake_par(64, four_threads);
        assert!(env.bake(64) == parallel, "the three layers together");
    }

    #[test]
    #[ignore = "writes PNGs: HDRI_NIGHT_VISUAL_DIR"]
    fn nebula_looks() {
        let mut base = super::presets::preset("Starry night").unwrap();
        base.sky.night.milky_way = Some(MilkyWayParams::default());
        for realism in [0.0f32, 0.5, 1.0] {
            let mut p = base.clone();
            p.sky.night.nebulas = Some(NebulaParams { amount: 0.6, realism, ..NebulaParams::default() });
            write_look(&format!("nebulas-r{realism:.1}"), &p);
        }
    }

    #[test]
    fn the_aurora_off_or_by_day_changes_nothing() {
        night_block_changes_nothing(&|p, on| p.sky.night.aurora = Some(AuroraParams { on, ..AuroraParams::default() }));
    }

    /// The three blocks together, absent against off, and on by day: the base's maps (D13).
    #[test]
    fn the_three_off_together_change_nothing() {
        night_block_changes_nothing(&|p, on| {
            p.sky.night.milky_way = Some(MilkyWayParams { on, ..MilkyWayParams::default() });
            p.sky.night.nebulas = Some(NebulaParams { on, ..NebulaParams::default() });
            p.sky.night.aurora = Some(AuroraParams { on, ..AuroraParams::default() });
        });
    }

    #[test]
    fn the_aurora_held_and_released_agree() {
        let base = super::presets::preset("Starry night").unwrap();
        held_and_released_agree(&base, &|p| p.sky.night.aurora = Some(AuroraParams { folds: 0.8, rays: 0.8, ..AuroraParams::default() }), 0.10, 0.03, "the aurora");
    }

    /// Plan hdri-7 §5 ("in exports at every width"): 1024 against 2048, by the held rule (ignored, release).
    #[test]
    #[ignore = "release: 1024 against 2048"]
    fn the_aurora_agrees_at_export_widths() {
        let base = super::presets::preset("Starry night").unwrap();
        widths_agree(&base, &|p| p.sky.night.aurora = Some(AuroraParams { folds: 0.8, rays: 0.8, ..AuroraParams::default() }), 1024, 0.10, 0.03, "the aurora");
    }

    #[test]
    fn the_aurora_sits_behind_the_weather_and_the_skyline() {
        let base = super::presets::preset("Starry night").unwrap();
        let set = |p: &mut HdriParams| p.sky.night.aurora = Some(AuroraParams::default());
        let mut on = base.clone();
        set(&mut on);
        let middle = Env::new(&on).night_features().unwrap().aurora_middle.expect("the arc draws");
        let (az, el) = az_el_from_dir(middle);
        a_layer_sits_behind_the_deck_the_fog_and_the_skyline(&base, &set, dir_from_az_el(az, el + 3.0), true);
    }

    /// Plan 1b D1: a diffuse sky source is never a key. With the aurora on and no moon there is no key; with the moon
    /// up the key, the hot spots and what the keyless copy takes off are what they were without it.
    #[test]
    fn the_aurora_is_never_a_key() {
        use super::presets::preset;
        let with = |mut p: HdriParams| {
            p.sky.night.aurora = Some(AuroraParams::default());
            p
        };
        let starry = preset("Starry night").unwrap();
        assert!(Env::new(&starry).sun().is_none() && Env::new(&with(starry)).sun().is_none(), "no key from the aurora");
        let moonlit = preset("Moonlit night").unwrap();
        let (off, on) = (Env::new(&moonlit), Env::new(&with(moonlit.clone())));
        assert!(off.sun().is_some());
        assert_eq!(off.sun(), on.sun(), "the moon's key, bit for bit");
        assert_eq!(off.hot_spots(), on.hot_spots());
        let (map_off, map_on) = (off.bake(128), on.bake(128));
        let (cut_off, cut_on) = (off.bake_keyless_par(&map_off, four_threads).unwrap(), on.bake_keyless_par(&map_on, four_threads).unwrap());
        for i in 0..map_on.data.len() {
            for c in 0..3 {
                let (taken_off, taken_on) = (map_off.data[i][c] - cut_off.data[i][c], map_on.data[i][c] - cut_on.data[i][c]);
                assert!((taken_on - taken_off).abs() <= 1.0e-5 * map_on.data[i][c].abs().max(1.0e-12), "texel {i}: the keyless copy takes {taken_on}, without the aurora {taken_off}");
            }
        }
    }

    /// Plan hdri-7 D9: the aurora's light is in the open dome too, so the ground's gain stays a cover ratio: under a sky
    /// all but clear it is what it was without the aurora, and under a closed deck the ground gains only what the deck's
    /// own light gains (the night ground's light times the gain is the deck's light: their ratios, with the aurora and
    /// without, agree). The deck's gain is not held to 1 (the plan's absolute form, gain <= 1, fails at the base without
    /// the aurora; this ratio form is the controller's ruling, 2026-10-24): since plan 6's `cloud_ambient` a closed
    /// night deck, lit by the sky's mean, already puts more on the ground than the open sky does (Starry night:
    /// (2.17, 1.46, 0.74) without the aurora).
    #[test]
    fn the_auroras_light_keeps_the_ground_gain_a_cover_ratio() {
        let mut thin = super::presets::preset("Starry night").unwrap();
        thin.sky.clouds.coverage = 0.001;
        let with = |p: &HdriParams| {
            let mut q = p.clone();
            q.sky.night.aurora = Some(AuroraParams { strength: 5.0, ..AuroraParams::default() });
            q
        };
        let (a, b) = (Env::new(&thin).ground_gain, Env::new(&with(&thin)).ground_gain);
        // within 0.5 %: the hair of cloud (nine of the dome's looks) is lit by the night's mean, the aurora in it, and
        // covers a little of it, two small terms of either sign that the aurora's shape moves (Task 4c, 2026-10-24: 0.08 %
        // at the default before the ribbons folded but 0.10 to 0.16 % at its other Folds, 0.16 to 0.28 % after, 0.14 to
        // 0.22 % after its fix round); without the aurora in the open dome the gain would grow by its share of the sky's
        // light, 35 to 52 % here
        println!("the ground's gain under a hair of cloud: {a:?}, with the aurora {b:?}");
        assert!((a - b).length() < 5.0e-3, "{a:?} against {b:?} with the aurora");
        let mut deck = thin.clone();
        deck.sky.clouds.coverage = 1.0;
        let (off, on) = (Env::new(&deck), Env::new(&with(&deck)));
        assert!(on.night.as_ref().unwrap().has_aurora() && off.night.as_ref().unwrap().moon_disc().is_none(), "the premise: the aurora draws, no moon");
        let down = vec3f(0.0, -1.0, 0.0);
        let ground = |e: &Env| e.night.as_ref().unwrap().ground(down, Vec3f::all(1.0)) * e.ground_gain;
        let (ground_off, ground_on) = (ground(&off), ground(&on));
        let (lit_off, lit_on) = (off.dome_light().irradiance, on.dome_light().irradiance);
        // the night sums the aurora over the dome's own looks (Task 4c), so to within the glow's two quadratures (0.07 % at
        // most); without the aurora in the open dome the ground would grow by its own share of the aurora's light more
        let channels = |v: Vec3f| [v.x, v.y, v.z];
        let (ground_off, ground_on, lit_off, lit_on) = (channels(ground_off), channels(ground_on), channels(lit_off), channels(lit_on));
        for c in 0..3 {
            let (grew, deck_grew) = (ground_on[c] / ground_off[c], lit_on[c] / lit_off[c]);
            assert!(deck_grew > 1.0 && (grew / deck_grew - 1.0).abs() < 1.0e-2, "channel {c}: the ground under the deck grows x{grew}, the deck's light x{deck_grew}");
        }
    }

    /// The aurora is in the strip's light (D9): its rings turn green toward it, and away from it they are what they were.
    #[test]
    fn the_aurora_reaches_the_strips_rings() {
        let base = super::presets::preset("Starry night").unwrap();
        let mut on = base.clone();
        on.sky.night.aurora = Some(AuroraParams { height_deg: 8.0, ..AuroraParams::default() });
        let (a, b) = (Env::new(&base).skyline_light(), Env::new(&on).skyline_light());
        let north = skyline::RING / 2;
        assert!(b.ring_high[north][1] > a.ring_high[north][1], "north: {:?} against {:?}", b.ring_high[north], a.ring_high[north]);
        assert_eq!(b.ring_high[0], a.ring_high[0], "south");
        assert!(b.ambient_rgb[1] > a.ambient_rgb[1], "the city's ambient gains its light");
    }

    /// Plan hdri-7 §4 ("it shows ... in the wet ground automatically, because those mirror the sky"): the engine's wet
    /// ground mirrors `sky_layers` at (x, -y, z) (plan 6 Task 4's `seen`), so under Wet ground 1.00 (all water) the mirror
    /// of a look 3 degrees into the curtain gains the aurora's light, green: at least the Fresnel term at that depression
    /// (about 0.17 at 18 degrees) times what the aurora adds above.
    #[test]
    fn the_aurora_shows_in_the_wet_ground() {
        let mut base = super::presets::preset("Starry night").unwrap();
        base.sky.weather = Some(WeatherParams { kind: "clear".to_string(), wet_ground: 1.0, ..WeatherParams::default() });
        let mut on = base.clone();
        on.sky.night.aurora = Some(AuroraParams::default());
        let (with, without) = (Env::new(&on), Env::new(&base));
        let middle = with.night_features().unwrap().aurora_middle.expect("the arc draws");
        let (az, el) = az_el_from_dir(middle);
        let up = dir_from_az_el(az, el + 3.0);
        let mirror = vec3f(up.x, -up.y, up.z);
        let (above, below) = (with.radiance(up) - without.radiance(up), with.radiance(mirror) - without.radiance(mirror));
        println!("the aurora 3 degrees into the curtain: {above:?} above, {below:?} in the wet ground");
        assert!(below.y > 0.0 && below.y > below.x && below.y > below.z, "the wet ground gains green: {below:?}");
        let lum = crate::sky::luminance;
        assert!(lum(below) >= 0.15 * lum(above), "a mirror by Fresnel: {} against {} above", lum(below), lum(above));
    }

    #[test]
    #[ignore = "writes PNGs: HDRI_NIGHT_VISUAL_DIR"]
    fn aurora_looks() {
        let base = super::presets::preset("Starry night").unwrap();
        for (name, a) in [
            ("aurora-classic", AuroraParams::default()),
            ("aurora-magenta-tops", AuroraParams { color: "magenta tops".into(), ..AuroraParams::default() }),
            ("aurora-storm", AuroraParams { color: "storm".into(), folds: 1.0, rays: 1.0, ..AuroraParams::default() }),
            ("aurora-high", AuroraParams { height_deg: 45.0, width_deg: 160.0, ..AuroraParams::default() }),
        ] {
            let mut p = base.clone();
            p.sky.night.aurora = Some(a);
            write_look(name, &p);
        }
    }
}
