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
use makepad_render::hdri::skyline::SkylineBand;
use std::sync::Arc;
use makepad_render::hdri::{AuroraParams, CasterParams, Env, GoboParams, HdriParams, ImageSet, LightParams, MilkyWayParams, NebulaParams, SkylineParams, VolumetricParams, WeatherParams};
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
            ("Starry night, the night's three layers", {
                let mut p = preset("Starry night").expect("a built-in preset loads");
                p.sky.night.milky_way = Some(MilkyWayParams::default());
                p.sky.night.nebulas = Some(NebulaParams::default());
                p.sky.night.aurora = Some(AuroraParams::default());
                p
            }),
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

/// R4: a soft shadow caster rasters its pattern once per Env, on the job's
/// thread before the bake, so a held drag pays it on every bake. A dense
/// leaves canopy (the dearest pattern to sample) over the sun on a 60 x 45
/// degree caster: at a slight softness the raster is at its sample budget,
/// at full softness it is small. Against the same map with the caster
/// sharp, which rasters nothing.
#[test]
#[ignore]
fn a_soft_casters_raster_stays_well_under_a_held_bake() {
    let with_caster = |softness: f32| {
        let mut p = cloudy_sky();
        p.lights = vec![LightParams {
            azimuth_deg: 200.0,
            elevation_deg: 30.0,
            gobo: Some(GoboParams { kind: "leaves".to_string(), density: 0.9, ..GoboParams::default() }),
            caster: Some(CasterParams { softness, ..CasterParams::default() }),
            ..LightParams::new_caster()
        }];
        p
    };
    let env_ms = |p: &HdriParams| {
        median(
            (0..5)
                .map(|_| {
                    let start = Instant::now();
                    let env = Env::new(p);
                    let ms = start.elapsed().as_secs_f64() * 1000.0;
                    drop(env);
                    ms
                })
                .collect(),
        )
    };
    let sharp = env_ms(&with_caster(0.0));
    let held = with_pool(|pool| median((0..5).map(|_| timed_bake(pool, &with_caster(0.0), 512).total_ms).collect()));
    println!("hdri_perf: a sharp caster's Env in {sharp:6.1} ms, its held 512 bake in {held:6.1} ms");
    for softness in [0.05f32, 1.0] {
        let raster = env_ms(&with_caster(softness)) - sharp;
        println!("hdri_perf: softness {softness}: the raster takes {raster:6.1} ms");
        assert!(raster < 50.0, "softness {softness}: the raster takes {raster:.1} ms, a held preview's whole budget");
    }
}

/// Redesign §5 (R5): a caster's bloom pre-pass costs well under the bake it is
/// for. This is the plan's hardest case for it: an 80 degree leaves caster in
/// front of the sun with 45 degrees of bloom, on a clear sky and on a volumetric
/// one, at the held (512) and released (1024) preview widths. The pre-pass is
/// what `Env::new_for_width_par` takes with the bloom on, less what it takes
/// with it off (medians of five), its rows spread over the pool as the app's
/// preview job spreads them (M5); the bake is that Env's row-parallel bake. The
/// pre-pass is held to a quarter of the bake on 8 cores: of the bake itself, or of its one-thread
/// time over 8 where that is less (a machine whose bake runs more than 8 times
/// faster than on one thread). The speed-up is measured, not taken from
/// `available_parallelism`, which counts logical processors: on an 8-core
/// machine with two threads a core (16 logical) the bake runs about as fast
/// as on 8 threads, so a quarter scaled by 16 / 8 would let a pre-pass take
/// half of it.
#[test]
#[ignore = "release timing; see the module comment"]
fn a_bloom_casters_pre_pass_stays_well_under_its_bake() {
    let with_caster = |sky: &HdriParams, bloom_strength: f32| {
        let mut p = sky.clone();
        p.lights = vec![LightParams {
            azimuth_deg: 200.0,
            elevation_deg: 30.0,
            width_deg: 80.0,
            height_deg: 80.0,
            gobo: Some(GoboParams { kind: "leaves".to_string(), density: 0.6, distance: 0.0, ..GoboParams::default() }),
            caster: Some(CasterParams { bloom_strength, bloom_length_deg: 45.0, ..CasterParams::default() }),
            ..LightParams::new_caster()
        }];
        p
    };
    let mut clear = cloudy_sky();
    clear.sky.clouds.coverage = 0.0;
    clear.sky.clouds.cirrus = 0.0;
    let mut volumetric = cloudy_sky();
    // A seed no other test in this binary bakes; its noise is built once, untimed, below.
    volumetric.seed = 0x0b10_0c45;
    volumetric.sky.volumetric = Some(VolumetricParams::default());
    let images = ImageSet::default();
    with_pool(|pool| {
        let mut slow = Vec::new();
        for (name, sky) in [("clear sky", &clear), ("volumetric sky", &volumetric)] {
            let (on, off) = (with_caster(sky, 1.0), with_caster(sky, 0.0));
            // Untimed: the pool's first fan-out and the volume's noise.
            let warm = Env::new_for_width(&off, &images, 512, &|| false).expect("nothing cancels it");
            let _ = warm.bake_par(512, |n, f| pool.fan_out(Lane::Heavy, n, f));
            for width in [512usize, 1024] {
                let build = |p: &HdriParams| {
                    let start = Instant::now();
                    let env = Env::new_for_width_par(p, &images, width, &|| false, &|n, f| pool.fan_out(Lane::Heavy, n, f)).expect("nothing cancels it");
                    (start.elapsed().as_secs_f64() * 1000.0, env)
                };
                let (mut pre_pass, mut bakes) = (Vec::new(), Vec::new());
                for _ in 0..5 {
                    let (with_ms, env) = build(&on);
                    let (without_ms, _) = build(&off);
                    pre_pass.push((with_ms - without_ms).max(0.0));
                    let start = Instant::now();
                    let map = env.bake_par(width, |n, f| pool.fan_out(Lane::Heavy, n, f));
                    bakes.push(start.elapsed().as_secs_f64() * 1000.0);
                    check_map(&map, width, name);
                }
                let (pre, bake) = (median(pre_pass), median(bakes));
                // The same bake on this thread alone, once: its speed-up over one thread.
                let env = build(&on).1;
                let start = Instant::now();
                let _ = env.bake_par(width, |n, f| (0..n).for_each(f));
                let speed_up = start.elapsed().as_secs_f64() * 1000.0 / bake;
                let allowed = 0.25 * (speed_up / 8.0).max(1.0);
                println!(
                    "hdri_perf: bloom pre-pass {width}x{} {name:<15} {pre:7.2} ms against its bake {bake:7.2} ms ({:4.1} %, allowed {:.0} % at {speed_up:4.1}x one thread)",
                    width / 2,
                    100.0 * pre / bake,
                    100.0 * allowed
                );
                if pre > allowed * bake {
                    slow.push(format!("{name} at {width}: {pre:.1} ms against {bake:.1} ms, over {:.0} %", 100.0 * allowed));
                }
            }
        }
        assert!(slow.is_empty(), "bloom pre-passes over their allowance: {}", slow.join(", "));
    });
}

/// M5 (plan 4's final review): four soft shadow casters with bloom stay well under the held
/// bake. Four 60 x 45 degree leaves casters at softness 0.3 with 10 degrees of bloom: one in
/// front of the sun, one on either side of it and one above it, on a clear sky, on the spec's
/// cloudy one and on a volumetric one, and on the clear sky with a light listed before them all
/// that stands behind all four: a 110 x 80 degree picture of soft blotches (`gobo_picture`, a
/// light with detail inside it) at EV 12, two EV over the casters' threshold at its brightest,
/// so few of the cells it reaches have corners alike (fix round 3 of S3's review: such a light's
/// cells took the light behind at every map texel, past its sample budget; fix round 4: the four
/// rasters share one budget, `caster::BLOOM_CELL_BUDGET`, and the held preview's softness
/// rasters are a little coarser, `caster::SOFT_HELD_TEXELS_PER_CELL`). At the held preview's
/// width (512).
/// Their pre-pass, the
/// softness rasters and the bloom rasters, is what `Env::new_for_width_par` takes with them
/// less what it takes with the same casters sharp and without bloom (medians of five), its
/// rows spread over the pool as the app's preview job spreads them. The held bake is what the
/// preview job pays without that pre-pass, as `a_soft_casters_raster_stays_well_under_a_held_bake`
/// counts it: that Env's build and its row-parallel bake. The pre-pass is held to a quarter of
/// the held bake on 8 cores (`a_bloom_casters_pre_pass_stays_well_under_its_bake`'s measured
/// allowance), so the four casters together cost no more than one caster's bloom may.
#[test]
#[ignore = "release timing; see the module comment"]
fn four_soft_blooming_casters_stay_well_under_a_held_bake() {
    use makepad_render::hdri::LightImageParams;
    // The sky's own lights first, then the casters.
    let with_casters = |sky: &HdriParams, softness: f32, bloom_strength: f32| {
        let mut p = sky.clone();
        p.lights.extend([(200.0f32, 30.0f32), (160.0, 35.0), (240.0, 25.0), (200.0, 70.0)]
            .iter()
            .enumerate()
            .map(|(i, &(azimuth_deg, elevation_deg))| LightParams {
                azimuth_deg,
                elevation_deg,
                width_deg: 60.0,
                height_deg: 45.0,
                gobo: Some(GoboParams { kind: "leaves".to_string(), density: 0.6, seed: 3 + i as u32, distance: 0.0, ..GoboParams::default() }),
                caster: Some(CasterParams { softness, bloom_strength, bloom_length_deg: 10.0, ..CasterParams::default() }),
                ..LightParams::new_caster()
            }));
        p
    };
    let mut clear = cloudy_sky();
    clear.sky.clouds.coverage = 0.0;
    clear.sky.clouds.cirrus = 0.0;
    let mut volumetric = cloudy_sky();
    // A seed no other test in this binary bakes; its noise is built once, untimed, below.
    volumetric.seed = 0x0b10_0c46;
    volumetric.sky.volumetric = Some(VolumetricParams::default());
    let mut lit = clear.clone();
    lit.lights = vec![LightParams {
        azimuth_deg: 200.0,
        elevation_deg: 40.0,
        width_deg: 110.0,
        height_deg: 80.0,
        intensity_ev: 12.0,
        image: Some(LightImageParams { key: "gobo.png".to_string(), ..LightImageParams::default() }),
        ..LightParams::default()
    }];
    let mut images = ImageSet::default();
    images.insert("gobo.png", gobo_picture());
    let width = 512;
    with_pool(|pool| {
        let mut slow = Vec::new();
        for (name, sky) in [("clear sky", &clear), ("cloudy sky", &cloudy_sky()), ("volumetric sky", &volumetric), ("lit clear sky", &lit)] {
            let (on, soft_only, off) = (with_casters(sky, 0.3, 1.0), with_casters(sky, 0.3, 0.0), with_casters(sky, 0.0, 0.0));
            // Untimed: the pool's first fan-out and the volume's noise.
            let warm = Env::new_for_width(&off, &images, width, &|| false).expect("nothing cancels it");
            let _ = warm.bake_par(width, |n, f| pool.fan_out(Lane::Heavy, n, f));
            let build = |p: &HdriParams| {
                let start = Instant::now();
                let env = Env::new_for_width_par(p, &images, width, &|| false, &|n, f| pool.fan_out(Lane::Heavy, n, f)).expect("nothing cancels it");
                (start.elapsed().as_secs_f64() * 1000.0, env)
            };
            let (mut pre_pass, mut softness, mut envs, mut bakes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for _ in 0..5 {
                let (with_ms, env) = build(&on);
                let (soft_ms, _) = build(&soft_only);
                let (without_ms, _) = build(&off);
                pre_pass.push((with_ms - without_ms).max(0.0));
                softness.push((soft_ms - without_ms).max(0.0));
                envs.push(without_ms);
                let start = Instant::now();
                let map = env.bake_par(width, |n, f| pool.fan_out(Lane::Heavy, n, f));
                bakes.push(start.elapsed().as_secs_f64() * 1000.0);
                check_map(&map, width, name);
            }
            let (pre, soft, env_ms, bake) = (median(pre_pass), median(softness), median(envs), median(bakes));
            let held = env_ms + bake;
            // The same bake on this thread alone, once: its speed-up over one thread.
            let env = build(&on).1;
            let start = Instant::now();
            let _ = env.bake_par(width, |n, f| (0..n).for_each(f));
            let speed_up = start.elapsed().as_secs_f64() * 1000.0 / bake;
            let allowed = 0.25 * (speed_up / 8.0).max(1.0);
            println!(
                "hdri_perf: four soft blooming casters {width}x{} {name:<15} pre-pass {pre:6.2} ms (softness {soft:5.2} ms) against the held bake {held:6.2} ms (Env {env_ms:6.2} + bake {bake:6.2}): {:4.1} %, allowed {:.0} % at {speed_up:4.1}x one thread",
                width / 2,
                100.0 * pre / held,
                100.0 * allowed
            );
            if pre > allowed * held {
                slow.push(format!("{name}: {pre:.1} ms against {held:.1} ms, over {:.0} %", 100.0 * allowed));
            }
        }
        assert!(slow.is_empty(), "four soft blooming casters' pre-passes over their allowance: {}", slow.join(", "));
    });
}

/// The skies the strip's light and its attachment are timed under: clear, a 2D deck, rain,
/// the overcast kind, and a marched volume (its 128 ring looks march it without a cache).
fn skyline_skies() -> Vec<(&'static str, HdriParams)> {
    let clear = HdriParams::default();
    let weather = |kind: &str| {
        let mut p = cloudy_sky();
        p.sky.weather = Some(WeatherParams { kind: kind.to_string(), ..WeatherParams::default() });
        p
    };
    let mut volume = cloudy_sky();
    volume.sky.volumetric = Some(VolumetricParams::default());
    vec![("clear", clear), ("clouds", cloudy_sky()), ("rain", weather("rain")), ("overcast", weather("overcast")), ("volume", volume)]
}

/// The sky's light the app hands the skyline (Task 4 of the skyline layer), read from the
/// map's own Env once it is built: the Env build is the bake's, so this is what the strip adds
/// per change. The first call on a fresh Env (nothing is kept between calls), the slowest of
/// five fresh Envs per sky. Measured 0.04 ms (clear) to 0.36 ms (a volume's unmarched looks);
/// the bound is about five times the slowest.
#[test]
#[ignore]
fn skyline_light_on_a_fresh_env_is_cheap() {
    let mut slow = Vec::new();
    for (name, p) in skyline_skies() {
        let mut times = Vec::new();
        for _ in 0..5 {
            let env = Env::new(&p);
            let t = Instant::now();
            std::hint::black_box(env.skyline_light());
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let worst = times.iter().cloned().fold(0.0, f64::max);
        println!("hdri_perf: skyline_light {name:<9} {worst:6.3} ms (slowest of 5 fresh Envs)");
        if worst > SKYLINE_LIGHT_MS {
            slow.push(format!("{name}: {worst:.2} ms"));
        }
    }
    assert!(slow.is_empty(), "skyline_light over {SKYLINE_LIGHT_MS} ms: {}", slow.join(", "));
}

/// `skyline_light_on_a_fresh_env_is_cheap`'s bound.
const SKYLINE_LIGHT_MS: f64 = 2.0;

/// Attaching the strip to a built map (`Env::with_skyline`): a 1024 x 64 band (a city light
/// every 16 texels) and a blooming caster with the low sun beside it, so its lights by region
/// and the caster's bloom are built again; against the same Env's build. The slowest of five.
/// Measured 2.7 ms against an 18 ms Env build; the bound is about five times that.
#[test]
#[ignore]
fn with_skyline_on_a_blooming_map_is_cheap() {
    let mut p = HdriParams::default();
    p.sky.sun.mode = "manual".to_string();
    p.sky.sun.elevation_deg = 4.0;
    p.sky.sun.azimuth_deg = 0.0;
    p.skyline = Some(SkylineParams { on: true, seed: 7, turn_deg: 0.0, lights_by_day: 0.0, settings: Default::default() });
    let settings = CasterParams { bloom_strength: 1.0, bloom_length_deg: 15.0, bloom_threshold_ev: 3.0, ..CasterParams::default() };
    p.lights = vec![LightParams { azimuth_deg: 10.0, elevation_deg: 16.0, width_deg: 30.0, height_deg: 16.0, gobo: None, caster: Some(settings), ..LightParams::new_caster() }];
    let (w, h) = (1024, 64);
    let mut texels = vec![[0.04, 0.04, 0.05, 1.0]; w * h];
    for (k, texel) in texels.iter_mut().enumerate() {
        if k % 16 == 5 && (k / w) % 4 == 1 {
            *texel = [30.0, 24.0, 16.0, 1.0];
        }
    }
    let band = Arc::new(SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: 10.0, bottom_deg: -2.0, eye_m: 0.0 });
    let (mut builds, mut attaches) = (Vec::new(), Vec::new());
    for _ in 0..5 {
        let t = Instant::now();
        let env = Env::new(&p);
        builds.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let env = env.with_skyline(band.clone());
        attaches.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(env.has_skyline());
    }
    let (build, attach) = (builds.iter().cloned().fold(0.0, f64::max), attaches.iter().cloned().fold(0.0, f64::max));
    println!("hdri_perf: with_skyline {w}x{h} band, a blooming caster: {attach:6.2} ms (the Env build {build:6.2} ms), slowest of 5");
    assert!(attach < WITH_SKYLINE_MS, "with_skyline took {attach:.1} ms, over {WITH_SKYLINE_MS} ms");
}

/// `with_skyline_on_a_blooming_map_is_cheap`'s bound.
const WITH_SKYLINE_MS: f64 = 15.0;

/// A strip like the app's held one (512 x 46 over +25.6 .. -6.4 degrees, the skyline's
/// defaults): clear sky above a ridge that rises and falls round the turn, opaque below it
/// (the ranges, the city, the ground and the sea), with a light every 8 texels of the city's rows.
fn held_strip() -> Arc<SkylineBand> {
    let (w, h, top, bottom) = (512, 46, 25.6f32, -6.4f32);
    let mut texels = vec![[0.0; 4]; w * h];
    for y in 0..h {
        let el = top - (y as f32 + 0.5) / h as f32 * (top - bottom);
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let ridge = 2.0 + 8.0 * (u * std::f32::consts::TAU * 3.0).sin().abs();
            if el < ridge {
                let light = x % 8 == 3 && el > 0.0 && el < 4.0;
                texels[y * w + x] = if light { [0.08, 0.05, 0.02, 1.0] } else { [0.0004, 0.0005, 0.0007, 1.0] };
            }
        }
    }
    Arc::new(SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: top, bottom_deg: bottom, eye_m: 0.0 })
}

/// A held drag's bake with the skyline on, against the same bake without it (plan hdri-5
/// Task 8): the Moonlit night with one light layer at 512, the app's held bake. The skyline adds
/// its attachment (`with_skyline_par`) and its texels in the bake, the band's part of a
/// held step that is not the strip's render. Medians of 15, after one untimed of each.
/// Measured +1.40 ms (17.33 ms without, 18.61 with, attaching 0.12) while the bake looked up
/// the sky behind the strip's opaque texels too; -0.3 ms since it does not (`Env::base_at`),
/// and the texels above the band and below its skirt skip its arcsine and arctangent.
#[test]
#[ignore = "release timing; see the module comment"]
fn a_held_bake_with_the_skyline_costs_little_more_than_without() {
    let mut p = preset("Moonlit night").expect("a built-in preset loads");
    p.lights = vec![LightParams::default()];
    let off = p.clone();
    p.skyline = Some(SkylineParams { on: true, seed: 7, turn_deg: 0.0, lights_by_day: 0.0, settings: Default::default() });
    let band = held_strip();
    with_pool(|pool| {
        let rows = |n: usize, f: &(dyn Fn(usize) + Sync)| pool.fan_out(Lane::Heavy, n, f);
        let images = ImageSet::default();
        let build = |p: &HdriParams| Env::new_for_width_par(p, &images, 512, &|| false, &rows).expect("nothing cancels it");
        let (mut plain, mut attach, mut banded) = (Vec::new(), Vec::new(), Vec::new());
        for round in 0..16 {
            let env = build(&off);
            let t = Instant::now();
            std::hint::black_box(env.bake_par(512, |n, f| pool.fan_out(Lane::Heavy, n, f)));
            let plain_ms = t.elapsed().as_secs_f64() * 1000.0;
            let env = build(&p);
            let t = Instant::now();
            let env = env.with_skyline_par(band.clone(), &|| false, &rows).expect("nothing cancels it");
            let attach_ms = t.elapsed().as_secs_f64() * 1000.0;
            let t = Instant::now();
            std::hint::black_box(env.bake_par(512, |n, f| pool.fan_out(Lane::Heavy, n, f)));
            let banded_ms = t.elapsed().as_secs_f64() * 1000.0;
            if round > 0 {
                (plain.push(plain_ms), attach.push(attach_ms), banded.push(banded_ms));
            }
        }
        let (plain, attach, banded) = (median(plain), median(attach), median(banded));
        let extra = attach + banded - plain;
        println!("hdri_perf: held 512 bake, Moonlit night: without the skyline {plain:6.2} ms; with it {banded:6.2} ms and the attachment {attach:5.2} ms: {extra:+5.2} ms");
        assert!(extra < HELD_SKYLINE_EXTRA_MS, "the skyline adds {extra:.2} ms to a held bake, over {HELD_SKYLINE_EXTRA_MS} ms");
    });
}

/// `a_held_bake_with_the_skyline_costs_little_more_than_without`'s bound.
const HELD_SKYLINE_EXTRA_MS: f64 = 1.0;

/// One held bake of `p` at `width` as the app's job pays it (plan hdri-7 §5): Env::new_for_width_par plus bake_par on
/// the Heavy lane, ms.
fn held_bake_ms(pool: &TaskPool, p: &HdriParams, width: usize) -> f64 {
    let rows = |n: usize, f: &(dyn Fn(usize) + Sync)| pool.fan_out(Lane::Heavy, n, f);
    let images = ImageSet::default();
    let t = Instant::now();
    let env = Env::new_for_width_par(p, &images, width, &|| false, &rows).expect("nothing cancels it");
    std::hint::black_box(env.bake_par(width, |n, f| pool.fan_out(Lane::Heavy, n, f)));
    t.elapsed().as_secs_f64() * 1000.0
}

/// The held cost of a night layer (plan hdri-7 §5): `preset_name` at `width` (`held_bake_ms`), without and with
/// `patch`, alternating, medians of 15 after one untimed round. (base ms, extra ms).
fn night_layer_extra(pool: &TaskPool, preset_name: &str, width: usize, patch: &dyn Fn(&mut HdriParams)) -> (f64, f64) {
    let off = preset(preset_name).expect("a built-in preset loads");
    let mut on = off.clone();
    patch(&mut on);
    let (mut base, mut with) = (Vec::new(), Vec::new());
    for round in 0..16 {
        let (b, w) = (held_bake_ms(pool, &off, width), held_bake_ms(pool, &on, width));
        if round > 0 {
            base.push(b);
            with.push(w);
        }
    }
    let (base, with) = (median(base), median(with));
    (base, with - base)
}

/// The night's layers a held drag pays for (plan hdri-7), each the patch that switches it on over a preset. Tasks 2
/// to 4 add a row each.
fn night_layers() -> Vec<(&'static str, fn(&mut HdriParams))> {
    vec![
        ("milky way", |p| p.sky.night.milky_way = Some(MilkyWayParams::default())),
        ("nebulas", |p| p.sky.night.nebulas = Some(NebulaParams::default())),
        ("aurora", |p| p.sky.night.aurora = Some(AuroraParams::default())),
    ]
}

/// The aurora at the corners of its ranges (Task 4c's fix round), each gated as a layer: its ribbons nearest (Height
/// 75), furthest (Height 1), longest (Width 180) and most folded (Folds 1), where its footprints carry the most points
/// and a held drag of Height, Width or Folds builds them on every step.
fn aurora_extremes() -> Vec<(&'static str, AuroraParams)> {
    let at = |height_deg: f32, width_deg: f32, folds: f32| AuroraParams { height_deg, width_deg, folds, ..AuroraParams::default() };
    let d = AuroraParams::default();
    vec![
        ("Height 75, Width 180, Folds 1", at(75.0, 180.0, 1.0)),
        ("Height 40, Width 180, Folds 1", at(40.0, 180.0, 1.0)),
        ("Height 15, Width 180, Folds 1", at(15.0, 180.0, 1.0)),
        ("Height 1, Width 180, Folds 1", at(1.0, 180.0, 1.0)),
        ("Height 75", at(75.0, d.width_deg, d.folds)),
        ("Height 1", at(1.0, d.width_deg, d.folds)),
    ]
}

/// Plan hdri-7 §5: a held Size drag over the nebulas: 24 held 512 bakes of `preset_name` with the nebulas at `seed` (a
/// seed no other row uses, so no sprite row is filled yet), their Size stepping from 0.25 to 4 (each step may read a
/// sprite level the last did not, whose rows fill then), against the same bakes without nebulas. (base median ms, the
/// drag's median extra ms, its worst extra ms.)
fn nebula_size_drag(pool: &TaskPool, preset_name: &str, seed: u32) -> (f64, f64, f64) {
    let off = preset(preset_name).expect("a built-in preset loads");
    held_bake_ms(pool, &off, 512);
    let (mut base, mut extra) = (Vec::new(), Vec::new());
    for i in 0..24 {
        let size = 0.25 * 16f32.powf(i as f32 / 23.0);
        let mut on = off.clone();
        on.sky.night.nebulas = Some(NebulaParams { amount: 1.0, size, seed, ..NebulaParams::default() });
        let (b, w) = (held_bake_ms(pool, &off, 512), held_bake_ms(pool, &on, 512));
        base.push(b);
        extra.push(w - b);
    }
    let worst = extra.iter().copied().fold(f64::MIN, f64::max);
    (median(base), median(extra), worst)
}

/// What one layer may add to a held 512 bake, and all of them together (plan hdri-7 §5: "about +20 %").
const HELD_NIGHT_LAYER_SHARE: f64 = 0.2;
const HELD_NIGHT_ALL_SHARE: f64 = 0.4;

/// Plan hdri-7 Task 1a: the held and released night bakes the layers' budgets are shares of.
#[test]
#[ignore = "release timing; see the module comment"]
fn the_held_night_bakes_are_measured() {
    with_pool(|pool| {
        for name in ["Moonlit night", "Starry night"] {
            for width in [512, 1024] {
                let (base, same) = night_layer_extra(pool, name, width, &|_| {});
                println!("hdri_perf: {name} at {width}: {base:6.2} ms (Env build + bake_par); the same settings twice differ by {same:+5.2} ms");
                assert!(base > 0.0);
            }
        }
    });
}

/// Plan hdri-7 §5: each night layer adds at most a fifth to a held 512 bake of "Moonlit night" and of "Starry night",
/// all of them together at most two fifths; the released 1024 cost is printed beside it. A held nebula Size drag
/// (`nebula_size_drag`) and the aurora at the corners of its ranges (`aurora_extremes`) are gated like a layer.
#[test]
#[ignore = "release timing; see the module comment"]
fn held_night_layers_cost_at_most_a_fifth_more() {
    let layers = night_layers();
    with_pool(|pool| {
        for name in ["Moonlit night", "Starry night"] {
            for (layer, patch) in &layers {
                let (base, extra) = night_layer_extra(pool, name, 512, patch);
                println!("hdri_perf: held 512, {name}, {layer}: {base:6.2} ms {extra:+5.2} ms ({:+.0} %)", 100.0 * extra / base);
                assert!(extra / base <= HELD_NIGHT_LAYER_SHARE, "{name}: {layer} adds {:.0} % to a held bake", 100.0 * extra / base);
                let (base, extra) = night_layer_extra(pool, name, 1024, patch);
                println!("hdri_perf: released 1024, {name}, {layer}: {base:6.2} ms {extra:+5.2} ms");
            }
            if layers.iter().any(|(layer, _)| *layer == "aurora") {
                for (label, aurora) in aurora_extremes() {
                    let (base, extra) = night_layer_extra(pool, name, 512, &|p| p.sky.night.aurora = Some(aurora.clone()));
                    println!("hdri_perf: held 512, {name}, the aurora at {label}: {base:6.2} ms {extra:+5.2} ms ({:+.0} %)", 100.0 * extra / base);
                    assert!(extra / base <= HELD_NIGHT_LAYER_SHARE, "{name}: the aurora at {label} adds {:.0} % to a held bake", 100.0 * extra / base);
                }
            }
            if layers.iter().any(|(layer, _)| *layer == "nebulas") {
                // a fresh seed per preset ("Moonlit night" and "Starry night" differ in length), filled by this drag alone
                let (base, extra, worst) = nebula_size_drag(pool, name, 0x7e57_0010 + name.len() as u32);
                println!("hdri_perf: held 512, {name}, a nebula Size drag: {base:6.2} ms, median {extra:+5.2} ms ({:+.0} %), worst step {worst:+5.2} ms", 100.0 * extra / base);
                assert!(extra / base <= HELD_NIGHT_LAYER_SHARE, "{name}: a nebula Size drag adds {:.0} % a step", 100.0 * extra / base);
            }
            if layers.len() > 1 {
                let all = |p: &mut HdriParams| layers.iter().for_each(|(_, patch)| patch(p));
                let (base, extra) = night_layer_extra(pool, name, 512, &all);
                println!("hdri_perf: held 512, {name}, all {} layers: {base:6.2} ms {extra:+5.2} ms ({:+.0} %)", layers.len(), 100.0 * extra / base);
                assert!(extra / base <= HELD_NIGHT_ALL_SHARE, "{name}: the layers together add {:.0} %", 100.0 * extra / base);
            }
        }
    });
}

/// Plan hdri-7 Task 3: the first bake of a new nebula seed fills the sprite rows it reads (reported, not gated: a seed
/// is typed, not dragged); the next bake of that seed reads them.
#[test]
#[ignore = "release timing; see the module comment"]
fn the_first_bake_of_a_new_nebula_seed_is_reported() {
    with_pool(|pool| {
        for (width, seed) in [(512usize, 0x7e57_0001u32), (1024, 0x7e57_0002)] {
            let mut p = preset("Starry night").expect("a built-in preset loads");
            p.sky.night.nebulas = Some(NebulaParams { amount: 1.0, seed, ..NebulaParams::default() });
            let (cold, warm) = (timed_bake(pool, &p, width).total_ms, timed_bake(pool, &p, width).total_ms);
            println!("hdri_perf: {width} with a new nebula seed: first {cold:6.2} ms, then {warm:6.2} ms");
        }
    });
}
