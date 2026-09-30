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

/// A game's colour grade (`game.grade({...})`), applied where the HDR
/// composite tone maps. Every default reproduces the stock look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorGrade {
    /// Exposure bias in EV on top of the metered (and adapted) exposure.
    pub exposure_ev: f32,
    /// Contrast about mid-grey, in the tone map's log domain (1 = stock).
    pub contrast: f32,
    /// Saturation multiplier (1 = stock).
    pub saturation: f32,
    /// Auto-exposure on; off holds the metered exposure (plus the bias).
    pub auto: bool,
    /// How far auto-exposure may move from the metered exposure, in EV.
    pub auto_min_ev: f32,
    pub auto_max_ev: f32,
    /// Screen-space ambient occlusion strength multiplier (1 = stock): how
    /// dark corners, wall feet and the undersides of things read.
    pub ao: f32,
    /// Tilt-shift (the diorama look): how far the frame above and below a
    /// sharp horizontal band softens into the blurred image, 0 = off.
    pub tilt: f32,
    /// The sharp band's centre (0 = top of the frame, 1 = bottom) and its
    /// half-height, in frame heights.
    pub tilt_center: f32,
    pub tilt_width: f32,
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            auto: true,
            // x0.75 .. x1.6 of the metered exposure.
            auto_min_ev: -0.415,
            auto_max_ev: 0.678,
            ao: 1.0,
            tilt: 0.0,
            tilt_center: 0.6,
            tilt_width: 0.2,
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


/// What surrounds a world: its background, image-based lighting and fog.
/// `Environment::default()` asks for nothing, and a host that leaves it so
/// keeps its own sky (`World::sky`) and analytic reflections.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Environment {
    pub background: Background,
    /// `Some` switches on image-based lighting (prefiltered specular and
    /// SH9 diffuse). `None` keeps the analytic sky reflection.
    pub ibl: Option<Ibl>,
    pub fog: Fog,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Background {
    /// The host's sky (`World::sky`, or none).
    #[default]
    Host,
    Color(Vec4f),
    /// Vertical gradient, zenith to nadir.
    Gradient { top: Vec4f, bottom: Vec4f },
    /// The IBL source image itself.
    Environment { blur: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ibl {
    pub source: IblSource,
    pub intensity: f32,
    /// Rotation about +Y, degrees.
    pub rotation_deg: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IblSource {
    /// An equirectangular HDR image (a resident texture handle).
    Hdri(crate::item::TextureRef),
    /// One of the built-in procedural environments, by index.
    Procedural(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Fog {
    /// The host's fog (`SkyConfig::fog`), or none.
    #[default]
    Host,
    None,
    Linear { color: Vec3f, start: f32, end: f32 },
    Exp2 { color: Vec3f, density: f32 },
    /// Exponential in distance, falling off with height above `base`.
    Height { color: Vec3f, density: f32, base: f32, falloff: f32 },
}

impl Environment {
    pub fn validate(&self) -> Result<(), &'static str> {
        let f = |v: f32| v.is_finite();
        if let Some(ibl) = &self.ibl {
            if !f(ibl.intensity) || ibl.intensity < 0.0 || !f(ibl.rotation_deg) {
                return Err("ibl intensity must be non-negative and its rotation finite");
            }
        }
        match self.fog {
            Fog::Linear { start, end, .. } if !(f(start) && f(end) && start < end) => Err("linear fog needs start < end"),
            Fog::Exp2 { density, .. } if !(f(density) && density >= 0.0) => Err("fog density must be non-negative"),
            Fog::Height { density, falloff, base, .. } if !(f(density) && density >= 0.0 && f(falloff) && falloff >= 0.0 && f(base)) => {
                Err("height fog needs a non-negative density and falloff")
            }
            _ => Ok(()),
        }
    }
}
