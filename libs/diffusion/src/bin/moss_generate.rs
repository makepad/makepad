//! MOSS-SoundEffect v2 prompt -> wav CLI.
//!
//! Usage:
//!   moss-generate --weights local/moss_ref/weights --prompt "..." \
//!     [--seconds 4] [--steps 100] [--cfg 4.0] [--seed 1001] [--out moss.wav] [--bench N]

use makepad_diffusion::moss::MOSS_SAMPLE_RATE;
use makepad_diffusion::moss_pipeline::MossPipeline;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str, default: &str| -> String {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    };
    let weights = PathBuf::from(arg("--weights", "local/moss_ref/weights"));
    let prompt = arg("--prompt", "wooden door creaking open slowly, old rusty hinges");
    let seconds: f64 = arg("--seconds", "4").parse().expect("--seconds");
    let steps: usize = arg("--steps", "100").parse().expect("--steps");
    let cfg: f32 = arg("--cfg", "4.0").parse().expect("--cfg");
    let seed: u64 = arg("--seed", "1001").parse().expect("--seed");
    let out = arg("--out", "moss.wav");
    let bench: usize = arg("--bench", "0").parse().expect("--bench");

    let t0 = std::time::Instant::now();
    let pipe = MossPipeline::load(
        weights.join("text_encoder"),
        weights.join("transformer"),
        weights.join("vae/vae_128d_48k.pth"),
        weights.join("tokenizer"),
        None,
    )
    .expect("pipeline load");
    let device = if makepad_diffusion::moss::moss_device_enabled() {
        Some(pipe.prepare_device().expect("prepare device"))
    } else {
        None
    };
    println!(
        "load {:.2}s (device: {})",
        t0.elapsed().as_secs_f64(),
        device.is_some()
    );

    let runs = bench.max(1);
    let mut audio = Vec::new();
    for run in 0..runs {
        let run_seed = seed + run as u64;
        let t0 = std::time::Instant::now();
        let mut last = std::time::Instant::now();
        audio = pipe
            .generate_auto(
                device.as_ref(),
                &prompt,
                seconds,
                steps,
                cfg,
                run_seed,
                Some(&mut |label: &str, _fraction: f64| {
                    if last.elapsed().as_secs() >= 30 {
                        println!("  {label}");
                        last = std::time::Instant::now();
                    }
                    Ok(())
                }),
                None,
            )
            .expect("generate");
        println!(
            "gen[{run}] seed={run_seed} {:.3}s ({} samples)",
            t0.elapsed().as_secs_f64(),
            audio.len()
        );
    }

    // mono 48k -> 16-bit PCM wav
    let mut bytes = Vec::with_capacity(44 + audio.len() * 2);
    let data_len = (audio.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&(MOSS_SAMPLE_RATE as u32).to_le_bytes());
    bytes.extend_from_slice(&((MOSS_SAMPLE_RATE * 2) as u32).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for v in &audio {
        bytes.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(&out, bytes).expect("write wav");
    println!("wrote {out}");
}
