// Included by obj/splat_sort.rs under #[cfg(test)].
use std::path::PathBuf;

fn lcg(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*seed >> 40) as f32) / (1u64 << 24) as f32
}

fn test_camera(radial: bool, camera_pos: Vec3f, target: Vec3f) -> SortCamera {
    let (w, h) = (1024.0f32, 768.0f32);
    let view = Mat4f::look_at(camera_pos, target, vec3(0.0, 1.0, 0.0));
    let projection = Mat4f::perspective(34.0, w / h, 0.05, 200.0);
    SortCamera {
        view,
        model: Mat4f::identity(),
        projection,
        radial,
        focal_px: projection.v[0].abs() * w * 0.5,
        ndc_per_px: vec2(2.0 / w, 2.0 / h),
        splat_std_dev: 2.8,
        coarse_cull_guard: 2.0,
        min_pixel_radius: 0.0,
        max_pixel_radius: 512.0,
        cull_margin_ndc: 0.0,
        behind_margin: 0.0,
        viewport_px: w * h,
    }
}

fn random_scene(count: usize, seed: &mut u64) -> SortScene {
    let centers: Vec<[f32; 3]> = (0..count)
        .map(|_| {
            [
                (lcg(seed) - 0.5) * 2.2,
                (lcg(seed) - 0.5) * 2.2,
                (lcg(seed) - 0.5) * 2.2,
            ]
        })
        .collect();
    let radius: Vec<f32> = (0..count).map(|_| 0.002 + lcg(seed) * 0.01).collect();
    let product: Vec<f32> = radius.iter().map(|r| r * r * 0.5).collect();
    SortScene::new(centers, radius, product)
}

fn metric_of(scene: &SortScene, cam: &SortCamera, id: usize) -> f32 {
    let mv = Mat4f::mul(&cam.view, &cam.model);
    let c = scene.centers[id];
    let v = mv.transform_vec4(vec4(c[0], c[1], c[2], 1.0));
    if cam.radial {
        -(v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
    } else {
        v.z
    }
}

/// Visibility exactly as the sort decides it (mirrors the key pass).
fn is_visible(scene: &SortScene, cam: &SortCamera, id: usize) -> bool {
    if scene.radius_bound[id] < 0.0 {
        return false;
    }
    let mv = Mat4f::mul(&cam.view, &cam.model);
    let c = scene.centers[id];
    let v = mv.transform_vec4(vec4(c[0], c[1], c[2], 1.0));
    if v.z > cam.behind_margin {
        return false;
    }
    let clip = cam.projection.transform_vec4(v);
    let inv_w = 1.0 / clip.w.abs().max(1e-6);
    let (nx, ny) = (clip.x * inv_w, clip.y * inv_w);
    // Same operation order as the key pass so borderline splats agree.
    let inv_depth = 1.0 / (-v.z).max(1e-6);
    let guard = 1.0 + cam.coarse_cull_guard.max(0.0) * nx.abs().max(ny.abs());
    let std_dev_bound = cam.splat_std_dev * 1.732051;
    let rough = std_dev_bound * scene.radius_bound[id] * cam.focal_px.max(1e-5) * inv_depth * guard;
    if rough < cam.min_pixel_radius {
        return false;
    }
    nx.abs() <= 1.0 + cam.cull_margin_ndc + rough * cam.ndc_per_px.x
        && ny.abs() <= 1.0 + cam.cull_margin_ndc + rough * cam.ndc_per_px.y
}

fn check_sorted(scene: &SortScene, cam: &SortCamera) -> SortStats {
    let mut scratch = SortScratch::default();
    let mut out = Vec::new();
    let stats = sort_visible(scene, cam, &mut scratch, &mut out);
    assert_eq!(out.len(), stats.visible);
    // Every visible splat appears exactly once, nothing culled appears.
    let mut seen = vec![false; scene.centers.len()];
    for &id in &out {
        let id = id as usize;
        assert!(!seen[id], "duplicate id {id}");
        seen[id] = true;
        assert!(is_visible(scene, cam, id), "culled id {id} in output");
    }
    for id in 0..scene.centers.len() {
        assert_eq!(seen[id], is_visible(scene, cam, id), "id {id} visibility");
    }
    // Far-to-near by 16-bit key (same quantization as the sort), stable
    // (ascending id) within a bucket.
    let mv = Mat4f::mul(&cam.view, &cam.model);
    let (lo, hi) = metric_range(scene, cam, &mv);
    let scale = if hi > lo {
        (BUCKETS as f32 - 1.0) / (hi - lo)
    } else {
        0.0
    };
    let key_of = |id: usize| -> u16 {
        ((metric_of(scene, cam, id) - lo) * scale).clamp(0.0, BUCKETS as f32 - 1.0) as u16
    };
    for pair in out.windows(2) {
        let (a, b) = (pair[0] as usize, pair[1] as usize);
        let (ka, kb) = (key_of(a), key_of(b));
        assert!(ka <= kb, "order violated: key {ka} (id {a}) before key {kb} (id {b})");
        if ka == kb {
            assert!(a < b, "unstable within bucket: {a} after {b}");
        }
    }
    stats
}

#[test]
fn sort_visible_orders_far_to_near_and_culls() {
    let mut seed = 11u64;
    let scene = random_scene(50_000, &mut seed);
    // Outside looking in (view-z metric) and inside looking out (radial).
    let outside = test_camera(false, vec3(0.0, 0.4, 2.5), vec3(0.0, 0.0, 0.0));
    let stats = check_sorted(&scene, &outside);
    assert!(stats.visible > 0 && stats.visible < scene.centers.len());
    let inside = test_camera(true, vec3(0.05, 0.0, 0.1), vec3(1.0, 0.0, 0.0));
    let stats = check_sorted(&scene, &inside);
    // Roughly a third of a cube is in a 34-degree frustum looking out of it.
    assert!(stats.visible > 0 && stats.visible < scene.centers.len() / 2);
    assert!(stats.est_quad_overdraw > 0.0);
}

#[test]
fn sort_visible_handles_degenerate_inputs() {
    let mut scratch = SortScratch::default();
    let mut out = vec![1.0, 2.0];
    let empty = SortScene::new(Vec::new(), Vec::new(), Vec::new());
    let cam = test_camera(false, vec3(0.0, 0.0, 2.0), vec3(0.0, 0.0, 0.0));
    let stats = sort_visible(&empty, &cam, &mut scratch, &mut out);
    assert_eq!(stats.visible, 0);
    assert!(out.is_empty());
    // All splats at one point (zero metric range) still sort.
    let same = SortScene::new(vec![[0.0, 0.0, 0.0]; 10], vec![0.01; 10], vec![0.0001; 10]);
    let stats = sort_visible(&same, &cam, &mut scratch, &mut out);
    assert_eq!(stats.visible, 10);
    let ids: Vec<usize> = out.iter().map(|v| *v as usize).collect();
    assert_eq!(ids, (0..10).collect::<Vec<_>>());
    // Everything behind the camera -> nothing.
    let behind = test_camera(false, vec3(0.0, 0.0, -5.0), vec3(0.0, 0.0, -10.0));
    let stats = sort_visible(&same, &behind, &mut scratch, &mut out);
    assert_eq!(stats.visible, 0);
}

#[test]
fn min_pixel_radius_culls_small_splats_and_margin_keeps_edge_splats() {
    let mut seed = 5u64;
    let scene = random_scene(20_000, &mut seed);
    let mut cam = test_camera(false, vec3(0.0, 0.0, 3.0), vec3(0.0, 0.0, 0.0));
    let base = check_sorted(&scene, &cam).visible;
    cam.min_pixel_radius = 8.0;
    let culled = check_sorted(&scene, &cam).visible;
    assert!(culled < base);
    cam.min_pixel_radius = 0.0;
    cam.cull_margin_ndc = 0.5;
    let with_margin = check_sorted(&scene, &cam).visible;
    assert!(with_margin >= base);
}

fn local_sample(name: &str) -> Option<PathBuf> {
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

/// cargo test -p makepad-xr --release splat_sort_bench -- --ignored --nocapture
#[test]
#[ignore]
fn splat_sort_bench() {
    for (name, radial) in [("biker.ply", false), ("coastal_world.ply", true)] {
        let Some(path) = local_sample(name) else {
            println!("{name}: not present, skipped");
            continue;
        };
        let loaded = makepad_splat::load_splat_from_path(&path).expect("load");
        // Normalize like ViewSplat does so the camera numbers mean the same.
        let center = [
            (loaded.bounds_min[0] + loaded.bounds_max[0]) * 0.5,
            (loaded.bounds_min[1] + loaded.bounds_max[1]) * 0.5,
            (loaded.bounds_min[2] + loaded.bounds_max[2]) * 0.5,
        ];
        let extent = (0..3)
            .map(|a| loaded.bounds_max[a] - loaded.bounds_min[a])
            .fold(1e-6f32, f32::max);
        let s = 2.2 / extent;
        let centers: Vec<[f32; 3]> = loaded
            .splats
            .iter()
            .map(|sp| {
                [
                    (sp.position[0] - center[0]) * s,
                    (sp.position[1] - center[1]) * s,
                    (sp.position[2] - center[2]) * s,
                ]
            })
            .collect();
        let (radius, product): (Vec<f32>, Vec<f32>) = loaded
            .splats
            .iter()
            .map(|sp| {
                let mut a = [sp.scale[0] * s, sp.scale[1] * s, sp.scale[2] * s];
                a.sort_by(|x, y| y.total_cmp(x));
                (a[0], a[0] * a[1])
            })
            .unzip();
        let scene = SortScene::new(centers, radius, product);
        let mut scratch = SortScratch::default();
        let mut out = Vec::new();
        for (label, pos, target) in [
            ("inside", vec3(0.0, 0.0, 0.3), vec3(0.0, 0.0, 0.0)),
            ("outside", vec3(0.0, 0.3, 1.5), vec3(0.0, 0.0, 0.0)),
        ] {
            let cam = test_camera(radial, pos, target);
            let _ = sort_visible(&scene, &cam, &mut scratch, &mut out); // warm
            let runs = 5;
            let started = Instant::now();
            let mut stats = SortStats::default();
            for _ in 0..runs {
                stats = sort_visible(&scene, &cam, &mut scratch, &mut out);
            }
            println!(
                "SORT_BENCH {name} {label}: total={} visible={} sort_ms={:.2} (avg of {runs}) overdraw_est={:.1} payload_mb={:.1}",
                stats.total,
                stats.visible,
                started.elapsed().as_secs_f64() * 1000.0 / runs as f64,
                stats.est_quad_overdraw,
                (out.len() * 4) as f64 / 1e6
            );
        }
    }
}
