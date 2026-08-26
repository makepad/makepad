//! One sun: the direction the SKY paints, the direction the LIGHT shades
//! with, and the rig (colour, night ramp) must all follow the same star.
//!
//! A host that supplies an explicit `dir` (Fab's NOAA solar position) must
//! see the whole frame follow it: the engine's own `time_of_day` model is a
//! fixed-declination look, and at a real site on a real date its sun can be
//! many degrees away from the true one — far enough that the engine's rig
//! called "night" while the true sun still stood golden above the horizon.

use makepad_draw::*;
use makepad_game_sim::SunConfig;
use makepad_render::sky::{luminance, noaa_solar_position, SkyDate};
use makepad_render::sun::{resolve_sun, solar_dir};

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
