//! Woosh DFlow prompt -> wav CLI (fixed 5.0 s, mono 48 kHz).
//!
//! Doubles as the perf bench harness for the (pending) CUDA phase: `--bench N`
//! reports per-run wall times and per-stage splits so warm timings can be
//! compared against the reference bars (DFlow e2e warm 0.059-0.076s on the
//! 5090; per-DiT-forward ~13.5ms).
//!
//! Usage:
//!   woosh-generate [--models local/models/woosh] --prompt "..."
//!     [--seed 1001] [--out woosh.wav] [--bench N]

use makepad_diffusion::woosh::WOOSH_SAMPLE_RATE;
use makepad_diffusion::woosh_pipeline::WooshPipeline;
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
    let models = PathBuf::from(arg("--models", "local/models/woosh"));
    let prompt = arg(
        "--prompt",
        "sword clash, metallic impact of two steel blades, sharp ring",
    );
    let seed: u64 = arg("--seed", "1001").parse().expect("--seed");
    let out = arg("--out", "woosh.wav");
    let bench: usize = arg("--bench", "0").parse().expect("--bench");

    let t0 = std::time::Instant::now();
    let pipe = WooshPipeline::load(
        models.join("checkpoints/TextConditionerA/weights.safetensors"),
        models.join("checkpoints/Woosh-DFlow/weights.safetensors"),
        models.join("checkpoints/Woosh-AE/weights.safetensors"),
        models.join("tokenizer.json"),
        None,
    )
    .expect("pipeline load");
    println!("load {:.2}s", t0.elapsed().as_secs_f64());

    let runs = bench.max(1);
    let mut audio = Vec::new();
    for run in 0..runs {
        let run_seed = seed + run as u64;
        let t0 = std::time::Instant::now();
        let mut last_label = String::new();
        let mut stage_start = std::time::Instant::now();
        audio = pipe
            .generate(
                &prompt,
                run_seed,
                Some(&mut |label: &str, _fraction: f64| {
                    let head = label.split_whitespace().next().unwrap_or("");
                    if head != last_label {
                        if !last_label.is_empty() {
                            println!("  {last_label}: {:.3}s", stage_start.elapsed().as_secs_f64());
                        }
                        last_label = head.to_string();
                        stage_start = std::time::Instant::now();
                    }
                    Ok(())
                }),
                None,
            )
            .expect("generate");
        if !last_label.is_empty() {
            println!("  {last_label}: {:.3}s", stage_start.elapsed().as_secs_f64());
        }
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
    bytes.extend_from_slice(&(WOOSH_SAMPLE_RATE as u32).to_le_bytes());
    bytes.extend_from_slice(&((WOOSH_SAMPLE_RATE * 2) as u32).to_le_bytes());
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
