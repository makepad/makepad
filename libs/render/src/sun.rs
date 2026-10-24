//! The one game light (shiny.md T7, game side).
//!
//! `draw::SceneSun` is the repo's single lighting model, but it speaks map
//! space (x east, y SOUTH, z up) while games are y-up. [`SunLight`] is that
//! same model expressed in game space, and it is the ONLY place the game
//! shaders get their light from — before this, cube/terrain/skinned each
//! carried their own hardcoded ambient/direct split and the sun direction
//! was a per-instance value set in five different script blocks.
//!
//! Axis mapping (map -> game): `x` stays east, map `z` (up) becomes game
//! `y`, map `y` (south) becomes game `z`. So a map dir `(x, y, z)` is a
//! game dir `(x, z, y)`.

use makepad_draw::*;

/// Direct/ambient split of the legacy game shading, kept as the default so
/// unifying the path did not restyle every existing game: the cube shader
/// was `color*0.28 + color*dp*0.72`.
const LEGACY_AMBIENT: f32 = 0.28;
const LEGACY_DIRECT: f32 = 0.72;
/// The stock rig's drop-shadow strength: what the shadow quads and hulls
/// are drawn at with a full key (they scale by `shadow_alpha / 0.35`).
const STOCK_SHADOW_ALPHA: f32 = 0.35;

/// The sun every game shader reads. Values are final multipliers — the
/// shaders apply them directly, they do not rescale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunLight {
    /// Points from the surface TOWARD the sun, normalized, y-up.
    pub dir: Vec3f,
    /// Direct term multiplier (sun tint * intensity).
    pub color: Vec3f,
    /// Hemisphere ambient from above.
    pub sky: Vec3f,
    /// Hemisphere ambient from below (ground bounce).
    pub ground: Vec3f,
    /// How dark cast shadows draw, 0..1.
    pub shadow_alpha: f32,
}

impl Default for SunLight {
    /// The look the game shaders had before unification: the `(0.35, 0.8,
    /// 0.45)` light direction every script block set, a white sun at 0.72
    /// and a flat 0.28 ambient (same value above and below, so the
    /// hemisphere term collapses to the old constant).
    fn default() -> Self {
        Self {
            dir: vec3f(0.35, 0.8, 0.45).normalize(),
            color: vec3f(LEGACY_DIRECT, LEGACY_DIRECT, LEGACY_DIRECT),
            sky: vec3f(LEGACY_AMBIENT, LEGACY_AMBIENT, LEGACY_AMBIENT),
            ground: vec3f(LEGACY_AMBIENT, LEGACY_AMBIENT, LEGACY_AMBIENT),
            shadow_alpha: STOCK_SHADOW_ALPHA,
        }
    }
}

/// What a moonless night leaves behind once the sun is fully down: a dim
/// cool floor, flat over both hemispheres. Low enough that street lamps and
/// emissive windows are the light in town, high enough that geometry still
/// separates from the near-black sky.
const NIGHT_AMBIENT: Vec3f = Vec3f {
    x: 0.10,
    y: 0.11,
    z: 0.15,
};

/// HDR output (see [`SunLight::to_hdr`]): direct-term gain over the legacy
/// multiplier, fill gain, and the share of the fill left at full night.
const HDR_SUN: f32 = 3.0;
const HDR_AMBIENT: f32 = 0.8;
const HDR_NIGHT_FLOOR: f32 = 0.22;
/// Exposure metering (see [`SunLight::hdr_exposure`]): the key a noon rig
/// maps to ~0.8, and the adaptation range (night gains at most ~2 stops).
const HDR_EXPOSURE_KEY: f32 = 0.75;
const HDR_EXPOSURE_MIN: f32 = 0.25;
const HDR_EXPOSURE_MAX: f32 = 3.2;

/// `SceneSun`'s ambient is tuned for the map's bright top-down bake; a game
/// viewed from inside the scene needs it lower or everything reads flat.
const MAP_AMBIENT_TO_GAME: f32 = 0.45;
/// Likewise the direct term: the map bakes at full strength, the game keeps
/// the legacy 0.72 headroom so emissive glow still reads.
const MAP_DIRECT_TO_GAME: f32 = LEGACY_DIRECT;

impl SunLight {
    /// Adopt a map-space [`SceneSun`], converting axes and rebalancing the
    /// map's bake-tuned levels for in-scene viewing.
    pub fn from_scene_sun(sun: &SceneSun) -> Self {
        let d = sun.dir;
        Self {
            dir: vec3f(d.x, d.z, d.y).normalize(),
            color: sun.color * MAP_DIRECT_TO_GAME,
            sky: sun.sky * MAP_AMBIENT_TO_GAME,
            ground: sun.ground * MAP_AMBIENT_TO_GAME,
            shadow_alpha: sun.shadow_alpha,
        }
    }

    /// This rig in the LINEAR, scene-referred units of the HDR output
    /// ([`crate::Renderer::set_hdr_output`]): the direct term becomes an
    /// irradiance about 3x the legacy 0.72 multiplier (a noon white wall sits
    /// near 2.0 before exposure), the dome fill keeps roughly a fifth of it
    /// (the sun-to-shade ratio of a clear day), and after dusk the fill drops
    /// to a dim moonlit floor so lamps and emissive windows carry the scene.
    /// The tone mapper, not a clamp, decides how that reaches the screen.
    pub fn to_hdr(&self) -> Self {
        let elev = self.dir.y.clamp(-1.0, 1.0).asin().to_degrees();
        let day = {
            let x = ((elev + 8.0) / 14.0).clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        let fill = HDR_AMBIENT * (HDR_NIGHT_FLOOR + (1.0 - HDR_NIGHT_FLOOR) * day);
        Self {
            dir: self.dir,
            color: self.color * HDR_SUN,
            sky: self.sky * fill,
            ground: self.ground * fill,
            shadow_alpha: self.shadow_alpha,
        }
    }

    /// Metered exposure for an HDR rig (the output of [`Self::to_hdr`]):
    /// the scene key is the fill plus half the sun on an up-facing surface,
    /// and exposure maps that key to mid-tone. Night is allowed two stops of
    /// adaptation over noon and no more, so it still reads as night while
    /// lamps and windows gain their pools.
    pub fn hdr_exposure(&self) -> f32 {
        let lum = |c: Vec3f| c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722;
        let key = (lum(self.sky) + lum(self.ground)) * 0.5 + lum(self.color) * self.dir.y.max(0.0) * 0.5;
        hdr_exposure_for_key(key)
    }

    /// Map-space view of this sun, for anything that wants the shared type.
    pub fn to_scene_sun(&self) -> SceneSun {
        let d = self.dir;
        SceneSun {
            dir: vec3f(d.x, d.z, d.y).normalize(),
            color: self.color / MAP_DIRECT_TO_GAME,
            sky: self.sky / MAP_AMBIENT_TO_GAME,
            ground: self.ground / MAP_AMBIENT_TO_GAME,
            shadow_alpha: self.shadow_alpha,
        }
    }

    /// The rig for `hours` (0..24) — the shared solar model, so the game and
    /// the map agree on where the sun is, but taken at its TRUE elevation:
    /// `SceneSun` clamps itself to a permanent daylight rig (its bake has no
    /// night), which is exactly what made a game's midnight render as a
    /// golden hour with the sun stuck 4.6 degrees up. Below the horizon this
    /// hands back the night rig instead, and the direction keeps sinking —
    /// which is what lights the analytic sky's night blend and its stars.
    pub fn from_time_of_day(hours: f32, latitude_deg: f32) -> Self {
        Self::from_time_of_day_balanced(hours, latitude_deg, None)
    }

    /// [`Self::from_time_of_day`] with the daylight split re-aimed at a
    /// given disc-to-dome luminance ratio — see
    /// [`makepad_scene::SunConfig::daylight_balance`]. `None` is the
    /// stock rig, unchanged in every bit.
    pub fn from_time_of_day_balanced(
        hours: f32,
        latitude_deg: f32,
        daylight_balance: Option<f32>,
    ) -> Self {
        let dir = solar_dir(hours, latitude_deg);
        let mut sun = Self::from_scene_sun(&SceneSun::from_time_of_day(hours, latitude_deg));
        sun.dir = dir;
        // BEFORE the night ramp: the balance is a property of DAYLIGHT. After
        // it, the same call would be scaling a twilight floor that has
        // nothing to do with a clear sky, and would drive the ambient of a
        // moonless midnight toward zero.
        if let Some(ratio) = daylight_balance {
            sun.rebalance_daylight(ratio);
        }
        sun.apply_night(dir.y.clamp(-1.0, 1.0).asin().to_degrees());
        sun
    }

    /// The daylight rig aimed AND styled by an explicit sun direction —
    /// the resolve path for a host that computes the true solar position
    /// itself (fab's NOAA model) and hands the engine `time_of_day` + `dir`.
    ///
    /// [`Self::from_time_of_day_balanced`] keys its warmth and night ramps
    /// off the engine's own fixed-declination sun, which at a real site on
    /// a real date can sit tens of degrees from the true one — far enough
    /// that the rig called "night" while the sky still painted a golden
    /// sun well above the horizon. Here every elevation-dependent term
    /// follows the DIRECTION the sky, the disc, the shadows and the
    /// shading all share:
    ///
    /// 1. the clear-sky split (`daylight_balance`) lands on the
    ///    full-daylight levels — it is a property of DAYLIGHT;
    /// 2. the direct term takes the sun's own colour: the sky model's
    ///    transmittance at this height ([`crate::sky::sun_transmittance`],
    ///    the disc/glow curve — not a second reddening ramp);
    /// 3. the dome fill dims as the sun sinks (the same fill curve the
    ///    hour rig uses), then rides the shared twilight ramp down to the
    ///    night floor — all at the TRUE elevation;
    /// 4. the direct term does NOT take that twilight ramp: its fade is the
    ///    transmittance itself, gated only where the disc drops below the
    ///    horizon — running both would double-count the sunset.
    pub fn from_direction_balanced(dir: Vec3f, daylight_balance: Option<f32>) -> Self {
        let dir = dir.normalize();
        let elev = dir.y.clamp(-1.0, 1.0).asin();
        // Full-daylight base: SceneSun's stock levels (the same numbers the
        // hour rig starts from, before its warmth curve), converted with
        // the same map->game factors.
        let base = SceneSun::default();
        let mut sun = Self {
            dir,
            color: base.color * MAP_DIRECT_TO_GAME,
            sky: base.sky * MAP_AMBIENT_TO_GAME,
            ground: base.ground * MAP_AMBIENT_TO_GAME,
            shadow_alpha: 0.16,
        };
        if let Some(ratio) = daylight_balance {
            sun.rebalance_daylight(ratio);
        }
        // The transmittance IS the sunset fade for the DIRECT term: at one
        // degree it is already down to a tenth of noon and deep gold, the
        // way a facade four minutes before sunset really looks. Running the
        // hour rig's +10..-3 degree twilight ramp on top would double-count
        // that fade and kill the glow the sun study exists to show — so the
        // direct term only GATES at the horizon, where the disc itself
        // slips away.
        let elev_deg = elev.to_degrees();
        let disc = {
            let x = ((elev_deg + 2.0) / 2.5).clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        sun.color = sun.color * crate::sky::sun_transmittance(dir.y) * disc;
        sun.shadow_alpha *= disc;
        // The dome fill keeps the shared twilight: dimming toward the
        // horizon, then the same ramp down to the night floor the hour rig
        // uses ([`Self::apply_night`]'s ambient half).
        let sky_dim = 0.7 + 0.3 * (elev / 0.9).clamp(0.0, 1.0);
        let s = {
            let x = ((elev_deg + 3.0) / 13.0).clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        let mix = |a: Vec3f, b: Vec3f, k: f32| a + (b - a) * k;
        sun.sky = mix(NIGHT_AMBIENT, sun.sky * sky_dim, s);
        sun.ground = mix(NIGHT_AMBIENT, sun.ground * sky_dim, s);
        sun
    }

    /// Move light between the disc and the dome WITHOUT changing how much
    /// there is: `direct + fill` keeps its luminance, so a fully lit white
    /// surface still lands where it did and nothing starts clipping. Only
    /// the split moves, and with it the depth of every shadow.
    ///
    /// Hue is preserved on both halves; only their strength changes. The
    /// hemisphere's own shape survives too — the ground bounce is scaled by
    /// the same factor as the sky, so an underside keeps its relationship
    /// to the sky above it.
    fn rebalance_daylight(&mut self, direct_over_fill: f32) {
        let ratio = direct_over_fill.max(0.0);
        let direct = crate::sky::luminance(self.color);
        let fill = crate::sky::luminance(self.sky);
        if direct <= 1.0e-6 || fill <= 1.0e-6 {
            return;
        }
        let total = direct + fill;
        let want_fill = total / (1.0 + ratio);
        let want_direct = total - want_fill;
        self.color = self.color * (want_direct / direct);
        let fill_scale = want_fill / fill;
        self.sky = self.sky * fill_scale;
        self.ground = self.ground * fill_scale;
    }

    /// Fade this daylight rig toward night by the sun's true elevation.
    /// One ramp: 0 below -3 degrees, 1 above 10, smooth between — the direct
    /// term and the cast shadow go out with the sun while the ambient sinks
    /// to a dim cool floor, so a town at midnight is carried by its lamps.
    fn apply_night(&mut self, elev_deg: f32) {
        let s = {
            let x = ((elev_deg + 3.0) / 13.0).clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        let mix = |a: Vec3f, b: Vec3f, k: f32| a + (b - a) * k;
        self.color = self.color * s;
        self.sky = mix(NIGHT_AMBIENT, self.sky, s);
        self.ground = mix(NIGHT_AMBIENT, self.ground, s);
        self.shadow_alpha *= s;
    }

    /// The single write path into a shader's sun fields. Every game shader
    /// goes through this, which is what makes "one sun" a compiler-enforced
    /// property rather than a convention.
    pub fn write_into(
        &self,
        light_dir: &mut Vec3f,
        color: &mut Vec3f,
        sky: &mut Vec3f,
        ground: &mut Vec3f,
    ) {
        *light_dir = self.dir;
        *color = self.color;
        *sky = self.sky;
        *ground = self.ground;
    }

    /// The single write path into a shader's sun UNIFORMS, for shaders whose
    /// batches are large enough that carrying the sun per instance is pure
    /// duplication (the cube family). Same values as [`Self::write_into`],
    /// different destination — so "one sun" still holds.
    pub fn write_uniforms(&self, cx: &Cx, vars: &mut DrawVars) {
        vars.set_uniform(cx, live_id!(sun_color), &[self.color.x, self.color.y, self.color.z]);
        vars.set_uniform(cx, live_id!(sun_sky), &[self.sky.x, self.sky.y, self.sky.z]);
        vars.set_uniform(cx, live_id!(sun_ground), &[self.ground.x, self.ground.y, self.ground.z]);
    }

    /// Horizontal (ground-plane) part of the sun direction, unit length.
    /// This is the direction a shadow is cast *away* from.
    pub fn dir_ground(&self) -> Vec2f {
        let d = vec2f(self.dir.x, self.dir.z);
        let len = (d.x * d.x + d.y * d.y).sqrt();
        if len < 1e-6 {
            // Sun overhead: no meaningful ground direction. Callers that
            // care (shadow.rs) special-case this; +x keeps it deterministic.
            vec2f(1.0, 0.0)
        } else {
            vec2f(d.x / len, d.y / len)
        }
    }

    /// Ground shadow offset per unit of caster height. Clamped so a sun at
    /// the horizon does not throw a shadow across the whole level.
    pub fn shadow_len_per_unit(&self) -> f32 {
        let h = (self.dir.x * self.dir.x + self.dir.z * self.dir.z).sqrt();
        (h / self.dir.y.max(0.05)).min(4.0)
    }
}

/// The sun's TRUE game-space direction (y up) for a local solar hour: the
/// shared solar model of [`makepad_draw::solar_dir`], axis-mapped, and NOT
/// clamped at the horizon — a game has a night to sink into.
pub fn solar_dir(hours: f32, latitude_deg: f32) -> Vec3f {
    let d = makepad_draw::solar_dir(hours, latitude_deg);
    vec3f(d.x, d.z, d.y).normalize()
}

/// The celestial pole (game space, y up) at `latitude_deg`: the axis the
/// whole sky — sun and stars alike — turns around. Due north, raised by the
/// latitude. Game `-z` is north (map y is south).
pub fn celestial_pole(latitude_deg: f32) -> Vec3f {
    let lat = latitude_deg.to_radians();
    vec3f(0.0, lat.sin(), -lat.cos())
}

/// World direction -> star-map direction for a local solar hour, as three
/// matrix rows (the sky shader's `star_r0..2`).
///
/// The rows ARE the celestial basis written in world coordinates: row 1 is
/// the pole, so the panorama's dec +90 lands on the true pole, and rows 0/2
/// spin around it with the hour angle. That makes one invariant hold, and
/// `stars_hold_still_around_the_sun` asserts it: the SUN's coordinates in
/// this frame do not move all day. Sun and stars ride one celestial sphere —
/// which is the whole reason the night sky wheels while the town sleeps.
pub fn celestial_rows(hours: f32, latitude_deg: f32) -> [Vec4f; 3] {
    let pole = celestial_pole(latitude_deg);
    // A reference direction in the celestial equator: the meridian point,
    // i.e. straight up with the pole's share removed. At the poles up IS the
    // axis, so fall back to north — any equator direction will do there.
    let up = vec3f(0.0, 1.0, 0.0);
    let along = up.y * pole.y;
    let mut u = vec3f(-pole.x * along, up.y - pole.y * along, -pole.z * along);
    if u.x * u.x + u.y * u.y + u.z * u.z < 1.0e-6 {
        u = vec3f(0.0, 0.0, -1.0);
        let a = u.z * pole.z;
        u = vec3f(-pole.x * a, -pole.y * a, u.z - pole.z * a);
    }
    let u = u.normalize();
    // w completes a right-handed (u, pole, w) frame.
    let w = vec3f(
        pole.y * u.z - pole.z * u.y,
        pole.z * u.x - pole.x * u.z,
        pole.x * u.y - pole.y * u.x,
    );
    let h = ((hours - 12.0) * 15.0).to_radians();
    let (c, s) = (h.cos(), h.sin());
    // Rotate the equator axes BACKWARD by the hour angle: the sun advances
    // west by h, so a frame that advances with it keeps the sun still.
    let x = vec3f(u.x * c - w.x * s, u.y * c - w.y * s, u.z * c - w.z * s);
    let z = vec3f(u.x * s + w.x * c, u.y * s + w.y * c, u.z * s + w.z * c);
    [
        vec4f(x.x, x.y, x.z, 1.0),
        vec4f(pole.x, pole.y, pole.z, 0.0),
        vec4f(z.x, z.y, z.z, 0.0),
    ]
}

/// Equatorial -> galactic (J2000), in the sky shader's axis order: `y` is
/// the pole of each frame, `x` and `z` span its equator.
const EQ_TO_GALACTIC: [[f32; 3]; 3] = [
    [-0.054_875_56, -0.483_835_02, -0.873_437_1],
    [-0.867_666_1, 0.455_983_78, -0.198_076_37],
    [0.494_109_43, 0.746_982_24, -0.444_829_63],
];

/// World direction -> star PANORAMA direction for a local solar hour: the
/// [`celestial_rows`] turned into the galactic frame the panorama is drawn
/// in (the Milky Way along its equator). Without this turn the band lay
/// along the celestial equator; with it the Milky Way crosses the sky at
/// its true 63 degrees to the equator, and still wheels with the clock.
pub fn star_rows(hours: f32, latitude_deg: f32) -> [Vec4f; 3] {
    let r = celestial_rows(hours, latitude_deg);
    let row = |g: [f32; 3], w: f32| {
        let v = r[0] * g[0] + r[1] * g[1] + r[2] * g[2];
        vec4f(v.x, v.y, v.z, w)
    };
    [
        row(EQ_TO_GALACTIC[0], r[0].w),
        row(EQ_TO_GALACTIC[1], 0.0),
        row(EQ_TO_GALACTIC[2], 0.0),
    ]
}

/// Resolved from the sim's [`makepad_scene::SunConfig`], which stores
/// only what script asked for (the sim cannot depend on `makepad_draw`).
///
/// `time_of_day` alone picks the rig — day, twilight or night — from the
/// engine's own solar model. An explicit `dir` WITHOUT an hour moves the
/// sun but keeps the default daylight rig, because a script that authors
/// only a direction is authoring a LOOK (the village's 38-degree sun is a
/// set dressing choice, not a time). A host that supplies BOTH is saying
/// "this is the TRUE sun for that hour" (fab's NOAA position): then the
/// whole rig — warmth, twilight, night — follows the direction, so the
/// light on the walls and the glow in the sky are one sun
/// ([`SunLight::from_direction_balanced`]). The overrides that follow are
/// exactly that: overrides.
pub fn resolve_sun(cfg: &makepad_scene::SunConfig) -> SunLight {
    let explicit_dir = cfg
        .dir
        .filter(|d| d.x != 0.0 || d.y != 0.0 || d.z != 0.0);
    let mut sun = match (cfg.time_of_day, explicit_dir) {
        (Some(_), Some(dir)) => {
            SunLight::from_direction_balanced(dir, cfg.daylight_balance)
        }
        (Some(hours), None) => {
            SunLight::from_time_of_day_balanced(hours, cfg.latitude, cfg.daylight_balance)
        }
        (None, dir) => {
            let mut sun = SunLight::default();
            if let Some(ratio) = cfg.daylight_balance {
                sun.rebalance_daylight(ratio);
            }
            if let Some(dir) = dir {
                sun.dir = dir.normalize();
            }
            sun
        }
    };
    if let Some(c) = cfg.color {
        sun.color = c;
    }
    if let Some(a) = cfg.ambient {
        sun.sky = a;
        sun.ground = a;
    }
    if let Some(s) = cfg.shadow_alpha {
        sun.shadow_alpha = s.clamp(0.0, 1.0);
    }
    sun
}

/// The exposure that maps a scene key to mid-tone, in the band
/// [`SunLight::hdr_exposure`] adapts within. One function so the rig's
/// meter and an environment's meter agree on the constants. A key of zero
/// (or below the floor, or NaN) meters as the floor's: the ceiling of the
/// band, never infinity; an infinite key is the band's floor.
pub fn hdr_exposure_for_key(key: f32) -> f32 {
    (HDR_EXPOSURE_KEY / key.max(1.0e-4)).clamp(HDR_EXPOSURE_MIN, HDR_EXPOSURE_MAX)
}

/// What a prepared environment lends the rig this frame. The renderer fills
/// it from the IBL preparation it currently draws with (renderer/ibl.rs);
/// tests fill it from `ibl::sh9` and `hdri::image::mean_luminance` of a
/// baked map. `sh` and `mean_luminance` are UNSCALED map radiance; `gain`
/// is the one factor the lane applies to the map, `Ibl.intensity`: the
/// environment is shown and lights at its own scale × its intensity
/// everywhere (the dome, `mat_ibl_*`, this rig, the fog), so the light on a
/// wall, the sky behind it and a mirror's reflection of that sky come from
/// the same numbers. `HDR_SKY_GAIN` is the analytic sky's and never enters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvLighting {
    /// `ibl::sh9` of the map's lighting copy (the sun's cone filled, the
    /// directional light carries it): cosine-convolved irradiance
    /// coefficients, `E(n) = Σ c_i Y_i(n)`; a constant map L gives E = πL.
    pub sh: [[f32; 3]; 9],
    /// Solid-angle weighted mean luminance of the map INCLUDING its
    /// declared sun (Rec.709): the exposure meter's key before the gain.
    /// The renderer adds the sun's share (`L·(1 − cos r)/2 × facing`) to the
    /// lighting copy's mean, because the stock meter counts the sun too.
    pub mean_luminance: f32,
    /// The lane's gain on the map's radiance: `Ibl.intensity`.
    pub gain: f32,
}

/// The values the ENVIRONMENT supplies to the rig are rounded to these
/// grids before they leave: a map re-baked on the day cycle moves its
/// detected sun by a fraction of a degree, and every consumer that keys on
/// the sun (the OnChange lightmap bake, the SDF sidecar era at 1e-3 in
/// `shadow_len_per_unit`) would churn on the noise. 1/512 in a unit vector
/// is about 0.1 degrees; 1/4096 keeps a moonlit fill at 0.005 smooth.
/// Authored `SunConfig` values are never rounded.
const RIG_DIR_STEP: f32 = 1.0 / 512.0;
const RIG_COLOR_STEP: f32 = 1.0 / 4096.0;

/// The direction of the environment's sun in world space, when the world
/// lets the environment place the sun. `Environment.sun` is in the map's
/// own frame (as `hdri::envmap::bake_env_map` / `detect_sun` report it)
/// and `Ibl.rotation_deg` turns the map, so the sun turns with it (ibl's
/// sign: +90 degrees takes -Z to -X). `None` when the world has no
/// environment sun, or when the script authored `SunConfig.dir` — a look,
/// the true sun for its hour (`resolve_sun`), or a host's own eased clock
/// (the sandbox's running day cycle over a re-baked map, which authors the
/// eased direction so the map's bake-grid steps, a quarter hour or 7.5
/// minutes while the sun is low, never steer its shadows),
/// none of which is the map's to override. A `Light::Sun` is the caller's
/// business (`world_sun_dir` first).
pub fn env_sun_dir(world: &makepad_scene::World) -> Option<Vec3f> {
    env_sun_dir_with(world, world.environment.sun)
}

/// [`env_sun_dir`] for a sun the caller supplies in place of the world's
/// `Environment.sun`: the renderer passes the key its bound preparation was
/// made with (the sun the world declared then, else a procedural hdri
/// preset's baked sun), so light and sky come from one preparation. Same
/// frame (the map's), same rules.
pub fn env_sun_dir_with(world: &makepad_scene::World, env_sun: Option<makepad_scene::EnvSun>) -> Option<Vec3f> {
    let authored = world.sun.dir.is_some_and(|d| d.x != 0.0 || d.y != 0.0 || d.z != 0.0);
    if authored {
        return None;
    }
    let ibl = world.environment.ibl.filter(|i| i.rotation_deg.is_finite())?;
    let sun = env_sun?;
    // `EnvSun::validate` accepts any direction of length 1e-3 or more, so the
    // turned direction is normalised here before it steers a light.
    let dir = crate::hdri::rotate_y(sun.dir, ibl.rotation_deg);
    (dir.is_finite() && dir.length() > 1.0e-6).then(|| dir.normalize())
}

/// The environment's own sun in world space for the "is it day?" switches
/// (N1: the lamps' photocell, a streamed city's night factor, the analytic
/// sky): `daylight` (`Environment.daylight_sun`, in the map's frame, as the
/// renderer's bound preparation holds it) turned with the map by
/// `Ibl.rotation_deg` and normalised. An authored `SunConfig.dir` does not
/// stop it, as it stops the key's direction: that direction aims a light (a
/// look, or a host's own clock), while this says whether the environment
/// shows day or night, and a moonlit map is night whatever aims its moon.
/// (Where the analytic sky draws its sun when the key IS the sun is the
/// light's business: [`env_key_is_its_sun`].) `None` when the world names no
/// IBL or the map knows no sun of its own: the switches then follow the
/// world's own sun.
pub fn env_daylight_dir_with(world: &makepad_scene::World, daylight: Option<Vec3f>) -> Option<Vec3f> {
    let ibl = world.environment.ibl.filter(|i| i.rotation_deg.is_finite())?;
    let dir = crate::hdri::rotate_y(daylight?, ibl.rotation_deg);
    (dir.is_finite() && dir.length() > 1.0e-6).then(|| dir.normalize())
}

/// How close to the map's own sun a key must point to BE that sun
/// ([`env_key_is_its_sun`]). A generated sun key and the sky's report are
/// one direction (`hdri::Env::sun` and `sun_dir`), while a moon keys only
/// once the sun is 6 degrees down and the moon is up (hdri/night.rs
/// `moon_key`), so it is never this close.
const KEY_IS_SUN_DEG: f32 = 1.0;

/// Whether the environment's key (`key`, as the bound preparation holds it)
/// is its own sun (`daylight`, `Environment.daylight_sun`), both in the
/// map's frame: then the light IS the sun, and wherever it is aimed (the
/// key's own direction, or an authored `SunConfig.dir`: a look, or a host's
/// eased clock while the map's report steps on its bake grid) is where the
/// analytic sky draws the sun, as for a world's own Sun, so the disc sits
/// where the shadows come from. A moon key, no key, or a map that reports
/// no sun of its own: `false`, and the sky follows the report (N1).
pub fn env_key_is_its_sun(key: Option<makepad_scene::EnvSun>, daylight: Option<Vec3f>) -> bool {
    let (Some(key), Some(sun)) = (key, daylight) else { return false };
    // NaN or a zero direction compares false.
    key.dir.normalize().dot(sun.normalize()) >= KEY_IS_SUN_DEG.to_radians().cos()
}

/// The rig with the environment's light folded in, in the lane's units:
/// call it on the HDR rig (after `to_hdr` and `hdr_fill_from_sky`) under
/// HDR output and on the plain rig otherwise, and BEFORE
/// `world_lights::apply_world_sun`, so a `Light::Sun` / `Light::Sky` still
/// has the last word. Without a prepared environment (`env` None) or a
/// world that names none, the rig comes back bit for bit — the legacy look
/// stays pinned by `the_default_sun_is_the_legacy_look`.
///
/// Units. A disc of radiance L and angular radius r delivers `E = L·Ω`,
/// `Ω = 2π(1 − cos r)`, to a surface facing it; the lanes light with
/// `color · N·L` in irradiance/π, so `color = L · 2(1 − cos r) · gain ·
/// facing`. `L` is `EnvSun.radiance`, the AVERAGE radiance over the cone
/// (C1's doc), so `L·Ω` is the key's whole emission for a generated sun, the
/// moon, a studio key and a detected sun alike; a studio key's is its
/// integral, not its peak colour, so a thin strip or a ring lights with what
/// it draws. A WIDE key delivers only part of that emission to a surface
/// facing its centre (the cosine across its disc: 0.78 for the Overcast dome
/// preset, 0.93 for the Top softbox): `EnvSun.facing` is that share, and the
/// one directional light, which the lanes shade at N·L = 1 there, carries
/// exactly it. The hemisphere terms are the SH irradiance at ±Y over π —
/// what `mat_ibl_ambient` returns — times the lane's scale
/// ([`env_lane_scale`]: 1 under HDR output, the map's exposure in the legacy
/// lane). The lanes that take the IBL lookups multiply the same scale into
/// both lookups (`ibl_ctl.y`: the stock IBL program, the skin lane, the IBL
/// item materials; renderer/stock_ibl.rs), so a stock cube and an IBL
/// material under one map agree in both lanes. `mat_ibl_*` themselves return
/// the map at its own scale × `Ibl.intensity` in every lane.
///
/// An environment WITHOUT a sun (overcast, a studio without a key light, a
/// sun that has set and a moon that has not risen, a zero key) has no direct
/// term: all its light is in the fill, never the analytic rig's colour, so a
/// sunset fades out instead of switching to a stock sun. (The direction is
/// then the caller's: `resolve_sun` keeps the cascades stable.) The legacy
/// lane has no composite to apply a meter, so the map's exposure is baked
/// into the values the environment supplies here, and since it has no tone
/// mapper either, the direct term is held to what the brighter fill leaves
/// under white ([`legacy_direct_within_white`]).
///
/// The drop shadows go with the key the environment supplies: unless the
/// script authored a strength, `shadow_alpha` is the stock strength times the
/// key's share of the light ([`env_shadow_alpha`]), none without a key.
///
/// Only the fields the environment supplies are quantised (RIG_*_STEP) and,
/// in the legacy lane, exposed; an authored `SunConfig` dir, colour, ambient
/// or shadow strength passes through bit for bit, in both lanes.
pub fn env_sun_rig(world: &makepad_scene::World, env: Option<&EnvLighting>, sun: SunLight, hdr: bool) -> SunLight {
    env_sun_rig_with(world, world.environment.sun, env, sun, hdr)
}

/// [`env_sun_rig`] for a sun the caller supplies in place of the world's
/// `Environment.sun` (see [`env_sun_dir_with`]).
pub fn env_sun_rig_with(
    world: &makepad_scene::World,
    env_sun: Option<makepad_scene::EnvSun>,
    env: Option<&EnvLighting>,
    sun: SunLight,
    hdr: bool,
) -> SunLight {
    let Some(env) = env else { return sun };
    if world.environment.ibl.filter(|i| i.intensity.is_finite() && i.rotation_deg.is_finite()).is_none() {
        return sun;
    }
    let exposure = env_lane_scale(Some(env), hdr);
    let gain = if env.gain.is_finite() { env.gain.max(0.0) } else { 0.0 };
    // Non-finite and negative values read as 0; everything else lands on
    // the colour grid.
    let q = |v: f32, step: f32| if v.is_finite() { (v.max(0.0) / step).round() * step } else { 0.0 };
    let qc = |v: Vec3f| vec3f(q(v.x, RIG_COLOR_STEP), q(v.y, RIG_COLOR_STEP), q(v.z, RIG_COLOR_STEP));
    let mut out = sun;
    if let Some(dir) = env_sun_dir_with(world, env_sun) {
        let r = |v: f32| (v / RIG_DIR_STEP).round() * RIG_DIR_STEP;
        let d = vec3f(r(dir.x), r(dir.y), r(dir.z));
        if d.is_finite() && d.length() > 1.0e-6 {
            out.dir = d.normalize();
        }
    }
    if world.sun.ambient.is_none() {
        let fill = |n: [f32; 3]| {
            let c = makepad_render_material::ibl::sh9_irradiance(&env.sh, n);
            qc(vec3f(c[0], c[1], c[2]) * (gain * exposure / std::f32::consts::PI))
        };
        out.sky = fill([0.0, 1.0, 0.0]);
        out.ground = fill([0.0, -1.0, 0.0]);
    }
    // After the fill: the legacy lane limits the direct term by the fill the
    // frame will actually show (an authored ambient included), the brighter
    // hemisphere per channel.
    if world.sun.color.is_none() {
        let direct = match env_sun {
            Some(s) => s.radiance * (env_sun_scale(&s) * gain * exposure),
            None => Vec3f::default(),
        };
        let fill = vec3f(out.sky.x.max(out.ground.x), out.sky.y.max(out.ground.y), out.sky.z.max(out.ground.z));
        out.color = qc(if hdr { direct } else { legacy_direct_within_white(direct, fill) });
        // The key is the environment's, and so are its drop shadows.
        if world.sun.shadow_alpha.is_none() {
            out.shadow_alpha = env_shadow_alpha(out.color, out.sky);
        }
    }
    out
}

/// N2: the drop shadows' strength under a key the environment supplies (the
/// character quads, the blob fallback and the caster hulls draw at
/// `shadow_alpha` alone): the stock strength times the key's share of the
/// light on the ground they fall on, `direct / (direct + sky fill)` in
/// luminance, in the lane's own values. A key that carries all the light
/// casts at the stock strength, a dim one (a moon under its own airglow, a
/// sun sinking into the fill) faintly, no key or a zero key not at all. Not
/// the rig's own strength: that one is faded by the WORLD's analytic clock,
/// which says nothing about the map (a moonlit map under a world at
/// midnight still has its key).
fn env_shadow_alpha(direct: Vec3f, sky: Vec3f) -> f32 {
    let (d, s) = (crate::sky::luminance(direct), crate::sky::luminance(sky).max(0.0));
    if !(d > 0.0) || !(d + s).is_finite() {
        return 0.0;
    }
    STOCK_SHADOW_ALPHA * (d / (d + s))
}

/// What the stock legacy rig gives a white surface that faces the sun, per
/// channel: `LEGACY_DIRECT + LEGACY_AMBIENT`. The legacy (display-referred)
/// lane has no tone mapper, so nothing lit may exceed it: it is white.
const LEGACY_WHITE: f32 = LEGACY_DIRECT + LEGACY_AMBIENT;

/// The environment's direct term for the legacy lane: `direct` (already
/// exposed) scaled, hue kept, so that a white surface facing the sun reads
/// at most white, `direct + fill <= LEGACY_WHITE` in every channel, where
/// `fill` is the brighter of the sky and the ground fill per channel: a wall
/// facing a low sun is lit by both hemispheres, and a sunlit ground (a
/// params noon's (0.375, 0.339, 0.281) under a sky of (0.069, 0.130, 0.243))
/// can be the brighter one. The HDR lane has a tone mapper and does not call
/// this; neither does a rig that already fits (its values come back
/// unchanged, so a dim or overcast map is what the meter alone gave it).
///
/// The meter counts a sun at a quarter of its weight (renderer/env_sun.rs),
/// right for a lane that tone maps and wrong for one that clips: the golden
/// hour preset's direct term sat at (3.4, 1.8, 0.6) and drew every sunlit
/// surface as saturated yellow (seen on a Windows capture). The fill is not
/// limited: it stays the map's own light at the metered exposure and shades
/// everything the sun does not reach. A sunny map's legacy rig is therefore
/// a sun and a fill that add up to white, as the stock rig's 0.72 + 0.28 do,
/// in the map's colours; the map's own sun-to-shade ratio is more than a
/// display with no tone curve can show.
fn legacy_direct_within_white(direct: Vec3f, fill: Vec3f) -> Vec3f {
    let mut scale = 1.0f32;
    for (d, s) in [(direct.x, fill.x), (direct.y, fill.y), (direct.z, fill.z)] {
        if d.is_finite() && d > 0.0 {
            // A fill that is white already leaves no room for the sun.
            let room = (LEGACY_WHITE - if s.is_finite() { s.max(0.0) } else { 0.0 }).max(0.0);
            scale = scale.min(room / d);
        }
    }
    direct * scale
}

/// What a key's radiance is multiplied by to give the directional light's
/// colour before the lane's gain and exposure: `2(1 − cos r) × facing`
/// (`E/π` of the key's emission that a surface facing its centre receives).
/// A key whose cone or share is not finite delivers nothing.
pub(crate) fn env_sun_scale(sun: &makepad_scene::EnvSun) -> f32 {
    if !sun.cos_radius.is_finite() || !sun.facing.is_finite() {
        return 0.0;
    }
    2.0 * (1.0 - sun.cos_radius.clamp(-1.0, 1.0)) * sun.facing.clamp(0.0, 1.0)
}

/// The exposure an environment meters for itself: its mean luminance in
/// the lane's units is the key (a white wall under a uniform map of
/// radiance L reads L, E/π = L), mapped to mid-tone within the rig's band,
/// so the dome, the fill and the sun it lights with land together.
pub fn env_exposure(env: &EnvLighting) -> f32 {
    let mean = if env.mean_luminance.is_finite() { env.mean_luminance.max(0.0) } else { 0.0 };
    let gain = if env.gain.is_finite() { env.gain.max(0.0) } else { 0.0 };
    hdr_exposure_for_key(mean * gain)
}

/// The scale a lane puts on everything it takes from an environment's map:
/// 1 under HDR output (the composite exposes the frame, map and all), the
/// map's own exposure ([`env_exposure`]) in the legacy lane, which has no
/// composite. [`env_sun_rig_with`] multiplies it into the fill and the key
/// it takes from the map, and the lanes that take the IBL lookups carry it
/// as `ibl_ctl.y` (renderer/stock_ibl.rs), so the hemisphere on a matte wall
/// and the lookups on the chrome prop beside it are exposed alike. Without
/// an environment's lighting (`env` None: the world names none, or nothing
/// is prepared) there is nothing to expose: 1.
pub fn env_lane_scale(env: Option<&EnvLighting>, hdr: bool) -> f32 {
    match env {
        Some(env) if !hdr => env_exposure(env),
        _ => 1.0,
    }
}

/// Whether the world authors the hemisphere fill itself: a script's
/// `SunConfig.ambient`, which [`env_sun_rig_with`] passes through, or a
/// world's own `Light::Sky`, which `world_lights::apply_world_sun` puts in
/// the rig's place. The stock lanes that take an environment's IBL lookups
/// then keep that fill and take only the reflection from the map
/// (`ibl_ctl.z`, renderer/stock_ibl.rs): an authored value passes through
/// there as it does in the rig.
pub fn fill_is_authored(world: &makepad_scene::World) -> bool {
    world.sun.ambient.is_some()
        || world.lights.iter().any(|l| matches!(l, makepad_scene::Light::Sky { .. }) && l.validate().is_ok())
}

/// The CPU twin of the legacy (display-referred) lane's environment dome
/// (renderer/ibl.rs, `DrawEnvBackground`'s legacy branch, itself the
/// analytic dome's steps from shaders/sky_dome.rs): Reinhard on the
/// LUMINANCE at `exposure`, normalised by the largest channel when that
/// exceeds 1, gamma 1/2.2. A fog colour run through it matches the dome
/// drawn behind it. Negative and non-finite input reads as black.
pub fn legacy_dome_rgb(c: Vec3f, exposure: f32) -> Vec3f {
    let clean = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let c = vec3f(clean(c.x), clean(c.y), clean(c.z));
    let lum = c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722;
    let yt = lum * clean(exposure);
    let reinhard = if yt.is_finite() { yt / (1.0 + yt) } else { 1.0 };
    let l = c * (reinhard / lum.max(1.0e-6));
    let m = l.x.max(l.y).max(l.z).max(1.0);
    vec3f((l.x / m).powf(0.4545454), (l.y / m).powf(0.4545454), (l.z / m).powf(0.4545454))
}

/// The fog colour a `Fog::Host` world takes from its environment under HDR
/// output: the map's horizon band, linear × the lane's gain (`Ibl.intensity`:
/// the environment's own scale, as the dome draws it; never HDR_SKY_GAIN),
/// and the composite exposes it together with the dome. The legacy lane
/// goes through [`legacy_dome_rgb`] at the dome's exposure instead
/// (renderer/env_sun.rs).
pub fn env_fog_color(horizon_rgb: Vec3f, env: &EnvLighting) -> Vec3f {
    let clean = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let gain = if env.gain.is_finite() { env.gain.max(0.0) } else { 0.0 };
    vec3f(clean(horizon_rgb.x), clean(horizon_rgb.y), clean(horizon_rgb.z)) * gain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    /// The clear-sky rig: same total light, a much harder key.
    #[test]
    fn the_daylight_balance_moves_light_without_making_more_of_it() {
        let stock = SunLight::from_time_of_day(13.0, 47.6);
        let clear = SunLight::from_time_of_day_balanced(13.0, 47.6, Some(9.0));
        let lum = crate::sky::luminance;
        // Same light, differently spent.
        assert!(
            (lum(stock.color) + lum(stock.sky) - lum(clear.color) - lum(clear.sky)).abs()
                < 1.0e-4,
            "stock {:?}/{:?} vs clear {:?}/{:?}",
            stock.color,
            stock.sky,
            clear.color,
            clear.sky
        );
        let ratio = lum(clear.color) / lum(clear.sky);
        assert!((ratio - 9.0).abs() < 0.05, "clear-sky ratio {ratio}");
        assert!(
            lum(stock.color) / lum(stock.sky) < 4.0,
            "the stock rig was already a clear sky"
        );
        // A harder sun and a deeper shadow, both.
        assert!(lum(clear.color) > lum(stock.color));
        assert!(lum(clear.sky) < lum(stock.sky));
        // Hue survives: only the strengths moved.
        let hue = |c: Vec3f| c * (1.0 / lum(c).max(1.0e-6));
        let (a, b) = (hue(stock.color), hue(clear.color));
        assert!((a.x - b.x).abs() < 1.0e-4 && (a.z - b.z).abs() < 1.0e-4);
        // The hemisphere keeps its shape: sky and ground scale together.
        assert!(
            ((lum(clear.ground) / lum(clear.sky)) - (lum(stock.ground) / lum(stock.sky))).abs()
                < 1.0e-4
        );
    }

    /// Daylight only. A rig the night ramp has already taken apart must not
    /// be re-split, or a moonless midnight loses the floor that keeps a town
    /// visible — and gains a sun that is not there.
    #[test]
    fn the_daylight_balance_leaves_the_night_alone() {
        let lum = crate::sky::luminance;
        for hour in [0.0f32, 2.0, 22.5] {
            let stock = SunLight::from_time_of_day(hour, 47.6);
            let clear = SunLight::from_time_of_day_balanced(hour, 47.6, Some(9.0));
            assert!(
                (lum(stock.sky) - lum(clear.sky)).abs() < 1.0e-4,
                "{hour}h: night fill moved from {:?} to {:?}",
                stock.sky,
                clear.sky
            );
            assert!(lum(clear.color) < 1.0e-4, "{hour}h: {:?}", clear.color);
        }
    }

    /// The whole point, stated as a picture: a surface facing the sun is
    /// many times brighter than the same surface facing away.
    #[test]
    fn a_clear_sky_separates_a_lit_face_from_a_shaded_one() {
        let sun = SunLight::from_time_of_day_balanced(13.0, 47.6, Some(9.0));
        let lum = crate::sky::luminance;
        // Facing the sun squarely against facing straight away: ambient only.
        let lit = lum(sun.color) + lum(sun.sky);
        let shaded = lum(sun.sky);
        assert!(lit / shaded > 8.0, "lit {lit} vs shaded {shaded}");
    }

    #[test]
    fn default_reproduces_the_legacy_shading_constants() {
        let sun = SunLight::default();
        // Flat hemisphere: sky == ground == the old 0.28 ambient, so the
        // unified shader collapses to exactly what the old one computed.
        assert!(approx(sun.sky.x, LEGACY_AMBIENT));
        assert_eq!(sun.sky, sun.ground);
        assert!(approx(sun.color.x, LEGACY_DIRECT));
        let want = vec3f(0.35, 0.8, 0.45).normalize();
        assert!(approx(sun.dir.x, want.x) && approx(sun.dir.y, want.y) && approx(sun.dir.z, want.z));
    }

    #[test]
    fn scene_sun_axis_mapping_round_trips() {
        let scene = SceneSun::default();
        let game = SunLight::from_scene_sun(&scene);
        // map z (up) -> game y (up)
        assert!(approx(game.dir.y, scene.dir.z / scene.dir.length()));
        let back = game.to_scene_sun();
        assert!(approx(back.dir.x, scene.dir.x));
        assert!(approx(back.dir.y, scene.dir.y));
        assert!(approx(back.dir.z, scene.dir.z));
        assert!(approx(back.color.x, scene.color.x));
        assert!(approx(back.sky.y, scene.sky.y));
    }

    #[test]
    fn daylight_hours_keep_the_sun_up_and_normalized() {
        for hour in [9.0f32, 12.0, 15.0] {
            let sun = SunLight::from_time_of_day(hour, 52.0);
            assert!(sun.dir.y > 0.0, "hour {hour} put the sun underground");
            let len = sun.dir.length();
            assert!(approx(len, 1.0), "hour {hour} dir not normalized: {len}");
        }
    }

    /// The bug the whole night restore turned on: `SceneSun` clamps its
    /// elevation to keep the MAP a daylight rig, and a game inheriting that
    /// clamp rendered midnight as a golden hour with the sun stuck 4.6
    /// degrees up ("it doesn't get night, just the sun gets low"). A game's
    /// sun must actually set.
    #[test]
    fn the_sun_sets_and_night_is_dark() {
        let noon = SunLight::from_time_of_day(12.0, 52.0);
        assert!(noon.dir.y > 0.5, "noon elevation: {:?}", noon.dir);
        let midnight = SunLight::from_time_of_day(0.0, 52.0);
        assert!(
            midnight.dir.y < -0.2,
            "midnight sun must be below the horizon: {:?}",
            midnight.dir
        );
        // No direct light and no cast shadow at night...
        let lum = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        assert!(lum(midnight.color) < 1.0e-4, "{:?}", midnight.color);
        assert!(midnight.shadow_alpha < 1.0e-4);
        // ...and the ambient floor is dim and cool, so lamps carry a town.
        assert_eq!(midnight.sky, NIGHT_AMBIENT);
        assert!(midnight.sky.z > midnight.sky.x, "night leans blue");
        assert!(lum(midnight.sky) < 0.15, "{:?}", midnight.sky);
        assert!(lum(midnight.sky) < 0.5 * lum(noon.sky));
    }

    /// Dawn and dusk are the same fade played in both directions, and the
    /// high sun is untouched by the night ramp — a game at 9 or 15 hours
    /// looks exactly as it did before the night existed.
    #[test]
    fn twilight_is_symmetric_and_daylight_is_untouched() {
        let lum = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        for hour in [9.0f32, 12.0, 15.0] {
            let sun = SunLight::from_time_of_day(hour, 52.0);
            let plain = SunLight::from_scene_sun(&SceneSun::from_time_of_day(hour, 52.0));
            assert!(approx(sun.color.x, plain.color.x), "hour {hour} restyled");
            assert!(approx(sun.sky.y, plain.sky.y), "hour {hour} restyled");
        }
        // The solar model is symmetric about local noon, so dusk and the
        // dawn that mirrors it must land on the same rig.
        for (dawn, dusk) in [(5.0f32, 19.0f32), (6.0, 18.0), (7.0, 17.0)] {
            let a = SunLight::from_time_of_day(dawn, 52.0);
            let b = SunLight::from_time_of_day(dusk, 52.0);
            assert!(approx(a.dir.y, b.dir.y), "{dawn} vs {dusk}: {a:?} {b:?}");
            assert!((lum(a.color) - lum(b.color)).abs() < 1.0e-5);
            assert!((lum(a.sky) - lum(b.sky)).abs() < 1.0e-5);
        }
        // And the fade is monotone through the evening.
        let mut last = f32::INFINITY;
        for h in 12..=24 {
            let l = lum(SunLight::from_time_of_day(h as f32, 52.0).color);
            assert!(l <= last + 1.0e-6, "hour {h} brightened: {l} after {last}");
            last = l;
        }
    }

    /// Sun and stars ride ONE celestial sphere: in the frame the star dome
    /// is sampled in, the sun does not move all day. Get this wrong and the
    /// constellations drift against the sunrise over a cycle.
    #[test]
    fn stars_hold_still_around_the_sun() {
        for lat in [0.0f32, 30.0, 52.0, -20.0] {
            let at = |h: f32| {
                let d = solar_dir(h, lat);
                let r = celestial_rows(h, lat);
                let row = |v: Vec4f| v.x * d.x + v.y * d.y + v.z * d.z;
                vec3f(row(r[0]), row(r[1]), row(r[2]))
            };
            let noon = at(12.0);
            for h in [0.0f32, 3.0, 6.0, 9.0, 15.0, 18.0, 21.0] {
                let c = at(h);
                assert!(
                    (c.x - noon.x).abs() < 1.0e-3
                        && (c.y - noon.y).abs() < 1.0e-3
                        && (c.z - noon.z).abs() < 1.0e-3,
                    "lat {lat} hour {h}: sun drifted {c:?} vs {noon:?}"
                );
            }
        }
    }

    /// The rows must be a rotation — the shader treats them as one. Rows
    /// unit length, mutually perpendicular, and the dome actually turning.
    #[test]
    fn celestial_rows_are_a_rotation_that_turns() {
        for lat in [0.0f32, 52.0, 90.0] {
            for h in [0.0f32, 6.0, 12.0, 18.0] {
                let r = celestial_rows(h, lat);
                let dot = |a: Vec4f, b: Vec4f| a.x * b.x + a.y * b.y + a.z * b.z;
                for row in r {
                    assert!(approx(dot(row, row), 1.0), "lat {lat} h {h}: {row:?}");
                }
                assert!(dot(r[0], r[1]).abs() < 1.0e-4);
                assert!(dot(r[1], r[2]).abs() < 1.0e-4);
                assert!(dot(r[0], r[2]).abs() < 1.0e-4);
            }
            // Row 1 IS the pole: the panorama's dec +90 lands on the axis.
            let r = celestial_rows(3.0, lat);
            let p = celestial_pole(lat);
            assert!(approx(r[1].x, p.x) && approx(r[1].y, p.y) && approx(r[1].z, p.z));
        }
        // Six hours of the clock is a quarter turn of the sky.
        let a = celestial_rows(12.0, 52.0)[0];
        let b = celestial_rows(18.0, 52.0)[0];
        let d = a.x * b.x + a.y * b.y + a.z * b.z;
        assert!(d.abs() < 1.0e-4, "quarter day should be a quarter turn: {d}");
    }

    /// The panorama frame is still a rotation, turning with the clock, and
    /// the celestial pole lands at galactic latitude +27.1 degrees (the
    /// Milky Way's tilt to the equator is 62.9).
    #[test]
    fn star_rows_turn_the_panorama_into_the_galactic_frame() {
        let dot = |a: Vec4f, b: Vec4f| a.x * b.x + a.y * b.y + a.z * b.z;
        let r = star_rows(21.0, 52.0);
        for i in 0..3 {
            assert!(approx(dot(r[i], r[i]), 1.0));
            for j in 0..i {
                assert!(dot(r[i], r[j]).abs() < 1.0e-4);
            }
        }
        let p = celestial_pole(52.0);
        let pole = vec4f(p.x, p.y, p.z, 0.0);
        let gal_lat = dot(r[1], pole).asin().to_degrees();
        assert!((gal_lat - 27.13).abs() < 0.1, "pole at galactic latitude {gal_lat}");
        let later = star_rows(23.0, 52.0);
        assert!(dot(r[0], later[0]) < 0.99, "the band wheels with the clock");
    }

    /// Plan hdri-7 §2: row 0 of the panorama frame is the galactic centre (l 0, b 0), in Sagittarius at declination
    /// -28.94 degrees, and row 1 the galactic pole at +27.13, at any hour and latitude.
    #[test]
    fn star_rows_row_0_is_the_galactic_centre() {
        let dot = |a: Vec4f, b: Vec4f| a.x * b.x + a.y * b.y + a.z * b.z;
        for (hours, lat) in [(22.0, 45.0), (5.0, -30.0)] {
            let (g, c) = (star_rows(hours, lat), celestial_rows(hours, lat));
            let dec = dot(g[0], c[1]).asin().to_degrees();
            assert!((dec + 28.94).abs() < 0.05, "the centre at declination {dec}");
            let pole = dot(g[1], c[1]).asin().to_degrees();
            assert!((pole - 27.13).abs() < 0.05, "the galactic pole at declination {pole}");
        }
    }

    #[test]
    fn noon_sun_is_higher_than_evening_sun() {
        let noon = SunLight::from_time_of_day(12.0, 52.0);
        let evening = SunLight::from_time_of_day(18.5, 52.0);
        assert!(noon.dir.y > evening.dir.y);
        // Low sun throws a longer shadow.
        assert!(evening.shadow_len_per_unit() > noon.shadow_len_per_unit());
    }

    #[test]
    fn shadow_length_is_clamped_at_the_horizon() {
        let mut sun = SunLight::default();
        sun.dir = vec3f(1.0, 0.001, 0.0).normalize();
        assert!(sun.shadow_len_per_unit() <= 4.0);
    }

    /// A host that supplies BOTH an hour and the true direction (fab's
    /// NOAA sun) gets a rig that follows the DIRECTION: the hour must not
    /// smuggle the engine's fixed-declination elevation back in.
    #[test]
    fn an_explicit_direction_carries_the_whole_rig() {
        let lum = crate::sky::luminance;
        let cfg = |hours: f32, dir: Vec3f| makepad_scene::SunConfig {
            time_of_day: Some(hours),
            latitude: 52.0,
            dir: Some(dir),
            ..Default::default()
        };
        // The engine model's midnight, but the TRUE sun 15 degrees up:
        // there is direct light, and it is warmer than the noon sun.
        let low = vec3f(0.9, 15.0f32.to_radians().sin(), 0.3).normalize();
        let evening = resolve_sun(&cfg(0.0, low));
        assert!(lum(evening.color) > 0.05, "{:?}", evening.color);
        assert!(
            evening.color.x / evening.color.z.max(1.0e-6) > 1.3,
            "low sun should be golden: {:?}",
            evening.color
        );
        // The engine model's noon, but the TRUE sun below the horizon:
        // night, regardless of the hour.
        let down = vec3f(0.5, -0.3, 0.5).normalize();
        let night = resolve_sun(&cfg(12.0, down));
        assert!(lum(night.color) < 1.0e-3, "{:?}", night.color);
        assert_eq!(night.sky, NIGHT_AMBIENT);
        // A high explicit sun stays effectively the stock daylight rig.
        let high = vec3f(0.2, 0.9, 0.3).normalize();
        let noon = resolve_sun(&cfg(20.0, high));
        assert!(
            noon.color.x / noon.color.z.max(1.0e-6) < 1.15,
            "high sun stays near-white: {:?}",
            noon.color
        );
        assert!((lum(noon.color) - LEGACY_DIRECT).abs() < 0.1, "{:?}", noon.color);
    }

    /// The clear-sky split holds through the direction path too: at a high
    /// explicit sun the disc-to-dome ratio is what was asked for.
    #[test]
    fn the_direction_rig_keeps_the_daylight_balance() {
        let lum = crate::sky::luminance;
        let sun = SunLight::from_direction_balanced(
            vec3f(0.2, 0.95, 0.2).normalize(),
            Some(9.0),
        );
        let ratio = lum(sun.color) / lum(sun.sky);
        assert!((ratio - 9.0).abs() < 0.9, "clear-sky ratio {ratio}");
    }

    /// The direct term's tint IS the sky's transmittance curve — one
    /// reddening in the whole engine.
    #[test]
    fn the_direction_rigs_gold_is_the_skys_gold() {
        let y = 6.0f32.to_radians().sin();
        let sun = SunLight::from_direction_balanced(
            vec3f((1.0 - y * y).sqrt(), y, 0.0),
            None,
        );
        let t = crate::sky::sun_transmittance(y);
        // Same channel RATIOS as the transmittance (the base colour is a
        // near-white constant on top).
        let rig_rb = sun.color.x / sun.color.z.max(1.0e-6);
        let sky_rb = t.x / t.z.max(1.0e-6);
        assert!(
            (rig_rb / sky_rb - 1.0).abs() < 0.08,
            "rig {rig_rb} vs sky {sky_rb}"
        );
    }

    #[test]
    fn resolve_applies_explicit_overrides_over_time_of_day() {
        let mut cfg = makepad_scene::SunConfig {
            time_of_day: Some(9.0),
            ..Default::default()
        };
        let base = resolve_sun(&cfg);
        cfg.color = Some(vec3f(1.0, 0.0, 0.0));
        cfg.shadow_alpha = Some(0.5);
        let tuned = resolve_sun(&cfg);
        // direction still from the time of day, color overridden
        assert_eq!(tuned.dir, base.dir);
        assert_eq!(tuned.color, vec3f(1.0, 0.0, 0.0));
        assert!(approx(tuned.shadow_alpha, 0.5));
    }

    // ---- The environment's rig (phase 2, C5) --------------------------------

    fn angle_deg(a: Vec3f, b: Vec3f) -> f32 {
        a.normalize().dot(b.normalize()).clamp(-1.0, 1.0).acos().to_degrees()
    }

    /// A world whose environment names a registered HDRI and carries `sun`.
    fn env_world(sun: Option<makepad_scene::EnvSun>, rotation_deg: f32) -> makepad_scene::World {
        let mut world = makepad_scene::World::new();
        world.environment.ibl = Some(makepad_scene::Ibl {
            source: makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)),
            intensity: 1.0,
            rotation_deg,
        });
        world.environment.sun = sun;
        world
    }

    /// The lighting of a constant grey map at `level` under `Ibl.intensity`
    /// 1 (the gain is the intensity: the environment's own scale, never
    /// HDR_SKY_GAIN): E = πL everywhere, so the hemisphere terms come out as
    /// `level × gain` = `level`.
    fn grey_lighting(level: f32) -> EnvLighting {
        // 128 wide: sh9's midpoint quadrature is 0.4 % high at 32 wide
        // (E/π = 1.0036 for a constant map), 0.02 % at 128.
        let map = makepad_render_material::ibl::EnvMap::constant(128, [level, level, level]);
        EnvLighting { sh: makepad_render_material::ibl::sh9(&map), mean_luminance: level, gain: 1.0 }
    }

    /// A sun disc of real angular size (radius 0.27 degrees) toward `dir`:
    /// a disc under a degree wide, so a surface facing it gets all of its
    /// emission (`facing` 1) and the cone it is averaged over holds it.
    fn disc(dir: Vec3f, radiance: f32) -> makepad_scene::EnvSun {
        let cos_radius = 0.27_f32.to_radians().cos();
        makepad_scene::EnvSun {
            dir: dir.normalize(),
            radiance: vec3f(radiance, radiance * 0.95, radiance * 0.9),
            cos_radius,
            facing: 1.0,
            cos_cover: cos_radius,
        }
    }

    #[test]
    fn an_environment_sun_becomes_the_direct_term() {
        let toward = vec3f(0.3, 0.8, -0.5);
        let world = env_world(Some(disc(toward, 60000.0)), 0.0);
        let rig = env_sun_rig(&world, Some(&grey_lighting(0.2)), SunLight::default().to_hdr(), true);
        assert!(angle_deg(rig.dir, toward) < 0.2, "the light points at the environment's sun: {:?}", rig.dir);
        // E = L·Ω with Ω = 2π(1 − cos r); the lanes take E/π = L·2(1 − cos r); × the
        // gain, Ibl.intensity 1 (the map's own scale: no HDR_SKY_GAIN).
        let want = 60000.0 * 2.0 * (1.0 - 0.27_f32.to_radians().cos());
        assert!((rig.color.x - want).abs() < 2.0e-3, "direct {} vs {want}", rig.color.x);
        assert!(rig.color.x > rig.color.z, "the disc's tint carries");
        // A constant map: E = πL, E/π = L = 0.2; × gain 1.
        assert!((rig.sky.x - 0.2).abs() < 2.0e-3, "sky {:?}", rig.sky);
        assert!((rig.ground.x - 0.2).abs() < 2.0e-3, "ground {:?}", rig.ground);
        // The gain is Ibl.intensity and nothing else: twice the intensity,
        // twice every value the environment supplies.
        let bright = env_sun_rig(&world, Some(&EnvLighting { gain: 2.0, ..grey_lighting(0.2) }), SunLight::default().to_hdr(), true);
        assert!((bright.sky.x - 0.4).abs() < 2.0e-3 && (bright.color.x - 2.0 * want).abs() < 4.0e-3, "{bright:?}");
        // N2: the drop shadows are the key's too, at its share of the light.
        let lum = crate::sky::luminance;
        let share = lum(rig.color) / (lum(rig.color) + lum(rig.sky));
        assert!((rig.shadow_alpha - STOCK_SHADOW_ALPHA * share).abs() < 1.0e-6, "{} vs {share}", rig.shadow_alpha);
    }

    /// N2: when the environment supplies the key (no authored colour), the
    /// drop shadows (the character quads, the blobs and the caster hulls,
    /// which draw at `shadow_alpha` alone) are as strong as the key's share of
    /// direct + fill makes them: the stock strength for a key that carries
    /// all the light, none without a key. An authored `shadow_alpha` wins; an
    /// authored colour is the script's key and keeps the rig's own shadows.
    /// The world's analytic clock does not fade them: a moon key under a
    /// world whose own hour is midnight casts.
    #[test]
    fn the_drop_shadows_follow_the_environments_key() {
        let lum = crate::sky::luminance;
        let lighting = grey_lighting(0.2);
        let toward = vec3f(0.3, 0.8, -0.5);
        for hdr in [true, false] {
            let lane = |s: SunLight| if hdr { s.to_hdr() } else { s };
            let stock = lane(SunLight::default());
            let mut shares = Vec::new();
            for radiance in [600.0, 6000.0, 60000.0] {
                let rig = env_sun_rig(&env_world(Some(disc(toward, radiance)), 0.0), Some(&lighting), stock, hdr);
                let share = lum(rig.color) / (lum(rig.color) + lum(rig.sky));
                assert!(share > 0.0 && share < 1.0, "{hdr} {radiance}: {rig:?}");
                assert!((rig.shadow_alpha - STOCK_SHADOW_ALPHA * share).abs() < 1.0e-6, "{hdr} {radiance}: {} for a share of {share}", rig.shadow_alpha);
                shares.push(rig.shadow_alpha);
            }
            assert!(shares[0] < shares[1], "{hdr}: a stronger key casts darker shadows: {shares:?}");
            // No key, a zero key: no directional light, no drop shadows.
            assert_eq!(env_sun_rig(&env_world(None, 0.0), Some(&lighting), stock, hdr).shadow_alpha, 0.0, "{hdr}");
            let zero = makepad_scene::EnvSun { radiance: Vec3f::default(), ..disc(toward, 1.0) };
            assert_eq!(env_sun_rig(&env_world(Some(zero), 0.0), Some(&lighting), stock, hdr).shadow_alpha, 0.0, "{hdr}");
            // An authored strength wins.
            let mut authored = env_world(Some(disc(toward, 6000.0)), 0.0);
            authored.sun.shadow_alpha = Some(0.6);
            assert_eq!(env_sun_rig(&authored, Some(&lighting), lane(resolve_sun(&authored.sun)), hdr).shadow_alpha, 0.6, "{hdr}");
            // An authored colour: the key is the script's, and so are its shadows.
            authored.sun.shadow_alpha = None;
            authored.sun.color = Some(vec3f(0.5, 0.5, 0.5));
            let input = lane(resolve_sun(&authored.sun));
            assert_eq!(env_sun_rig(&authored, Some(&lighting), input, hdr).shadow_alpha, input.shadow_alpha, "{hdr}");
            // A world whose own clock says midnight (its rig casts nothing) under a moonlit map.
            let mut night = env_world(Some(disc(toward, 6000.0)), 0.0);
            night.sun.time_of_day = Some(0.0);
            night.sun.latitude = 52.0;
            let input = lane(resolve_sun(&night.sun));
            assert_eq!(input.shadow_alpha, 0.0, "premise: the analytic midnight casts nothing");
            assert!(env_sun_rig(&night, Some(&lighting), input, hdr).shadow_alpha > 0.0, "{hdr}: the map's key casts");
        }
        // Without an environment the rig's own strength stays, bit for bit.
        let stock = SunLight::default();
        assert_eq!(env_sun_rig(&env_world(Some(disc(toward, 6000.0)), 0.0), None, stock, false).shadow_alpha, stock.shadow_alpha);
    }

    /// N1, one sun in the sky: a key is its map's own sun exactly when the
    /// generator's sun is the key (every sky preset whose sun is up, faded or
    /// clouded), never the moon (keyed only once the sun is 6 degrees down)
    /// and never without a report (a studio). Within a degree is the sun;
    /// a NaN or zero direction or a missing half is not.
    #[test]
    fn a_key_is_its_maps_sun_only_when_the_generators_sun_is_the_key() {
        let horizon = -(1.0f32.to_radians().sin());
        for name in crate::hdri::presets::PRESET_NAMES {
            let env = crate::hdri::Env::new(&crate::hdri::presets::preset(name).unwrap());
            let (key, daylight) = (env.sun(), env.sun_dir());
            let want = key.is_some() && daylight.is_some_and(|d| d.y > horizon);
            assert_eq!(env_key_is_its_sun(key, daylight), want, "{name}: {key:?} {daylight:?}");
        }
        let sun = crate::hdri::dir_from_az_el(200.0, 30.0);
        let key = |dir: Vec3f| Some(makepad_scene::EnvSun { dir, radiance: vec3f(1.0, 1.0, 1.0), cos_radius: 0.9999, facing: 1.0, cos_cover: 0.9999 });
        assert!(env_key_is_its_sun(key(crate::hdri::dir_from_az_el(200.5, 30.0)), Some(sun)), "0.43 degrees off");
        assert!(!env_key_is_its_sun(key(crate::hdri::dir_from_az_el(200.0, 32.0)), Some(sun)), "2 degrees off");
        assert!(env_key_is_its_sun(key(sun * 3.0), Some(sun * 0.5)), "lengths do not count");
        assert!(!env_key_is_its_sun(key(vec3f(f32::NAN, 1.0, 0.0)), Some(sun)));
        assert!(!env_key_is_its_sun(key(sun), Some(Vec3f::default())));
        assert!(!env_key_is_its_sun(key(sun), None) && !env_key_is_its_sun(None, Some(sun)));
    }

    /// K3: the legacy lane holds a white wall at white whichever hemisphere
    /// is the brighter: a params noon's ground (0.375, 0.339, 0.281) outshines
    /// its sky (0.069, 0.130, 0.243) in every channel, so a wall facing a low
    /// sun, lit by the ground's bounce, would clip if the sun were held
    /// against the sky alone.
    #[test]
    fn the_legacy_lane_holds_the_sun_within_white_over_a_ground_brighter_than_its_sky() {
        let (sky, ground) = ([0.069f32, 0.130, 0.243], [0.375f32, 0.339, 0.281]);
        let map = makepad_render_material::ibl::EnvMap::from_fn(128, |d| if d[1] >= 0.0 { sky } else { ground });
        // A mean at the meter's key: exposure 1, so the legacy fill is the map's own.
        let lighting = EnvLighting { sh: makepad_render_material::ibl::sh9(&map), mean_luminance: 0.75, gain: 1.0 };
        assert_eq!(env_exposure(&lighting), 1.0);
        let warm = makepad_scene::EnvSun { radiance: vec3f(60000.0, 40000.0, 20000.0), ..disc(vec3f(0.6, 0.3, -0.7), 1.0) };
        let rig = env_sun_rig(&env_world(Some(warm), 0.0), Some(&lighting), SunLight::default(), false);
        let (c, s, g) = (rig.color, rig.sky, rig.ground);
        assert!(g.x > s.x && g.y > s.y && g.z > s.z, "premise: the ground is the brighter fill: sky {s:?} ground {g:?}");
        let channels = [(c.x, s.x.max(g.x)), (c.y, s.y.max(g.y)), (c.z, s.z.max(g.z))];
        for (direct, fill) in channels {
            // 1/4096 is the colour grid.
            assert!(direct + fill <= LEGACY_WHITE + 2.5e-4, "{direct} + {fill} clips: {rig:?}");
        }
        // The sun is held exactly as far as its tightest channel needs, hue kept.
        let tight = channels.iter().map(|(d, f)| d + f).fold(0.0f32, f32::max);
        assert!((tight - LEGACY_WHITE).abs() < 5.0e-4, "{rig:?}");
        assert!((c.y / c.x - 40.0 / 60.0).abs() < 2.0e-3 && (c.z / c.x - 20.0 / 60.0).abs() < 2.0e-3, "{c:?}");
    }

    #[test]
    fn a_wide_key_delivers_its_facing_share_to_a_facing_surface() {
        // Phase 2 amendment: the light on a wall that faces a wide studio key is
        // `facing` of the key's emission (a 110 degree dome's cosine across its
        // disc), so the one directional light carries `facing` × the whole.
        let preset = |name: &str| crate::hdri::presets::preset(name).unwrap();
        let key = crate::hdri::Env::new(&preset("Overcast dome")).sun().expect("the overcast dome is a studio key");
        assert!((key.facing - 0.78).abs() < 0.01, "premise: the dome's facing {}", key.facing);
        let stock = SunLight::default().to_hdr();
        let lighting = grey_lighting(0.2);
        let whole = env_sun_rig(&env_world(Some(makepad_scene::EnvSun { facing: 1.0, ..key }), 0.0), Some(&lighting), stock, true);
        let rig = env_sun_rig(&env_world(Some(key), 0.0), Some(&lighting), stock, true);
        assert!(whole.color.x > 0.1, "a key that lights: {:?}", whole.color);
        for (got, whole) in [(rig.color.x, whole.color.x), (rig.color.y, whole.color.y), (rig.color.z, whole.color.z)] {
            // 1/4096 is the colour grid.
            assert!((got - whole * key.facing).abs() < 2.5e-4, "{got} vs {whole} x {}", key.facing);
        }
        // The same holds for a disc a surface sees whole: facing 1 changes nothing.
        let sun = disc(vec3f(0.3, 0.8, -0.5), 60000.0);
        assert_eq!(
            env_sun_rig(&env_world(Some(sun), 0.0), Some(&lighting), stock, true),
            env_sun_rig(&env_world(Some(makepad_scene::EnvSun { facing: 1.0, ..sun }), 0.0), Some(&lighting), stock, true)
        );
        // A broken share reads as what it can be trusted for: nothing.
        let broken = env_sun_rig(&env_world(Some(makepad_scene::EnvSun { facing: f32::NAN, ..sun }), 0.0), Some(&lighting), stock, true);
        assert_eq!(broken.color, Vec3f::default());
    }

    #[test]
    fn without_a_prepared_environment_the_rig_is_bit_identical() {
        let stock = SunLight::default();
        assert_eq!(env_sun_rig(&makepad_scene::World::new(), None, stock, false), stock);
        assert_eq!(env_sun_rig(&makepad_scene::World::new(), None, stock.to_hdr(), true), stock.to_hdr());
        // A world that names an environment the renderer has not prepared.
        assert_eq!(env_sun_rig(&env_world(Some(disc(vec3f(0.0, 1.0, 0.0), 1.0)), 0.0), None, stock, false), stock);
        // A prepared map under a world without an IBL.
        assert_eq!(env_sun_rig(&makepad_scene::World::new(), Some(&grey_lighting(0.2)), stock, false), stock);
        assert!(env_sun_dir(&makepad_scene::World::new()).is_none());
    }

    #[test]
    fn authored_sun_values_beat_the_environment() {
        let toward = vec3f(0.3, 0.8, -0.5);
        let lighting = grey_lighting(0.2);
        let mut world = env_world(Some(disc(toward, 60000.0)), 0.0);
        world.sun.dir = Some(vec3f(0.0, 1.0, 0.0));
        assert!(env_sun_dir(&world).is_none(), "an authored dir is a look, or the true sun for its hour");
        let input = resolve_sun(&world.sun).to_hdr();
        let rig = env_sun_rig(&world, Some(&lighting), input, true);
        assert_eq!(rig.dir, input.dir);
        assert!(rig.color.x > 0.0, "the environment still supplies the colour");
        world.sun.color = Some(vec3f(0.1, 0.2, 0.3));
        let input = resolve_sun(&world.sun).to_hdr();
        let rig = env_sun_rig(&world, Some(&lighting), input, true);
        assert_eq!(rig.color, input.color, "an authored colour stays");
        world.sun.ambient = Some(vec3f(0.05, 0.05, 0.05));
        let input = resolve_sun(&world.sun).to_hdr();
        let rig = env_sun_rig(&world, Some(&lighting), input, true);
        assert_eq!((rig.sky, rig.ground), (input.sky, input.ground), "an authored ambient stays");
        // The legacy lane neither quantises nor exposes authored values.
        let input = resolve_sun(&world.sun);
        let rig = env_sun_rig(&world, Some(&lighting), input, false);
        assert_eq!(rig, input, "dir, colour and ambient all authored: the rig passes through bit for bit");
    }

    #[test]
    fn the_ibl_rotation_turns_the_environment_sun() {
        // ibl's sign: +90 degrees takes a light at -Z to -X (EnvMap::procedural).
        let world = env_world(Some(disc(vec3f(0.0, 0.0, -1.0), 60000.0)), 90.0);
        let dir = env_sun_dir(&world).unwrap();
        // 0.05: an f32 acos cannot resolve less than 0.02 degrees near 0.
        assert!(angle_deg(dir, vec3f(-1.0, 0.0, 0.0)) < 0.05, "{dir:?}");
        let rig = env_sun_rig(&world, Some(&grey_lighting(0.2)), SunLight::default().to_hdr(), true);
        assert!(angle_deg(rig.dir, vec3f(-1.0, 0.0, 0.0)) < 0.2, "{:?}", rig.dir);
    }

    #[test]
    fn a_key_that_is_not_a_unit_vector_still_steers_a_unit_light() {
        // `EnvSun::validate` takes any direction of length 1e-3 or more, so a host-built key of
        // length 0.5 or 50 is valid: the light's direction is normalised here.
        for len in [0.5f32, 50.0] {
            let mut key = disc(vec3f(0.3, 0.8, -0.5), 60000.0);
            key.dir = key.dir * len;
            assert!(key.validate().is_ok());
            let world = env_world(Some(key), 30.0);
            let dir = env_sun_dir(&world).unwrap();
            assert!((dir.length() - 1.0).abs() < 1.0e-6, "{len}: {dir:?}");
            let want = crate::hdri::rotate_y(vec3f(0.3, 0.8, -0.5).normalize(), 30.0);
            assert!(angle_deg(dir, want) < 0.05, "{len}");
            let rig = env_sun_rig(&world, Some(&grey_lighting(0.2)), SunLight::default().to_hdr(), true);
            assert!((rig.dir.length() - 1.0).abs() < 1.0e-6 && angle_deg(rig.dir, want) < 0.2, "{len}: {:?}", rig.dir);
        }
    }

    #[test]
    fn a_sun_the_renderer_supplies_lights_like_a_declared_one() {
        // IblSource::Procedural(7..): the world declares no sun and the
        // renderer passes the preset's baked one (renderer/env_sun.rs).
        let toward = vec3f(0.3, 0.8, -0.5);
        let lighting = grey_lighting(0.2);
        let stock = SunLight::default().to_hdr();
        let declared = env_world(Some(disc(toward, 60000.0)), 30.0);
        let bare = env_world(None, 30.0);
        let supplied = Some(disc(toward, 60000.0));
        assert_eq!(
            env_sun_rig_with(&bare, supplied, Some(&lighting), stock, true),
            env_sun_rig(&declared, Some(&lighting), stock, true)
        );
        assert_eq!(env_sun_dir_with(&bare, supplied), env_sun_dir(&declared));
        assert!(env_sun_dir(&bare).is_none(), "the world itself declares none");
        // An authored direction still wins over a supplied sun.
        let mut authored = env_world(None, 0.0);
        authored.sun.dir = Some(vec3f(0.0, 1.0, 0.0));
        assert!(env_sun_dir_with(&authored, supplied).is_none());
    }

    #[test]
    fn a_sunless_environment_has_no_direct_term() {
        let world = env_world(None, 0.0);
        let rig = env_sun_rig(&world, Some(&grey_lighting(0.2)), SunLight::default().to_hdr(), true);
        assert_eq!(rig.color, Vec3f::default(), "an overcast map lights with its fill alone");
        assert!(rig.sky.x > 0.0 && rig.ground.x > 0.0);
        assert!(env_sun_dir(&world).is_none());
        // Never the analytic rig's colour: not in the HDR lane, not in the legacy one.
        let legacy = env_sun_rig(&world, Some(&grey_lighting(0.2)), SunLight::default(), false);
        assert_eq!(legacy.color, Vec3f::default());
        assert_ne!(SunLight::default().color, Vec3f::default(), "premise: the stock rig has a colour");
        // A zero key is the same as none.
        let zero = makepad_scene::EnvSun { radiance: Vec3f::default(), ..disc(vec3f(0.3, 0.8, -0.5), 1.0) };
        assert_eq!(env_sun_rig(&env_world(Some(zero), 0.0), Some(&grey_lighting(0.2)), SunLight::default().to_hdr(), true).color, Vec3f::default());
    }

    /// One exposure for everything a lane takes from the map: the scale the
    /// IBL lanes carry is the factor the rig's fill is made with, so for one
    /// normal the hemisphere and `mat_ibl_ambient` x the scale agree, under
    /// a bright map and a dark one, in both lanes.
    #[test]
    fn the_lanes_scale_is_the_exposure_the_rig_puts_in_its_fill() {
        // A clear noon's mean (exposure 0.417) and a night's (the ceiling, 3.2).
        for level in [1.8, 0.004] {
            let lighting = grey_lighting(level);
            let exposure = env_exposure(&lighting);
            assert!((exposure - 1.0).abs() > 0.5, "premise: {level} meters far from 1 ({exposure})");
            assert_eq!(env_lane_scale(Some(&lighting), true), 1.0, "HDR output: the composite exposes");
            assert_eq!(env_lane_scale(Some(&lighting), false), exposure, "the legacy lane: the map's own exposure");
            let world = env_world(None, 0.0);
            for hdr in [true, false] {
                let rig = env_sun_rig(&world, Some(&lighting), SunLight::default(), hdr);
                let scale = env_lane_scale(Some(&lighting), hdr);
                for (n, fill) in [([0.0, 1.0, 0.0], rig.sky), ([0.0, -1.0, 0.0], rig.ground)] {
                    // What mat_ibl_ambient returns for n (SH irradiance x Ibl.intensity / pi), at the lane's scale.
                    let e = makepad_render_material::ibl::sh9_irradiance(&lighting.sh, n);
                    let lookup = e[0].max(0.0) * lighting.gain / std::f32::consts::PI * scale;
                    // The rig's values sit on the colour grid.
                    assert!((fill.x - lookup).abs() <= 0.6 * RIG_COLOR_STEP, "level {level} hdr {hdr} n {n:?}: rig {} lookup {lookup}", fill.x);
                }
            }
        }
        // No environment's lighting: nothing to expose, in either lane.
        assert_eq!(env_lane_scale(None, false), 1.0);
        assert_eq!(env_lane_scale(None, true), 1.0);
    }

    /// The fill is the author's while a script sets `SunConfig.ambient` or
    /// the world carries a valid `Light::Sky` (the two values the rig lets
    /// through, `env_sun_rig_with` and `world_lights::apply_world_sun`); a
    /// Sun, a lamp or a broken Sky is not a fill.
    #[test]
    fn an_authored_ambient_or_a_worlds_sky_is_an_authored_fill() {
        use makepad_scene::Light;
        let world = env_world(None, 0.0);
        assert!(!fill_is_authored(&world), "an environment alone: the map's fill");
        let mut ambient = world.clone();
        ambient.sun.ambient = Some(vec3f(0.55, 0.55, 0.55));
        assert!(fill_is_authored(&ambient), "SunConfig.ambient");
        let mut sky = world.clone();
        sky.lights.push(Light::Sky { top: vec3f(0.2, 0.3, 0.4), ground: vec3f(0.1, 0.1, 0.1), intensity: 1.0 });
        assert!(fill_is_authored(&sky), "a world's Light::Sky");
        let mut others = world.clone();
        others.sun.color = Some(vec3f(1.0, 0.9, 0.8));
        others.lights.push(Light::Sun { dir: vec3f(0.3, 1.0, 0.2), color: vec3f(1.0, 1.0, 1.0), lux: 1.0, shadow: Default::default() });
        others.lights.push(Light::Lamp { pos: vec3f(0.0, 3.0, 0.0), color: vec3f(1.0, 1.0, 1.0), intensity: 4.0, range: 10.0, shadow: false });
        others.lights.push(Light::Sky { top: vec3f(f32::NAN, 0.0, 0.0), ground: vec3f(0.0, 0.0, 0.0), intensity: 1.0 });
        assert!(!fill_is_authored(&others), "a colour, a Sun, a lamp and a Sky that does not validate author no fill");
        // What apply_world_sun does with the same lights: the pair agrees.
        let rig = SunLight::default();
        assert_ne!(crate::world_lights::apply_world_sun(&sky, rig).sky, rig.sky, "premise: a valid Sky replaces the fill");
        assert_eq!(crate::world_lights::apply_world_sun(&others, rig).sky, rig.sky, "premise: a broken Sky does not");
    }

    #[test]
    fn the_meter_reads_the_map_and_the_legacy_lane_bakes_it_in() {
        // Mean 0.5 × gain (Ibl.intensity) 1 = key 0.5 -> exposure 0.75 / 0.5 = 1.5, inside the band.
        let lighting = grey_lighting(0.5);
        assert!((env_exposure(&lighting) - 1.5).abs() < 1.0e-5);
        // The key is mean × intensity: twice the intensity meters one stop darker.
        assert!((env_exposure(&EnvLighting { gain: 2.0, ..lighting }) - 0.75).abs() < 1.0e-5);
        assert_eq!(hdr_exposure_for_key(0.0), HDR_EXPOSURE_MAX, "a black map hits the ceiling, never infinity");
        assert_eq!(hdr_exposure_for_key(f32::INFINITY), HDR_EXPOSURE_MIN);
        // A sun that fits under white next to the exposed fill (0.2 + 0.75):
        // the legacy lane's values are the meter's alone.
        let world = env_world(Some(disc(vec3f(0.3, 0.8, -0.5), 6000.0)), 0.0);
        let hdr = env_sun_rig(&world, Some(&lighting), SunLight::default().to_hdr(), true);
        let legacy = env_sun_rig(&world, Some(&lighting), SunLight::default(), false);
        // The legacy lane has no composite: the exposure is in the values,
        // and the up-facing fill lands on the meter's key (0.75).
        assert!((legacy.sky.x - 0.75).abs() < 2.0e-3, "{:?}", legacy.sky);
        assert!((legacy.sky.x - hdr.sky.x * 1.5).abs() < 2.0e-3);
        assert!((legacy.color.x - hdr.color.x * 1.5).abs() < 2.0e-3);
        assert_eq!(legacy.dir, hdr.dir);
        // The hdr rig itself is NOT exposed (the composite does that): the
        // map's own fill, E/π = 0.5.
        assert!((hdr.sky.x - 0.5).abs() < 2.0e-3, "{:?}", hdr.sky);
    }

    #[test]
    fn the_legacy_lane_holds_the_environments_sun_within_white() {
        // The golden hour preset's metered direct term was (3.4, 1.8, 0.6) in
        // a lane with no tone mapper and drew every sunlit surface as
        // saturated yellow. A white surface facing the sun reads at most
        // white (the stock rig's own 0.72 + 0.28), whatever the map's sun.
        let lighting = grey_lighting(0.5);
        for radiance in [6000.0, 60000.0, 6.0e6] {
            let world = env_world(Some(disc(vec3f(0.3, 0.8, -0.5), radiance)), 0.0);
            let hdr = env_sun_rig(&world, Some(&lighting), SunLight::default().to_hdr(), true);
            let legacy = env_sun_rig(&world, Some(&lighting), SunLight::default(), false);
            let exposure = env_exposure(&lighting);
            // The fill is the meter's, the direction the map's, whatever the sun does.
            assert!((legacy.sky.x - hdr.sky.x * exposure).abs() < 2.0e-3, "{radiance}: {:?}", legacy.sky);
            assert_eq!(legacy.dir, hdr.dir);
            for (direct, fill) in [(legacy.color.x, legacy.sky.x), (legacy.color.y, legacy.sky.y), (legacy.color.z, legacy.sky.z)] {
                // 1/4096 is the colour grid.
                assert!(direct + fill <= LEGACY_WHITE + 2.5e-4, "{radiance}: {direct} + {fill} clips");
            }
            // The hdr lane is the map's own (the composite tone maps it).
            let want = radiance * 2.0 * (1.0 - 0.27_f32.to_radians().cos());
            assert!((hdr.color.x - want).abs() < 2.0e-3 * want.max(1.0), "{radiance}: {:?} vs {want}", hdr.color);
            if hdr.color.x * exposure + legacy.sky.x > LEGACY_WHITE {
                // The sun does not fit: the channel with the least room is
                // exactly white on a facing wall, and the sun's hue is kept
                // (the disc's tint is 1 : 0.95 : 0.9).
                assert!((legacy.color.x + legacy.sky.x - LEGACY_WHITE).abs() < 5.0e-4, "{radiance}: {:?} {:?}", legacy.color, legacy.sky);
                assert!((legacy.color.y / legacy.color.x - 0.95).abs() < 2.0e-3, "{radiance}: hue kept {:?}", legacy.color);
                assert!((legacy.color.z / legacy.color.x - 0.9).abs() < 2.0e-3, "{radiance}: hue kept {:?}", legacy.color);
            } else {
                assert!((legacy.color.x - hdr.color.x * exposure).abs() < 2.0e-3, "{radiance}: a sun that fits is left alone");
            }
        }
        // A warm sun (red far above blue) is limited by its red channel; the
        // colour balance stays.
        let warm = makepad_scene::EnvSun { radiance: vec3f(60000.0, 30000.0, 10000.0), ..disc(vec3f(0.3, 0.8, -0.5), 1.0) };
        let legacy = env_sun_rig(&env_world(Some(warm), 0.0), Some(&lighting), SunLight::default(), false);
        assert!((legacy.color.x + legacy.sky.x - LEGACY_WHITE).abs() < 5.0e-4, "{:?}", legacy.color);
        assert!((legacy.color.y / legacy.color.x - 0.5).abs() < 2.0e-3 && (legacy.color.z / legacy.color.x - 1.0 / 6.0).abs() < 2.0e-3, "{:?}", legacy.color);
        // A fill that is white already leaves no room: no sun, never a negative one.
        // (A mean of 2 at gain 3 meters past the band's floor: exposure 0.25, fill 1.5.)
        let washed = EnvLighting { gain: 3.0, ..grey_lighting(2.0) };
        let rig = env_sun_rig(&env_world(Some(disc(vec3f(0.3, 0.8, -0.5), 60000.0)), 0.0), Some(&washed), SunLight::default(), false);
        assert!(rig.sky.x >= LEGACY_WHITE && rig.color == Vec3f::default(), "{rig:?}");
        // Authored values are the script's: an authored colour is not held back
        // (it is in display units already), and an authored ambient is the fill
        // the environment's sun is limited against.
        let mut authored = env_world(Some(disc(vec3f(0.3, 0.8, -0.5), 60000.0)), 0.0);
        authored.sun.color = Some(vec3f(3.0, 2.0, 1.0));
        let rig = env_sun_rig(&authored, Some(&lighting), resolve_sun(&authored.sun), false);
        assert_eq!(rig.color, vec3f(3.0, 2.0, 1.0));
        authored.sun.color = None;
        authored.sun.ambient = Some(vec3f(0.5, 0.5, 0.5));
        let rig = env_sun_rig(&authored, Some(&lighting), resolve_sun(&authored.sun), false);
        assert_eq!(rig.sky, vec3f(0.5, 0.5, 0.5));
        assert!((rig.color.x + 0.5 - LEGACY_WHITE).abs() < 5.0e-4, "{:?}", rig.color);
        // No sun, no direct term (unchanged), and never NaN.
        assert_eq!(env_sun_rig(&env_world(None, 0.0), Some(&lighting), SunLight::default(), false).color, Vec3f::default());
        let broken = makepad_scene::EnvSun { radiance: vec3f(f32::NAN, f32::INFINITY, 1.0), ..disc(vec3f(0.3, 0.8, -0.5), 1.0) };
        let rig = env_sun_rig(&env_world(Some(broken), 0.0), Some(&lighting), SunLight::default(), false);
        assert!(rig.color.is_finite(), "{:?}", rig.color);
    }

    #[test]
    fn nearby_suns_quantise_to_the_same_rig() {
        // A re-baked map moves its detected sun by a hair; the rig must not
        // move at all, or the OnChange bake and the SDF caches churn
        // (frame.rs:301-308, model_instance.rs:369).
        let lighting = grey_lighting(0.2);
        let a = env_sun_rig(&env_world(Some(disc(vec3f(0.3, 0.8, -0.5), 60000.0)), 0.0), Some(&lighting), SunLight::default().to_hdr(), true);
        let b = env_sun_rig(&env_world(Some(disc(vec3f(0.30002, 0.8, -0.5), 60000.7)), 0.0), Some(&lighting), SunLight::default().to_hdr(), true);
        assert_eq!(a, b);
        assert!((a.dir.length() - 1.0).abs() < 1.0e-6, "still a unit vector after rounding");
        // Non-finite map values never reach the rig.
        let broken = makepad_scene::EnvSun {
            dir: vec3f(0.0, 1.0, 0.0),
            radiance: vec3f(f32::NAN, 1.0, 1.0),
            cos_radius: f32::NAN,
            facing: f32::NAN,
            cos_cover: f32::NAN,
        };
        let rig = env_sun_rig(&env_world(Some(broken), 0.0), Some(&lighting), SunLight::default().to_hdr(), true);
        assert!(rig.color.is_finite() && rig.dir.is_finite() && rig.sky.is_finite());
    }

    /// The core's key on the default params' evening, as `Env::sun` reports
    /// it: 21 June at 45 N under the default clear sky, `hour` o'clock.
    fn evening_key(hour: f32) -> Option<makepad_scene::EnvSun> {
        let mut p = crate::hdri::HdriParams::default();
        p.sky.sun.hour = hour;
        crate::hdri::Env::new(&p).sun()
    }

    #[test]
    fn the_rig_has_no_jump_over_an_evening_and_never_falls_back_to_the_analytic_colour() {
        // Phase 2 amendment, item 3: sampled the way the core's
        // `the_key_has_no_jump_over_an_evening` samples the key (every two
        // minutes from 16:00 to 23:00) but on the resolved rig. The sun fades
        // out over a couple of minutes of sun time and the moon fades in at -6
        // degrees, so the directional colour is continuous; between the two
        // there is no key, and then the colour is zero, not the analytic rig's.
        // The bound is coarse on purpose (two minutes is as long as the sun's
        // fade; the core's elevation sweeps carry the continuity proper).
        let stock = SunLight::default().to_hdr();
        assert!(crate::sky::luminance(stock.color) > 1.0, "premise: the analytic rig has a colour");
        let lighting = grey_lighting(0.2);
        let world = env_world(None, 0.0);
        let lum = crate::sky::luminance;
        let rigs: Vec<(f32, Option<makepad_scene::EnvSun>, SunLight)> = (0..211)
            .map(|i| {
                let hour = 16.0 + i as f32 / 30.0;
                let key = evening_key(hour);
                (hour, key, env_sun_rig_with(&world, key, Some(&lighting), stock, true))
            })
            .collect();
        let peak = rigs.iter().map(|r| lum(r.2.color)).fold(0.0, f32::max);
        assert!(peak > 0.5, "a sunny afternoon lights: {peak}");
        let (mut dark, mut last, mut worst) = (0, None::<f32>, 0.0f32);
        for &(hour, key, rig) in &rigs {
            let got = lum(rig.color);
            assert!(rig.color.is_finite() && rig.dir.is_finite(), "{hour} h");
            if key.is_none() {
                dark += 1;
                assert_eq!(rig.color, Vec3f::default(), "{hour} h: no key, no direct light (the analytic colour is {:?})", stock.color);
            }
            if let Some(prev) = last {
                worst = worst.max((got - prev).abs() / peak);
                assert!((got - prev).abs() < 0.05 * peak, "{hour} h: {prev} -> {got} of a peak {peak}");
            }
            last = Some(got);
        }
        assert!(dark > 10, "the key is out for a while between sunset and the moon: {dark} samples");
        println!("largest step of the resolved rig's colour: {:.1} % of the evening's peak", worst * 100.0);
    }

    #[test]
    fn an_authored_dir_holds_while_the_key_comes_and_goes() {
        // A host that re-bakes a `params:` map on a grid under its day clock
        // (the sandbox's: a quarter hour, 7.5 minutes while the sun is low)
        // authors `SunConfig.dir` from its eased hour: the shadows stay on
        // that sun while the map's key (whose direction steps by the bake
        // grid) fades in and out. `authored_sun_values_beat_the_
        // environment` pins the rule for a steady key; this is the same rule
        // across the sunset.
        let d = vec3f(-0.4, 0.5, 0.3).normalize();
        let mut world = env_world(None, 0.0);
        world.sun.dir = Some(d);
        let lighting = grey_lighting(0.2);
        let input = resolve_sun(&world.sun).to_hdr();
        let toward = vec3f(0.3, 0.8, -0.5);
        let golden = Some(disc(toward, 60000.0));
        let fading = Some(disc(toward, 60000.0 * 0.03));
        let rigs = [golden, fading, None].map(|key| env_sun_rig_with(&world, key, Some(&lighting), input, true));
        for rig in &rigs {
            assert_eq!(rig.dir, input.dir, "the authored direction, bit for bit");
            assert_eq!(rig.dir, d);
        }
        assert!(rigs[0].color.x > rigs[1].color.x && rigs[1].color.x > 0.0, "{:?} {:?}", rigs[0].color, rigs[1].color);
        assert_eq!(rigs[2].color, Vec3f::default(), "no key: no direct light");
        // The fill is the environment's, the same in all three.
        assert!(rigs[0].sky.x > 0.0 && rigs[0].ground.x > 0.0);
        for rig in &rigs {
            assert_eq!((rig.sky, rig.ground), (rigs[0].sky, rigs[0].ground));
        }
        let steady = env_sun_rig_with(&env_world(None, 0.0), None, Some(&lighting), SunLight::default().to_hdr(), true);
        assert_eq!((rigs[0].sky, rigs[0].ground), (steady.sky, steady.ground), "the fill of the map, whoever aims the light");
    }

    #[test]
    fn the_environments_horizon_becomes_the_host_fog() {
        let env = grey_lighting(1.0); // gain = Ibl.intensity 1
        let horizon = vec3f(0.9, 0.7, 0.5);
        assert_eq!(env_fog_color(horizon, &env), horizon, "HDR: linear at the map's own scale, the composite exposes it with the dome");
        // The gain is Ibl.intensity alone (no HDR_SKY_GAIN): the fog moves with the dome.
        assert_eq!(env_fog_color(horizon, &EnvLighting { gain: 2.0, ..env }), horizon * 2.0);
        // A negative or non-finite texel never gets through.
        let bad = env_fog_color(vec3f(-1.0, f32::NAN, 0.5), &env);
        assert_eq!((bad.x, bad.y), (0.0, 0.0));
        assert!(bad.z > 0.0);
        // The legacy lane's fog is the legacy dome's own tone map
        // (renderer/ibl.rs, DrawEnvBackground): a white map of 1.0 at the
        // dome's exposure 0.12 lands at (0.12 / 1.12)^(1/2.2) = 0.36.
        let t = legacy_dome_rgb(vec3f(1.0, 1.0, 1.0), 0.12);
        assert!((t.x - (0.12f32 / 1.12).powf(0.4545454)).abs() < 1.0e-5 && t.x == t.z, "{t:?}");
        // Reinhard on the luminance keeps the hue order and the ratios.
        let warm = legacy_dome_rgb(horizon, 0.12);
        assert!(warm.x > warm.y && warm.y > warm.z && warm.z > 0.0 && warm.x < 1.0, "{warm:?}");
        // A blinding texel normalises to 1, never above; bad input is black.
        let hot = legacy_dome_rgb(vec3f(1.0e9, 1.0e9, 1.0e9), 0.12);
        assert!(hot.x <= 1.0 && hot.x > 0.999, "{hot:?}");
        assert_eq!(legacy_dome_rgb(vec3f(-1.0, f32::NAN, 0.0), 0.12), Vec3f::default());
        assert_eq!(legacy_dome_rgb(vec3f(1.0, 1.0, 1.0), f32::NAN), Vec3f::default(), "no exposure, no light");
    }
}
