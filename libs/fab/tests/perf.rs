//! The performance gates from the viewer architecture, run against
//! the synthetic stress model rather than a sample (samples are untracked, and
//! neither of them is anywhere near 5 M triangles).
//!
//! These are slow and meaningless in a debug build, so they are `#[ignore]`d:
//!
//! ```text
//! cargo test --release -p makepad-fab-shell -- --ignored --nocapture
//! ```

use fab::model::makepad_math::Vec3f;
use fab::model::*;

fn rng(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed >> 8) as f32 / 16_777_216.0
}

#[test]
#[ignore = "stress test: run with --release -- --ignored"]
fn five_million_triangles_stay_interactive() {
    let t0 = std::time::Instant::now();
    let model = demo::synthetic_model(5_000_000, 4_000);
    let gen_ms = t0.elapsed().as_secs_f32() * 1000.0;

    let t1 = std::time::Instant::now();
    let mut stages: Vec<(&'static str, f32)> = Vec::new();
    let scene = Scene::from_model_with(model, &mut |stage, f| {
        if f >= 1.0 || stages.last().map_or(true, |(s, _)| *s != stage) {
            stages.push((stage, t1.elapsed().as_secs_f32() * 1000.0));
        }
    });
    let build_ms = t1.elapsed().as_secs_f32() * 1000.0;

    println!(
        "synthetic: {} tris, {} verts, {} elements, {} batches, {} bvh nodes\n\
         generate {gen_ms:.0} ms, build {build_ms:.0} ms, geometry {:.1} MB\n\
         stages: {:?}",
        scene.stats.triangles,
        scene.stats.vertices,
        scene.stats.elements,
        scene.stats.batches,
        scene.stats.bvh_nodes,
        scene.stats.geometry_bytes as f64 / 1e6,
        stages,
    );

    assert!(scene.stats.triangles > 4_500_000);
    // Batch count is the whole point of the (material × cell) planner: the
    // Metal backend draws a geometry's index buffer whole, so a batch is the
    // smallest thing frustum culling can skip. Too few = no culling, too many
    // = draw-call bound.
    assert!(
        scene.stats.batches >= 8 && scene.stats.batches <= 128,
        "{} batches",
        scene.stats.batches
    );
    // ≤ 40 B/vertex + 4 B/index is the §5 budget for the *packed* GPU stream;
    // the CPU-side batches are the 48-byte std140 layout plus contours, and
    // must still stay inside 2× that.
    let budget = scene.stats.vertices as u64 * 96 + scene.stats.triangles as u64 * 12;
    assert!(
        scene.stats.geometry_bytes < budget,
        "{} bytes over budget {}",
        scene.stats.geometry_bytes,
        budget
    );

    // ---- pick: < 1 ms at 5 M triangles ---------------------------------
    let state = SceneState::default();
    let mask = state.visibility_mask(&scene);
    let c = aabb_center(&scene.bounds);
    let r = aabb_radius(&scene.bounds);
    let mut seed = 7u32;
    let mut hits = 0;
    let n = 500;
    let t2 = std::time::Instant::now();
    for _ in 0..n {
        let origin = Vec3f {
            x: c.x + (rng(&mut seed) - 0.5) * r,
            y: c.y + (rng(&mut seed) - 0.5) * r,
            z: scene.bounds.max.z + r,
        };
        let dir = Vec3f {
            x: (rng(&mut seed) - 0.5) * 0.4,
            y: (rng(&mut seed) - 0.5) * 0.4,
            z: -1.0,
        };
        if scene
            .pick_masked(&Ray::new(origin, dir), &state, &mask)
            .is_some()
        {
            hits += 1;
        }
    }
    let per_pick_us = t2.elapsed().as_secs_f64() * 1e6 / n as f64;
    println!("pick: {per_pick_us:.1} µs average over {n} rays, {hits} hits");
    assert!(hits > n / 4, "only {hits}/{n} rays hit anything");
    assert!(per_pick_us < 1000.0, "pick took {per_pick_us:.0} µs");

    // ---- culling -------------------------------------------------------
    let mut visible = Vec::new();
    let mut cam_dist = r * 2.0;
    let t3 = std::time::Instant::now();
    for _ in 0..100 {
        cam_dist *= 1.0;
        let eye = Vec3f {
            x: c.x,
            y: c.y - cam_dist,
            z: c.z + cam_dist * 0.4,
        };
        let view = fab::model::makepad_math::Mat4f::look_at(
            eye,
            c,
            Vec3f { x: 0.0, y: 0.0, z: 1.0 },
        );
        let proj = fab::model::makepad_math::Mat4f::perspective(
            40.0,
            1.6,
            0.1,
            r * 10.0,
        );
        let vp = fab::model::makepad_math::Mat4f::mul(&proj, &view);
        let f = Frustum::from_view_proj(&vp);
        scene.visible_elements(&f, &state, &mut visible);
    }
    let cull_us = t3.elapsed().as_secs_f64() * 1e6 / 100.0;
    println!(
        "cull: {cull_us:.1} µs per frame, {} of {} elements visible",
        visible.len(),
        scene.stats.elements
    );
    assert!(cull_us < 5000.0, "culling took {cull_us:.0} µs");

    // ---- the lookup texture ------------------------------------------
    let mut lookup = Vec::new();
    let t4 = std::time::Instant::now();
    state.element_lookup(&scene, None, &mut lookup);
    let lookup_us = t4.elapsed().as_secs_f64() * 1e6;
    println!("lookup: {lookup_us:.0} µs for {} floats", lookup.len());
    assert_eq!(lookup.len(), scene.elements.len() * 8);
}

#[test]
#[ignore = "stress test: run with --release -- --ignored"]
fn bvh_beats_brute_force_and_agrees_with_it() {
    let scene = Scene::from_model(demo::synthetic_model(400_000, 400), &mut |_| {});
    let state = SceneState::default();
    let mask = state.visibility_mask(&scene);
    let c = aabb_center(&scene.bounds);
    let r = aabb_radius(&scene.bounds);
    let mut seed = 99u32;
    let mut checked = 0;
    for _ in 0..200 {
        let origin = Vec3f {
            x: c.x + (rng(&mut seed) - 0.5) * r,
            y: c.y + (rng(&mut seed) - 0.5) * r,
            z: scene.bounds.max.z + r * 0.5,
        };
        let dir = Vec3f {
            x: (rng(&mut seed) - 0.5) * 0.6,
            y: (rng(&mut seed) - 0.5) * 0.6,
            z: -1.0,
        };
        let ray = Ray::new(origin, dir);
        let fast = scene.pick_masked(&ray, &state, &mask);

        // Brute force over every triangle of every batch.
        let mut slow: Option<(ElementId, f32)> = None;
        for b in &scene.batches {
            for t in 0..b.triangle_count() as u32 {
                let (p, q, s) = b.triangle(t);
                if let Some((h, _, _)) = ray.intersect_triangle(p, q, s) {
                    if slow.map_or(true, |(_, bt)| h < bt) {
                        slow = Some((b.element_of_triangle(t).unwrap(), h));
                    }
                }
            }
        }
        match (fast, slow) {
            (Some(f), Some((e, t))) => {
                assert!((f.t - t).abs() < 1e-3, "t {} vs {t}", f.t);
                assert_eq!(f.element, e);
                checked += 1;
            }
            (None, None) => {}
            (a, b) => panic!("bvh {a:?} vs brute {b:?}"),
        }
    }
    println!("{checked} rays agreed with brute force");
    assert!(checked > 50);
}
