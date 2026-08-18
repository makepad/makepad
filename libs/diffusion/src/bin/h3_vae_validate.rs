//! MiniMax H3 video-VAE decoder validation against the diffusers oracle dump:
//! feeds dump_m0/vae_in.npy (denormalized latents the reference decoder saw)
//! through our decoder and compares against vae_out_raw.npy (the reference's
//! raw ImageNet-normalized output), reporting max/mean abs, cosine and PSNR.
//!
//! Usage: h3-vae-validate [--dump <dir>] [--models <MiniMax-H3 dir>]

use makepad_diffusion::h3::H3ShardedWeights;
use makepad_diffusion::h3_vae::{h3_vae_decode, H3VaeDecoderPrepared, H3_PIXEL_MEAN, H3_PIXEL_STD};
use std::collections::HashMap;
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
            "<f2" => Ok(self
                .data
                .chunks_exact(2)
                .map(|c| {
                    makepad_diffusion::h3::f16_word_to_f32(u16::from_le_bytes([c[0], c[1]]))
                })
                .collect()),
            other => Err(format!("npy descr {other} not f32-convertible")),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts: HashMap<String, String> = HashMap::new();
    let mut key: Option<String> = None;
    for arg in &args[1..] {
        if let Some(name) = arg.strip_prefix("--") {
            key = Some(name.to_string());
            opts.entry(name.to_string()).or_default();
        } else if let Some(name) = key.take() {
            opts.insert(name, arg.clone());
        }
    }
    let dump = PathBuf::from(
        opts.get("dump")
            .map(String::as_str)
            .unwrap_or(r"C:\ai\out\dump_m0"),
    );
    let models = PathBuf::from(
        opts.get("models")
            .map(String::as_str)
            .unwrap_or(r"C:\ai\models\MiniMax-H3"),
    );
    if let Err(err) = run(&dump, &models) {
        eprintln!("h3-vae-validate FAILED: {err}");
        std::process::exit(1);
    }
    println!("H3-VAE-VALIDATE-DONE");
}

fn run(dump: &Path, models: &Path) -> Result<(), String> {
    let input = load_npy(&dump.join("vae_in.npy"))?;
    if input.shape.len() != 4 {
        return Err(format!("vae_in shape {:?}, expected (24, T, lh, lw)", input.shape));
    }
    let (channels, frames, lh, lw) =
        (input.shape[0], input.shape[1], input.shape[2], input.shape[3]);
    println!("vae_in: ({channels}, {frames}, {lh}, {lw}) {}", input.descr);
    let latents = input.as_f32()?;

    let vae_dir = models.join("vae");
    println!("loading VAE shards from {}", vae_dir.display());
    let weights = H3ShardedWeights::load(&vae_dir).map_err(|err| err.to_string())?;
    let prepared = H3VaeDecoderPrepared::prepare(&weights).map_err(|err| err.to_string())?;

    let start = std::time::Instant::now();
    let run = h3_vae_decode(&weights, &prepared, &latents, frames, lh, lw)
        .map_err(|err| err.to_string())?;
    println!(
        "decode: {:.2}s -> ({}, {}, {}, {})",
        start.elapsed().as_secs_f64(),
        run.raw.c,
        run.raw.f,
        run.raw.h,
        run.raw.w
    );

    let reference = load_npy(&dump.join("vae_out_raw.npy"))?;
    println!("vae_out_raw: {:?} {}", reference.shape, reference.descr);
    let reference = reference.as_f32()?;
    if reference.len() != run.raw.data.len() {
        return Err(format!(
            "length mismatch: ours {} ref {}",
            run.raw.data.len(),
            reference.len()
        ));
    }

    // Raw-space comparison.
    let mut max_abs = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (a, b) in run.raw.data.iter().zip(reference.iter()) {
        let a = *a as f64;
        let b = *b as f64;
        let diff = (a - b).abs();
        max_abs = max_abs.max(diff);
        sum_abs += diff;
        dot += a * b;
        norm_a += a * a;
        norm_b += b * b;
    }
    let n = reference.len() as f64;
    println!(
        "[vae.raw] max_abs={max_abs:.6e} mean_abs={:.6e} cosine={:.8}",
        sum_abs / n,
        dot / (norm_a.sqrt() * norm_b.sqrt()).max(1e-30),
    );

    // PSNR in reverted [0,1] pixel space (per-channel ImageNet revert).
    let plane = run.raw.f * run.raw.h * run.raw.w;
    let mut mse = 0.0f64;
    for c in 0..3 {
        for i in 0..plane {
            let ours = (run.raw.data[c * plane + i] * H3_PIXEL_STD[c] + H3_PIXEL_MEAN[c])
                .clamp(0.0, 1.0);
            let refv = (reference[c * plane + i] * H3_PIXEL_STD[c] + H3_PIXEL_MEAN[c])
                .clamp(0.0, 1.0);
            let diff = (ours - refv) as f64;
            mse += diff * diff;
        }
    }
    mse /= n;
    let psnr = if mse > 0.0 { 10.0 * (1.0 / mse).log10() } else { f64::INFINITY };
    println!("[vae.pixels] mse={mse:.3e} psnr={psnr:.2}dB");

    Ok(())
}
