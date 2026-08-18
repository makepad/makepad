//! Native IndexTTS-2.5 end-to-end benchmark — the counterpart of
//! `ref_bench.py` (which froze the official-reference bar on the same box):
//! load / cold first synthesis (voice conditioning + lazy CUDA init) /
//! N warm syntheses with the cached voice (median, p95, RTF) / one long-text
//! point, printing `BENCH` lines that align 1:1 with the reference report.
//!
//! Token sequences are deterministic per run (fixed seed) but not identical
//! to torch's RNG stream, so wall-clock is compared alongside RTF (seconds
//! per second of audio), which normalizes generated-length differences.
//!
//! Usage:
//!   indextts-native-bench [--checkpoints <dir>] [--ref-wav <path>]
//!                         [--warm <n>] [--out <dir>]

use makepad_diffusion::indextts::INDEXTTS_SAMPLE_RATE;
use makepad_diffusion::indextts_pipeline::{
    IndexTtsPipeline, IndexTtsSynthesisParams, IndexTtsWeightPaths,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

const SEED: u64 = 42;
const TEXT: &str = "The old lighthouse keeper smiled as the storm finally passed.";
const TEXT_LONG: &str = "The old lighthouse keeper smiled as the storm finally passed. \
Far below, the waves still hammered the rocks, but the beam swung \
steady and bright across the clearing sky. He poured a last cup of \
tea and watched the first fishing boats slip out of the harbour.";

fn die(msg: &str) -> ! {
    eprintln!("indextts-native-bench: {msg}");
    std::process::exit(1);
}

// Minimal RIFF/WAVE PCM16 reader (mono or stereo-averaged), enough for the
// fixture reference clip.
fn read_wav_mono(path: &Path) -> (Vec<f32>, u32) {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        die(&format!("{}: not a RIFF/WAVE file", path.display()));
    }
    let mut pos = 12;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + size + (size & 1);
    }
    let data = data.unwrap_or_else(|| die(&format!("{}: no data chunk", path.display())));
    if bits != 16 || channels == 0 {
        die(&format!("{}: expected PCM16, got {bits}-bit x{channels}", path.display()));
    }
    let ch = channels as usize;
    let frames = data.len() / (2 * ch);
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut acc = 0f32;
        for c in 0..ch {
            let o = (f * ch + c) * 2;
            acc += i16::from_le_bytes(data[o..o + 2].try_into().unwrap()) as f32 / 32768.0;
        }
        out.push(acc / ch as f32);
    }
    (out, rate)
}

fn write_wav_mono(path: &Path, samples: &[f32], rate: u32) {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
}

fn main() {
    let mut checkpoints = PathBuf::from("checkpoints");
    let mut ref_wav = PathBuf::from("spk_ref_kokoro.wav");
    let mut out_dir = PathBuf::from(".");
    let mut warm_runs = 12usize;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--checkpoints" => {
                i += 1;
                checkpoints = PathBuf::from(
                    args.get(i).unwrap_or_else(|| die("--checkpoints needs a value")),
                );
            }
            "--ref-wav" => {
                i += 1;
                ref_wav =
                    PathBuf::from(args.get(i).unwrap_or_else(|| die("--ref-wav needs a value")));
            }
            "--out" => {
                i += 1;
                out_dir = PathBuf::from(args.get(i).unwrap_or_else(|| die("--out needs a value")));
            }
            "--warm" => {
                i += 1;
                warm_runs = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--warm needs a number"));
            }
            // Per-stage wall-time attribution (STAGE lines from the
            // pipeline); in-process env set beats quoting it through the
            // detached cmd.exe launch chain on the bench box.
            "--stage-timing" => std::env::set_var("INDEXTTS_STAGE_TIMING", "1"),
            other => die(&format!("unknown arg {other}")),
        }
        i += 1;
    }

    let (ref_samples, ref_rate) = read_wav_mono(&ref_wav);
    let mut params = IndexTtsSynthesisParams::default();
    params.sampling.seed = SEED;

    let t0 = Instant::now();
    let pipeline = IndexTtsPipeline::load(
        &IndexTtsWeightPaths::reference_layout(&checkpoints),
        None,
    )
    .unwrap_or_else(|e| die(&format!("load: {e:?}")));
    println!("BENCH load {:.2}s", t0.elapsed().as_secs_f64());

    // Cold: voice conditioning + first synthesis (lazy CUDA weight uploads,
    // first-launch kernel costs) — the reference cold_infer counterpart.
    let t0 = Instant::now();
    let voice = pipeline
        .prepare_voice(&ref_samples, ref_rate, None)
        .unwrap_or_else(|e| die(&format!("prepare_voice: {e:?}")));
    let wav = pipeline
        .synthesize(&voice, TEXT, &params, None)
        .unwrap_or_else(|e| die(&format!("cold synthesize: {e:?}")));
    let cold_s = t0.elapsed().as_secs_f64();
    let audio_s = wav.len() as f64 / INDEXTTS_SAMPLE_RATE as f64;
    println!("BENCH cold_infer {cold_s:.2}s audio {audio_s:.2}s");
    write_wav_mono(&out_dir.join("out_native_cold.wav"), &wav, INDEXTTS_SAMPLE_RATE);

    // Warm block: cached voice, fixed seed -> identical tokens every run.
    let mut warm = Vec::with_capacity(warm_runs);
    let mut last = Vec::new();
    for run in 0..warm_runs {
        let t0 = Instant::now();
        last = pipeline
            .synthesize(&voice, TEXT, &params, None)
            .unwrap_or_else(|e| die(&format!("warm synthesize: {e:?}")));
        warm.push(t0.elapsed().as_secs_f64());
        println!("BENCH warm[{run}] {:.3}s", warm[run]);
    }
    let audio_s = last.len() as f64 / INDEXTTS_SAMPLE_RATE as f64;
    write_wav_mono(&out_dir.join("out_native_neutral.wav"), &last, INDEXTTS_SAMPLE_RATE);
    let mut sorted = warm.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = if sorted.is_empty() {
        0.0
    } else if sorted.len() % 2 == 1 {
        sorted[sorted.len() / 2]
    } else {
        0.5 * (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2])
    };
    // Same estimator as ref_bench.py for n < 20: order statistic at
    // ceil(0.95n)-1.
    let p95 = sorted[((sorted.len() as f64 * 0.95).ceil() as usize).max(1) - 1];
    println!(
        "BENCH warm_median {median:.3}s p95 {p95:.3}s audio {audio_s:.2}s rtf {:.3}",
        median / audio_s
    );

    // Long-text single data point.
    let t0 = Instant::now();
    let wav = pipeline
        .synthesize(&voice, TEXT_LONG, &params, None)
        .unwrap_or_else(|e| die(&format!("long synthesize: {e:?}")));
    let long_s = t0.elapsed().as_secs_f64();
    let audio_s = wav.len() as f64 / INDEXTTS_SAMPLE_RATE as f64;
    println!("BENCH long {long_s:.2}s audio {audio_s:.2}s rtf {:.3}", long_s / audio_s);
    write_wav_mono(&out_dir.join("out_native_long.wav"), &wav, INDEXTTS_SAMPLE_RATE);

    println!("NATIVE BENCH DONE");
}
