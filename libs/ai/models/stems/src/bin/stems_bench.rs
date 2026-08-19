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

    let input = read_npy_f32(Path::new(&args[2]));
    assert!(input.len() >= AUDIO_CHANNELS * CHUNK_SAMPLES);
    let chunk = StereoBuf {
        left: input[..CHUNK_SAMPLES].to_vec(),
        right: input[CHUNK_SAMPLES..2 * CHUNK_SAMPLES].to_vec(),
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
