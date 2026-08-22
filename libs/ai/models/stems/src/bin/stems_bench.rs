//! Warm per-chunk timing for the separator.
//!
//! ```text
//! cargo run -p makepad-ai-stems --release --bin stems_bench -- \
//!     <checkpoint.ckpt> <chunk.npy> [repeats]
//! ```
//!
//! `chunk.npy` is `oracle.py taps`' `00_input.npy` — a `(1, 2, 485100)` f32
//! array. The first iteration is reported separately because it pays for
//! pipeline compilation and the first weight residency pass; the reported
//! realtime factor uses the warm iterations only, which is what a track-long
//! demix actually costs.
//!
//! Passing the literal word `synth` instead of a path generates a fixed
//! pseudo-musical chunk from a seeded integer recurrence. It is bit-identical
//! on every machine and needs no fixture file, which makes it the input for
//! cross-store agreement checks: run the same command on a Metal client and on
//! a CUDA box and the per-stem statistics below must match.

use makepad_ai_stems::config::{AUDIO_CHANNELS, CHUNK_SAMPLES, CHUNK_STEP, SAMPLE_RATE};
use makepad_ai_stems::{StemsModel, StereoBuf};
use std::path::Path;

fn read_npy_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(&bytes[0..6], b"\x93NUMPY");
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    bytes[10 + header_len..]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: stems_bench <checkpoint.ckpt> <chunk.npy> [repeats]");
        std::process::exit(2);
    }
    let repeats: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(4);

    let load = std::time::Instant::now();
    let mut model = StemsModel::load(&args[1]).expect("load separator");
    println!("load+compile      {:>7.2} s", load.elapsed().as_secs_f64());

    let chunk = if args[2] == "synth" {
        synth_chunk()
    } else {
        let input = read_npy_f32(Path::new(&args[2]));
        assert!(input.len() >= AUDIO_CHANNELS * CHUNK_SAMPLES);
        StereoBuf {
            left: input[..CHUNK_SAMPLES].to_vec(),
            right: input[CHUNK_SAMPLES..2 * CHUNK_SAMPLES].to_vec(),
        }
    };

    let mut warm = Vec::new();
    for i in 0..repeats {
        let t = std::time::Instant::now();
        let stems = model.separate_chunk(&chunk).expect("separate_chunk");
        let secs = t.elapsed().as_secs_f64();
        let peak = stems
            .iter()
            .map(|s| s.left.iter().fold(0.0f32, |a, v| a.max(v.abs())))
            .fold(0.0f32, f32::max);
        println!("chunk {i:>2}          {secs:>7.3} s   (peak {peak:.4})");
        if i == 0 {
            report_stems(&stems);
        }
        if i > 0 {
            warm.push(secs);
        }
    }
    if warm.is_empty() {
        return;
    }
    let mean = warm.iter().sum::<f64>() / warm.len() as f64;
    let best = warm.iter().cloned().fold(f64::INFINITY, f64::min);
    // Each chunk finalizes CHUNK_STEP samples of output, not CHUNK_SAMPLES —
    // 2x overlap means every sample is separated twice. The realtime factor a
    // user experiences is therefore step/chunk_time.
    let step_secs = CHUNK_STEP as f64 / SAMPLE_RATE as f64;
    println!(
        "warm mean         {mean:>7.3} s   best {best:.3} s\n\
         audio per chunk   {step_secs:>7.3} s (finalized)\n\
         REALTIME FACTOR   {:>7.2} x (mean)   {:>5.2} x (best)",
        step_secs / mean,
        step_secs / best
    );
}

/// A deterministic stand-in for music: four decaying harmonic stacks over a
/// noise bed, from an integer recurrence so it is identical everywhere.
/// Rich enough that every band of the 62-band split sees real energy, which a
/// pure tone would not do.
fn synth_chunk() -> StereoBuf {
    let mut state: u64 = 0x9e3779b97f4a7c15;
    let mut left = Vec::with_capacity(CHUNK_SAMPLES);
    let mut right = Vec::with_capacity(CHUNK_SAMPLES);
    let rate = SAMPLE_RATE as f64;
    for n in 0..CHUNK_SAMPLES {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let noise = ((state >> 40) as f64 / 8_388_608.0) - 1.0;
        let t = n as f64 / rate;
        let mut sample = 0.0f64;
        for (index, base) in [55.0f64, 220.0, 440.0, 1318.5].iter().enumerate() {
            let env = (-(t % 0.5) * (2.0 + index as f64)).exp();
            for harmonic in 1..=4u32 {
                let freq = base * harmonic as f64;
                sample += env * (0.12 / harmonic as f64)
                    * (std::f64::consts::TAU * freq * t + index as f64).sin();
            }
        }
        sample += 0.03 * noise;
        // A small, fixed inter-channel difference so a stereo-axis bug in the
        // feature packing cannot hide behind two identical channels.
        left.push(sample as f32);
        right.push((sample * 0.85 + 0.05 * noise) as f32);
    }
    StereoBuf { left, right }
}

/// Number of random projections in the cross-machine fingerprint. Each is an
/// unbiased estimator of the squared norm of whatever differs between two
/// runs, so PROBES of them estimate that norm to about
/// `sqrt(2 / PROBES)` relative — 18% at 64, i.e. +/- 0.7 dB of SNR.
const PROBES: usize = 64;

/// Per-stem statistics, plus a projection fingerprint that makes a real SNR
/// computable across two machines without moving the audio.
///
/// `energy` is the exact squared norm of the stem. Each `p` is the stem's
/// inner product with a fixed Rademacher (+/-1) vector, generated from a
/// constant seed so both machines probe with the same directions. Given two
/// runs A and B, `mean_k (pA_k - pB_k)^2` estimates `||A - B||^2`, and
/// `10*log10(energy / that)` is the SNR between them. Printed with ten
/// significant digits because the interesting differences are ~1e-8 of the
/// value.
fn report_stems(stems: &[StereoBuf]) {
    use makepad_ai_stems::config::STEM_NAMES;
    for (index, stem) in stems.iter().enumerate() {
        for (channel, samples) in [("L", &stem.left), ("R", &stem.right)] {
            let peak = samples.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            let energy = samples.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>();
            let rms = (energy / samples.len() as f64).sqrt();
            println!(
                "  stem {:<7}{channel}  peak {peak:.8}  rms {rms:.10}  energy {energy:.10e}",
                STEM_NAMES[index],
            );
            let probes = project(samples);
            for (probe, value) in probes.iter().enumerate() {
                println!("    p {:<7}{channel} {probe:>2} {value:.10e}", STEM_NAMES[index]);
            }
        }
    }
}

/// `PROBES` Rademacher projections of `samples`, in f64 so the accumulation
/// itself cannot be what differs between two machines.
fn project(samples: &[f32]) -> [f64; PROBES] {
    let mut out = [0.0f64; PROBES];
    let mut state: u64 = 0xda7a_5eed_1234_5678;
    for (index, sample) in samples.iter().enumerate() {
        // One draw per sample, reused across all probes by rotating the word:
        // 64 independent-enough sign bits per sample, one PRNG step.
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407 ^ index as u64);
        let value = *sample as f64;
        for (probe, acc) in out.iter_mut().enumerate() {
            if (state >> probe) & 1 == 1 {
                *acc += value;
            } else {
                *acc -= value;
            }
        }
    }
    out
}
