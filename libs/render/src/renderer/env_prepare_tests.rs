//! HDRI phase 2: the environment preparation (prefilter atlas, dome, SH9,
//! meter) runs exactly once per key — never per frame, never per poll — a
//! re-registered map (another `Arc`) is a new key, and a world that names
//! another environment while its job runs ends with the last key's
//! texture. Device-free: `Cx::new` gives a real pool. On a closed pool
//! (wasm without atomics) every key prepares on the spot, so the flip below
//! prepares 4 times there, not 3; the counts are asserted for each.
use super::*;
use makepad_render_material::ibl::{sh9_irradiance, EnvMap};
use makepad_scene::{Environment, Ibl, IblSource, KeyReflection, TextureRef};
use std::sync::Arc;

fn env_for(source: IblSource) -> Environment {
    Environment { ibl: Some(Ibl { key_reflection: makepad_scene::KeyReflection::Light, source, intensity: 1.0, rotation_deg: 0.0 }), ..Default::default() }
}

/// A world that carries `env` (`env_lighting` takes the world).
fn world_of(env: &Environment) -> World {
    let mut w = World::new();
    w.environment = *env;
    w
}

/// Drive `resolve_ibl` until the preparation for `env` has landed. A
/// preparation is a real `prefilter(.., 256, 6)` on a debug build (the
/// workspace's dev profile sets no opt-level) while the rest of
/// `renderer::` runs in parallel: seconds each, so the guard is a clock,
/// as in renderer/ibl.rs's own tests, not a poll count.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn settle(r: &mut Renderer, cx: &mut Cx, env: &Environment) {
    let start = std::time::Instant::now();
    loop {
        r.resolve_ibl(cx, env);
        if r.ibl_texture().is_some() && !r.ibl_pending() {
            return;
        }
        assert!(start.elapsed().as_secs() < 180, "the environment never prepared");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Resolve until no job is in flight, a cancelled one winding down
/// included, so whatever it returns has been taken.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn wind_down(r: &mut Renderer, cx: &mut Cx, env: &Environment) {
    let start = std::time::Instant::now();
    while r.ibl_job_in_flight() {
        assert!(start.elapsed().as_secs() < 180, "the job did not wind down within 180 s");
        std::thread::sleep(std::time::Duration::from_millis(5));
        r.resolve_ibl(cx, env);
    }
}

#[test]
fn key_reflection_follows_the_bound_job_and_the_frames_final_sun() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut r = Renderer::default();
    let source = TextureRef(19);
    let (key, _) = super::sun_tests::preset_keys("Three-point");
    let key = key.expect("the studio has a declared key");
    let levels = crate::env_map::prepare::ATLAS_LEVELS;
    let cut = crate::env_map::prepare::key_cut_level(&key, levels);
    let keyed = Some((cut as u32, levels as u32));
    let gain = |bound: Option<(u32, u32)>| bound.map_or([0.0, 0.0], |(k, levels)| [k as f32, (levels - 1) as f32]);
    let empty = world_of(&env_for(IblSource::Hdri(source)));
    assert_eq!(r.ibl_key_levels(), None, "no key levels before the first preparation binds");
    assert_eq!(r.key_spec(&empty, key.dir), [0.0, 0.0], "an unbound renderer keeps every directional lobe");
    let keyless = Arc::new(EnvMap::constant(32, [0.2; 3]));
    let map = Arc::new(EnvMap::from_fn(32, |d| {
        let d = vec3f(d[0], d[1], d[2]);
        if d.dot(key.dir) > key.cos_radius { [10.0; 3] } else { [0.2; 3] }
    }));
    r.register_generated_environment(source, map, Some(keyless));
    let mut env = env_for(IblSource::Hdri(source));
    env.sun = Some(key);
    env.ibl.as_mut().unwrap().rotation_deg = 73.0;
    settle(&mut r, &mut cx, &env);
    assert_eq!(r.ibl_key_levels(), None, "Light binds an atlas without the declared key");
    let world = world_of(&env);
    assert_eq!(r.key_spec(&world, r.frame_sun(&world).dir), [0.0, 0.0], "Light's lobe multiplier is exactly the identity");

    let pooled = !cfg!(all(target_arch = "wasm32", not(target_feature = "atomics")));
    for mode in [KeyReflection::Map, KeyReflection::Light] {
        let old_levels = r.ibl_key_levels();
        let old_texture = r.ibl_texture().unwrap().texture_id();
        env.ibl.as_mut().unwrap().key_reflection = mode;
        r.resolve_ibl(&mut cx, &env);
        if pooled {
            assert_eq!(r.ibl_key_levels(), old_levels, "{mode:?}: the wanted mode cannot change the bound levels before adoption");
            assert_eq!(r.ibl_texture().unwrap().texture_id(), old_texture, "{mode:?}: old textures remain bound while the new job runs");
            let world = world_of(&env);
            assert_eq!(r.key_spec(&world, r.frame_sun(&world).dir), gain(old_levels), "{mode:?}: the frame's gain follows the old textures until adoption");
        }
        settle(&mut r, &mut cx, &env);
        let kept = if mode == KeyReflection::Map { keyed } else { None };
        assert_eq!(r.ibl_key_levels(), kept, "{mode:?}: the levels flip with the adopted job");
        assert_ne!(r.ibl_texture().unwrap().texture_id(), old_texture, "{mode:?}: the new mode binds its own texture");
        let mut world = world_of(&env);
        let rig = r.frame_sun(&world);
        assert_eq!(r.key_spec(&world, rig.dir), gain(kept), "{mode:?}: the rig's rotated key partitions only a Map lobe");
        // Authored directions must be compared with the map, not mistaken
        // for the map's key by the rig's authored-direction override.
        world.sun.dir = Some(rig.dir);
        assert_eq!(r.key_spec(&world, r.frame_sun(&world).dir), gain(kept), "{mode:?}: an authored aim on the rotated key uses the same gain");
        world.sun.dir = Some(vec3f(rig.dir.x, -rig.dir.y, rig.dir.z));
        assert_eq!(r.key_spec(&world, r.frame_sun(&world).dir), [0.0, 0.0], "{mode:?}: a key aimed under the plane keeps its directional lobe");
        let mut mirror = Renderer::default();
        mirror.mirror_environment_from(&r);
        assert_eq!(mirror.ibl_key_levels(), kept, "{mode:?}: a mirror copies the bound levels with its textures");
    }
    cx.use_closed_task_pool();
    env.ibl.as_mut().unwrap().key_reflection = KeyReflection::Map;
    env.sun = None;
    settle(&mut r, &mut cx, &env);
    assert_eq!(r.ibl_key_levels(), None, "Map without a declared key binds no keyed levels");
    env.sun = Some(key);
    settle(&mut r, &mut cx, &env);
    assert_eq!(r.ibl_key_levels(), keyed, "the synchronous fallback prepares and binds Map with its keyed levels");
    r.resolve_ibl(&mut cx, &Environment::default());
    assert_eq!(r.ibl_key_levels(), None, "dropping the preparation clears the bound key levels");
    let world = World::new();
    assert_eq!(r.key_spec(&world, r.frame_sun(&world).dir), [0.0, 0.0], "without a bound preparation the lobe multiplier is exactly the identity");
}

#[test]
fn a_key_change_prepares_exactly_once() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut r = Renderer::default();
    r.register_environment(TextureRef(7), Arc::new(EnvMap::constant(32, [0.5, 0.6, 0.7])));
    r.register_environment(TextureRef(8), Arc::new(EnvMap::constant(32, [0.9, 0.2, 0.1])));
    let seven = env_for(IblSource::Hdri(TextureRef(7)));
    let eight = env_for(IblSource::Hdri(TextureRef(8)));

    settle(&mut r, &mut cx, &seven);
    assert_eq!(r.ibl_preparations(), 1, "the first frame prepares");
    let first = r.ibl_texture().unwrap().texture_id();
    for _ in 0..60 {
        r.resolve_ibl(&mut cx, &seven);
    }
    assert_eq!(r.ibl_preparations(), 1, "an unchanged key never prepares again");
    assert_eq!(r.ibl_texture().unwrap().texture_id(), first, "and keeps its texture");
    assert!(r.env_lighting(&world_of(&seven)).is_some(), "the rig sees the preparation");

    settle(&mut r, &mut cx, &eight);
    assert_eq!(r.ibl_preparations(), 2, "a new source prepares once");
    let eight_texture = r.ibl_texture().unwrap().texture_id();
    assert_ne!(eight_texture, first);
    let sh = *r.ibl_sh9().expect("the landed SH");
    let e = sh9_irradiance(&sh, [0.0, 1.0, 0.0]);
    assert!(e[0] > e[2], "eight is red over blue: {e:?}");

    // The world names seven and, before its job lands, eight again: naming
    // another environment is a change of map, so seven's job is cancelled
    // (renderer/ibl.rs, K1), and eight is still bound, so nothing more is
    // submitted. The cancelled job is let wind down and whatever it returns
    // is taken (and rejected) before the counts: exactly one preparation for
    // seven, and the texture is still eight's. 4 would mean seven's result
    // landed and eight had to be prepared again.
    let pooled = !cfg!(all(target_arch = "wasm32", not(target_feature = "atomics")));
    r.resolve_ibl(&mut cx, &seven);
    settle(&mut r, &mut cx, &eight);
    wind_down(&mut r, &mut cx, &eight);
    let after_flip = r.ibl_preparations();
    if pooled {
        assert_eq!(after_flip, 3, "seven's job only");
        assert_eq!(r.ibl_texture().unwrap().texture_id(), eight_texture, "eight's texture stays bound");
    } else {
        assert_eq!(after_flip, 4, "a closed pool prepares seven on the spot, then eight again");
    }
    let sh = *r.ibl_sh9().expect("the landed SH");
    let e = sh9_irradiance(&sh, [0.0, 1.0, 0.0]);
    assert!(e[0] > e[2], "the stale result was dropped: {e:?}");
    for _ in 0..60 {
        r.resolve_ibl(&mut cx, &eight);
    }
    assert_eq!(r.ibl_preparations(), after_flip, "settled again");

    // Re-registering the CURRENT source is a change (renderer/ibl.rs drops
    // its key), so the new map prepares exactly once; until it lands the
    // rig keeps the old map's numbers (no flicker on a day-cycle re-bake).
    r.register_environment(TextureRef(8), Arc::new(EnvMap::constant(32, [0.1, 0.1, 0.9])));
    r.resolve_ibl(&mut cx, &eight);
    if pooled {
        assert!(r.ibl_pending(), "a real pool prepares the new map in the background");
        let e = sh9_irradiance(r.ibl_sh9().expect("the old numbers stay while the new map prepares"), [0.0, 1.0, 0.0]);
        assert!(e[0] > e[2], "still the old (red) map's SH: {e:?}");
        assert!(r.env_lighting(&world_of(&eight)).is_some(), "the rig does not fall back to the analytic values");
    }
    settle(&mut r, &mut cx, &eight);
    assert_eq!(r.ibl_preparations(), after_flip + 1);
    let e = sh9_irradiance(r.ibl_sh9().expect("the landed SH"), [0.0, 1.0, 0.0]);
    assert!(e[2] > e[0], "the re-registered map landed: {e:?}");
    // Re-registering a source that is NOT current prepares nothing now.
    r.register_environment(TextureRef(7), Arc::new(EnvMap::constant(32, [0.3, 0.3, 0.3])));
    for _ in 0..30 {
        r.resolve_ibl(&mut cx, &eight);
    }
    assert_eq!(r.ibl_preparations(), after_flip + 1);

    // No environment: nothing prepares and the texture is gone; naming an
    // unregistered handle is the same.
    r.resolve_ibl(&mut cx, &Environment::default());
    assert!(r.ibl_texture().is_none());
    assert!(r.env_lighting(&world_of(&eight)).is_none(), "no preparation, no rig contribution");
    r.resolve_ibl(&mut cx, &env_for(IblSource::Hdri(TextureRef(99))));
    assert!(r.ibl_texture().is_none() && !r.ibl_pending());
    assert_eq!(r.ibl_preparations(), after_flip + 1);
}
