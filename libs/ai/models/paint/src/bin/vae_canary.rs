//! Native SD VAE encode vs the official Hunyuan paint VAE dump.

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    if let Err(error) = run() {
        eprintln!("PBR_VAE_CANARY_FAIL {error}");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("PBR_VAE_CANARY_FAIL CUDA host required");
    std::process::exit(1);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run() -> Result<(), String> {
    use makepad_ai_paint::sd_vae::{ramp_rgb_nchw, SdVae};
    use std::path::PathBuf;
    use std::time::Instant;

    std::env::set_var("MAKEPAD_PBR_TAP_PARITY", "1");
    let weights = std::env::var("MAKEPAD_HUNYUAN_VAE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(r"C:\ai\Hunyuan3D-2.1\weights\hunyuan3d-paintpbr-v2-1\vae\diffusion_pytorch_model.bin")
        });
    let size: usize = std::env::var("PBR_VAE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let t0 = Instant::now();
    let vae = SdVae::load(&weights)?;
    println!("PBR_VAE_LOAD_S {:.3}", t0.elapsed().as_secs_f64());
    let rgb = ramp_rgb_nchw(size, size);
    let t0 = Instant::now();
    let latent = vae.encode_mean_nchw(&rgb, size, size)?;
    println!(
        "PBR_VAE_ENCODE_S {:.3} latent={} digest={}",
        t0.elapsed().as_secs_f64(),
        latent.len(),
        makepad_ai_paint::numerical_fixtures::digest_f32(&latent)
    );
    println!("PBR_VAE_HEAD {:?}", &latent[..16.min(latent.len())]);
    let t0 = Instant::now();
    let (recon, rw, rh) = vae.decode_rgb01(&latent, size / 8, size / 8)?;
    println!(
        "PBR_VAE_DECODE_S {:.3} rgb={}x{}x3 head={:?}",
        t0.elapsed().as_secs_f64(),
        rw,
        rh,
        &recon[..16.min(recon.len())]
    );
    if let Ok(path) = std::env::var("PBR_VAE_ORACLE") {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let expect = parse_latent_array(&text)?;
        if expect.len() != latent.len() && expect.len() != 16 {
            return Err(format!(
                "oracle latent len {} vs native {}",
                expect.len(),
                latent.len()
            ));
        }
        let n = expect.len().min(latent.len());
        let mut max_abs = 0.0f32;
        for (a, e) in latent.iter().zip(expect.iter()).take(n) {
            max_abs = max_abs.max((a - e).abs());
        }
        println!("PBR_VAE_VS_ORACLE max_abs={max_abs:.9e} compared={n}");
        let limit = if n > 16 { 5e-3 } else { 2e-3 };
        if max_abs > limit {
            return Err(format!("VAE mean vs oracle {max_abs}"));
        }
        if let Ok(recon_ref) = parse_json_array(&text, "\"recon_head\"") {
            let mut rmax = 0.0f32;
            for (a, e) in recon.iter().zip(recon_ref.iter()) {
                rmax = rmax.max((a - e).abs());
            }
            println!(
                "PBR_VAE_RECON_VS_ORACLE max_abs={rmax:.9e} compared={}",
                recon_ref.len()
            );
        }
    }
    println!("PBR_VAE_CANARY_OK");
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn parse_json_array(json: &str, key: &str) -> Result<Vec<f32>, String> {
    let start = json.find(key).ok_or_else(|| format!("no {key}"))?;
    let rest = &json[start + key.len()..];
    let lb = rest.find('[').ok_or("no [")?;
    let rb = rest[lb..].find(']').ok_or("no ]")?;
    let body = &rest[lb + 1..lb + rb];
    body.split(',')
        .map(|s| s.trim().parse::<f32>().map_err(|e| e.to_string()))
        .collect()
}

fn parse_latent_array(json: &str) -> Result<Vec<f32>, String> {
    parse_json_array(json, "\"latent_values\"")
        .or_else(|_| parse_json_array(json, "\"latent_head\""))
}
