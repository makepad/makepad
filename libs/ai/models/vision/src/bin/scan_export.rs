//! Run `scan-export <out-dir> [shape...]` to write the synthetic evaluation's inputs as raw files, for running
//! another implementation on exactly the inputs `scan-eval` gives the port. Host only, no model, no GPU.
//!
//! Per shape (default: every evaluation shape), `<out-dir>/<shape>/` holds, per view i, little-endian raw files:
//! `view<i>_rgb.u8` (518*518*3, row-major RGB), `view<i>_k.f64` (fx, fy, cx, cy with pixel (x, y)'s centre at
//! (x, y)), `view<i>_c2w.f64` (row-major 4x4 camera-to-world, OpenCV axes, metres), and for every variant with
//! depth `view<i>_depth_<variant>.f32` (518*518 z depth in metres, 0 = unknown). `<out-dir>/index.json` lists
//! the size, the view count, the shapes and the variants with whether each gives poses and depth.

use makepad_ai_vision::mapanything::MAPANYTHING_SIZE;
use makepad_ai_vision::scan_eval::{eval_cameras, eval_shapes, eval_variants, sparse_depth_z, DepthInput};
use makepad_ai_vision::scan_scenes::{render, Scene};
use std::path::PathBuf;

fn write(path: PathBuf, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: scan-export <out-dir> [shape...]";
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().ok_or(usage)?);
    let wanted: Vec<String> = args.collect();
    let shapes: Vec<_> = eval_shapes().into_iter().filter(|(name, _)| wanted.is_empty() || wanted.iter().any(|w| w == name)).collect();
    if !wanted.is_empty() && shapes.len() != wanted.len() {
        return Err(format!("unknown shape in {wanted:?}").into());
    }
    let cameras = eval_cameras();
    let variants = eval_variants();
    for (name, shape) in &shapes {
        let dir = out.join(name);
        std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let scene = Scene { shape: shape.clone(), ground: true };
        for (view, camera) in cameras.iter().enumerate() {
            let rendered = render(&scene, camera);
            write(dir.join(format!("view{view}_rgb.u8")), &rendered.rgb)?;
            // The renders put pixel centres at x + 0.5; the files use centres at x.
            let k = [camera.fx, camera.fy, camera.cx - 0.5, camera.cy - 0.5];
            write(dir.join(format!("view{view}_k.f64")), &k.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>())?;
            let c2w: Vec<u8> = camera.cam_to_world.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
            write(dir.join(format!("view{view}_c2w.f64")), &c2w)?;
            for (variant, depth, _) in &variants {
                if let DepthInput::Sparse { stride, noise, seed } = *depth {
                    let sparse = sparse_depth_z(&rendered.depth_z, MAPANYTHING_SIZE, MAPANYTHING_SIZE, stride, noise, seed)?;
                    write(dir.join(format!("view{view}_depth_{variant}.f32")), &sparse.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>())?;
                }
            }
        }
    }
    let shape_list: Vec<String> = shapes.iter().map(|(name, _)| format!("\"{name}\"")).collect();
    let variant_list: Vec<String> = variants
        .iter()
        .map(|(name, depth, poses)| format!("{{\"name\": \"{name}\", \"poses\": {poses}, \"depth\": {}}}", matches!(depth, DepthInput::Sparse { .. })))
        .collect();
    let index = format!(
        "{{\"size\": {MAPANYTHING_SIZE}, \"views\": {}, \"shapes\": [{}], \"variants\": [{}]}}\n",
        cameras.len(), shape_list.join(", "), variant_list.join(", ")
    );
    write(out.join("index.json"), index.as_bytes())?;
    println!("scan-export: {} shapes x {} views into {}", shapes.len(), cameras.len(), out.display());
    Ok(())
}

fn main() {
    makepad_ai_common::backend::metal::run_model_worker("scan_export", model_main);
}

fn model_main() {
    if let Err(error) = run() {
        eprintln!("scan-export: {error}");
        std::process::exit(1);
    }
}
