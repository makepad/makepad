//! IndexTTS-2.5 mel front-end + BigVGAN vocoder validation against the
//! reference oracle dumps (local/indextts_ref/dump_oracle.py).
//!
//! Stages:
//!   mel     — dumps/audio_22k.npy (1,66702) -> dumps/ref_mel.npy (1,80,260)
//!             gate: cosine >= 0.999, max-abs <= 2e-3 (log domain)
//!   bigvgan — dumps/vc_target.npy (1,80,316) -> dumps/bigvgan_wav.npy
//!             (1,80896); gate: cosine >= 0.9999, max-abs <= 2e-3
//!
//! Usage:
//!   indextts-bigvgan-validate [--dumps <dir>] [--weights <dir>]
//!                             [--stage all|mel|bigvgan]
//!
//! Exits nonzero when any gate fails. Run with --release: the vocoder is 6
//! transposed-conv stages at up to 1536 channels over 80896 samples.

use makepad_diffusion::indextts::{reference_checkpoints_dir, reference_dumps_dir};
use makepad_diffusion::indextts_bigvgan::{IndexTtsBigVgan, BIGVGAN_HOP, BIGVGAN_MELS};
use makepad_diffusion::indextts_mel::{mel_spectrogram_22k, MEL_BANDS};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// npy loader (crate convention: each validate bin carries its own copy; see
// src/bin/moss_validate.rs).
// ---------------------------------------------------------------------------

struct Npy {
    shape: Vec<usize>,
    descr: String,
    fortran: bool,
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
    let fortran = header.contains("'fortran_order': True");
    Ok(Npy {
        shape,
        descr,
        fortran,
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_f32(&self) -> Result<Vec<f32>, String> {
        if self.fortran && self.shape.len() >= 2 {
            return Err("fortran-order npy not supported here".to_string());
        }
        match self.descr.as_str() {
            "<f4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()),
            "<f8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| {
                    f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32
                })
                .collect()),
            other => Err(format!("npy descr {other} not f32-convertible")),
        }
    }
}

// ---------------------------------------------------------------------------
// Comparison + gates
// ---------------------------------------------------------------------------

fn compare(tag: &str, ours: &[f32], reference: &[f32]) -> (f64, f32) {
    assert_eq!(ours.len(), reference.len(), "{tag}: length mismatch");
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut max_abs = 0f32;
    let mut ref_absmax = 0f32;
    for (&a, &b) in ours.iter().zip(reference) {
        dot += a as f64 * b as f64;
        na += a as f64 * a as f64;
        nb += b as f64 * b as f64;
        max_abs = max_abs.max((a - b).abs());
        ref_absmax = ref_absmax.max(b.abs());
    }
    let cos = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
    println!("  {tag}: cos {cos:.7} max_abs {max_abs:.3e} (ref absmax {ref_absmax:.3})");
    (cos, max_abs)
}

fn gate(tag: &str, cos: f64, max_abs: f32, min_cos: f64, max_max_abs: f32, failed: &mut bool) {
    let pass = cos >= min_cos && max_abs <= max_max_abs;
    println!(
        "  {tag}: {} (gate cos >= {min_cos}, max_abs <= {max_max_abs:e})",
        if pass { "PASS" } else { "FAIL" }
    );
    if !pass {
        *failed = true;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let dumps = arg("--dumps")
        .map(PathBuf::from)
        .unwrap_or_else(reference_dumps_dir);
    let weights = arg("--weights")
        .map(PathBuf::from)
        .unwrap_or_else(reference_checkpoints_dir);
    let stage = arg("--stage").unwrap_or_else(|| "all".to_string());
    if !matches!(stage.as_str(), "all" | "mel" | "bigvgan") {
        eprintln!("unknown --stage {stage} (expected all|mel|bigvgan)");
        std::process::exit(2);
    }
    let run = |name: &str| stage == "all" || stage == name;
    let load = |name: &str| -> Npy {
        load_npy(&dumps.join(format!("{name}.npy"))).unwrap_or_else(|e| panic!("{e}"))
    };

    let mut failed = false;

    if run("mel") {
        let audio = load("audio_22k");
        let reference = load("ref_mel");
        assert_eq!(reference.shape[..2], [1, MEL_BANDS], "ref_mel shape");
        let samples = audio.as_f32().unwrap();
        let ref_mel = reference.as_f32().unwrap();
        let t0 = std::time::Instant::now();
        let (mel, frames) = mel_spectrogram_22k(&samples);
        println!(
            "mel: {} samples -> (80, {frames}) in {:.3}s",
            samples.len(),
            t0.elapsed().as_secs_f32()
        );
        assert_eq!(
            frames, reference.shape[2],
            "mel frame count vs ref_mel.npy"
        );
        let (cos, max_abs) = compare("mel", &mel, &ref_mel);
        gate("mel", cos, max_abs, 0.999, 2e-3, &mut failed);
    }

    if run("bigvgan") {
        let mel = load("vc_target");
        let reference = load("bigvgan_wav");
        assert_eq!(mel.shape[..2], [1, BIGVGAN_MELS], "vc_target shape");
        let frames = mel.shape[2];
        let mel_data = mel.as_f32().unwrap();
        let ref_wav = reference.as_f32().unwrap();
        assert_eq!(ref_wav.len(), frames * BIGVGAN_HOP, "bigvgan_wav length");

        let checkpoint = weights.join("hf_cache/bigvgan/bigvgan_generator.pt");
        println!("loading bigvgan weights from {}...", checkpoint.display());
        let t0 = std::time::Instant::now();
        let model = IndexTtsBigVgan::load(&checkpoint).unwrap_or_else(|e| panic!("{e}"));
        println!("loaded in {:.1}s", t0.elapsed().as_secs_f32());

        let t0 = std::time::Instant::now();
        let wav = model.synthesize(&mel_data, frames).unwrap_or_else(|e| panic!("{e}"));
        let secs = t0.elapsed().as_secs_f64();
        println!(
            "bigvgan: (80, {frames}) -> {} samples ({:.2}s of audio) in {secs:.2}s \
             ({:.2}x realtime)",
            wav.len(),
            wav.len() as f64 / 22_050.0,
            wav.len() as f64 / 22_050.0 / secs
        );
        let (cos, max_abs) = compare("bigvgan", &wav, &ref_wav);
        gate("bigvgan", cos, max_abs, 0.9999, 2e-3, &mut failed);
    }

    if failed {
        println!("VALIDATION FAILED");
        std::process::exit(1);
    }
    println!("done");
}
