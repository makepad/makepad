//! The one-frame `GameWorld` consumed by Fab's realtime viewport.
//!
//! The building carries its own site, so this stage has no terrain or extra
//! entities. Its default sky selects the engine's analytic dome; Fab supplies
//! only the NOAA sun direction and user atmosphere controls.

use crate::api::*;
use crate::viewport::pack::to_render;
use makepad_game_sim::{GameWorld, SkyConfig, SunConfig};

/// Sun elevation below which the shadow rig is off and the sky takes over.
const NIGHT_DEG: f32 = 0.5;
/// The engine's stock outdoor density is 0.004 inverse metres.
const MAX_HAZE_DENSITY: f32 = 0.004;

pub(crate) fn haze_density(amount: f32) -> f32 {
    amount.clamp(0.0, 1.0) * MAX_HAZE_DENSITY
}

pub fn stage_world(state: &AppState, camera: &Camera) -> GameWorld {
    let mut world = GameWorld::new();
    world.entities.clear();
    world.next_id = 1;
    world.terrain = None;
    world.cam_target = to_render(camera.target);
    world.cam_distance = camera.distance().max(0.5);
    world.cam_fov = camera.fov_y_deg.clamp(20.0, 120.0);

    let sky = &state.sun;
    let sun_up = sky.elevation_deg() > NIGHT_DEG;
    world.sky = Some(SkyConfig {
        fog: haze_density(sky.haze),
        turbidity: sky.turbidity,
        sky_strength: 1.0,
        sun_strength: 4.0,
        exposure_ev: sky.exposure_ev,
        ..SkyConfig::default()
    });
    world.sun = SunConfig {
        // Time selects the engine's day/night lighting and celestial frame;
        // the NOAA direction then aims the sun at the real site position.
        time_of_day: Some(sky.time_local.clamp(0.0, 24.0)),
        latitude: sky.latitude,
        dir: Some(to_render(sky.direction())),
        // The engine owns the rig — its warm low sun, its twilight and night
        // ramps, its celestial frame. Fab authors only the DAYLIGHT BALANCE:
        // a building viewer needs the clear sky its sun study is about, not
        // the soft key a stylised game wants.
        color: None,
        ambient: None,
        daylight_balance: Some(SkyState::DAYLIGHT_BALANCE),
        shadow_alpha: Some(if state.sun_shadows && sun_up { 0.85 } else { 0.0 }),
    };
    world.mark_render_dirty();
    world
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_noaa_sun_turns_with_the_world() {
        let mut state = AppState::default();
        state.sun.time_local = 12.0;
        state.sun.longitude = 0.0;
        state.sun.tz_offset = 0.0;
        state.sun.north_deg = 0.0;
        let world = stage_world(&state, &Camera::default());
        let direction = world.sun.dir.expect("noon sun direction");
        assert_eq!(direction, to_render(state.sun.direction()));
        assert_eq!(world.sun.time_of_day, Some(state.sun.time_local));
        assert!(direction.y > 0.5, "{direction:?}");
        assert!(direction.z > 0.0, "{direction:?}");
    }

    #[test]
    fn night_drops_the_shadow_rig() {
        let mut state = AppState::default();
        state.sun.time_local = 1.0;
        let world = stage_world(&state, &Camera::default());
        assert!(world.sun.dir.expect("night sun direction").y < 0.0);
        assert_eq!(world.sun.shadow_alpha, Some(0.0));
    }

    #[test]
    fn stage_delegates_sky_and_lighting_to_the_engine() {
        let world = stage_world(&AppState::default(), &Camera::default());
        assert!(world.entities.is_empty());
        assert!(world.terrain.is_none());
        assert!(world.sky.is_some());
        assert_eq!(world.sun.color, None);
        assert_eq!(world.sun.ambient, None);
        // ...with one thing said out loud: the daylight is a CLEAR sky.
        assert_eq!(
            world.sun.daylight_balance,
            Some(SkyState::DAYLIGHT_BALANCE)
        );
    }

    /// The user-visible law: on a clear day the sun IS the light and the sky
    /// is the fill, not the other way round. A viewport that gets this wrong
    /// draws a correct sun direction, correct shadow maps and correct
    /// materials, and still looks overcast.
    #[test]
    fn daylight_is_a_clear_sky_and_dusk_still_has_its_floor() {
        use makepad_render::sky::luminance;
        use makepad_render::sun::resolve_sun;

        for hour in [10.0f32, 12.0, 14.0, 16.0] {
            let mut state = AppState::default();
            state.sun.time_local = hour;
            let rig = resolve_sun(&stage_world(&state, &Camera::default()).sun);
            let ratio = luminance(rig.color) / luminance(rig.sky);
            println!(
                "{hour:>5.1}h: elevation={:>5.1}deg direct={:.4} fill={:.4} ratio={ratio:.2}x",
                state.sun.elevation_deg(),
                luminance(rig.color),
                luminance(rig.sky),
            );
            assert!(ratio > 8.0, "{hour}h: the sun is only {ratio:.2}x the sky");
        }
        // After sunset the balance inverts by itself: no disc, so the fill is
        // all there is — and it must still BE there.
        let mut night = AppState::default();
        night.sun.time_local = 1.0;
        let rig = resolve_sun(&stage_world(&night, &Camera::default()).sun);
        assert!(luminance(rig.color) < luminance(rig.sky));
        assert!(luminance(rig.sky) > 0.05, "night fill {:?}", rig.sky);
    }

    #[test]
    fn haze_zero_is_off_and_the_default_is_a_light_touch() {
        assert_eq!(haze_density(0.0), 0.0);
        let density = haze_density(SkyState::default().haze);
        let house_blend = 1.0 - (-density * 20.0).exp();
        let hillside_blend = 1.0 - (-density * 120.0).exp();
        assert!(house_blend < 0.04, "house haze {house_blend}");
        assert!(hillside_blend > 0.12, "hill haze {hillside_blend}");
    }



}
