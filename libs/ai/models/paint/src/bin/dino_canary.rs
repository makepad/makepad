//! Native DINOv2-giant vs official AutoModel last_hidden_state dumps.

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    if let Err(e) = run() {
        eprintln!("PBR_DINO_CANARY_FAIL {e}");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("PBR_DINO_CANARY_FAIL CUDA host required");
    std::process::exit(1);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run() -> Result<(), String> {
    use makepad_ai_paint::dino_vit::{
        default_snapshot_path, preprocess_official, ramp_rgb8, DinoVit, HIDDEN, PROC_CROP, TOKENS,
    };
    use std::path::PathBuf;
    use std::time::Instant;

    std::env::set_var("MAKEPAD_PBR_TAP_PARITY", "1");
    let weights = std::env::var("MAKEPAD_DINO_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_snapshot_path());
    let dumps = std::env::var("MAKEPAD_DINO_ORACLE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\Users\playe\makepad\local\pbrpaint\dino"));
    let case = std::env::var("PBR_DINO_CASE").unwrap_or_else(|_| "black".into());
    let isolated_only = std::env::var("PBR_DINO_ISOLATED").ok().as_deref() == Some("1");

    let t0 = Instant::now();
    let vit = DinoVit::load(&weights)?;
    println!("PBR_DINO_LOAD_S {:.3}", t0.elapsed().as_secs_f64());

    let official_pixels = load_f32(&dumps.join(format!("{case}_pixels.f32")), 3 * PROC_CROP * PROC_CROP)?;
    let official_emb = load_f32(&dumps.join(format!("{case}_embeddings.f32")), TOKENS * HIDDEN)?;
    let official_b0 = load_f32(&dumps.join(format!("{case}_block0.f32")), TOKENS * HIDDEN)?;
    let official_last = load_f32(&dumps.join(format!("{case}_last_hidden.f32")), TOKENS * HIDDEN)?;

    let rgb = if case == "ramp" {
        ramp_rgb8(512)
    } else {
        vec![0u8; 512 * 512 * 3]
    };
    let native_pixels = preprocess_official(&rgb, 512, 512).map_err(|e| e.to_string())?;
    // One ImageNet-normalized LSB is ~1/255/0.229 ≈ 1.7e-2 (PIL vs our cubic).
    report("pixels", &native_pixels, &official_pixels, 2e-2)?;

    let t0 = Instant::now();
    let emb = vit.embeddings(&official_pixels)?;
    println!("PBR_DINO_EMB_S {:.4}", t0.elapsed().as_secs_f64());
    report("embeddings", &emb, &official_emb, 1e-3)?;

    let t0 = Instant::now();
    let b0_iso = vit.block_at(&official_emb, 0)?;
    println!("PBR_DINO_BLOCK0_ISO_S {:.4}", t0.elapsed().as_secs_f64());
    report("block0_isolated", &b0_iso, &official_b0, 1e-3)?;

    if isolated_only {
        println!("PBR_DINO_CANARY_OK isolated {case}");
        return Ok(());
    }

    let t0 = Instant::now();
    let b0 = vit.block_at(&emb, 0)?;
    println!("PBR_DINO_BLOCK0_S {:.4}", t0.elapsed().as_secs_f64());
    report("block0", &b0, &official_b0, 1e-3)?;

    let t0 = Instant::now();
    let last = vit.forward(&official_pixels)?;
    println!("PBR_DINO_FWD_S {:.4}", t0.elapsed().as_secs_f64());
    // Isolated first-block is the hard bar (1e-3). Full last_hidden_state
    // vs official CUDA SDPA is reported; 40-layer SDPA/kernel drift is larger.
    report("last_hidden", &last, &official_last, 5e-3)?;

    let unet = std::env::var("MAKEPAD_HUNYUAN_UNET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(
                r"C:\ai\Hunyuan3D-2.1\weights\hunyuan3d-paintpbr-v2-1\unet\diffusion_pytorch_model.bin",
            )
        });
    if unet.is_file() {
        let proj = makepad_ai_paint::dino_proj::DinoProj::load_from_unet_bin(&unet)?;
        let got = proj.forward(&last, TOKENS)?;
        let exp = proj.forward(&official_last, TOKENS)?;
        report("proj_from_hidden", &got, &exp, 5e-3)?;
        let dumped = dumps.join(format!("{case}_proj.f32"));
        if dumped.is_file() {
            let official_proj = load_f32(&dumped, TOKENS * 4 * 1024)?;
            report("proj_vs_oracle", &got, &official_proj, 5e-3)?;
        }
    }

    println!("PBR_DINO_CANARY_OK {case}");
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn load_f32(path: &std::path::Path, n: usize) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() != n * 4 {
        return Err(format!(
            "{} len {} vs {} f32",
            path.display(),
            bytes.len(),
            n
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn report(name: &str, got: &[f32], exp: &[f32], limit: f32) -> Result<(), String> {
    use makepad_ai_paint::dino_vit::{HIDDEN, TOKENS};
    if got.len() != exp.len() {
        return Err(format!("{name} len {} vs {}", got.len(), exp.len()));
    }
    let mut max_abs = 0.0f32;
    let mut sum = 0.0f64;
    let mut argmax = 0usize;
    for (i, (a, e)) in got.iter().zip(exp.iter()).enumerate() {
        let d = (a - e).abs();
        if d > max_abs {
            max_abs = d;
            argmax = i;
        }
        sum += d as f64;
    }
    let mean = sum / got.len().max(1) as f64;
    let token = if name.contains("proj") {
        argmax / (4 * 1024)
    } else if got.len() == TOKENS * HIDDEN {
        argmax / HIDDEN
    } else {
        argmax
    };
    println!(
        "PBR_DINO_{} max_abs={max_abs:.9e} mean_abs={mean:.9e} n={} argmax={argmax} token={token} head_got={:?} head_exp={:?}",
        name.to_ascii_uppercase(),
        got.len(),
        &got[..8.min(got.len())],
        &exp[..8.min(exp.len())]
    );
    if max_abs > limit {
        return Err(format!("{name} max_abs {max_abs} > {limit}"));
    }
    Ok(())
}
