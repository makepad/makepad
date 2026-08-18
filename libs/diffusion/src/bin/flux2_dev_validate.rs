//! FLUX.2-dev t2i parity validator + warm benchmark.
//!
//! Compares the native CUDA port against the ComfyUI fp8mixed oracle dumps
//! (the official 32GB-card recipe: 20 steps, 1024x1024, euler, guidance 4.0,
//! `flux2_dev_fp8mixed` + `mistral_3_small_flux2_fp8` + `flux2-vae`).
//!
//! Oracle layout (`--dumps`, written by the flux2_oracle_hook custom node):
//!   context.npy        (1, 512, 15360)  zero-left-padded conditioning
//!   step000_x.npy      (1, 128, H, W)   step-0 latent = the seed's noise
//!   stepNNN_pred.npy   (1, 128, H, W)   per-step model output
//!   vae_in.npy         (1, 128, H, W)   final latent fed to VAE.decode
//!   vae_out.npy        (1, Hpx, Wpx, 3) decoded image in [0,1]
//!   oracle.png         SaveImage output
//!   te_ids.json        token ids (from the timing hook log)
//!
//! Gates (native vs oracle): input_ids exact, TE conditioning cosine,
//! step-0 pred (teacher noise + teacher context), final latent, decoded
//! PNG u8 max_abs / f32 cosine, warm e2e vs the official warm wall.
//!
//! Usage: flux2-dev-validate --models <dir> --dumps <dir>
//!        [--steps N] [--size N] [--own-te] [--warm-runs N]

use makepad_diffusion::backend::gpu_device_available;
use makepad_diffusion::flux2_pipeline::{
    flux2_dev_pad_conditioning, flux2_dev_paths_from_root, Flux2DevPipeline,
    Flux2GenerateRequest, FLUX2_DEV_DEFAULT_GUIDANCE,
};
use std::path::{Path, PathBuf};

struct Npy {
    shape: Vec<usize>,
    descr: String,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        )
    };
    let header =
        String::from_utf8_lossy(&bytes[header_start..header_start + header_len]).to_string();
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(|| format!("{}: no descr", path.display()))?
        .to_string();
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("{}: no shape", path.display()))?;
    let shape: Vec<usize> = shape_text
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect();
    Ok(Npy {
        shape,
        descr,
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_f32(&self) -> Result<Vec<f32>, String> {
        match self.descr.as_str() {
            "<f4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()),
            other => Err(format!("npy descr {other} not f32")),
        }
    }
}

fn compare(got: &[f32], exp: &[f32]) -> Result<(f64, f64), String> {
    if got.len() != exp.len() {
        return Err(format!("length {} vs oracle {}", got.len(), exp.len()));
    }
    let mut max_abs = 0.0f64;
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (a, b) in got.iter().zip(exp.iter()) {
        let d = (*a as f64 - *b as f64).abs();
        if d > max_abs {
            max_abs = d;
        }
        dot += *a as f64 * *b as f64;
        na += *a as f64 * *a as f64;
        nb += *b as f64 * *b as f64;
    }
    let cosine = if na > 0.0 && nb > 0.0 {
        dot / (na.sqrt() * nb.sqrt())
    } else {
        0.0
    };
    Ok((max_abs, cosine))
}

/// Comfy `(1, C, H, W)` plane dump -> token-major `[H*W, C]`.
fn planar_to_tokens(npy: &Npy) -> Result<Vec<f32>, String> {
    let values = npy.as_f32()?;
    if npy.shape.len() != 4 || npy.shape[0] != 1 {
        return Err(format!("expected (1,C,H,W), got {:?}", npy.shape));
    }
    let (c, h, w) = (npy.shape[1], npy.shape[2], npy.shape[3]);
    let plane = h * w;
    let mut tokens = vec![0.0f32; plane * c];
    for channel in 0..c {
        for pixel in 0..plane {
            tokens[pixel * c + channel] = values[channel * plane + pixel];
        }
    }
    Ok(tokens)
}

const PROMPT: &str = "A weathered lighthouse keeper's cottage on a basalt cliff at golden hour: \
whitewashed stone walls streaked with salt, a red tin roof, warm lamplight glowing in one round \
window, gulls circling the rusted lantern tower, waves bursting into spray on black rocks below, \
long shadows across wind-bent grass, thin volumetric sea mist, shot on 35mm film with fine grain.";

fn main() {
    if let Err(err) = run() {
        eprintln!("flux2-dev-validate: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    if !gpu_device_available() {
        return Err("CUDA is required; refusing CPU/Metal fallback".into());
    }
    let mut models = PathBuf::from(r"C:\ai\flux2dev");
    let mut dumps = PathBuf::from(r"C:\ai\flux2dev\oracle");
    let mut steps = 20usize;
    let mut size = 1024u32;
    let mut own_te = false;
    let mut warm_runs = 2usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--models" => models = args.next().map(PathBuf::from).ok_or("--models value")?,
            "--dumps" => dumps = args.next().map(PathBuf::from).ok_or("--dumps value")?,
            "--steps" => {
                steps = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--steps value")?
            }
            "--size" => {
                size = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--size value")?
            }
            "--own-te" => own_te = true,
            "--warm-runs" => {
                warm_runs = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--warm-runs value")?
            }
            other => return Err(format!("unknown arg {other}")),
        }
    }

    let paths = flux2_dev_paths_from_root(&models).map_err(|err| err.to_string())?;
    let mut pipe = Flux2DevPipeline::load(paths).map_err(|err| err.to_string())?;

    let mut failed = 0usize;
    let mut rows: Vec<String> = Vec::new();

    // --- TE gate: native tokenize + encode vs oracle context ---------------
    let oracle_context = load_npy(&dumps.join("context.npy"))?;
    let oracle_ctx_values = oracle_context.as_f32()?;
    if oracle_context.shape.len() != 3 || oracle_context.shape[1] != 512 {
        return Err(format!("context.npy shape {:?}", oracle_context.shape));
    }

    let ids_path = dumps.join("te_ids.json");
    let oracle_ids: Option<Vec<u32>> = if ids_path.is_file() {
        let text = std::fs::read_to_string(&ids_path).map_err(|err| err.to_string())?;
        Some(
            text.split(|c: char| !c.is_ascii_digit())
                .filter(|part| !part.is_empty())
                .filter_map(|part| part.parse::<u32>().ok())
                .collect(),
        )
    } else {
        None
    };

    let native_ids = pipe.tokenizer.encode_t2i_unpadded(
        makepad_diffusion::flux2::FLUX2_SYSTEM_MESSAGE,
        PROMPT,
    );
    if let Some(oracle_ids) = &oracle_ids {
        let exact = *oracle_ids == native_ids;
        rows.push(format!(
            "input_ids        exact={exact} native_len={} oracle_len={}",
            native_ids.len(),
            oracle_ids.len()
        ));
        if !exact {
            failed += 1;
        }
    } else {
        rows.push("input_ids        SKIP (no te_ids.json)".into());
    }

    let te_started = std::time::Instant::now();
    let (native_cond, _) = makepad_diffusion::flux2_dev_text::flux2_dev_text_encode(
        &pipe.text_encoder,
        &pipe.text_prepared,
        &native_ids,
        None,
        false,
    )
    .map_err(|err| err.to_string())?;
    let te_ms = te_started.elapsed().as_secs_f64() * 1000.0;
    let native_padded =
        flux2_dev_pad_conditioning(&native_cond, native_ids.len(), 15360, 512)
            .map_err(|err| err.to_string())?;
    let (max_abs, cosine) = compare(&native_padded, &oracle_ctx_values)?;
    rows.push(format!(
        "te_conditioning  max_abs={max_abs:.5} cosine={cosine:.6} te_ms={te_ms:.0}"
    ));
    if cosine < 0.995 {
        failed += 1;
    }

    // --- DiT step-0 gate: teacher noise + teacher context -------------------
    let noise = planar_to_tokens(&load_npy(&dumps.join("step000_x.npy"))?)?;
    let oracle_pred0 = planar_to_tokens(&load_npy(&dumps.join("step000_pred.npy"))?)?;
    let teacher_embeds = if own_te {
        None
    } else {
        Some(oracle_ctx_values.clone())
    };
    let request = Flux2GenerateRequest {
        prompt: PROMPT.into(),
        width: size,
        height: size,
        steps,
        guidance: FLUX2_DEV_DEFAULT_GUIDANCE,
        seed: 7,
        noise: Some(noise.clone()),
        teacher_embeds: teacher_embeds.clone(),
    };
    let result = pipe.generate(&request).map_err(|err| err.to_string())?;
    let (pred0_max, pred0_cos) = compare(
        &result.step_predictions[0].1,
        &oracle_pred0,
    )?;
    rows.push(format!(
        "step0_pred       max_abs={pred0_max:.5} cosine={pred0_cos:.6}"
    ));
    if pred0_cos < 0.995 {
        failed += 1;
    }

    // --- final latent gate ---------------------------------------------------
    let vae_in_path = dumps.join("vae_in.npy");
    if vae_in_path.is_file() {
        let oracle_final = planar_to_tokens(&load_npy(&vae_in_path)?)?;
        let native_final = result.packed_latents.to_tokens();
        let (final_max, final_cos) = compare(&native_final, &oracle_final)?;
        rows.push(format!(
            "final_latents    max_abs={final_max:.5} cosine={final_cos:.6}"
        ));
        if final_cos < 0.99 {
            failed += 1;
        }
    } else {
        rows.push("final_latents    SKIP (no vae_in.npy)".into());
    }

    // --- decoded image gate --------------------------------------------------
    let vae_out_path = dumps.join("vae_out.npy");
    if vae_out_path.is_file() {
        let oracle_img = load_npy(&vae_out_path)?;
        let oracle_values = oracle_img.as_f32()?; // (1, H, W, 3) in [0,1]
        let (h, w) = (oracle_img.shape[1], oracle_img.shape[2]);
        let plane = h * w;
        // native image.data is planar [-1,1]; oracle interleaved [0,1].
        let mut native_interleaved = vec![0.0f32; plane * 3];
        for i in 0..plane {
            for ch in 0..3 {
                native_interleaved[i * 3 + ch] =
                    (result.image.data[ch * plane + i] + 1.0) * 0.5;
            }
        }
        let (img_max, img_cos) = compare(&native_interleaved, &oracle_values)?;
        let mut u8_max = 0u32;
        for (a, b) in native_interleaved.iter().zip(oracle_values.iter()) {
            let ua = (a.clamp(0.0, 1.0) * 255.0).round() as i32;
            let ub = (b.clamp(0.0, 1.0) * 255.0).round() as i32;
            u8_max = u8_max.max((ua - ub).unsigned_abs());
        }
        rows.push(format!(
            "decoded_image    max_abs={img_max:.5} cosine={img_cos:.6} u8_max={u8_max}"
        ));
        if img_cos < 0.995 {
            failed += 1;
        }
    } else {
        rows.push("decoded_image    SKIP (no vae_out.npy)".into());
    }

    // save the native PNG next to the dumps for eyeballing
    let out_png = dumps.join("native_generate.png");
    std::fs::write(&out_png, &result.png).map_err(|err| err.to_string())?;

    // --- warm timing ---------------------------------------------------------
    // Same request repeated: conditioning cached, weights resident. Official
    // warm bar (2026-08-18, ComfyUI 0.22.0 fp8mixed on this 5090):
    // wall 20117ms = denoise 19160 + vae 295 + overhead.
    let mut warm_walls = Vec::new();
    for _ in 0..warm_runs {
        let warm = pipe.generate(&request).map_err(|err| err.to_string())?;
        warm_walls.push((warm.denoise_ms, warm.decode_ms, warm.total_ms));
    }
    for (denoise, decode, total) in &warm_walls {
        rows.push(format!(
            "warm             denoise_ms={denoise:.0} decode_ms={decode:.0} total_ms={total:.0} \
             (official: denoise 19160, vae 295, wall 20117)"
        ));
    }
    if let Some((_, _, best)) = warm_walls
        .iter()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
    {
        let pass = *best < 20117.0;
        rows.push(format!("warm_e2e_gate    best={best:.0}ms < 20117ms pass={pass}"));
        if !pass {
            failed += 1;
        }
    }

    println!("=== flux2-dev-validate ({}x{size}, {steps} steps, own_te={own_te}) ===", size);
    for row in &rows {
        println!("{row}");
    }
    println!("failed_gates={failed}");
    if failed > 0 {
        std::process::exit(2);
    }
    Ok(())
}
