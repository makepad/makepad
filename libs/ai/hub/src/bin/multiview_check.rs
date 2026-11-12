//! End-to-end synthetic scan through the hub's native multi-view job backend.
//! Run `multiview-check <weights-dir> <model-id> <views>` once per model load.

use makepad_ai_hub::backend::{
    BackendCtx, CancelToken, ContentBackend, GenerateParams, NamedInput,
};
use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::multiview_backend::{
    decode_result, MultiviewBackend, MultiviewFrame, MultiviewResult,
    MULTIVIEW_SIZE,
};
use makepad_ai_hub::protocol::GenerateRequestJson;
use makepad_ai_hub::{registry, testpattern};
use makepad_ai_vision::mapanything_prep::{quaternion_to_rotation, rotation_to_quaternion};
use makepad_ai_vision::scan_scenes::{
    error_stats, look_at, orbit, render, Camera, Render, Scene, Shape,
};
use makepad_micro_serde::{SerJson, SerJsonState};
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(SerJson)]
struct Pose {
    quat: [f32; 4],
    trans: [f32; 3],
}

#[derive(SerJson)]
struct View {
    k: [f32; 4],
    pose: Pose,
}

#[derive(SerJson)]
struct Views {
    metric: bool,
    views: Vec<View>,
}

fn named(name: String, content_type: &str, bytes: Vec<u8>) -> NamedInput {
    NamedInput {
        name,
        content_type: content_type.into(),
        bytes,
    }
}

fn request(
    model: &str,
    cameras: &[Camera],
    renders: &[Render],
) -> Result<GenerateParams, Box<dyn Error>> {
    let mut input_b64 = None;
    let mut inputs = Vec::new();
    let mut views = Vec::new();
    for (index, (camera, render)) in cameras.iter().zip(renders).enumerate() {
        let rgba: Vec<u8> = render
            .rgb
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect();
        let png = testpattern::encode_png_rgba(&rgba, MULTIVIEW_SIZE, MULTIVIEW_SIZE)?;
        if index == 0 {
            input_b64 = Some(String::from_utf8(makepad_base64::base64_encode(
                &png,
                &makepad_base64::BASE64_STANDARD,
            ))?);
        } else {
            inputs.push(named(format!("view_{index}"), "image/png", png));
        }
        let rotation =
            std::array::from_fn(|i| std::array::from_fn(|j| camera.cam_to_world[i][j] as f32));
        views.push(View {
            k: [
                camera.fx as f32,
                camera.fy as f32,
                (camera.cx - 0.5) as f32,
                (camera.cy - 0.5) as f32,
            ],
            pose: Pose {
                quat: rotation_to_quaternion(rotation),
                trans: std::array::from_fn(|i| camera.cam_to_world[i][3] as f32),
            },
        });
        // Identical to scan_eval::eval_views: restart the LCG for each view,
        // and advance it only on positive depths on the stride-8 grid.
        let mut state = 7_u64;
        let mut sparse = vec![0.0_f32; MULTIVIEW_SIZE * MULTIVIEW_SIZE];
        for y in (0..MULTIVIEW_SIZE).step_by(8) {
            for x in (0..MULTIVIEW_SIZE).step_by(8) {
                let i = y * MULTIVIEW_SIZE + x;
                if render.depth_z[i] > 0.0 {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let u = 2.0 * (state >> 11) as f64 / (1_u64 << 53) as f64 - 1.0;
                    sparse[i] = (f64::from(render.depth_z[i]) * (1.0 + 0.01 * u)) as f32;
                }
            }
        }
        inputs.push(named(
            format!("depth_{index}"),
            "application/octet-stream",
            sparse.iter().flat_map(|v| v.to_le_bytes()).collect(),
        ));
    }
    inputs.push(named(
        "views".into(),
        "application/json",
        Views {
            metric: true,
            views,
        }
        .serialize_json()
        .into_bytes(),
    ));
    let mut params = GenerateParams::from_request(&GenerateRequestJson {
        model: model.into(),
        input_b64,
        ..Default::default()
    })?;
    params.inputs = inputs;
    Ok(params)
}

fn memory_info() -> Result<(usize, usize), String> {
    // The driver call exists whether or not the kernels were built (the `cuda`
    // feature without a toolkit builds none); a kernel-free build's link-clean
    // stub reports at runtime that CUDA is unavailable.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        makepad_ai_cuda::mem_get_info().map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err("multiview-check requires a CUDA platform".into())
    }
}

fn score(
    shape: &Shape,
    cameras: &[Camera],
    renders: &[Render],
    result: &MultiviewResult,
) -> Result<(), Box<dyn Error>> {
    let origin = result
        .views
        .first()
        .ok_or("result has no view 0")?
        .pose
        .1
        .map(f64::from);
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (view, camera) in result.views.iter().zip(cameras).skip(1) {
        for (axis, &c0) in origin.iter().enumerate() {
            let c = f64::from(view.pose.1[axis]) - c0;
            // Match the f32 translations actually sent on the wire.
            let g = f64::from(camera.cam_to_world[axis][3] as f32) - c0;
            numerator += c * g;
            denominator += c * c;
        }
    }
    if !denominator.is_finite() || denominator <= 0.0 {
        return Err("predicted camera centres have no finite nonzero baseline".into());
    }
    let scale = numerator / denominator;
    if !scale.is_finite() {
        return Err("camera-centre scale is not finite".into());
    }
    let mut object_pixels = 0;
    let mut points = Vec::new();
    for (view, render) in result.views.iter().zip(renders) {
        let [fx, fy, cx, cy] = view.k.map(f64::from);
        if fx == 0.0 || fy == 0.0 {
            return Err("result k has a zero focal length".into());
        }
        let rotation = quaternion_to_rotation(view.pose.0).map(|row| row.map(f64::from));
        for (pixel, &hit) in render.hit.iter().enumerate() {
            if hit != 1 {
                continue;
            }
            object_pixels += 1;
            if view.mask[pixel] != 1 {
                continue;
            }
            let z = f64::from(view.depth_z[pixel]);
            let camera_point = [
                ((pixel % MULTIVIEW_SIZE) as f64 - cx) / fx * z,
                ((pixel / MULTIVIEW_SIZE) as f64 - cy) / fy * z,
                z,
            ];
            let world: [f64; 3] = std::array::from_fn(|i| {
                rotation[i]
                    .iter()
                    .zip(camera_point)
                    .map(|(r, p)| r * p)
                    .sum::<f64>()
                    + f64::from(view.pose.1[i])
            });
            if !world.iter().all(|v| v.is_finite()) {
                return Err("non-finite reconstructed point".into());
            }
            points.push(world);
        }
    }
    let valid = vec![true; points.len()];
    let stats = error_stats(shape, &points, &valid);
    println!("SCORE box scale=model object_px={object_pixels} kept={} mean_mm={:.4} p95_mm={:.4} max_mm={:.4}",
        stats.count, stats.mean_mm, stats.p95_mm, stats.max_mm);
    for point in &mut points {
        for i in 0..3 {
            point[i] = origin[i] + scale * (point[i] - origin[i]);
        }
    }
    let stats = error_stats(shape, &points, &valid);
    println!("SCORE box scale=poses s={scale:.6} object_px={object_pixels} kept={} mean_mm={:.4} p95_mm={:.4} max_mm={:.4}",
        stats.count, stats.mean_mm, stats.p95_mm, stats.max_mm);
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let usage = "usage: multiview-check <weights-dir> <model-id> <views> (>= 1)";
    let mut args = std::env::args_os().skip(1);
    let cache_dir = PathBuf::from(args.next().ok_or(usage)?);
    let model_id = args
        .next()
        .and_then(|s| s.into_string().ok())
        .ok_or(usage)?;
    let count = args
        .next()
        .and_then(|s| s.into_string().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .ok_or(usage)?;
    if args.next().is_some() {
        return Err(usage.into());
    }
    let spec = registry::embedded_registry()
        .find(&model_id)
        .ok_or_else(|| format!("unknown model {model_id}"))?;
    if spec.backend != "multiview-native" {
        return Err(format!("model {model_id} is not multiview-native").into());
    }
    // Fail before loading if the node's cache is incomplete.
    for file in &spec.files {
        if !file.dest_path(&cache_dir).is_file() {
            return Err(format!("missing cached model file {}", file.cache_as).into());
        }
    }
    let camera = Camera {
        width: MULTIVIEW_SIZE,
        height: MULTIVIEW_SIZE,
        fx: 400.0,
        fy: 400.0,
        cx: 259.0,
        cy: 259.0,
        skew: 0.0,
        distortion: [0.0; 5],
        cam_to_world: look_at([0.4, -0.25, 0.0], [0.0, -0.04, 0.0]),
    };
    let cameras = orbit(
        &camera,
        count,
        0.40,
        0.25,
        0.04,
        0.0,
        2.0 * std::f64::consts::PI / 3.0,
        0.0,
        0.0,
        1,
    );
    let scene = Scene {
        shape: Shape::Box {
            half: [0.05, 0.04, 0.03],
        },
        ground: true,
    };
    let renders: Vec<_> = cameras
        .iter()
        .map(|camera| render(&scene, camera))
        .collect();
    let params = request(&model_id, &cameras, &renders)?;
    let downloader = Downloader::new("http://127.0.0.1:1", None)?;
    let cancel = CancelToken::new();
    let mut download_progress = |_| {};
    let mut progress = |_: &str, _: f64| {};
    let mut ctx = BackendCtx {
        spec,
        cache_dir: &cache_dir,
        downloader: &downloader,
        download_progress: &mut download_progress,
        cancel: &cancel,
        progress: &mut progress,
    };
    let mut backend = MultiviewBackend::new_native(&model_id);
    let (free, total) = memory_info()?;
    let baseline = total - free;
    let stop = AtomicBool::new(false);
    let (artifacts, load_s, generate_s, after_load, peak) =
        std::thread::scope(|scope| -> Result<_, Box<dyn Error>> {
            let sampler = scope.spawn(|| -> Result<usize, String> {
                let mut peak = baseline;
                loop {
                    let (free, total) = memory_info()?;
                    peak = peak.max(total - free);
                    if stop.load(Ordering::Acquire) {
                        return Ok(peak);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            let outcome = (|| -> Result<_, Box<dyn Error>> {
                let start = Instant::now();
                backend.ensure_loaded(&mut ctx)?;
                let load_s = start.elapsed().as_secs_f64();
                let (free, total) = memory_info()?;
                let after_load = total - free;
                let start = Instant::now();
                let generated = backend.generate(&params, &mut |_, _| {}, &cancel);
                let generate_s = start.elapsed().as_secs_f64();
                stop.store(true, Ordering::Release);
                Ok((generated?, load_s, generate_s, after_load))
            })();
            // Join on failures too: a load/generation error must not leave a sampler alive.
            stop.store(true, Ordering::Release);
            let sampled = sampler.join().map_err(|_| "VRAM sampler panicked");
            let (artifacts, load_s, generate_s, after_load) = outcome?;
            let peak = sampled??.max(after_load);
            Ok((artifacts, load_s, generate_s, after_load, peak))
        })?;
    let mib = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
    println!("VRAM model={model_id} views={count} baseline_mib={:.2} after_load_mib={:.2} peak_mib={:.2} peak_minus_baseline_mib={:.2} total_mib={:.2}",
        mib(baseline), mib(after_load), mib(peak), mib(peak - baseline), mib(total));
    println!("RUN model={model_id} views={count} load_s={load_s:.3} generate_s={generate_s:.3}");
    let [json, bin] = artifacts.as_slice() else {
        return Err("expected two result artifacts".into());
    };
    let (result, frame) = decode_result(&json.bytes, &bin.bytes)?;
    if frame != MultiviewFrame::Input {
        return Err("result frame is not input".into());
    }
    if result.views.len() != count {
        return Err("result view count differs from request".into());
    }
    for (index, view) in result.views.iter().enumerate() {
        if !view
            .depth_z
            .iter()
            .chain(&view.confidence)
            .all(|v| v.is_finite())
        {
            return Err(format!("view {index}: non-finite depth or confidence").into());
        }
    }
    if count == 6 {
        score(&scene.shape, &cameras, &renders, &result)?;
    } else {
        println!("SCORE skipped");
    }
    Ok(())
}

fn main() {
    makepad_ai_common::backend::metal::run_model_worker("multiview_check", model_main);
}

fn model_main() {
    if let Err(error) = run() {
        eprintln!("multiview-check: {error}");
        std::process::exit(1);
    }
}
