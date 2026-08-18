//! SA3 Small SFX text-to-audio CLI on the CPU port: prompt -> stereo wav.
//!
//! Usage:
//!   sa3-generate --prompt "sword clash" [--seconds 4] [--seed 1001]
//!                [--steps 8] [--out out.wav] [--weights <dir>]
//!
//! Noise comes from the built-in seeded RNG (deterministic per seed, not
//! torch-compatible: same seed gives a valid draw from the same
//! distribution, not the reference's exact sample).

use makepad_diffusion::sa3_pipeline::{Sa3Pipeline, Sa3SeededNoise};
use makepad_diffusion::sa3_tokenizer::Sa3Tokenizer;
use makepad_diffusion::sa3_transformer::Sa3PadMode;
use std::io::Write;
use std::path::PathBuf;

fn write_wav_stereo16(path: &std::path::Path, channels: &[Vec<f32>], sample_rate: u32) -> std::io::Result<()> {
    let frames = channels[0].len();
    let data_len = (frames * channels.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(channels.len() as u16).to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * channels.len() as u32 * 2).to_le_bytes());
    out.extend_from_slice(&((channels.len() * 2) as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            let v = (channel[frame].clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    std::fs::File::create(path)?.write_all(&out)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts = std::collections::HashMap::new();
    let mut i = 1;
    while i + 1 < args.len() {
        if let Some(key) = args[i].strip_prefix("--") {
            opts.insert(key.to_string(), args[i + 1].clone());
        }
        i += 2;
    }
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let weights = PathBuf::from(opts.get("weights").cloned().unwrap_or_else(|| {
        repo.join("local/sa3_ref/weights/stable-audio-3-small-sfx")
            .to_string_lossy()
            .into_owned()
    }));
    let prompt = opts.get("prompt").cloned().unwrap_or_else(|| "sword clash".into());
    let seconds: f64 = opts.get("seconds").and_then(|v| v.parse().ok()).unwrap_or(4.0);
    let seed: u64 = opts.get("seed").and_then(|v| v.parse().ok()).unwrap_or(0);
    let steps: usize = opts.get("steps").and_then(|v| v.parse().ok()).unwrap_or(8);
    let out = PathBuf::from(opts.get("out").cloned().unwrap_or_else(|| "sa3_out.wav".into()));

    let t0 = std::time::Instant::now();
    let tokenizer = Sa3Tokenizer::load(weights.join("t5gemma-b-b-ul2/tokenizer.model"))
        .expect("tokenizer load");
    let pipeline = Sa3Pipeline::load(
        weights.join("model.safetensors"),
        weights.join("t5gemma-b-b-ul2/model.safetensors"),
        None,
    )
    .expect("pipeline load");
    println!(
        "LOAD {:.1}s device={}",
        t0.elapsed().as_secs_f32(),
        pipeline.device_active()
    );

    let (ids, mask) = tokenizer.tokenize_padded(&prompt);
    // --bench N: N warm repeats after one warm-up run, per-run times printed.
    let bench: usize = opts.get("bench").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut audio = Vec::new();
    let mut gen_s = 0f32;
    let runs = if bench > 0 { bench + 1 } else { 1 };
    for run in 0..runs {
        let mut noise = Sa3SeededNoise::new(seed + run as u64);
        let t0 = std::time::Instant::now();
        audio = pipeline
            .generate(&ids, &mask, seconds, steps, Sa3PadMode::VZero, &mut noise, None, None)
            .expect("generate");
        gen_s = t0.elapsed().as_secs_f32();
        if bench > 0 {
            let tag = if run == 0 { "warmup" } else { "run" };
            println!("BENCH {tag} {gen_s:.3}s");
        }
    }

    // Waveform stats.
    let mono_rms: f64 = {
        let sum: f64 = audio[0]
            .iter()
            .zip(&audio[1])
            .map(|(a, b)| {
                let m = 0.5 * (a + b) as f64;
                m * m
            })
            .sum();
        (sum / audio[0].len() as f64).sqrt()
    };
    let peak = audio
        .iter()
        .flat_map(|c| c.iter())
        .fold(0f32, |m, v| m.max(v.abs()));
    println!(
        "GEN {gen_s:.1}s prompt={prompt:?} seconds={seconds} seed={seed} samples={} rms_db={:.1} peak={peak:.3}",
        audio[0].len(),
        20.0 * mono_rms.max(1e-12).log10()
    );
    write_wav_stereo16(&out, &audio, 44_100).expect("wav write");
    println!("WROTE {}", out.display());
}
