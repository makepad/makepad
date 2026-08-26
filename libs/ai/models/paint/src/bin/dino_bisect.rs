//! Bisect one DINOv2-giant block against official sub-op dumps.
//! MAKEPAD_DINO_BISECT_DIR holds `in.f32` (layer input, [257,1536]) and
//! `official_{n1,attn,h,n2,mlp,out}.f32`; PBR_DINO_LAYER picks the block.

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    if let Err(e) = run() {
        eprintln!("PBR_DINO_BISECT_FAIL {e}");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("PBR_DINO_BISECT_FAIL CUDA host required");
    std::process::exit(1);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run() -> Result<(), String> {
    use makepad_ai_paint::dino_vit::{default_snapshot_path, DinoVit, HIDDEN, TOKENS};
    use std::path::PathBuf;

    let weights = std::env::var("MAKEPAD_DINO_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_snapshot_path());
    let dir = PathBuf::from(
        std::env::var("MAKEPAD_DINO_BISECT_DIR").map_err(|_| "MAKEPAD_DINO_BISECT_DIR unset")?,
    );
    let layer: usize = std::env::var("PBR_DINO_LAYER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24);
    let vit = DinoVit::load(&weights)?;
    let input = load_f32(&dir.join("in.f32"), TOKENS * HIDDEN)?;
    let taps = vit.block_taps_at(&input, layer)?;
    let names = ["n1", "attn", "h", "n2", "mlp", "out"];
    for (name, ours) in names.iter().zip(taps.iter()) {
        let official = load_f32(&dir.join(format!("official_{name}.f32")), TOKENS * HIDDEN)?;
        let mut max_abs = 0.0f32;
        let mut sum_sq = 0.0f64;
        let mut ref_sq = 0.0f64;
        let mut worst_tok = 0usize;
        let mut worst = 0.0f64;
        for t in 0..TOKENS {
            let mut tok_sq = 0.0f64;
            for c in 0..HIDDEN {
                let i = t * HIDDEN + c;
                let d = ours[i] - official[i];
                max_abs = max_abs.max(d.abs());
                tok_sq += (d as f64) * (d as f64);
                ref_sq += (official[i] as f64) * (official[i] as f64);
            }
            sum_sq += tok_sq;
            let tok_rms = (tok_sq / HIDDEN as f64).sqrt();
            if tok_rms > worst {
                worst = tok_rms;
                worst_tok = t;
            }
        }
        let n = (TOKENS * HIDDEN) as f64;
        println!(
            "PBR_DINO_BISECT L{layer} {name}: rms_diff={:.5} max_abs={:.4} ref_rms={:.4} worst_tok={worst_tok} worst={:.4}",
            (sum_sq / n).sqrt(),
            max_abs,
            (ref_sq / n).sqrt(),
            worst
        );
    }
    println!("PBR_DINO_BISECT_OK");
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn load_f32(path: &std::path::Path, expect: usize) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() != expect * 4 {
        return Err(format!("{}: {} bytes, expected {}", path.display(), bytes.len(), expect * 4));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}
