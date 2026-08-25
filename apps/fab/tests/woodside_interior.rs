//! Converted-sample glazing and interior-light regression.

use fab::model::makepad_math::vec3;
use fab::model::{Loader, Scene};
use fab::render::{scene_input_from_snapshot, TrackFile};
use fab::AppState;
use makepad_fab_loader_gltf::GltfLoader;
use makepad_micro_serde::DeJson;
use makepad_raytrace::cpu_ref::cpu_tracer;
use makepad_raytrace::pack::PackedScene;
use makepad_raytrace::{Camera, Sun};
use std::path::Path;
use std::sync::Arc;

#[test]
fn woodside_interior_keys_are_lit_through_converted_glass() {
    // Heavy: loads the converted house, packs the scene and runs the CPU
    // tracer over the interior keys. Its peak memory reached ~120 GB on the
    // 2026-08-25 build, so it runs only when asked for explicitly.
    if std::env::var("FAB_HEAVY_TESTS").is_err() {
        eprintln!("skipped: set FAB_HEAVY_TESTS=1 to run the interior daylight test");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("app crate is inside the workspace");
    let sample = root.join("local/models/samples/woodside.glb");
    let track_path = root.join("local/models/tours/woodside-full-track.json");
    if !sample.exists() || !track_path.exists() {
        eprintln!(
            "skipping: converted sample or track absent ({} / {})",
            sample.display(),
            track_path.display()
        );
        return;
    }

    let track_text = std::fs::read_to_string(&track_path).expect("read Woodside track");
    let track = TrackFile::deserialize_json(&track_text).expect("valid Woodside track");
    let document = GltfLoader
        .load(&sample, &mut |_| {})
        .expect("load converted Woodside GLB");
    eprintln!("Woodside material dump (name, transmission, ior, double-sided):");
    for material in document.materials() {
        eprintln!(
            "  {:?}: transmission={:.3} ior={:.3} double_sided={}",
            material.name, material.transmission, material.ior, material.double_sided
        );
    }
    let glass_materials = document
        .materials()
        .iter()
        .filter(|material| material.transmission > 0.0)
        .count();
    assert!(glass_materials > 0, "Woodside has no transmissive materials");
    assert!(document
        .materials()
        .iter()
        .filter(|material| material.transmission > 0.0)
        .all(|material| material.double_sided));

    let scene = Arc::new(Scene::from_document(document, &mut |_| {}));
    let mut state = AppState::default();
    state.set_scene(scene);
    let mut input = scene_input_from_snapshot(state.snapshot.as_ref().expect("scene snapshot"), &state);
    input.sun = Sun {
        dir: vec3(0.35, 0.25, 0.9).normalize(),
        turbidity: 2.5,
        sky_strength: 1.0,
        sun_strength: 4.0,
    };
    let glass_triangles = input
        .tri_material
        .iter()
        .filter(|&&material| {
            input
                .materials
                .get(material as usize)
                .is_some_and(|material| material.transmission > 0.0)
        })
        .count();
    assert!(glass_triangles > 0, "Woodside has no transmissive triangles");

    let packed = PackedScene::pack(&input);
    for key_index in [2300_usize, 2600, 2880, 2900] {
        let key = track
            .keys
            .get(key_index)
            .unwrap_or_else(|| panic!("Woodside track omitted key {key_index}"));
        input.camera = Camera {
            pos: vec3(key.pos[0], key.pos[1], key.pos[2]),
            target: vec3(key.look_at[0], key.look_at[1], key.look_at[2]),
            up: vec3(key.up[0], key.up[1], key.up[2]),
            fov_y: key.fov_y_deg.to_radians(),
            f_stop: 0.0,
            ..Default::default()
        };
        let tracer = cpu_tracer(&input, &packed);
        let image = tracer.render(16, 10, 64, key_index as u32);
        let mean = image
            .iter()
            .map(|pixel| (pixel[0] + pixel[1] + pixel[2]) / 3.0)
            .sum::<f32>()
            / image.len() as f32;
        let metered_exposure = makepad_raytrace::gpu::metered_exposure_from_rgb(
            &image, 16, 10, 1.0,
        );
        let display_mean = |exposure| {
            image
                .iter()
                .map(|&pixel| {
                    let display = makepad_raytrace::gpu::tonemap_rgb(pixel, exposure);
                    0.2126 * display[0] + 0.7152 * display[1] + 0.0722 * display[2]
                })
                .sum::<f32>()
                / image.len() as f32
        };
        let fixed_display = display_mean(1.0) * 255.0;
        let metered_display = display_mean(metered_exposure) * 255.0;
        eprintln!(
            "Woodside key {key_index}: linear mean {mean:.6}, display fixed={fixed_display:.2}/255 metered={metered_display:.2}/255 at {metered_exposure:.2}x, glass {glass_materials} materials/{glass_triangles} triangles, stats {:?}",
            tracer.stats()
        );
        assert!(mean > 0.0, "Woodside key {key_index} stayed black");
        if key_index == 2880 {
            assert!(
                metered_display > 80.0,
                "Woodside key 2880 metered mean {metered_display}/255"
            );
        }
    }
}
