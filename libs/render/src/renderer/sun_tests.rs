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
