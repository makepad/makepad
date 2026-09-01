//! Numeric validation of the pure-Rust Silero VAD against onnxruntime.
//!
//! The reference probabilities in `tests/fixtures/` were produced by
//! onnxruntime 1.28 running the official `silero_vad.onnx` (see the oracle
//! script noted in `src/vad.rs`). Both tests need the model file, resolved
//! from `MAKEPAD_VAD_MODEL` or the repo root; they skip quietly if it is
//! missing so the suite still passes on machines without the weights.

use makepad_ai_speech::vad::{SileroVad, VAD_CHUNK_SAMPLES};
use std::path::{Path, PathBuf};

fn repo_root_file(name: &str) -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(name);
    path.is_file().then_some(path)
}

fn load_vad() -> Option<SileroVad> {
    let path = std::env::var("MAKEPAD_VAD_MODEL")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| repo_root_file("silero_vad.onnx"));
    let Some(path) = path else {
        eprintln!("skipping: silero_vad.onnx not found (repo root or MAKEPAD_VAD_MODEL)");
        return None;
    };
    Some(SileroVad::load(path.to_str().unwrap()).expect("model should load"))
}

fn load_fixture(name: &str) -> Vec<f32> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .lines()
        .map(|line| line.trim().parse().expect("fixture float"))
        .collect()
}

fn compare(vad: &mut SileroVad, signal: &[f32], expected: &[f32], what: &str) {
    vad.reset();
    let mut max_diff = 0.0f32;
    for (index, expect) in expected.iter().enumerate() {
        let chunk = &signal[index * VAD_CHUNK_SAMPLES..(index + 1) * VAD_CHUNK_SAMPLES];
        let prob = vad.process_chunk(chunk);
        let diff = (prob - expect).abs();
        if diff > max_diff {
            max_diff = diff;
        }
        assert!(
            diff < 1.5e-3,
            "{what} chunk {index}: rust {prob} vs onnxruntime {expect}"
        );
    }
    eprintln!("{what}: {} chunks, max diff {max_diff:.2e}", expected.len());
}

/// The deterministic signal must match the oracle script exactly: silence,
/// then LCG noise at 0.05, a vibrato tone, and LCG noise at 0.3 (the LCG
/// stream continues across the two noise segments).
fn synth_signal() -> Vec<f32> {
    use std::f64::consts::PI;
    let mut sig = vec![0.0f32; 16000 * 4];
    let mut lcg_state: u32 = 12345;
    let mut lcg = move || -> f64 {
        lcg_state = lcg_state.wrapping_mul(1664525).wrapping_add(1013904223);
        (lcg_state as f64 / 4294967296.0) * 2.0 - 1.0
    };
    for sample in &mut sig[16000..32000] {
        *sample = (0.05 * lcg()) as f32;
    }
    for (index, sample) in sig[32000..48000].iter_mut().enumerate() {
        let t = index as f64 / 16000.0;
        let f = 220.0 + 40.0 * (2.0 * PI * 3.0 * t).sin();
        *sample = (0.2 * (2.0 * PI * f * t).sin() + 0.05 * (2.0 * PI * 2.0 * f * t).sin()) as f32;
    }
    for sample in &mut sig[48000..64000] {
        *sample = (0.3 * lcg()) as f32;
    }
    sig
}

#[test]
fn silero_matches_onnxruntime_on_synthetic_signal() {
    let Some(mut vad) = load_vad() else { return };
    let expected = load_fixture("silero_ref_synth.txt");
    compare(&mut vad, &synth_signal(), &expected, "synth");
}

#[test]
fn silero_matches_onnxruntime_on_speech() {
    let Some(mut vad) = load_vad() else { return };
    let Some(wav_path) = repo_root_file("kokoro_ref_16k.wav") else {
        eprintln!("skipping: kokoro_ref_16k.wav not found at repo root");
        return;
    };
    let signal = read_wav_16k_mono(&wav_path);
    let expected = load_fixture("silero_ref_speech.txt");
    compare(&mut vad, &signal, &expected, "speech");
}

fn read_wav_16k_mono(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("wav read");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12;
    let mut samples = Vec::new();
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
        if id == b"fmt " {
            let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            assert_eq!((channels, rate, bits), (1, 16000, 16), "expect 16k mono s16");
        } else if id == b"data" {
            for chunk in body.chunks_exact(2) {
                samples.push(i16::from_le_bytes(chunk.try_into().unwrap()) as f32 / 32768.0);
            }
        }
        pos += 8 + size + (size & 1);
    }
    samples
}
