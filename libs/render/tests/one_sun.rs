//! One sun: the direction the SKY paints, the direction the LIGHT shades
//! with, and the rig (colour, night ramp) must all follow the same star.
//!
//! A host that supplies an explicit `dir` (Fab's NOAA solar position) must
//! see the whole frame follow it: the engine's own `time_of_day` model is a
//! fixed-declination look, and at a real site on a real date its sun can be
//! many degrees away from the true one — far enough that the engine's rig
//! called "night" while the true sun still stood golden above the horizon.

use makepad_draw::*;
use makepad_scene::SunConfig;
use makepad_render::sky::{luminance, noaa_solar_position, SkyDate};
use makepad_render::sun::{resolve_sun, solar_dir};
use makepad_render::hdri::{self, HdriParams};
use makepad_render::hdri::envmap::{bake_env_map, remove_sun};
use makepad_render::makepad_render_material::ibl;
use makepad_render::sun::{env_exposure, env_sun_dir, env_sun_rig, EnvLighting};
use makepad_scene::{Ibl, IblSource, TextureRef, World};

/// Fab's default site (libs/fab api::SkyState::default): Amsterdam-ish,
/// midsummer, CEST.
const LAT: f32 = 52.37;
const LON: f32 = 4.9;
const TZ: f32 = 2.0;
const DATE: SkyDate = SkyDate {
    year: 2024,
    month: 6,
    day: 21,
};

/// Fab's `SkyState::direction()` + `to_render`, reproduced here: NOAA
/// elevation/azimuth to the render world's y-up frame (x east, -z north).
fn noaa_render_dir(hour: f32) -> Vec3f {
    let (elevation, azimuth) = noaa_solar_position(DATE, hour, TZ, LAT, LON);
    let elevation = elevation.to_radians();
    let azimuth = azimuth.to_radians();
    let horizontal = elevation.cos();
    // Fab space (z up, +y north): (sin az * h, cos az * h, sin el),
    // then to_render (x, z, -y).
    vec3(
        horizontal * azimuth.sin(),
        elevation.sin(),
        -horizontal * azimuth.cos(),
    )
    .normalize()
}

fn fab_sun_config(hour: f32) -> SunConfig {
    SunConfig {
        time_of_day: Some(hour),
        latitude: LAT,
        dir: Some(noaa_render_dir(hour)),
        color: None,
        ambient: None,
        daylight_balance: Some(9.0),
        shadow_alpha: Some(0.85),
    }
}

fn angle_deg(a: Vec3f, b: Vec3f) -> f32 {
    a.normalize()
        .dot(b.normalize())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

fn elev_deg(d: Vec3f) -> f32 {
    d.y.clamp(-1.0, 1.0).asin().to_degrees()
}

/// The measurement behind the fix, printed for the record: at Fab's default
/// site the engine's fixed-declination sun and the true NOAA sun disagree by
/// double-digit degrees through the day, and at 20:00 the engine model is
/// BELOW the horizon while the true sun is still up.
#[test]
fn the_two_solar_models_disagree_at_a_real_site() {
    println!(
        "{:>6} {:>18} {:>18} {:>10} {:>12}",
        "hour", "noaa el/az", "engine el/az", "angle", "night-ramp-el"
    );
    for hour in [8.0f32, 14.0, 18.5, 20.0] {
        let noaa = noaa_render_dir(hour);
        let simple = solar_dir(hour, LAT);
        let az = |d: Vec3f| (d.x.atan2(-d.z).to_degrees()).rem_euclid(360.0);
        println!(
            "{:>6.1} {:>8.1}/{:>8.1} {:>8.1}/{:>8.1} {:>9.1}d {:>11.1}d",
            hour,
            elev_deg(noaa),
            az(noaa),
            elev_deg(simple),
            az(simple),
            angle_deg(noaa, simple),
            elev_deg(simple),
        );
    }
    // The concrete split-brain of the bug report: at 20:00 the true sun is
    // still above the horizon while the engine's fixed-declination model has
    // already set.
    let noaa = noaa_render_dir(20.0);
    assert!(elev_deg(noaa) > 2.0, "true sun at 20:00: {noaa:?}");
    assert!(
        elev_deg(solar_dir(20.0, LAT)) < elev_deg(noaa),
        "the models should disagree at 20:00 for this site"
    );
}

/// THE pinned contract: with an explicit direction, the direction the sky
/// paints (resolve_sun().dir feeds the Preetham frame, the disc, the CSM
/// projection and the shading) IS the explicit one, at every hour.
#[test]
fn sky_and_light_share_one_direction() {
    for hour in [0.0f32, 4.0, 8.0, 12.0, 14.0, 18.5, 20.0, 22.0] {
        let cfg = fab_sun_config(hour);
        let rig = resolve_sun(&cfg);
        let want = cfg.dir.unwrap();
        assert!(
            angle_deg(rig.dir, want) < 1.0e-3,
            "{hour}h: light {:?} vs sky {:?}",
            rig.dir,
            want
        );
    }
    // ...and a time-of-day-only game keeps the engine's own model.
    let cfg = SunConfig {
        time_of_day: Some(15.0),
        latitude: LAT,
        ..Default::default()
    };
    assert!(angle_deg(resolve_sun(&cfg).dir, solar_dir(15.0, LAT)) < 1.0e-3);
}

/// The rig must FOLLOW the explicit sun: while the true sun is up, there is
/// direct light — even at an hour where the engine's own model says night.
#[test]
fn the_rig_follows_the_explicit_sun_not_the_hour() {
    // 20:00 at the default site: the true sun is a few degrees up, golden.
    let rig = resolve_sun(&fab_sun_config(20.0));
    let true_el = elev_deg(noaa_render_dir(20.0));
    assert!(true_el > 2.0, "premise: sun still up at 20:00 ({true_el})");
    assert!(
        luminance(rig.color) > 0.05,
        "sun above the horizon but no direct light: {:?}",
        rig.color
    );
    // The low sun is WARM: the direct term leans red over blue — the sky
    // model's own transmittance at 15.8 degrees (20:00 is still two hours
    // before this site's midsummer sunset; the gold deepens as it sinks).
    let warmth = |c: Vec3f| c.x / c.z.max(1.0e-6);
    assert!(
        warmth(rig.color) > 1.45,
        "20:00 direct should be golden: {:?}",
        rig.color
    );
    // And a high sun stays effectively white, so the 20:00 warmth is a
    // sunset property, not a permanent cast.
    let noon = resolve_sun(&fab_sun_config(13.5));
    assert!(
        warmth(noon.color) < 1.2,
        "noon direct should stay near-white: {:?}",
        noon.color
    );
    assert!(warmth(rig.color) > 1.3 * warmth(noon.color));
    // Deeper into the sunset the gold keeps deepening.
    let later = resolve_sun(&fab_sun_config(21.3));
    assert!(
        warmth(later.color) > warmth(rig.color),
        "21:18 {:?} vs 20:00 {:?}",
        later.color,
        rig.color
    );
    // Below the true horizon the direct term still goes out.
    let night = resolve_sun(&fab_sun_config(23.0));
    assert!(
        luminance(night.color) < 1.0e-3,
        "night direct: {:?}",
        night.color
    );
}

/// Same contract in the other convention: an explicit dir built in a z-up
/// host and converted with fab's `to_render` mapping lands on the same
/// render-space sun as building it in render space directly.
#[test]
fn the_conventions_agree_on_the_same_sky() {
    for hour in [8.0f32, 14.0, 18.5, 20.0] {
        let (elevation, azimuth) = noaa_solar_position(DATE, hour, TZ, LAT, LON);
        let (el, az) = (elevation.to_radians(), azimuth.to_radians());
        // Fab space: x east, y north, z up.
        let fab = vec3(az.sin() * el.cos(), az.cos() * el.cos(), el.sin());
        let to_render = vec3(fab.x, fab.z, -fab.y);
        assert!(
            angle_deg(to_render, noaa_render_dir(hour)) < 1.0e-3,
            "{hour}h"
        );
    }
}

/// A clear sky with its sun placed by hand at (azimuth, elevation), no clouds.
fn manual_sky(azimuth_deg: f32, elevation_deg: f32) -> HdriParams {
    let mut p = HdriParams::default();
    p.sky.sun.mode = "manual".to_string();
    p.sky.sun.azimuth_deg = azimuth_deg;
    p.sky.sun.elevation_deg = elevation_deg;
    p.sky.clouds.coverage = 0.0;
    p.clamp();
    p
}

/// `bake_env_map`'s row runner on the test thread.
fn serial(n: usize, f: &(dyn Fn(usize) + Sync)) {
    for i in 0..n {
        f(i);
    }
}

/// The lighting the renderer lends a map at `Ibl.intensity` 1: the gain is
/// the intensity (the environment's own scale, never HDR_SKY_GAIN).
fn lighting_of(map: &ibl::EnvMap) -> EnvLighting {
    EnvLighting { sh: ibl::sh9(map), mean_luminance: hdri::image::mean_luminance(map), gain: 1.0 }
}

fn env_world(sun: Option<makepad_scene::EnvSun>, rotation_deg: f32) -> World {
    let mut world = World::new();
    world.environment.ibl = Some(Ibl { source: IblSource::Hdri(TextureRef(1)), intensity: 1.0, rotation_deg });
    world.environment.sun = sun;
    world
}

/// Phase 2's pinned contract: the sun the environment map carries is the
/// direction the light shades with — at every azimuth, elevation and map
/// rotation — and a sky lights from above.
#[test]
fn the_environments_sun_equals_the_resolved_light_direction() {
    for (az, el) in [(135.0f32, 40.0f32), (20.0, 10.0), (300.0, 65.0)] {
        let (map, sun) = bake_env_map(&manual_sky(az, el), 64, serial);
        let sun = sun.expect("a sky with the sun up reports its sun");
        // 0.05 degrees: this file's f32 acos cannot resolve less than 0.02.
        assert!(angle_deg(sun.dir, hdri::dir_from_az_el(az, el)) < 0.05, "the bake reports where it drew the sun");
        // The renderer lights with the map's lighting copy: the sun's cone
        // filled with the sky around it, because the directional light
        // carries the sun (renderer/ibl.rs).
        let mut lit = map.clone();
        remove_sun(&mut lit, &sun);
        let lighting = lighting_of(&lit);
        for rotation in [0.0f32, 90.0, -45.0] {
            let world = env_world(Some(sun), rotation);
            let want = hdri::rotate_y(hdri::dir_from_az_el(az, el), rotation);
            let dir = env_sun_dir(&world).expect("the environment places the sun");
            assert!(angle_deg(dir, want) < 0.05, "az {az} el {el} rot {rotation}: {dir:?} vs {want:?}");
            let rig = env_sun_rig(&world, Some(&lighting), resolve_sun(&world.sun).to_hdr(), true);
            assert!(angle_deg(rig.dir, want) < 0.2, "the rig follows the map's sun: {:?} vs {want:?}", rig.dir);
            assert!(luminance(rig.color) > 0.0, "the sun lights");
            // The fill comes from the right hemispheres: the up term is the blue sky's,
            // the down term the warm ground's (not "the sky is brighter": a sunlit
            // ground out-shines a clear sky at 40 degrees).
            assert!(rig.sky.z > rig.sky.x, "the up term is the sky's: {:?}", rig.sky);
            assert!(rig.ground.x > rig.ground.z, "the down term is the ground's: {:?}", rig.ground);
            assert!(rig.color.is_finite() && rig.sky.is_finite() && rig.ground.is_finite());
        }
    }
}

/// An authored direction (Fab's NOAA sun for the hour) is not the map's to
/// override; the map still supplies the colour and the fill.
#[test]
fn an_authored_sun_direction_beats_the_environment() {
    let (map, sun) = bake_env_map(&manual_sky(135.0, 40.0), 64, serial);
    let mut world = env_world(sun, 0.0);
    world.sun = fab_sun_config(14.0);
    assert!(env_sun_dir(&world).is_none());
    let input = resolve_sun(&world.sun).to_hdr();
    let rig = env_sun_rig(&world, Some(&lighting_of(&map)), input, true);
    assert_eq!(rig.dir, input.dir, "the authored direction passes through bit for bit: never quantised");
    assert!(angle_deg(rig.dir, world.sun.dir.unwrap()) < 0.05);
    assert!(luminance(rig.color) > 0.0);
}

/// The exposure is metered from the map's mean at the environment's own
/// scale × `Ibl.intensity` (the gain): a brighter map (a higher IBL
/// intensity) meters darker, within the rig's adaptation band.
#[test]
fn the_environment_meters_the_exposure_from_its_mean() {
    let (map, _) = bake_env_map(&manual_sky(180.0, 45.0), 64, serial);
    let lighting = lighting_of(&map);
    assert!(lighting.mean_luminance > 0.0 && lighting.mean_luminance.is_finite());
    let one = env_exposure(&lighting);
    assert!((0.25..=3.2).contains(&one), "{one}");
    // An intensity that puts the key (mean × intensity) at 1 meters the key
    // 0.75 / 1; twice that intensity meters exactly one stop darker (both
    // inside the band, whatever the bake's absolute level).
    let unit = env_exposure(&EnvLighting { gain: 1.0 / lighting.mean_luminance, ..lighting });
    let twice = env_exposure(&EnvLighting { gain: 2.0 / lighting.mean_luminance, ..lighting });
    assert!((unit - 0.75).abs() < 1.0e-3, "{unit}");
    assert!((twice * 2.0 - unit).abs() < 1.0e-3, "one stop: {unit} vs {twice}");
}

/// What a surface facing `dir` receives from `map`, by quadrature over the
/// texels' exact solid angles: Σ L(d) max(d·dir, 0) dΩ, in luminance.
fn irradiance_toward(map: &ibl::EnvMap, dir: Vec3f) -> f32 {
    use std::f64::consts::{PI, TAU};
    let (w, h) = (map.width, map.height);
    let mut sum = 0.0f64;
    for y in 0..h {
        let (top, bottom) = (PI * y as f64 / h as f64, PI * (y + 1) as f64 / h as f64);
        let cell = TAU / w as f64 * (top.cos() - bottom.cos());
        for x in 0..w {
            let t = map.data[y * w + x];
            let d = hdri::vec(ibl::equirect_uv_to_dir([(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32]));
            sum += luminance(vec3f(t[0], t[1], t[2])) as f64 * d.dot(dir).max(0.0) as f64 * cell;
        }
    }
    sum as f32
}

/// Phase 2 amendment: the one directional light delivers what a surface
/// facing a wide studio key receives from the map. The map here is the key
/// alone (the backdrop black, no other light), so what a surface facing it
/// receives, by quadrature, is the key's whole delivery; the lane's light is
/// `colour · N·L` in irradiance / π, so π × the rig's colour must be that
/// number, `facing` included. Without the facing share the light would be
/// 1 / facing times too bright.
#[test]
fn a_wide_studio_key_lights_a_facing_surface_with_what_the_map_delivers() {
    for name in ["Overcast dome", "Top softbox"] {
        let mut params = hdri::presets::preset(name).unwrap();
        params.studio.top = [0.0; 3];
        params.studio.horizon = [0.0; 3];
        params.studio.floor = [0.0; 3];
        params.lights.retain(|l| l.enabled && l.key);
        assert_eq!(params.lights.len(), 1, "{name}: the key alone");
        let (map, key) = bake_env_map(&params, 128, serial);
        let key = key.unwrap_or_else(|| panic!("{name} has a key"));
        assert!(key.facing < 0.95, "{name}: a wide key, facing {}", key.facing);
        let delivered = irradiance_toward(&map, key.dir);
        let world = env_world(Some(key), 0.0);
        let rig = env_sun_rig(&world, Some(&lighting_of(&map)), resolve_sun(&world.sun).to_hdr(), true);
        assert!(angle_deg(rig.dir, key.dir) < 0.2);
        let lane = std::f32::consts::PI * luminance(rig.color);
        assert!((lane - delivered).abs() < 0.03 * delivered, "{name}: the light carries {lane}, the map delivers {delivered} (facing {})", key.facing);
        // The whole emission (radiance x the cone) is more than a facing surface gets.
        assert!(luminance(key.irradiance()) > lane * 1.02, "{name}");
    }
}
