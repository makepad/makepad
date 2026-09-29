use super::*;

/// T7 on the game side: every game shader must read ONE sun. Before the
/// unification each shader carried its own ambient/direct constants and
/// five script blocks set the light direction by hand, so changing one
/// silently left the others behind. `write_into` is the single write
/// path — apply_sun calls it once per shader and draw_skinned_inner
/// calls it for the skinned struct, so uniformity is compiler-enforced
/// and this asserts the payload rather than eyeballing a capture.
#[test]
fn write_into_sets_every_sun_field() {
    let sun = SunLight::from_time_of_day(8.0, 52.0);
    let (mut dir, mut color, mut sky, mut ground) = (
        Vec3f::default(),
        Vec3f::default(),
        Vec3f::default(),
        Vec3f::default(),
    );
    sun.write_into(&mut dir, &mut color, &mut sky, &mut ground);
    assert_eq!(dir, sun.dir);
    assert_eq!(color, sun.color);
    assert_eq!(sky, sun.sky);
    assert_eq!(ground, sun.ground);
}

/// Two shaders fed by the same sun end up with identical values — the
/// property that used to fail silently.
#[test]
fn two_targets_receive_identical_values() {
    let sun = SunLight::from_time_of_day(17.0, 52.0);
    let mut a = [Vec3f::default(); 4];
    let mut b = [Vec3f::default(); 4];
    let [a0, a1, a2, a3] = &mut a;
    sun.write_into(a0, a1, a2, a3);
    let [b0, b1, b2, b3] = &mut b;
    sun.write_into(b0, b1, b2, b3);
    assert_eq!(a, b);
}

/// A default world must light exactly as the pre-unification shaders
/// did, so adopting SceneSun did not restyle every existing game.
#[test]
fn the_default_sun_is_the_legacy_look() {
    let sun = crate::sun::resolve_sun(&makepad_scene::SunConfig::default());
    assert_eq!(sun, SunLight::default());
    // Flat hemisphere collapses mix(ground, sky, h) to the old constant.
    assert_eq!(sun.sky, sun.ground);
}

/// A painted sky on a running clock takes the analytic dome — so it sets,
/// dusks and goes dark — tinted by its palette in daylight and neutral at
/// night; a painted sky under a fixed hour keeps its gradient.
#[test]
fn a_painted_sky_on_a_clock_runs_the_day_cycle() {
    let mut world = World::new();
    world.sky = Some(makepad_scene::SkyConfig {
        top: vec4(0.62, 0.71, 0.84, 1.0),
        ..Default::default()
    });
    world.sun.time_of_day = Some(12.0);
    world.sun.latitude = 52.0;
    let noon = crate::sun::solar_dir(12.0, 52.0);
    assert!(analytic_sky_frame(&world, noon, true, true, false).is_none(), "a fixed hour keeps the painted gradient");
    let day = analytic_sky_frame(&world, noon, true, true, true).expect("a running clock takes the analytic dome");
    assert!(day.dome_tint.x > day.dome_tint.z, "a pale, warm top tints the day dome");
    let night = analytic_sky_frame(&world, crate::sun::solar_dir(0.0, 52.0), true, true, true).unwrap();
    assert!((night.dome_tint - vec3f(1.0, 1.0, 1.0)).length() < 1.0e-4, "every night is the stock night");
    world.sky = Some(makepad_scene::SkyConfig::default());
    let stock = analytic_sky_frame(&world, noon, true, true, false).unwrap();
    assert_eq!(stock.dome_tint, vec3f(1.0, 1.0, 1.0), "the stock sky is untinted");
}
