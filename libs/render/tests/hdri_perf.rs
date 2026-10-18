//! Release timing for the HDRI bake against the spec's targets: a 512x256
//! preview in under 50 ms and a 1024x512 one in under 200 ms, a 4096x2048
//! map in under 10 s (all on 8 cores), and the released-edit atlas
//! (`ibl_texture`: GGX prefilter plus SH9, single-threaded) in under 1 s for
//! a 1024-wide map. An unoptimised build is many times slower, so every test
//! here is #[ignore]d and run by hand:
//!
//!     cargo test -p makepad-render --release --test hdri_perf -- --ignored --nocapture --test-threads=1
//!
//! The bake time covers Env::new (the atmosphere LUT and the cloud and star
//! layers) plus the row-parallel bake into an EnvMap: what the app's preview
//! job pays per change.

// A native-only timing test: the std clock is what it measures with.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use makepad_draw::*;
use makepad_render::hdri::presets::{preset, PRESET_NAMES};
use makepad_render::hdri::{Env, HdriParams, ImageSet, VolumetricParams};
use makepad_render_material::ibl::{self, EnvMap};
use std::time::Instant;

/// The targets are for 8 cores. The bake is row-parallel and scales close to
/// linearly, so a smaller machine gets proportionally more time; a bigger one
/// gets no less.
fn budget_ms(on_8_cores: f64) -> f64 {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8);
    on_8_cores * 8.0 / cores as f64
}

/// A pool whose `fan_out` really fans out. On the thread that made the Cx
/// (the UI thread, which it must never block) it runs serially in release and
/// asserts in debug, so the work runs on a scoped thread, the way the app runs
/// it inside a submitted Heavy job (apps/files' treemap tests do the same).
fn with_pool<R: Send>(f: impl FnOnce(&TaskPool) -> R + Send) -> R {
    let cx = Cx::new(Box::new(|_, _| {}));
    let pool = cx.task_pool();
    std::thread::scope(|scope| scope.spawn(|| f(&pool)).join().unwrap())
}

struct Timing {
    total_ms: f64,
    env_ms: f64,
    map: EnvMap,
}

fn timed_bake(pool: &TaskPool, params: &HdriParams, width: usize) -> Timing {
    let start = Instant::now();
    let env = Env::new(params);
    let env_ms = start.elapsed().as_secs_f64() * 1000.0;
    // The same `run` the app's job passes: fan_out on the Heavy lane.
    let map = env.bake_par(width, |n, f| pool.fan_out(Lane::Heavy, n, f));
    Timing { total_ms: start.elapsed().as_secs_f64() * 1000.0, env_ms, map }
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// The spec's case: a daytime sky (the default mode) with both cloud layers on.
fn cloudy_sky() -> HdriParams {
    let mut p = HdriParams::default();
    p.sky.sun.mode = "manual".to_string();
    p.sky.sun.elevation_deg = 30.0;
    p.sky.sun.azimuth_deg = 200.0;
    p.sky.clouds.coverage = 0.6;
    p.sky.clouds.cirrus = 0.4;
    p
}

/// Every bake must be the asked size, finite, non-negative, opaque and not
/// all black.
fn check_map(map: &EnvMap, width: usize, what: &str) {
    assert_eq!((map.width, map.height), (width, width / 2), "{what}: wrong size");
    assert_eq!(map.data.len(), width * (width / 2), "{what}: wrong texel count");
    assert!(
        map.data.iter().all(|t| t[0].is_finite() && t[1].is_finite() && t[2].is_finite() && t[0] >= 0.0 && t[1] >= 0.0 && t[2] >= 0.0),
        "{what}: a texel is negative or not finite"
    );
    assert!(map.data.iter().all(|t| t[3] == 1.0), "{what}: alpha is not 1");
    assert!(map.data.iter().any(|t| t[0] > 0.0 || t[1] > 0.0 || t[2] > 0.0), "{what}: the bake is black");
}

#[test]
#[ignore = "release timing; see the module comment"]
fn preview_bakes_512_under_50_ms() {
    let budget = budget_ms(50.0);
    with_pool(|pool| {
        println!("hdri_perf: {} heavy workers plus the calling thread", pool.heavy_workers());
        // The spec names a cloudy sky; every preset is timed too, so a slow
        // layer (supersampled stars, many lights) shows up by name.
        let mut cases = vec![("sky with clouds".to_string(), cloudy_sky())];
        for name in PRESET_NAMES {
            cases.push((name.to_string(), preset(name).expect("a built-in preset loads")));
        }
        // The first bake starts the workers and faults the pages in; not timed.
        let _ = timed_bake(pool, &cases[0].1, 512);
        let mut slow = Vec::new();
        for (name, params) in &cases {
            let mut totals = Vec::new();
            let mut env_times = Vec::new();
            for _ in 0..5 {
                let t = timed_bake(pool, params, 512);
                check_map(&t.map, 512, name);
                totals.push(t.total_ms);
                env_times.push(t.env_ms);
            }
            let ms = median(totals);
            println!(
                "hdri_perf: 512x256 {name:<16} {ms:7.2} ms (Env::new {:5.2} ms), budget {budget:.0} ms",
                median(env_times)
            );
            if ms > budget {
                slow.push(format!("{name}: {ms:.1} ms"));
            }
        }
        assert!(slow.is_empty(), "512x256 bakes over the {budget:.0} ms budget: {}", slow.join(", "));
    });
}

/// The released-slider size: what the app bakes after every finished edit.
#[test]
#[ignore = "release timing; see the module comment"]
fn preview_bakes_1024_under_200_ms() {
    let budget = budget_ms(200.0);
    with_pool(|pool| {
        let cases = [
            ("sky with clouds", cloudy_sky()),
            ("Starry night", preset("Starry night").expect("a built-in preset loads")),
            ("Three-point", preset("Three-point").expect("a built-in preset loads")),
        ];
        let _ = timed_bake(pool, &cases[0].1, 512);
        let mut slow = Vec::new();
        for (name, params) in &cases {
            let mut totals = Vec::new();
            for _ in 0..3 {
                let t = timed_bake(pool, params, 1024);
                check_map(&t.map, 1024, name);
                totals.push(t.total_ms);
            }
            let ms = median(totals);
            println!("hdri_perf: 1024x512 {name:<16} {ms:7.2} ms, budget {budget:.0} ms");
            if ms > budget {
                slow.push(format!("{name}: {ms:.1} ms"));
            }
        }
        assert!(slow.is_empty(), "1024x512 bakes over the {budget:.0} ms budget: {}", slow.join(", "));
    });
}

#[test]
#[ignore = "release timing; see the module comment"]
fn full_bakes_4096_under_10_s() {
    let budget = budget_ms(10_000.0);
    with_pool(|pool| {
        let cases = [
            ("sky with clouds", cloudy_sky()),
            ("Starry night", preset("Starry night").expect("a built-in preset loads")),
        ];
        // Warm the pool first, as above.
        let _ = timed_bake(pool, &cases[0].1, 512);
        for (name, params) in &cases {
            let t = timed_bake(pool, params, 4096);
            check_map(&t.map, 4096, name);
            println!(
                "hdri_perf: 4096x2048 {name:<16} {:8.1} ms (Env::new {:5.2} ms), budget {budget:.0} ms",
                t.total_ms, t.env_ms
            );
            assert!(t.total_ms < budget, "{name}: the 4096x2048 bake took {:.0} ms, over {budget:.0} ms", t.total_ms);
        }
    });
}

/// The atlas the released bake also builds (`Jobs::request_preview` with
/// `with_ibl`): ibl's prefilter (256 wide, 6 levels) plus SH9. Single
/// threaded, so the budget does not scale with the core count. The plan
/// guessed about 0.2 s; an i9-9900K measures 0.66 s, almost all of it
/// `ibl::prefilter` (sh9 takes 4 ms, packing 2 ms), which is what
/// `ibl::prefiltered`'s own doc says ("a large part of a second"), so the
/// budget is 1 s and the app must never pay it per drag tick.
#[test]
#[ignore = "release timing; see the module comment"]
fn ibl_texture_of_a_1024_map_under_1_s() {
    let budget_ms = 1000.0;
    with_pool(|pool| {
        let map = timed_bake(pool, &cloudy_sky(), 1024).map;
        check_map(&map, 1024, "sky with clouds");
        // Once untimed for the page faults, then the measurement.
        let _ = ibl::ibl_texture(&map, 1.0, 0.0);
        let start = Instant::now();
        let texture = ibl::ibl_texture(&map, 1.0, 0.0);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(texture.width > 0 && texture.height > 0 && texture.data.len() == texture.width * texture.height * 4);
        assert!(texture.data.iter().all(|v| v.is_finite()), "the atlas holds a non-finite value");
        println!("hdri_perf: ibl_texture of 1024x512 {ms:7.1} ms, budget {budget_ms:.0} ms");
        assert!(ms < budget_ms, "ibl_texture took {ms:.0} ms, over {budget_ms:.0} ms");
    });
}

/// Volumetric clouds (plan 1b, B16): the spec's target is a 4K map in 60 s
/// on 8 cores. The bake marches the volume at 2K at most and reads it
/// bilinearly (`volumetric::MARCH_MAX_WIDTH`); the time includes the first
/// build of the seed's noise volume and the light grid.
#[test]
#[ignore = "release timing; see the module comment"]
fn a_volumetric_sky_bakes_4096_under_60_s() {
    let budget = budget_ms(60_000.0);
    with_pool(|pool| {
        // Warm the pool with the flat sky, so everything the volume costs
        // (the noise, the light grid, the march) is inside the timing.
        let _ = timed_bake(pool, &cloudy_sky(), 512);
        let mut params = cloudy_sky();
        // A seed no other test in this binary bakes: its noise is built here.
        params.seed = 0x0b16_5eed;
        params.sky.volumetric = Some(VolumetricParams::default());
        let t = timed_bake(pool, &params, 4096);
        check_map(&t.map, 4096, "volumetric sky");
        println!(
            "hdri_perf: 4096x2048 volumetric sky {:8.1} ms (Env::new {:5.2} ms), budget {budget:.0} ms",
            t.total_ms, t.env_ms
        );
        assert!(t.total_ms < budget, "the volumetric 4096x2048 bake took {:.0} ms, over {budget:.0} ms", t.total_ms);
        // The volume was really marched: a small bake of the same settings
        // differs from the flat deck's.
        let small = timed_bake(pool, &params, 512);
        let mut flat_params = params.clone();
        flat_params.sky.volumetric = None;
        // (assert!, not assert_ne!: a failure must not print two whole maps.)
        assert!(small.map != timed_bake(pool, &flat_params, 512).map, "the volume is not the 2D deck");
    });
}

/// A picture as big as a light's picture is kept (`MAX_LIGHT_IMAGE_SIDE`,
/// 2048), of soft blotches with a cut-out corner, so a gobo of it masks
/// part of its light and its summed-area table is as large as one gets.
fn gobo_picture() -> EnvMap {
    let side = makepad_render::hdri::image_light::MAX_LIGHT_IMAGE_SIDE;
    let data = (0..side * side)
        .map(|i| {
            let (x, y) = ((i % side) as f32 / side as f32, (i / side) as f32 / side as f32);
            let v = 0.5 + 0.5 * ((x * 23.0).sin() * (y * 17.0).cos());
            [v, v, v, if x + y < 0.3 { 0.0 } else { 1.0 }]
        })
        .collect();
    EnvMap { width: side, height: side, data }
}

/// D15's heavy set-up: a studio whose key has a honeycomb grid, barn doors,
/// a worn face and an image gobo off its face that it casts onto the
/// backdrop, and four fills, each with an image gobo of the same picture at
/// its own distance, one of them worn and one showing the picture as its
/// content. `key_azimuth` is what the drag moves.
fn heavy_studio(key_azimuth: f32) -> HdriParams {
    use makepad_render::hdri::{AimParams, GoboParams, ImperfectionParams, LightImageParams, LightParams};
    let gobo = |distance: f32| GoboParams { kind: "image".to_string(), image: "gobo.png".to_string(), distance, ..GoboParams::default() };
    let worn = ImperfectionParams { wrinkle: 0.5, dust: 0.5, weave: 0.5, seed: 7 };
    let mut p = HdriParams::default();
    p.mode = "studio".to_string();
    p.lights.push(LightParams {
        key: true,
        azimuth_deg: key_azimuth,
        elevation_deg: 35.0,
        width_deg: 40.0,
        height_deg: 30.0,
        aim: Some(AimParams { yaw_deg: 20.0, pitch_deg: 0.0, grid: 0.6, barn_doors: [30.0, 0.0, 20.0, 0.0] }),
        gobo: Some(GoboParams { project_on_backdrop: true, ..gobo(0.5) }),
        imperfections: Some(worn.clone()),
        ..LightParams::default()
    });
    for (i, distance) in [0.2f32, 0.35, 0.5, 0.8].into_iter().enumerate() {
        p.lights.push(LightParams {
            azimuth_deg: 120.0 + 60.0 * i as f32,
            elevation_deg: 15.0 + 5.0 * i as f32,
            width_deg: 50.0,
            height_deg: 35.0,
            intensity_ev: 1.0,
            gobo: Some(gobo(distance)),
            imperfections: if i == 1 { Some(worn.clone()) } else { None },
            image: if i == 2 { Some(LightImageParams { key: "gobo.png".to_string(), ..LightImageParams::default() }) } else { None },
            ..LightParams::default()
        });
    }
    p
}

/// D15 (M5, M6): a held drag on a heavy set-up stays fast. While a slider is
/// held the app bakes 512 x 256 previews back to back, each a fresh
/// `Env::new_with` and a row-parallel bake (`jobs::run_preview_bake`), and
/// never asks for the key. Here the key's azimuth is dragged over twenty
/// ticks after one untimed tick (the first build of the set-up's pictures'
/// tables, as when they load); each tick's whole job is timed, and its
/// median must be within the 512 preview budget.
#[test]
#[ignore = "release timing; see the module comment"]
fn a_held_drag_on_a_heavy_studio_stays_fast() {
    let budget = budget_ms(50.0);
    let mut images = ImageSet::default();
    images.insert("gobo.png", gobo_picture());
    with_pool(|pool| {
        let tick = |azimuth: f32| {
            let params = heavy_studio(azimuth);
            let start = Instant::now();
            let env = Env::new_with(&params, &images);
            let env_ms = start.elapsed().as_secs_f64() * 1000.0;
            let map = env.bake_par(512, |n, f| pool.fan_out(Lane::Heavy, n, f));
            Timing { total_ms: start.elapsed().as_secs_f64() * 1000.0, env_ms, map }
        };
        let first = tick(30.0);
        check_map(&first.map, 512, "heavy studio");
        println!("hdri_perf: held drag, first tick {:7.1} ms (Env::new_with {:6.1} ms)", first.total_ms, first.env_ms);
        let (mut totals, mut env_times) = (Vec::new(), Vec::new());
        for step in 1..=20 {
            let t = tick(30.0 + 0.5 * step as f32);
            check_map(&t.map, 512, "heavy studio");
            totals.push(t.total_ms);
            env_times.push(t.env_ms);
        }
        let slowest = totals.iter().cloned().fold(0.0, f64::max);
        let ms = median(totals);
        println!(
            "hdri_perf: held drag of a heavy studio, 512x256: median {ms:7.1} ms (Env::new_with {:6.1} ms), slowest {slowest:7.1} ms, budget {budget:.0} ms",
            median(env_times)
        );
        assert!(ms < budget, "a held tick of the heavy studio took {ms:.1} ms, over {budget:.0} ms");
    });
}

/// P1, P2: a 2048 x 2048 light picture's softened mask (Mask softness) is
/// one summed-area table of four f64 sums an entry, (2049 x 2049) x 32 bytes
/// = 134.3 MB, built for the first light that softens the picture. Timed as
/// the first `Env::new_with` that softens a fresh picture against one that
/// does not (both walk the picture for its mean), the median of three; a
/// later `Env` (a held drag's) shares the table, and the 512 x 256 preview
/// of the softened light is timed against the unsoftened one's.
#[test]
#[ignore = "release timing; see the module comment"]
fn a_softened_2048_picture_builds_its_table_once() {
    use makepad_render::hdri::{LightImageParams, LightParams};
    let params = |soften: bool| HdriParams {
        mode: "studio".to_string(),
        lights: vec![LightParams {
            key: true,
            azimuth_deg: 30.0,
            elevation_deg: 30.0,
            width_deg: 60.0,
            height_deg: 40.0,
            image: Some(LightImageParams {
                key: "logo.png".to_string(),
                fit: "fit".to_string(),
                alpha_as_mask: true,
                penumbra: Some(0.3),
                mask_softness: Some(if soften { 1.0 } else { 0.0 }),
            }),
            ..LightParams::default()
        }],
        ..HdriParams::default()
    };
    // A fresh picture each time, so each first Env builds what it needs.
    let first_env = |soften: bool| {
        let mut images = ImageSet::default();
        images.insert("logo.png", gobo_picture());
        let start = Instant::now();
        let env = Env::new_with(&params(soften), &images);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        drop((env, images));
        makepad_render::hdri::composite::drop_dead_facts();
        ms
    };
    let plain = median((0..3).map(|_| first_env(false)).collect());
    let soft = median((0..3).map(|_| first_env(true)).collect());
    let side = makepad_render::hdri::image_light::MAX_LIGHT_IMAGE_SIDE;
    let bytes = (side + 1) * (side + 1) * 4 * std::mem::size_of::<f64>();
    println!(
        "hdri_perf: a softened {side} x {side} picture: first Env::new_with {soft:7.1} ms against {plain:7.1} ms unsoftened: the table {:7.1} ms, {bytes} bytes ({:.1} MB)",
        soft - plain,
        bytes as f64 / 1.0e6
    );
    let mut images = ImageSet::default();
    images.insert("logo.png", gobo_picture());
    let _built = Env::new_with(&params(true), &images);
    let start = Instant::now();
    let shared = Env::new_with(&params(true), &images);
    let later = start.elapsed().as_secs_f64() * 1000.0;
    let unsoftened = Env::new_with(&params(false), &images);
    with_pool(|pool| {
        let bake = |env: &Env| {
            let times: Vec<f64> = (0..5)
                .map(|_| {
                    let start = Instant::now();
                    let map = env.bake_par(512, |n, f| pool.fan_out(Lane::Heavy, n, f));
                    check_map(&map, 512, "softened picture");
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            median(times)
        };
        let (soft_bake, plain_bake) = (bake(&shared), bake(&unsoftened));
        println!("hdri_perf: a later Env::new_with that softens it {later:7.1} ms; 512x256 bake {soft_bake:7.1} ms softened, {plain_bake:7.1} ms not");
        assert!(later < 0.5 * soft.max(1.0), "a later Env built its own table: {later:.1} ms against {soft:.1} ms");
    });
}

/// D15 (L:229): the first build of a seed's cloud noise, about 2 s of
/// `Env::new_with` in release, stops soon after a newer request cancels the
/// job: the app's released bake of a new seed is stopped by the next drag
/// tick, and held a worker for the whole build before. The build polls
/// before each slice of its noise tiles; here the poll turns true 200 ms in,
/// and the build must be gone within 50 ms of that.
#[test]
#[ignore = "release timing; see the module comment"]
fn a_cancelled_first_cloud_noise_build_stops_at_once() {
    let mut params = cloudy_sky();
    // A seed no other test in this binary bakes: its noise is built here.
    params.seed = 0x4837_0d15;
    params.sky.volumetric = Some(VolumetricParams::default());
    let start = Instant::now();
    let cancel_at = 0.2;
    let cancelled = || start.elapsed().as_secs_f64() > cancel_at;
    let env = Env::new_cancellable(&params, &ImageSet::default(), &cancelled);
    let late_ms = (start.elapsed().as_secs_f64() - cancel_at) * 1000.0;
    assert!(env.is_none(), "the build ran to its end ({late_ms:.0} ms after the cancel)");
    println!("hdri_perf: the first cloud noise build stopped {late_ms:5.1} ms after its cancel");
    assert!(late_ms < 50.0, "it stopped {late_ms:.0} ms after the cancel");
    // Not cancelled, the same map builds whole (and keeps its noise).
    let start = Instant::now();
    assert!(Env::new_cancellable(&params, &ImageSet::default(), &|| false).is_some());
    println!("hdri_perf: the same seed built whole in {:6.1} ms", start.elapsed().as_secs_f64() * 1000.0);
}
