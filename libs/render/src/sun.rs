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
            shadow_alpha: 0.35,
        }
    }
}

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

    /// A daylight rig for `hours` (0..24) — delegates to the shared solar
    /// model so the game and the map agree on where the sun is.
    pub fn from_time_of_day(hours: f32, latitude_deg: f32) -> Self {
        Self::from_scene_sun(&SceneSun::from_time_of_day(hours, latitude_deg))
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

/// Resolved from the sim's [`makepad_game_sim::SunConfig`], which stores
/// only what script asked for (the sim cannot depend on `makepad_draw`).
pub fn resolve_sun(cfg: &makepad_game_sim::SunConfig) -> SunLight {
    let mut sun = match cfg.time_of_day {
        Some(hours) => SunLight::from_time_of_day(hours, cfg.latitude),
        None => SunLight::default(),
    };
    if let Some(dir) = cfg.dir {
        if dir.x != 0.0 || dir.y != 0.0 || dir.z != 0.0 {
            sun.dir = dir.normalize();
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
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
    fn time_of_day_keeps_the_sun_above_the_horizon() {
        for hour in [6.0f32, 9.0, 12.0, 15.0, 18.0] {
            let sun = SunLight::from_time_of_day(hour, 52.0);
            assert!(sun.dir.y > 0.0, "hour {hour} put the sun underground");
            let len = sun.dir.length();
            assert!(approx(len, 1.0), "hour {hour} dir not normalized: {len}");
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

    #[test]
    fn resolve_applies_explicit_overrides_over_time_of_day() {
        let mut cfg = makepad_game_sim::SunConfig {
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
}
