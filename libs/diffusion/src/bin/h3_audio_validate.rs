//! MiniMax H3 audio-VAE decoder validation against the diffusers oracle:
//! feeds audio_vae_in.npy / audio_vae_in2.npy (the exact denormalized
//! `(2, 32, n_lat)` latents decode() saw) through our BigVGAN decoder and
//! compares against audio_vae_out.npy / audio_vae_out2.npy (`(2, 1, samples)`
//! torch CPU f32 output). Gate: cosine >= 0.9999 for both pairs.
//!
//! Usage: h3-audio-validate --audio-vae <dir-with-config+safetensors>
//!                          --fixtures <dir-with-npy-pairs>

use makepad_diffusion::h3_audio_vae::H3AudioVae;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// --- minimal npy read (copied from h3_generate.rs / h3_vae_validate.rs) -----

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
                .map(|c| makepad_diffusion::h3::f16_word_to_f32(u16::from_le_bytes([c[0], c[1]])))
                .collect()),
            other => Err(format!("npy descr {other} not f32-convertible")),
        }
    }
}

// --- comparison --------------------------------------------------------------

struct PairReport {
    cosine: f64,
    max_abs_diff: f64,
    ref_max_abs: f64,
    rms_diff: f64,
    rms_ref: f64,
    decode_secs: f64,
}

fn validate_pair(
    vae: &H3AudioVae,
    fixtures: &Path,
    in_name: &str,
    out_name: &str,
) -> Result<PairReport, String> {
    let input = load_npy(&fixtures.join(in_name))?;
    if input.shape.len() != 3 || input.shape[0] != 2 {
        return Err(format!(
            "{in_name}: shape {:?}, expected (2, latent_channels, n_lat)",
            input.shape
        ));
    }
    let n_lat = input.shape[2];
    println!(
        "{in_name}: ({}, {}, {}) {}",
        input.shape[0], input.shape[1], n_lat, input.descr
    );
    let latents = input.as_f32()?;

    let start = std::time::Instant::now();
    let ours = vae
        .decode_latents(&latents, n_lat)
        .map_err(|err| err.to_string())?;
    let decode_secs = start.elapsed().as_secs_f64();
    println!(
        "decode: {decode_secs:.2}s -> (2, {}) @ {} Hz",
        ours.len() / 2,
        vae.sampling_rate()
    );

    let reference = load_npy(&fixtures.join(out_name))?;
    println!("{out_name}: {:?} {}", reference.shape, reference.descr);
    let reference = reference.as_f32()?;
    if reference.len() != ours.len() {
        return Err(format!(
            "{out_name}: length mismatch, ours {} ref {}",
            ours.len(),
            reference.len()
        ));
    }

    let mut max_abs_diff = 0.0f64;
    let mut ref_max_abs = 0.0f64;
    let mut sum_sq_diff = 0.0f64;
    let mut sum_sq_ref = 0.0f64;
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (&a, &b) in ours.iter().zip(reference.iter()) {
        let a = a as f64;
        let b = b as f64;
        let diff = a - b;
        max_abs_diff = max_abs_diff.max(diff.abs());
        ref_max_abs = ref_max_abs.max(b.abs());
        sum_sq_diff += diff * diff;
        sum_sq_ref += b * b;
        dot += a * b;
        norm_a += a * a;
        norm_b += b * b;
    }
    let n = reference.len() as f64;
    Ok(PairReport {
        cosine: dot / (norm_a.sqrt() * norm_b.sqrt()).max(1e-30),
        max_abs_diff,
        ref_max_abs,
        rms_diff: (sum_sq_diff / n).sqrt(),
        rms_ref: (sum_sq_ref / n).sqrt(),
        decode_secs,
    })
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
    let (Some(audio_vae), Some(fixtures)) = (opts.get("audio-vae"), opts.get("fixtures")) else {
        eprintln!("usage: h3-audio-validate --audio-vae <dir> --fixtures <dir>");
        std::process::exit(2);
    };
    let audio_vae = PathBuf::from(audio_vae);
    let fixtures = PathBuf::from(fixtures);

    println!("loading audio VAE from {}", audio_vae.display());
    let start = std::time::Instant::now();
    let vae = match H3AudioVae::load(&audio_vae) {
        Ok(vae) => vae,
        Err(err) => {
            eprintln!("h3-audio-validate FAILED: {err}");
            std::process::exit(1);
        }
    };
    println!("load: {:.2}s", start.elapsed().as_secs_f64());

    let pairs = [
        ("audio_vae_in.npy", "audio_vae_out.npy"),
        ("audio_vae_in2.npy", "audio_vae_out2.npy"),
    ];
    let mut all_pass = true;
    for (in_name, out_name) in pairs {
        match validate_pair(&vae, &fixtures, in_name, out_name) {
            Ok(report) => {
                let pass = report.cosine >= 0.9999;
                println!(
                    "[{in_name}] cosine={:.8} max_abs_diff={:.6e} ref_max_abs={:.6} \
                     rms_diff={:.6e} rms_ref={:.6} decode={:.2}s -> {}",
                    report.cosine,
                    report.max_abs_diff,
                    report.ref_max_abs,
                    report.rms_diff,
                    report.rms_ref,
                    report.decode_secs,
                    if pass { "PASS" } else { "FAIL (cosine < 0.9999)" },
                );
                all_pass &= pass;
            }
            Err(err) => {
                eprintln!("[{in_name}] FAILED: {err}");
                all_pass = false;
            }
        }
    }
    if all_pass {
        println!("AUDIOVAE:OK");
    } else {
        eprintln!("AUDIOVAE:FAIL");
        std::process::exit(1);
    }
}
