//! The one global light (shiny.md): a single sun direction + color +
//! hemisphere ambient shared by every lighting consumer — map tile bake,
//! terrain hillshade worker, map pixel shaders and the XR/DrawPbr rigs.
//! There is deliberately no light list and no per-shader copy of the
//! numbers: everything reads from one `SceneSun`.

use crate::makepad_platform::*;

/// Map space convention: x east, y SOUTH (screen down when north-up),
/// z up. `dir` points from the surface TOWARD the sun, normalized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneSun {
    pub dir: Vec3f,
    /// Sun tint * intensity for the direct term.
    pub color: Vec3f,
    /// Hemisphere ambient from above (cool sky).
    pub sky: Vec3f,
    /// Hemisphere ambient from below (warm ground bounce).
    pub ground: Vec3f,
    /// How dark baked shadow geometry draws (0 = invisible).
    pub shadow_alpha: f32,
}

impl Default for SceneSun {
    /// Matches the legacy hardcoded map bake exactly: NW sun whose
    /// horizontal part is the unit vector (-0.55, -0.835) the wall shade
    /// used, with the ball bake's 1.05 vertical component.
    fn default() -> Self {
        Self {
            dir: vec3f(-0.55, -0.835, 1.05).normalize(),
            color: vec3f(1.0, 0.98, 0.94),
            sky: vec3f(0.55, 0.62, 0.72),
            ground: vec3f(0.38, 0.35, 0.31),
            shadow_alpha: 0.22,
        }
    }
}

impl SceneSun {
    /// Simple solar-position model good enough for a map light: hour angle
    /// from local solar `hours` (0..24), fixed early-summer declination.
    /// Clamps elevation to stay a daylight rig — this drives a look, not
    /// an ephemeris.
    pub fn from_time_of_day(hours: f32, latitude_deg: f32) -> Self {
        let decl = 15.0f32.to_radians();
        let lat = latitude_deg.to_radians();
        let hour_angle = ((hours - 12.0) * 15.0).to_radians();
        let sin_elev =
            (lat.sin() * decl.sin() + lat.cos() * decl.cos() * hour_angle.cos()).clamp(-1.0, 1.0);
        let elev = sin_elev.asin().max(0.08);
        // Azimuth from north, clockwise (compass), toward the sun.
        let az = (hour_angle.sin() * decl.cos())
            .atan2(hour_angle.cos() * decl.cos() * lat.sin() - decl.sin() * lat.cos())
            + std::f32::consts::PI;
        let cos_e = elev.cos();
        // north component -> map -y (y is south/screen-down).
        let dir = vec3f(az.sin() * cos_e, -az.cos() * cos_e, elev.sin()).normalize();
        // Warm the sun and dim the sky toward the horizon hours.
        let warmth = (1.0 - (elev / 0.9).clamp(0.0, 1.0)).powi(2);
        let color = vec3f(
            1.0,
            0.98 - 0.25 * warmth,
            0.94 - 0.52 * warmth,
        );
        let sky_dim = 0.7 + 0.3 * (elev / 0.9).clamp(0.0, 1.0);
        Self {
            dir,
            color,
            sky: vec3f(0.55, 0.62, 0.72) * sky_dim,
            ground: vec3f(0.38, 0.35, 0.31) * sky_dim,
            shadow_alpha: 0.16 + 0.14 * warmth,
        }
    }

    /// Horizontal (map-plane) part of the sun direction, unit length —
    /// what the 2D wall-facing shade and shadow projection use.
    pub fn dir_2d(&self) -> Vec2f {
        let d = vec2f(self.dir.x, self.dir.y);
        let len = (d.x * d.x + d.y * d.y).sqrt();
        if len < 1e-6 {
            vec2f(-0.55, -0.835)
        } else {
            vec2f(d.x / len, d.y / len)
        }
    }

    /// Ground shadow length per meter of caster height:
    /// `horizontal / vertical` of the sun direction.
    pub fn shadow_len_per_m(&self) -> f32 {
        let h = (self.dir.x * self.dir.x + self.dir.y * self.dir.y).sqrt();
        (h / self.dir.z.max(0.05)).min(6.0)
    }
}

/// Material ids carried in `param3` of shape-0 map geometry (walls, roofs,
/// balls, plain fills). The pixel shader dispatches per-material tricks on
/// this channel; 0 keeps the legacy untouched path.
pub const MAT_NONE: f32 = 0.0;
pub const MAT_WALL: f32 = 1.0;
pub const MAT_ROOF: f32 = 2.0;
pub const MAT_WATER: f32 = 3.0;
pub const MAT_CANOPY: f32 = 4.0;
pub const MAT_GREEN: f32 = 5.0;
pub const MAT_SHADOW: f32 = 6.0;
pub const MAT_ROUTE_GLOW: f32 = 7.0;

/// Every shiny.md feature behind its own switch, zero-cost when off:
/// bake flags simply don't emit geometry/colors (a restyle applies the
/// change), draw flags become float uniform gates, pass flags mean the
/// pass is never allocated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShinyConfig {
    // Bake-time (apply via restyle/rebake; zero GPU cost when off).
    pub bake_ao: bool,
    pub bake_bounce: bool,
    pub bake_shadows: bool,
    pub terrain_shadows: bool,
    // Draw-time (float uniform gates, DrawPbr u_enable_* pattern).
    pub dynamic_sun: bool,
    pub water_fx: bool,
    pub building_sheen: bool,
    pub foliage_fx: bool,
    pub route_glow: bool,
    // Pass-level (pass not allocated/scheduled when off).
    pub bloom: bool,
    pub tilt_shift: bool,
    pub xr_shadow_map: bool,
    pub sun: SceneSun,
}

impl Default for ShinyConfig {
    fn default() -> Self {
        Self {
            bake_ao: false,
            bake_bounce: false,
            bake_shadows: false,
            terrain_shadows: false,
            dynamic_sun: false,
            water_fx: false,
            building_sheen: false,
            foliage_fx: false,
            route_glow: false,
            bloom: false,
            tilt_shift: false,
            xr_shadow_map: false,
            sun: SceneSun::default(),
        }
    }
}
