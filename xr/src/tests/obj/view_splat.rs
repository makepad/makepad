// Included by obj/view_splat.rs under #[cfg(test)].
//
// CPU-side benchmarks for the splat depth sort. Ignored by default (they
// need the local sample scenes); run with:
//   cargo test -p makepad-xr --release splat_sort_bench -- --ignored --nocapture
fn local_sample(name: &str) -> Option<PathBuf> {
    // `local/` lives in the main checkout; worktrees reach it four levels up.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("MAKEPAD_LOCAL_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    candidates.push(manifest.join("../local"));
    candidates.push(manifest.join("../../../../local"));
    candidates
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| path.exists())
}

fn sample_centers(name: &str) -> Option<Vec<[f32; 3]>> {
    let path = local_sample(name)?;
    let scene = makepad_splat::load_splat_from_path(&path).ok()?;
    Some(scene.splats.iter().map(|s| s.position).collect())
}

fn bench_camera(step: usize) -> (Mat4f, Mat4f, Vec3f) {
    let yaw = step as f32 * 0.05;
    let camera_pos = vec3(yaw.sin() * 1.5, 0.2, yaw.cos() * 1.5);
    let view = Mat4f::look_at(camera_pos, vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0));
    (view, Mat4f::identity(), camera_pos)
}

#[test]
#[ignore]
fn splat_sort_bench() {
    for (name, radial) in [("biker.ply", true), ("coastal_world.ply", true)] {
        let Some(centers) = sample_centers(name) else {
            println!("{name}: not present, skipped");
            continue;
        };
        let mut keys = Vec::new();
        let mut order_a = Vec::new();
        let mut order_b = Vec::new();
        let mut indices = Vec::new();
        let steps = 6;
        let mut sort_ms = 0.0;
        let mut build_ms = 0.0;
        for step in 0..steps {
            let (view, model, camera_pos) = bench_camera(step);
            let t0 = Instant::now();
            sort_splats_radix(
                view,
                model,
                camera_pos,
                radial,
                &centers,
                &mut keys,
                &mut order_a,
                &mut order_b,
            );
            let t1 = Instant::now();
            build_sorted_triangle_indices(&order_a, &mut indices);
            let t2 = Instant::now();
            if step > 0 {
                sort_ms += (t1 - t0).as_secs_f64() * 1000.0;
                build_ms += (t2 - t1).as_secs_f64() * 1000.0;
            }
        }
        let n = (steps - 1) as f64;
        println!(
            "SORT_BENCH {name}: splats={} sort_ms={:.1} index_build_ms={:.1} payload_mb={:.1}",
            centers.len(),
            sort_ms / n,
            build_ms / n,
            (indices.len() * 4) as f64 / 1e6
        );
    }
}
