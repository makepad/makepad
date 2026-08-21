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

/// What a moonless night leaves behind once the sun is fully down: a dim
/// cool floor, flat over both hemispheres. Low enough that street lamps and
/// emissive windows are the light in town, high enough that geometry still
/// separates from the near-black sky.
const NIGHT_AMBIENT: Vec3f = Vec3f {
    x: 0.10,
    y: 0.11,
    z: 0.15,
};

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

    /// The rig for `hours` (0..24) — the shared solar model, so the game and
    /// the map agree on where the sun is, but taken at its TRUE elevation:
    /// `SceneSun` clamps itself to a permanent daylight rig (its bake has no
    /// night), which is exactly what made a game's midnight render as a
    /// golden hour with the sun stuck 4.6 degrees up. Below the horizon this
    /// hands back the night rig instead, and the direction keeps sinking —
    /// which is what lights the analytic sky's night blend and its stars.
    pub fn from_time_of_day(hours: f32, latitude_deg: f32) -> Self {
        let dir = solar_dir(hours, latitude_deg);
        let mut sun = Self::from_scene_sun(&SceneSun::from_time_of_day(hours, latitude_deg));
        sun.dir = dir;
        sun.apply_night(dir.y.clamp(-1.0, 1.0).asin().to_degrees());
        sun
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

/// Resolved from the sim's [`makepad_game_sim::SunConfig`], which stores
/// only what script asked for (the sim cannot depend on `makepad_draw`).
///
/// `time_of_day` picks the rig — day, twilight or night. The overrides that
/// follow are exactly that: overrides. An explicit `dir` MOVES the sun
/// without re-deciding which rig it is, because a script that authors a
/// direction is authoring a LOOK (the village's 38-degree sun is a set
/// dressing choice, not a time); a script that wants night asks for the
/// hour.
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
