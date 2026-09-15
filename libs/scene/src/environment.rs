use makepad_math::*;

/// Sky + atmosphere, set from script with game.sky({...}). Off by default so
/// existing indoor/abstract games keep their dark backdrop.
#[derive(Clone, Copy)]
pub struct SkyConfig {
    pub top: Vec4f,
    pub horizon: Vec4f,
    pub ground: Vec4f,
    pub ground_bottom: Vec4f,
    /// Exponential distance-fog density toward the horizon color.
    pub fog: f32,
    /// Inputs for the shared analytic daylight/twilight model.
    pub turbidity: f32,
    pub sky_strength: f32,
    pub sun_strength: f32,
    /// User compensation on top of mean-luminance auto exposure.
    pub exposure_ev: f32,
}

impl Default for SkyConfig {
    fn default() -> Self {
        // The sky every app gets. The warm, slightly deeper horizon and the
        // thin haze were tuned on the sandbox's village demo and looked
        // right there — a sunset that reads as air rather than a grey veil —
        // so they belong to the engine, not to one scene: the viewer and
        // every game now open on the same sky.
        Self {
            top: vec4(0.32, 0.58, 0.9, 1.0),
            horizon: vec4(0.66, 0.76, 0.80, 1.0),
            ground: vec4(0.68, 0.75, 0.66, 1.0),
            ground_bottom: vec4(0.3, 0.4, 0.3, 1.0),
            fog: 0.0015,
            turbidity: 2.5,
            sky_strength: 1.0,
            sun_strength: 4.0,
            exposure_ev: 0.0,
        }
    }
}

/// What `game.sun({...})` asked for. The sim stores only the request — it
/// cannot depend on `makepad_draw`, so the renderer resolves this against
/// the shared `SceneSun` model (see game_render's `resolve_sun`). Lighting
/// is presentation: nothing here is ever read by the step.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SunConfig {
    /// Local solar hour 0..24. `None` keeps the default rig.
    pub time_of_day: Option<f32>,
    /// Latitude for the solar model, degrees.
    pub latitude: f32,
    /// Explicit direction toward the sun (y-up), overriding `time_of_day`.
    pub dir: Option<Vec3f>,
    /// Direct-term multiplier.
    pub color: Option<Vec3f>,
    /// Flat ambient, applied to both hemisphere terms.
    pub ambient: Option<Vec3f>,
    /// How much brighter the DISC is than the DOME at full daylight, as a
    /// ratio of luminances. `None` keeps the stock split (roughly 2.6:1),
    /// which is a soft, forgiving key for a stylised world. A viewer that
    /// wants a CLEAR sky asks for around 9: measured clear daylight puts
    /// only about a tenth of the light in the dome, and that is the
    /// difference between shadows that read and shadows that fill in.
    ///
    /// Applied to the DAYLIGHT rig only — the twilight and night ramps run
    /// on top of it untouched, so an evening keeps its own floor.
    pub daylight_balance: Option<f32>,
    /// How dark cast shadows draw, 0..1.
    pub shadow_alpha: Option<f32>,
}

