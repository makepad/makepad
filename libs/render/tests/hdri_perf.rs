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
use makepad_render::hdri::{Env, HdriParams, VolumetricParams};
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
