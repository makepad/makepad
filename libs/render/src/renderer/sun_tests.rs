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

fn env_sun_of(dir: Vec3f, radiance: f32, radius_deg: f32, facing: f32) -> makepad_scene::EnvSun {
    let cos_radius = radius_deg.to_radians().cos();
    makepad_scene::EnvSun {
        dir: dir.normalize(),
        radiance: vec3f(radiance, radiance, radiance),
        cos_radius,
        facing,
        cos_cover: cos_radius,
    }
}

fn env_named(source: makepad_scene::IblSource, sun: Option<makepad_scene::EnvSun>) -> World {
    let mut world = World::new();
    world.environment.ibl = Some(makepad_scene::Ibl { source, intensity: 1.0, rotation_deg: 0.0 });
    world.environment.sun = sun;
    world
}

/// Phase 2: with no environment prepared, the environment's share of the
/// rig is a no-op — the legacy look stays bit for bit, also for a world
/// that names an IBL the renderer has not prepared.
#[test]
fn without_a_prepared_environment_the_rig_is_untouched() {
    let renderer = Renderer::default();
    let world = World::new();
    let sun = crate::sun::resolve_sun(&world.sun);
    assert_eq!(renderer.env_sun_rig(&world, sun), sun);
    assert_eq!(renderer.env_sun_rig(&world, sun), SunLight::default());
    assert!(renderer.env_sun_dir(&world).is_none());
    let named = env_named(
        makepad_scene::IblSource::Procedural(0),
        Some(makepad_scene::EnvSun {
            dir: vec3f(0.0, 1.0, 0.0),
            radiance: vec3f(1.0, 1.0, 1.0),
            cos_radius: 0.99,
            facing: 1.0,
            cos_cover: 0.98,
        }),
    );
    assert_eq!(renderer.env_sun_rig(&named, sun), sun, "named but unprepared: nothing changes");
    assert!(renderer.env_sun_dir(&named).is_none());
    assert!(!renderer.ibl_pending());
    assert_eq!(renderer.ibl_preparations(), 0);
}

/// The meter counts the key the way the directional light delivers it: its
/// share of the sphere's mean, `L (1 - cos r) / 2`, times what a surface
/// facing it receives (`facing`), on top of the lighting copy's mean (which
/// has the key's cone filled). Only a world that names an IBL reads the
/// renderer's numbers: an aux draw's default environment never picks up the
/// scene's.
#[test]
fn the_meter_adds_the_keys_delivered_share_back_and_only_for_a_world_that_names_an_ibl() {
    let mut renderer = Renderer::default();
    let sh = makepad_render_material::ibl::sh9(&makepad_render_material::ibl::EnvMap::constant(32, [0.5, 0.5, 0.5]));
    renderer.feed_environment_numbers_for_tests(sh, 0.5, vec3f(0.5, 0.5, 0.5));
    let world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), None);
    let sunless = renderer.env_lighting(&world).expect("a landed preparation under a world that names an IBL");
    assert_eq!(sunless.mean_luminance, 0.5, "no key, no share");
    assert_eq!(sunless.gain, 1.0);
    let wide = env_sun_of(vec3f(0.3, 0.8, -0.5), 10.0, 55.0, 0.78);
    let whole = makepad_scene::EnvSun { facing: 1.0, ..wide };
    // The key is the bound preparation's (M1), hand-fed here; the world
    // declares the same one.
    let share = |renderer: &mut Renderer, sun: makepad_scene::EnvSun, intensity: f32| {
        renderer.feed_environment_sun_for_tests(Some(sun));
        let mut world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), Some(sun));
        world.environment.ibl.as_mut().unwrap().intensity = intensity;
        renderer.env_lighting(&world).expect("lighting").mean_luminance - 0.5
    };
    let want = 10.0 * (1.0 - 55.0_f32.to_radians().cos()) * 0.5;
    let got = share(&mut renderer, whole, 1.0);
    assert!((got - want).abs() < 1.0e-4, "{got} vs {want}");
    let got = share(&mut renderer, wide, 1.0);
    assert!((got - want * 0.78).abs() < 1.0e-4, "the share carries the key's facing: {got}");
    // The intensity is the gain, not part of the mean (the meter multiplies it).
    assert!((share(&mut renderer, wide, 2.0) - want * 0.78).abs() < 1.0e-4);
    assert_eq!(renderer.env_lighting(&world).unwrap().gain, 1.0);
    // What the world declares is not what lights: with no key in the bound
    // preparation, a declared one adds no share (it lights once its own
    // preparation lands).
    renderer.feed_environment_sun_for_tests(None);
    let declares = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), Some(wide));
    assert_eq!(renderer.env_lighting(&declares).unwrap().mean_luminance, 0.5);
    // A world that names none gets nothing, whatever the scene last bound.
    let aux = World::new();
    assert!(renderer.env_lighting(&aux).is_none());
    let sun = crate::sun::resolve_sun(&aux.sun);
    assert_eq!(renderer.env_sun_rig(&aux, sun), sun);
    assert!(renderer.env_sun_dir(&aux).is_none());
}

/// Resolve until no preparation is pending (a real pool job; the clock and
/// the sleep are the test's: it runs natively).
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn settle_world(renderer: &mut Renderer, cx: &mut Cx, world: &World) {
    let start = std::time::Instant::now();
    loop {
        renderer.resolve_ibl(cx, &world.environment);
        if !renderer.ibl_pending() {
            return;
        }
        assert!(start.elapsed().as_secs() < 180, "the preparation did not finish within 180 s");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// The whole path on a real preparation: a registered map with a hot disc
/// and the sun declared for it. The frame's rig points at the sun, carries
/// its colour and the map's fill, meters the map (disc included) in both
/// lanes, and a world that no longer declares the sun keeps the bound one
/// until the new preparation lands (the dome and the light are one map).
#[test]
fn a_landed_environment_lights_the_rig_in_both_lanes() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut renderer = Renderer::default();
    let toward = crate::hdri::dir_from_az_el(90.0, 45.0);
    let sun = env_sun_of(toward, 1.0e3, 3.0, 1.0);
    let map = makepad_render_material::ibl::EnvMap::from_fn(128, |d| {
        if crate::hdri::vec(d).dot(toward) >= sun.cos_radius { [1.0e3; 3] } else { [0.5; 3] }
    });
    renderer.register_environment(makepad_scene::TextureRef(1), std::sync::Arc::new(map));
    let mut world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), Some(sun));
    settle_world(&mut renderer, &mut cx, &world);
    assert_eq!(renderer.ibl_preparations(), 1);
    let stock = SunLight::default();

    // The lighting copy's mean is the 0.5 sky (the cone is filled); the key's
    // share, L (1 - cos r) / 2 = 0.69, is added back for the meter.
    let key_share = 1.0e3 * (1.0 - sun.cos_radius) * 0.5;
    let lighting = renderer.env_lighting(&world).expect("landed");
    assert!((lighting.mean_luminance - (0.5 + key_share)).abs() < 0.02, "{} vs {}", lighting.mean_luminance, 0.5 + key_share);
    assert_eq!(renderer.ibl_sun(), Some(sun));
    let dir = renderer.env_sun_dir(&world).expect("the environment places the sun");
    assert!((dir - toward).length() < 1.0e-4, "{dir:?}");

    // HDR lane: the map's own units, the composite exposes.
    renderer.hdr_output = true;
    let hdr = renderer.env_sun_rig(&world, stock.to_hdr());
    assert!(hdr.dir.dot(toward) > 0.9999, "{:?}", hdr.dir);
    let want = 1.0e3 * 2.0 * (1.0 - sun.cos_radius);
    assert!((hdr.color.x - want).abs() < 2.0e-3 * want, "direct {:?} vs {want}", hdr.color);
    assert!((hdr.sky.x - 0.5).abs() < 0.01 && (hdr.ground.x - 0.5).abs() < 0.01, "the fill is the sky's: {:?} {:?}", hdr.sky, hdr.ground);
    let exposure = crate::sun::env_exposure(&lighting);
    assert!((exposure - 0.75 / (0.5 + key_share)).abs() < 1.0e-3, "meters the map, the disc included: {exposure}");
    // Legacy lane: the fill exposed in the values; the sun exposed too, but
    // held within white next to that fill (hdr.color.x * exposure = 1.74 would
    // clip: no tone mapper in this lane), so a wall facing it reads 1.0.
    renderer.hdr_output = false;
    let legacy = renderer.env_sun_rig(&world, stock);
    assert!((legacy.sky.x - hdr.sky.x * exposure).abs() < 2.0e-3, "{:?}", legacy.sky);
    assert!(hdr.color.x * exposure > 1.0, "premise: this sun would clip when exposed");
    assert!((legacy.color.x + legacy.sky.x - 1.0).abs() < 5.0e-4, "{:?} {:?}", legacy.color, legacy.sky);
    assert_eq!(legacy.dir, hdr.dir);
    // N2: the drop shadows go with the map's key, at its share of the light
    // in each lane (the stock rig's 0.35 is what a key with all of it casts).
    let lum = crate::sky::luminance;
    for rig in [hdr, legacy] {
        let share = lum(rig.color) / (lum(rig.color) + lum(rig.sky));
        assert!((rig.shadow_alpha - stock.shadow_alpha * share).abs() < 1.0e-6, "{rig:?}");
    }

    // An authored colour and ambient are the script's, in the lane's units.
    let mut authored = world.clone();
    authored.sun.color = Some(vec3f(0.3, 0.2, 0.1));
    authored.sun.ambient = Some(vec3f(0.05, 0.05, 0.05));
    let rig = renderer.env_sun_rig(&authored, crate::sun::resolve_sun(&authored.sun));
    assert_eq!((rig.color, rig.sky, rig.ground), (vec3f(0.3, 0.2, 0.1), vec3f(0.05, 0.05, 0.05), vec3f(0.05, 0.05, 0.05)));
    assert!(rig.dir.dot(toward) > 0.9999, "the map still places the sun");

    // The world stops declaring the sun: a new preparation starts, and the
    // bound one (the old sun filled in its lighting copy) keeps lighting.
    world.environment.sun = None;
    renderer.resolve_ibl(&mut cx, &world.environment);
    assert!(renderer.ibl_pending());
    assert_eq!(renderer.ibl_sun(), Some(sun), "one key behind the world until the new preparation lands");
    renderer.hdr_output = true;
    assert!(renderer.env_sun_rig(&world, stock.to_hdr()).color.x > 0.0);
    settle_world(&mut renderer, &mut cx, &world);
    assert_eq!(renderer.ibl_preparations(), 2);
    assert_eq!(renderer.ibl_sun(), None);
    assert_eq!(renderer.env_sun_rig(&world, stock.to_hdr()).color, Vec3f::default(), "no key: no direct light, never the analytic colour");
    assert!(renderer.env_sun_dir(&world).is_none(), "the shadows stay on the rig's own direction");
}

/// M1: light and sky come from the same preparation. While one is bound,
/// the rig lights with that preparation's own key (`ibl_sun`), never with
/// the key the world declares meanwhile: a world that moves its key from A
/// to B keeps A's direction, colour and meter until B's preparation lands,
/// and then lights with B.
#[test]
fn the_rig_lights_with_the_bound_preparations_key_until_the_next_lands() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut renderer = Renderer::default();
    renderer.hdr_output = true;
    let a = env_sun_of(crate::hdri::dir_from_az_el(90.0, 45.0), 1.0e3, 3.0, 1.0);
    let b = env_sun_of(crate::hdri::dir_from_az_el(200.0, 30.0), 2.0e3, 3.0, 1.0);
    // A plain map: either key's filled cone is the same grey, so the two
    // preparations differ only in the key they were made with.
    let map = makepad_render_material::ibl::EnvMap::constant(64, [0.5, 0.5, 0.5]);
    renderer.register_environment(makepad_scene::TextureRef(1), std::sync::Arc::new(map));
    let mut world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), Some(a));
    settle_world(&mut renderer, &mut cx, &world);
    let stock = SunLight::default().to_hdr();
    let lit_a = renderer.env_sun_rig(&world, stock);
    assert!(lit_a.dir.dot(a.dir) > 0.9999 && lit_a.color.x > 0.0, "{lit_a:?}");
    let meter_a = renderer.env_lighting(&world).unwrap().mean_luminance;

    world.environment.sun = Some(b);
    renderer.resolve_ibl(&mut cx, &world.environment);
    assert!(renderer.ibl_pending(), "B prepares");
    assert_eq!(renderer.ibl_sun(), Some(a));
    assert_eq!(renderer.env_sun_rig(&world, stock), lit_a, "A lights until B lands");
    assert!(renderer.env_sun_dir(&world).is_some_and(|d| d.dot(a.dir) > 0.9999), "the shadows follow A too");
    assert_eq!(renderer.env_lighting(&world).unwrap().mean_luminance, meter_a, "and A meters");

    settle_world(&mut renderer, &mut cx, &world);
    assert_eq!(renderer.ibl_sun(), Some(b));
    let lit_b = renderer.env_sun_rig(&world, stock);
    assert!(lit_b.dir.dot(b.dir) > 0.9999 && lit_b.color.x > lit_a.color.x, "B lights once it lands: {lit_b:?}");

    // A key that is not valid is not declared at all: it prepares as none
    // (no directional light), never as a key.
    world.environment.sun = Some(makepad_scene::EnvSun { facing: 2.0, ..b });
    settle_world(&mut renderer, &mut cx, &world);
    assert_eq!(renderer.ibl_sun(), None);
    assert_eq!(renderer.env_sun_rig(&world, stock).color, Vec3f::default());
}

/// A street lamp, harvested the way `harvest_lamps` sizes one.
fn street_lamp() -> crate::lightmap::LmLight {
    let (radius, strength) = crate::lightmap::lamp_photometry(2.82);
    crate::lightmap::LmLight {
        pos: vec3f(0.0, 2.82, 0.0),
        color: vec3f(strength, strength * 0.775, strength * 0.475),
        radius,
        dir: vec3f(0.0, -1.0, 0.0),
        spot: 1.0,
        ..Default::default()
    }
}

/// A renderer in the given lane with one street lamp and a landed
/// preparation hand-fed: a flat map of `level`, its key, its own sun.
fn lamp_renderer(hdr: bool, clustered: bool, level: f32, key: Option<makepad_scene::EnvSun>, daylight: Option<Vec3f>) -> Renderer {
    let mut r = Renderer::default();
    r.set_hdr_output(hdr);
    r.set_clustered_lighting(clustered);
    r.set_static_lights(vec![street_lamp()]);
    let sh = makepad_render_material::ibl::sh9(&makepad_render_material::ibl::EnvMap::constant(32, [level, level, level]));
    r.feed_environment_numbers_for_tests(sh, level, vec3f(level, level, level));
    r.feed_environment_sun_for_tests(key);
    r.feed_environment_daylight_for_tests(daylight);
    r
}

/// N1: a moon key lights the scene and steers its light (and the cascades),
/// but the "is it day?" switches follow the real sun, which the moonlit
/// preset reports 30 degrees under the horizon: in the clustered HDR lane
/// the street lamps' photocell is fully on and the analytic sky (a host
/// background) reads night. Read from the moon's direction, as the frame did,
/// the lamps were off and the sky was a day sky at the moon's place. The
/// world's own clock says 14:00 here: the environment's sun decides.
#[test]
fn a_moon_key_lights_the_scene_while_the_lamps_and_the_sky_follow_the_sun() {
    let preset = crate::hdri::Env::new(&crate::hdri::presets::preset("Moonlit night").unwrap());
    let (moon, sun_dir) = (preset.sun().expect("the moon is the key"), preset.sun_dir().expect("a sky has its sun"));
    let mut r = lamp_renderer(true, true, 0.02, Some(moon), Some(sun_dir));
    let mut world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), Some(moon));
    world.environment.daylight_sun = Some(sun_dir);
    world.sky = Some(makepad_scene::SkyConfig::default());
    world.sun.time_of_day = Some(14.0);
    world.sun.latitude = 52.0;
    let (sun, daylight) = r.frame_lamps(&world, Vec3f::default());
    assert!(sun.dir.dot(moon.dir) > 0.9999, "the moon aims the light: {:?}", sun.dir);
    let rig = r.lane_rig(&world, sun);
    assert!(rig.dir.dot(moon.dir) > 0.999 && rig.color.x > 0.0, "and lights: {rig:?}");
    assert!(daylight.dot(sun_dir) > 0.9999, "the switches read the sun: {daylight:?}");
    assert_eq!(r.frame_lights[0].color, street_lamp().color, "the photocell is fully on");
    let sky = r.frame_sky(&world, &crate::world_lights::apply_world_sun(&world, rig), daylight, true).expect("the analytic sky");
    assert!(sky.zenith.w > 0.95, "the analytic sky reads night: {}", sky.zenith.w);
    // The moon's own direction says day: what the switches must not read.
    assert_eq!(Renderer::lamp_photocell(moon.dir), 0.0);
    assert!(analytic_sky_frame(&world, moon.dir, true, true, false).unwrap().zenith.w < 0.05);
}

/// N1: a map that knows no sun of its own (a studio, a loaded file) leaves
/// the switches on the world's own sun, as without an environment: a studio
/// key 30 or more degrees up lights a world whose clock says 23:00, and the
/// lamps are on; at 13:00 they are off.
#[test]
fn a_studio_map_leaves_the_daylight_switches_on_the_worlds_sun() {
    let preset = crate::hdri::Env::new(&crate::hdri::presets::preset("Three-point").unwrap());
    let key = preset.sun().expect("a studio key");
    assert!(preset.sun_dir().is_none() && Renderer::lamp_photocell(key.dir) == 0.0, "premise: the key alone would say day: {key:?}");
    let mut r = lamp_renderer(true, true, 0.2, Some(key), None);
    let mut world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), Some(key));
    world.sun.latitude = 52.0;
    world.sun.time_of_day = Some(23.0);
    let (sun, daylight) = r.frame_lamps(&world, Vec3f::default());
    assert!(sun.dir.dot(key.dir) > 0.9999, "the key aims the light");
    assert_eq!(daylight, crate::sun::resolve_sun(&world.sun).dir, "the world's own sun");
    assert!(daylight.y < 0.0);
    assert_eq!(r.frame_lights[0].color, street_lamp().color, "night by the world's clock: lamps on");
    world.sun.time_of_day = Some(13.0);
    let (_, daylight) = r.frame_lamps(&world, Vec3f::default());
    assert!(daylight.y > 0.5);
    assert_eq!(r.frame_lights[0].color, Vec3f::default(), "day by the world's clock: lamps off");
}

/// N1, one sun in the sky: the analytic sky the frame draws (a host
/// background) puts its sun where the light comes from whenever the light
/// is the sun: a world's own Sun, or a key that is the map's own sun, also
/// when an authored `SunConfig.dir` aims it (a look, or a host's eased
/// clock: the sandbox authors its eased sun while the map's report steps on
/// its bake grid (a quarter hour, 7.5 minutes while the sun is low) a
/// preparation late, and a disc drawn there would not be where the shadows
/// come from). A moon key or none, aimed or not,
/// leaves the sky on the map's own sun, so a moonlit night stays night;
/// without an environment the sky is the rig's, as before plan 2.
#[test]
fn the_frames_analytic_sky_puts_its_sun_where_the_light_is_when_the_key_is_the_sun() {
    // The frame's chain: the switches and the lamps, the lane's rig, the
    // world's own lights, then the sky. (light, the sky's sun, night blend)
    let frame_sky = |r: &mut Renderer, world: &World| {
        let (sun, daylight) = r.frame_lamps(world, Vec3f::default());
        let rig = crate::world_lights::apply_world_sun(world, r.lane_rig(world, sun));
        let sky = r.frame_sky(world, &rig, daylight, true).expect("the analytic sky");
        (rig.dir, vec3f(sky.sun_true.x, sky.sun_true.y, sky.sun_true.z), sky.zenith.w)
    };
    let preset = |name: &str| {
        let env = crate::hdri::Env::new(&crate::hdri::presets::preset(name).unwrap());
        (env.sun(), env.sun_dir().expect("a sky has its sun"))
    };
    let sky_world = |key: Option<makepad_scene::EnvSun>, daylight: Vec3f| {
        let mut world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), key);
        world.environment.daylight_sun = Some(daylight);
        world.sky = Some(makepad_scene::SkyConfig::default());
        world.sun.latitude = 52.0;
        world.sun.time_of_day = Some(14.0);
        world
    };
    let aimed = crate::hdri::dir_from_az_el(120.0, 20.0);

    // Golden hour: the key is the map's sun. Aimed elsewhere by an authored
    // direction, the light and the sky's sun go there together.
    let (key, sun_dir) = preset("Golden hour");
    let mut r = lamp_renderer(true, true, 0.5, key, Some(sun_dir));
    let mut world = sky_world(key, sun_dir);
    world.sun.dir = Some(aimed);
    let (light, sky, _) = frame_sky(&mut r, &world);
    assert!(sun_dir.dot(aimed) < 0.9, "premise: the map's sun is elsewhere");
    assert!(light.dot(aimed) > 0.9999, "premise: the authored direction aims the light: {light:?}");
    assert!(sky.dot(aimed) > 0.9999, "the sky's sun is the light's: {sky:?}, not the map's {sun_dir:?}");
    // Not aimed: the key aims the light, and the sky takes the light's own
    // direction (on the rig's grid), not the report's.
    world.sun.dir = None;
    let (light, sky, _) = frame_sky(&mut r, &world);
    assert!(light.dot(sun_dir) > 0.9999, "the key aims the light: {light:?}");
    assert!((sky - light).length() < 1.0e-6, "the sky's sun is the light's: {sky:?} vs {light:?}");
    // A world's own Sun steers the sky as it steers the rig.
    let own = vec3f(0.3, 0.6, -0.5).normalize();
    world.lights.push(makepad_scene::Light::Sun { dir: own, color: vec3f(1.0, 1.0, 1.0), lux: 1.0, shadow: Default::default() });
    let (light, sky, _) = frame_sky(&mut r, &world);
    assert!(light.dot(own) > 0.9999 && sky.dot(own) > 0.9999, "{light:?} {sky:?}");

    // Moonlit night: the moon is the key, the map's sun is 30 degrees down.
    // Aimed or not, the light is the moon's and the sky is the night's.
    let (moon, sun_dir) = preset("Moonlit night");
    let mut r = lamp_renderer(true, true, 0.02, moon, Some(sun_dir));
    let mut world = sky_world(moon, sun_dir);
    for dir in [None, Some(aimed)] {
        world.sun.dir = dir;
        let (light, sky, night) = frame_sky(&mut r, &world);
        assert!(light.dot(dir.unwrap_or(moon.unwrap().dir)) > 0.9999, "{dir:?}: the light: {light:?}");
        assert!(sky.dot(sun_dir) > 0.9999 && night > 0.95, "{dir:?}: the sky is the night's: {sky:?} at {night}");
    }

    // Blue hour: no key at all. The sky stays on the map's sun.
    let (none, sun_dir) = preset("Blue hour");
    assert!(none.is_none(), "premise: no key at blue hour: {none:?}");
    let mut r = lamp_renderer(true, true, 0.05, None, Some(sun_dir));
    let mut world = sky_world(None, sun_dir);
    world.sun.dir = Some(aimed);
    let (_, sky, _) = frame_sky(&mut r, &world);
    assert!(sky.dot(sun_dir) > 0.9999, "the map's sun: {sky:?}");

    // No environment: the rig's own direction, as before plan 2.
    let mut world = World::new();
    world.sky = Some(makepad_scene::SkyConfig::default());
    world.sun.latitude = 52.0;
    world.sun.time_of_day = Some(14.0);
    let mut r = Renderer::default();
    r.set_hdr_output(true);
    let (light, sky, _) = frame_sky(&mut r, &world);
    assert_eq!(light, crate::sun::resolve_sun(&world.sun).dir);
    assert!((sky - light).length() < 1.0e-6, "{sky:?} vs {light:?}");
}

/// M7: the direction the frame lights from, for a host that tests against
/// the light itself (the sandbox's held view model has no shadow map, so it
/// ray-tests the eye toward this): the frame's final rig direction, in
/// either lane. Under a preset it is the map's key, the sun's or the moon's,
/// where the shadows come from; an authored direction (the sandbox's eased
/// sun) and a world's own Sun aim it as they aim the light; without an
/// environment it is the rig's own, bit for bit. The world's analytic sun
/// (`resolve_sun`), which the sandbox's ray took, is elsewhere under a map:
/// a held model sunlit inside the key's shadow, or the reverse.
#[test]
fn the_frames_sun_direction_is_where_its_light_comes_from() {
    // The frame's chain: the switches and the lamps, the lane's rig, the
    // world's own lights.
    let frame_dir = |r: &mut Renderer, world: &World| {
        let (sun, _) = r.frame_lamps(world, Vec3f::default());
        crate::world_lights::apply_world_sun(world, r.lane_rig(world, sun)).dir
    };
    let preset = |name: &str| {
        let env = crate::hdri::Env::new(&crate::hdri::presets::preset(name).unwrap());
        (env.sun().expect("a keyed preset"), env.sun_dir().expect("a sky has its sun"))
    };
    let aimed = crate::hdri::dir_from_az_el(120.0, 20.0);
    let own = vec3f(0.3, 0.6, -0.5).normalize();
    for hdr in [false, true] {
        for (name, level) in [("Golden hour", 0.5), ("Moonlit night", 0.02)] {
            let (key, sun_dir) = preset(name);
            let mut r = lamp_renderer(hdr, true, level, Some(key), Some(sun_dir));
            let mut world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), Some(key));
            world.environment.daylight_sun = Some(sun_dir);
            world.sun.latitude = 52.0;
            world.sun.time_of_day = Some(14.0);
            let at = format!("{name}, hdr {hdr}");
            assert!(crate::sun::resolve_sun(&world.sun).dir.dot(key.dir) < 0.99, "{at}: premise: the world's analytic sun is elsewhere");
            let dir = r.frame_sun_dir(&world);
            assert!(dir.dot(key.dir) > 0.9999, "{at}: the key's direction: {dir:?}, the key {:?}", key.dir);
            assert_eq!(dir, frame_dir(&mut r, &world), "{at}: the frame's own");
            world.sun.dir = Some(aimed);
            let dir = r.frame_sun_dir(&world);
            assert!(dir.dot(aimed) > 0.9999, "{at}: an authored direction aims it: {dir:?}");
            assert_eq!(dir, frame_dir(&mut r, &world), "{at}: aimed, the frame's own");
            world.lights.push(makepad_scene::Light::Sun { dir: own, color: vec3f(1.0, 1.0, 1.0), lux: 1.0, shadow: Default::default() });
            let dir = r.frame_sun_dir(&world);
            assert!(dir.dot(own) > 0.9999, "{at}: a world's own Sun aims it: {dir:?}");
            assert_eq!(dir, frame_dir(&mut r, &world), "{at}: a world Sun, the frame's own");
        }
    }
    // No environment: the rig's own direction, bit for bit.
    let mut world = World::new();
    world.sun.latitude = 52.0;
    world.sun.time_of_day = Some(9.0);
    let mut r = Renderer::default();
    assert_eq!(r.frame_sun_dir(&world), crate::sun::resolve_sun(&world.sun).dir);
    assert_eq!(r.frame_sun_dir(&world), frame_dir(&mut r, &world));
}

/// I1c: the frame's lamps are railed against the rig the frame lights
/// with, the environment's light folded in, which is the rig the bake
/// snapshots (legacy lane, the lamp atlas path: MAKEPAD_CLUSTERED=0). A
/// bright map over a world whose own clock says midnight: the analytic rig
/// would leave the lamps their whole pool, the environment's daylight less,
/// and the per-frame lamps and the baked pools must agree on which.
#[test]
fn the_frame_lamps_and_the_bake_take_one_rig() {
    let key = env_sun_of(crate::hdri::dir_from_az_el(200.0, 45.0), 6.0e3, 0.27, 1.0);
    let mut r = lamp_renderer(false, false, 0.5, Some(key), Some(key.dir));
    let mut world = env_named(makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)), Some(key));
    world.environment.daylight_sun = Some(key.dir);
    world.sun.latitude = 52.0;
    world.sun.time_of_day = Some(0.0);
    let (sun, _) = r.frame_lamps(&world, Vec3f::default());
    // What the frame hands the shaders, the cascades and the bake (no world
    // Sun or Sky here, so the world's lights change nothing).
    let bake = crate::world_lights::apply_world_sun(&world, r.lane_rig(&world, sun));
    assert_ne!(Renderer::legacy_daylight_key(&crate::sun::resolve_sun(&world.sun)), r.lamp_daylight_key(&bake), "premise: the analytic midnight rails the lamps otherwise");
    assert_eq!(r.lamp_cache_rev.map(|k| k.1), Some(r.lamp_daylight_key(&bake)), "the lamps are keyed on the bake's rig");
    assert_eq!(r.frame_lights[0].color, r.static_lights_for(&bake)[0].color, "the same lamp, seen twice");
}

/// A prepared environment that is the background colours the host's fog
/// and feeds the rig; an authored `Fog`, an MR stage, a missing preparation
/// and a background that is not the environment leave the fog alone.
#[test]
fn a_prepared_environment_colours_the_host_fog_only() {
    use makepad_render_material::ibl::{sh9, EnvMap};
    let mut r = Renderer::default();
    r.set_hdr_output(true);
    // 128 wide: sh9's midpoint quadrature is 0.4 % high at 32 wide (1.5 %
    // at 16), outside the 2e-3 asserts below.
    let map = EnvMap::constant(128, [1.0, 1.0, 1.0]);
    r.feed_environment_numbers_for_tests(sh9(&map), 1.0, vec3f(0.5, 0.6, 0.7));
    let mut world = World::new();
    world.environment.ibl = Some(makepad_scene::Ibl {
        source: makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)),
        intensity: 1.0,
        rotation_deg: 0.0,
    });
    // K4: the fog is the dome's horizon, so it is the environment's only
    // where the environment is the sky. Under the host's background (the
    // analytic sky or a colour) the host's fog stands, whatever lights.
    assert!(r.env_fog_color(&world, true).is_none(), "Background::Host keeps the host's fog");
    world.environment.background = makepad_scene::Background::Color(vec4(0.2, 0.3, 0.4, 1.0));
    assert!(r.env_fog_color(&world, true).is_none(), "so does a colour background");
    world.environment.background = makepad_scene::Background::Environment { blur: 0.0, intensity: 1.0 };
    // HDR lane: the band at the map's own scale × Ibl.intensity 1, exactly
    // (HDR_SKY_GAIN is the analytic sky's and never reaches the
    // environment's fog).
    assert_eq!(r.env_fog_color(&world, true), Some(vec3f(0.5, 0.6, 0.7)));
    assert!(r.env_fog_color(&world, false).is_none(), "MR: the room supplies the horizon");
    // Legacy lane: the fog is the legacy dome's own horizon, i.e. the band
    // through the dome's tone map at the dome's exposure (metered mean
    // 1.0 × intensity 1, no exposure_ev: EXPOSURE_KEY / 1.0).
    r.set_hdr_output(false);
    assert_eq!(
        r.env_fog_color(&world, true),
        Some(crate::sun::legacy_dome_rgb(vec3f(0.5, 0.6, 0.7), crate::sky::EXPOSURE_KEY)),
        "the legacy fog meets the legacy dome"
    );
    // At intensity 2 the dome shows twice the radiance and meters twice the
    // mean: the legacy fog is the band × 2 at EXPOSURE_KEY / 2, as the dome.
    world.environment.ibl.as_mut().unwrap().intensity = 2.0;
    assert_eq!(
        r.env_fog_color(&world, true),
        Some(crate::sun::legacy_dome_rgb(vec3f(0.5, 0.6, 0.7) * 2.0, crate::sky::EXPOSURE_KEY / 2.0))
    );
    world.environment.ibl.as_mut().unwrap().intensity = 1.0;
    r.set_hdr_output(true);
    world.environment.fog = makepad_scene::Fog::Exp2 { color: vec3f(1.0, 1.0, 1.0), density: 0.01 };
    assert!(r.env_fog_color(&world, true).is_none(), "an authored fog keeps its colour");
    world.environment.fog = makepad_scene::Fog::Host;
    // The same preparation feeds the rig: a white map, no sun -> fill only,
    // E/π = 1.0 at the map's own scale.
    let rig = r.env_sun_rig(&world, SunLight::default().to_hdr());
    assert_eq!(rig.color, Vec3f::default());
    assert!((rig.sky.x - 1.0).abs() < 2.0e-3, "{:?}", rig.sky);
    // Intensity is the gain, so fill and fog move together.
    world.environment.ibl.as_mut().unwrap().intensity = 2.0;
    assert_eq!(r.env_fog_color(&world, true), Some(vec3f(0.5, 0.6, 0.7) * 2.0));
    assert!((r.env_sun_rig(&world, SunLight::default().to_hdr()).sky.x - 2.0).abs() < 2.0e-3);
    // A non-finite intensity is no environment at all.
    world.environment.ibl.as_mut().unwrap().intensity = f32::NAN;
    assert!(r.env_fog_color(&world, true).is_none());
    assert_eq!(r.env_sun_rig(&world, SunLight::default()), SunLight::default());
    // The meter counts the environment's sun: the preparation metered the
    // lighting copy (cone filled, 1.0 here), env_lighting adds the sun's
    // share of the sphere mean, L (1 - cos r) / 2 = 1e4 x 1e-4 / 2 = 0.5.
    world.environment.ibl.as_mut().unwrap().intensity = 1.0;
    world.environment.sun = Some(makepad_scene::EnvSun {
        dir: vec3f(0.0, 1.0, 0.0),
        radiance: vec3f(1.0e4, 1.0e4, 1.0e4),
        cos_radius: 0.9999,
        facing: 1.0,
        cos_cover: 0.9999,
    });
    r.feed_environment_sun_for_tests(world.environment.sun);
    let metered = r.env_lighting(&world).unwrap().mean_luminance;
    assert!(metered > 1.4 && metered < 1.6, "1.0 + 0.5, got {metered}");
    assert!(crate::sun::env_exposure(&r.env_lighting(&world).unwrap()) < crate::sun::env_exposure(&crate::sun::EnvLighting { mean_luminance: 1.0, ..r.env_lighting(&world).unwrap() }), "a sunny map meters darker than its sky alone");
    // And the rig lights with that sun (the bound preparation's key, as a
    // declared sun or a procedural hdri preset's baked one).
    assert_eq!(r.ibl_sun(), world.environment.sun);
    assert!(r.env_sun_rig(&world, SunLight::default().to_hdr()).color.x > 0.0);
}

/// The fog is the dome's horizon, so it takes what the dome takes: the
/// background's own intensity (the shader multiplies it in after the IBL's)
/// in both lanes; and with no preparation landed there is no environment
/// fog at all, whatever the world names.
#[test]
fn the_host_fog_takes_the_backgrounds_intensity_and_waits_for_the_preparation() {
    let mut world = World::new();
    world.environment.ibl = Some(makepad_scene::Ibl {
        source: makepad_scene::IblSource::Hdri(makepad_scene::TextureRef(1)),
        intensity: 2.0,
        rotation_deg: 0.0,
    });
    world.environment.background = makepad_scene::Background::Environment { blur: 0.0, intensity: 1.0 };
    let mut r = Renderer::default();
    r.set_hdr_output(true);
    assert!(r.env_fog_color(&world, true).is_none(), "nothing prepared yet: the host's fog stands");
    let sh = makepad_render_material::ibl::sh9(&makepad_render_material::ibl::EnvMap::constant(32, [1.0, 1.0, 1.0]));
    r.feed_environment_numbers_for_tests(sh, 1.0, vec3f(0.5, 0.6, 0.7));
    let band = vec3f(0.5, 0.6, 0.7);
    assert_eq!(r.env_fog_color(&world, true), Some(band * 2.0));
    // K4: not the environment's sky, not its fog: the host's stands.
    world.environment.background = makepad_scene::Background::Host;
    assert!(r.env_fog_color(&world, true).is_none(), "Background::Host keeps the host's fog");
    world.environment.background = makepad_scene::Background::Environment { blur: 0.0, intensity: 0.5 };
    assert_eq!(r.env_fog_color(&world, true), Some(band * 0.5 * 2.0), "HDR: the dome's radiance x Ibl.intensity x the background's");
    // Legacy: the same radiance through the dome's tone map, metered on the
    // map's mean x Ibl.intensity alone (what draw_environment_background
    // passes; the background's intensity is not part of the meter).
    r.set_hdr_output(false);
    assert_eq!(
        r.env_fog_color(&world, true),
        Some(crate::sun::legacy_dome_rgb(band * 0.5 * 2.0, crate::sky::EXPOSURE_KEY / 2.0))
    );
    // The sky's exposure compensation moves the dome's exposure, so the fog's.
    world.sky = Some(makepad_scene::SkyConfig { exposure_ev: 1.0, ..Default::default() });
    assert_eq!(
        r.env_fog_color(&world, true),
        Some(crate::sun::legacy_dome_rgb(band * 0.5 * 2.0, crate::sky::EXPOSURE_KEY / 2.0 * 2.0))
    );
}
