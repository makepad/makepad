//! Run `scan-eval <model.safetensors> <model-id> <f32|bf16> [reference-dir]` to score synthetic scans in mm.
//! The model id keys the device weight cache (`mapanything` or `mapanything-apache`).
//!
//! With a reference directory, every (shape, variant) that has a reference run at
//! `<reference-dir>/<model-id>/<shape>/<variant>/` is also scored from the reference's own outputs
//! (`REFSCALE`, `REFSCORE`) and compared with the port's outputs on the same inputs (`PARITY`).
//! A reference run holds, per view i, raw little-endian f32 files `view<i>_depth_along_ray.f32`
//! (518*518), `view<i>_ray_directions.f32` (518*518*3), `view<i>_conf.f32`, `view<i>_mask_logits.f32`,
//! `view<i>_quat.f32` (xyzw) and `view<i>_trans.f32`, plus `metric_scale.f32`.

use makepad_ai_vision::mapanything::{MapAnything, MapAnythingPrecision, MapAnythingViewOutput, MAPANYTHING_SIZE};
use makepad_ai_vision::mapanything_prep::pose_in_frame_of;
use makepad_ai_vision::scan_eval::{
    eval_cameras, eval_shapes, eval_variants, eval_views, pose_error, scale_from_camera_centres, scale_from_truth_points,
    score_views, ViewScore,
};
use makepad_ai_vision::scan_scenes::{render, Camera, Render, Scene, Shape};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn print_score(out: &mut impl Write, tag: &str, shape: &str, variant: &str, scale: &str, view: impl std::fmt::Display, score: &ViewScore) -> std::io::Result<()> {
    writeln!(out, "{tag} {shape} {variant} scale={scale} view={view} object_px={} kept={} mean_mm={:.4} p95_mm={:.4} max_mm={:.4}",
        score.object_pixels, score.kept, score.error.mean_mm, score.error.p95_mm, score.error.max_mm)
}

fn read_f32(path: &Path, len: usize) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() != len * 4 {
        return Err(format!("{}: {} bytes, expected {}", path.display(), bytes.len(), len * 4));
    }
    Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

fn read_array<const N: usize>(path: &Path) -> Result<[f32; N], String> {
    let values = read_f32(path, N)?;
    values.try_into().map_err(|_| format!("{}: expected {N} values", path.display()))
}

/// The reference's outputs for one run, as the port's output type, and its metric scale.
fn read_reference(dir: &Path, views: usize) -> Result<(Vec<MapAnythingViewOutput>, f32), String> {
    let pixels = MAPANYTHING_SIZE * MAPANYTHING_SIZE;
    let mut outputs = Vec::with_capacity(views);
    for view in 0..views {
        let file = |name: &str| dir.join(format!("view{view}_{name}.f32"));
        outputs.push(MapAnythingViewOutput {
            rays: read_f32(&file("ray_directions"), pixels * 3)?,
            depth_along_ray: read_f32(&file("depth_along_ray"), pixels)?,
            confidence: read_f32(&file("conf"), pixels)?,
            mask_logits: read_f32(&file("mask_logits"), pixels)?,
            quat: read_array(&file("quat"))?,
            trans: read_array(&file("trans"))?,
        });
    }
    let [scale] = read_array(&dir.join("metric_scale.f32"))?;
    Ok((outputs, scale))
}

/// Port against reference on the same inputs: relative L2 of depth along the ray over all pixels, the mean
/// and max absolute depth difference over object pixels in mm, and the worst pose difference.
fn print_parity(
    out: &mut impl Write,
    shape: &str,
    variant: &str,
    renders: &[Render],
    port: &[MapAnythingViewOutput],
    port_scale: f32,
    reference: &[MapAnythingViewOutput],
    reference_scale: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut diff2, mut ref2, mut object_sum, mut object_max, mut object_count) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0usize);
    let (mut rot_max, mut trans_max) = (0.0f64, 0.0f64);
    for ((render, port), reference) in renders.iter().zip(port).zip(reference) {
        for ((&ours, &theirs), &hit) in port.depth_along_ray.iter().zip(&reference.depth_along_ray).zip(&render.hit) {
            let diff = f64::from(ours) - f64::from(theirs);
            diff2 += diff * diff;
            ref2 += f64::from(theirs) * f64::from(theirs);
            if hit == 1 {
                object_sum += diff.abs();
                object_max = object_max.max(diff.abs());
                object_count += 1;
            }
        }
        let (rotation, translation) = pose_error((reference.quat, reference.trans), (port.quat, port.trans));
        rot_max = rot_max.max(rotation);
        trans_max = trans_max.max(translation);
    }
    if ref2 == 0.0 || object_count == 0 {
        return Err(format!("{shape} {variant}: empty reference depth").into());
    }
    writeln!(out, "PARITY {shape} {variant} depth_rel_l2={:.3e} object_mean_mm={:.4} object_max_mm={:.4} rot_max_deg={rot_max:.4} trans_max_mm={trans_max:.4} scale_rel={:.3e}",
        (diff2 / ref2).sqrt(), object_sum / object_count as f64 * 1000.0, object_max * 1000.0,
        (f64::from(port_scale) - f64::from(reference_scale)).abs() / f64::from(reference_scale).abs())?;
    Ok(())
}

/// The three scales and their scores for one set of outputs, tagged `SCALE`/`SCORE` or `REFSCALE`/`REFSCORE`.
fn print_scores(
    out: &mut impl Write,
    reference: bool,
    shape_name: &str,
    variant: &str,
    shape: &Shape,
    cameras: &[Camera],
    renders: &[Render],
    poses: &[([f32; 4], [f32; 3])],
    outputs: &[MapAnythingViewOutput],
    metric_scale: f32,
) -> Result<f64, Box<dyn std::error::Error>> {
    let camera0 = cameras.first().ok_or("no cameras")?;
    let s_poses = scale_from_camera_centres(poses, outputs)?;
    let s_oracle = scale_from_truth_points(cameras, renders, outputs)?;
    let (scale_tag, score_tag) = if reference { ("REFSCALE", "REFSCORE") } else { ("SCALE", "SCORE") };
    writeln!(out, "{scale_tag} {shape_name} {variant} model={metric_scale:.6} poses={s_poses:.6} oracle={s_oracle:.6}")?;
    for (scale_name, scale) in [("model", 1.0), ("poses", s_poses), ("oracle", s_oracle)] {
        let (scores, all) = score_views(shape, camera0, renders, outputs, scale)?;
        if scale_name == "poses" && !reference {
            for (view, score) in scores.iter().enumerate() {
                print_score(out, score_tag, shape_name, variant, scale_name, view, score)?;
            }
        }
        print_score(out, score_tag, shape_name, variant, scale_name, "all", &all)?;
    }
    Ok(s_poses)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: scan-eval <model.safetensors> <model-id> <f32|bf16> [reference-dir]";
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or(usage)?;
    let model_id = args.next().and_then(|id| id.into_string().ok()).ok_or(usage)?;
    let precision = match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("f32") => MapAnythingPrecision::StrictF32,
        Some("bf16") => MapAnythingPrecision::Bf16,
        _ => return Err(usage.into()),
    };
    let reference_root = args.next().map(PathBuf::from);
    if args.next().is_some() { return Err(usage.into()); }
    let model = MapAnything::load(path, &model_id, precision, None)?;
    let cameras = eval_cameras();
    let mut stdout = std::io::stdout().lock();
    for (name, shape) in eval_shapes() {
        let scene = Scene { shape, ground: true };
        let renders: Vec<_> = cameras.iter().map(|camera| render(&scene, camera)).collect();
        for (variant, depth, with_poses) in eval_variants() {
            let inputs = eval_views(&cameras, &renders, &depth)?;
            let views = inputs.views(with_poses);
            let start = Instant::now();
            let output = model.forward(&views, None, None)?;
            let seconds = start.elapsed().as_secs_f64();
            writeln!(stdout, "RUN {name} {variant} forward_s={seconds:.3} metric_scale={:.6}", output.metric_scale)?;
            let s_poses = print_scores(&mut stdout, false, name, variant, &scene.shape, &cameras, &renders, &inputs.poses, &output.views, output.metric_scale)?;
            let &(q0, t0) = inputs.poses.first().ok_or("inputs contain no poses")?;
            for (view, (&(q, t), predicted)) in inputs.poses.iter().zip(&output.views).enumerate().skip(1) {
                let truth = pose_in_frame_of(q0, t0, q, t);
                for (scale_name, scale) in [("model", 1.0), ("poses", s_poses)] {
                    let trans = predicted.trans.map(|value| (f64::from(value) * scale) as f32);
                    let (rotation, translation) = pose_error(truth, (predicted.quat, trans));
                    writeln!(stdout, "POSE {name} {variant} scale={scale_name} view={view} rot_deg={rotation:.4} trans_mm={translation:.4}")?;
                }
            }
            if let Some(root) = &reference_root {
                let dir = root.join(&model_id).join(name).join(variant);
                if dir.is_dir() {
                    let (reference, reference_scale) = read_reference(&dir, cameras.len())?;
                    print_scores(&mut stdout, true, name, variant, &scene.shape, &cameras, &renders, &inputs.poses, &reference, reference_scale)?;
                    print_parity(&mut stdout, name, variant, &renders, &output.views, output.metric_scale, &reference, reference_scale)?;
                }
            }
        }
    }
    stdout.flush()?;
    Ok(())
}

fn main() {
    makepad_ai_common::backend::metal::run_model_worker("scan_eval", model_main);
}

fn model_main() {
    if let Err(error) = run() {
        eprintln!("scan-eval: {error}");
        std::process::exit(1);
    }
}
