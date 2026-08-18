//! Tiny NativeHunyuanExec job: 64², 2 views. Not a service claim.

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    if let Err(e) = run() {
        eprintln!("PBR_EXEC_CANARY_FAIL {e}");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("PBR_EXEC_CANARY_FAIL CUDA host required");
    std::process::exit(1);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run() -> Result<(), String> {
    use makepad_pbr_paint::native_exec::NativeHunyuanExec;
    use makepad_pbr_paint::pipeline::{PaintCondition, PaintModelExec, ViewConditioning};
    use std::time::Instant;

    let size = std::env::var("MAKEPAD_PBR_EXEC_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64u32);
    if size == 0 || size % 8 != 0 {
        return Err(format!("MAKEPAD_PBR_EXEC_SIZE={size} must be a positive multiple of 8"));
    }
    let n_views = std::env::var("MAKEPAD_PBR_EXEC_VIEWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2usize)
        .clamp(1, 6);
    let px = (size * size * 3) as usize;
    let mut normal = vec![128u8; px];
    let mut position = vec![200u8; px];
    // Keep a non-white foreground so voxel cells are valid.
    for i in 0..px / 3 {
        position[i * 3] = 80;
        position[i * 3 + 1] = 90;
        position[i * 3 + 2] = 100;
        if i % 17 == 0 {
            normal[i * 3] = 180;
        }
    }
    let azims = [0.0f32, 90.0, 180.0, 270.0, 45.0, 135.0];
    let views: Vec<ViewConditioning> = (0..n_views)
        .map(|i| ViewConditioning {
            azim: azims[i],
            elev: 0.0,
            weight: 1.0,
            size,
            normal_map_rgb: normal.clone(),
            position_map_rgb: position.clone(),
        })
        .collect();
    let reference = vec![64u8; px];
    let cond = PaintCondition {
        reference_rgb: &reference,
        ref_width: size,
        ref_height: size,
        views: &views,
        seed: 0,
        resolution: size,
    };
    let t0 = Instant::now();
    let mut exec = NativeHunyuanExec::discover().map_err(|e| e.to_string())?;
    exec.warm().map_err(|e| e.to_string())?;
    println!("PBR_EXEC_WARM_S {:.3}", t0.elapsed().as_secs_f64());
    let t1 = Instant::now();
    let mut last = 0u32;
    let out = exec
        .run_multiview(&cond, &mut |step, total| {
            last = step;
            println!("PBR_EXEC_STEP {step}/{total}");
            true
        })
        .map_err(|e| e.to_string())?;
    println!(
        "PBR_EXEC_JOB_S {:.3} size={} views={} last_step={}",
        t1.elapsed().as_secs_f64(),
        out.size,
        out.albedo.len(),
        last
    );
    if out.albedo.len() != n_views || out.mr.len() != n_views {
        return Err(format!("views {} / {} want {n_views}", out.albedo.len(), out.mr.len()));
    }
    let expect = (out.size as usize) * (out.size as usize) * 3;
    let mut fnv = 0xcbf29ce484222325u64;
    let mut n = 0usize;
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for img in out.albedo.iter().chain(out.mr.iter()) {
        if img.len() != expect || img.iter().any(|v| !v.is_finite()) {
            return Err("non-finite or wrong-sized view".into());
        }
        for &v in img {
            min = min.min(v);
            max = max.max(v);
            n += 1;
            let bits = v.to_bits();
            fnv ^= bits as u64;
            fnv = fnv.wrapping_mul(0x100000001b3);
            fnv ^= (bits as u64) >> 32;
            fnv = fnv.wrapping_mul(0x100000001b3);
        }
    }
    println!("PBR_EXEC_OUT_FNV {fnv:016x} n={n} min={min:.6} max={max:.6}");
    println!("PBR_EXEC_CANARY_OK");
    if let Ok(dir) = std::env::var("MAKEPAD_PBR_DUMP_DIR") {
        dump_exec_pngs(&dir, out.size, &out.albedo, &out.mr)?;
        println!("PBR_EXEC_DUMP {dir}");
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn dump_exec_pngs(
    dir: &str,
    size: u32,
    albedo: &[Vec<f32>],
    mr: &[Vec<f32>],
) -> Result<(), String> {
    use makepad_pbr_paint::png::{encode_png, PngColor};
    std::fs::create_dir_all(dir).map_err(|e| format!("dump dir {dir}: {e}"))?;
    let to_u8 = |img: &[f32]| -> Vec<u8> {
        img.iter()
            .map(|&v| {
                let x = if v.is_finite() { v } else { 0.0 };
                (x.clamp(0.0, 1.0) * 255.0).round() as u8
            })
            .collect()
    };
    for (i, img) in albedo.iter().enumerate() {
        let path = format!("{dir}/view{i}_albedo.png");
        let png = encode_png(size, size, PngColor::Rgb, &to_u8(img));
        std::fs::write(&path, png).map_err(|e| format!("write {path}: {e}"))?;
    }
    for (i, img) in mr.iter().enumerate() {
        let path = format!("{dir}/view{i}_mr.png");
        let png = encode_png(size, size, PngColor::Rgb, &to_u8(img));
        std::fs::write(&path, png).map_err(|e| format!("write {path}: {e}"))?;
    }
    Ok(())
}
