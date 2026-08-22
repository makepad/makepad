//! GPU-vs-CPU parity harness for `makepad-voice`.
//!
//! Transcribes one clip twice in a single process — once with the accelerator
//! (CUDA on Linux/Windows, Metal on Apple) and once with it forced off — and
//! diffs the sampled token ids and the segment timestamps. Greedy decoding at
//! temperature 0 is deterministic, so the pass condition is exact:
//!
//! * token ids identical, and
//! * every segment boundary within one mel frame (10 ms) — in practice they
//!   are bit-identical, because timestamps are derived from token ids.
//!
//! Usage:
//!
//! ```text
//! MAKEPAD_VOICE_TOKEN_TRACE=1 \
//!   ./target/release/whisper-parity <model.bin> <clip.wav> [max_seconds]
//! ```
//!
//! Exit code 0 = parity, 1 = mismatch, 2 = the accelerator never engaged (so
//! the run proved nothing). Set `MAKEPAD_VOICE_CUDA_ELEMENTWISE=1` to also put
//! the elementwise ops on the device for the accelerated pass.

use makepad_voice::{Segment, WhisperModel, WhisperParams, WhisperState};
use std::io::{Read, Seek, SeekFrom};

fn read_wav_pcm_f32(path: &str) -> Vec<f32> {
    let mut f = std::fs::File::open(path).expect("failed to open wav");

    let mut riff_header = [0u8; 12];
    f.read_exact(&mut riff_header)
        .expect("failed to read RIFF header");
    assert_eq!(&riff_header[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&riff_header[8..12], b"WAVE", "not a WAVE file");

    let mut channels = 1u16;
    let mut sample_rate = 16000u32;
    let mut bits_per_sample = 16u16;
    let mut audio_data = Vec::new();

    loop {
        let mut chunk_header = [0u8; 8];
        if f.read_exact(&mut chunk_header).is_err() {
            break;
        }
        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes([
            chunk_header[4],
            chunk_header[5],
            chunk_header[6],
            chunk_header[7],
        ]) as usize;

        if chunk_id == b"fmt " {
            let mut fmt = vec![0u8; chunk_size];
            f.read_exact(&mut fmt).expect("failed to read fmt chunk");
            channels = u16::from_le_bytes([fmt[2], fmt[3]]);
            sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);
        } else if chunk_id == b"data" {
            audio_data = vec![0u8; chunk_size];
            f.read_exact(&mut audio_data)
                .expect("failed to read data chunk");
            break;
        } else {
            f.seek(SeekFrom::Current(chunk_size as i64))
                .expect("failed to skip chunk");
        }
    }

    assert_eq!(sample_rate, 16000, "expected 16kHz");
    assert_eq!(bits_per_sample, 16, "expected 16-bit PCM");

    let n_samples = audio_data.len() / 2;
    let mut samples = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let s = i16::from_le_bytes([audio_data[i * 2], audio_data[i * 2 + 1]]);
        samples.push(s as f32 / 32768.0);
    }
    if channels == 2 {
        samples = samples.iter().step_by(2).copied().collect();
    }
    samples
}

struct Run {
    label: &'static str,
    backend: &'static str,
    seconds: f64,
    tokens: Vec<i32>,
    segments: Vec<Segment>,
}

fn run_once(
    label: &'static str,
    accel: bool,
    model: &WhisperModel,
    samples: &[f32],
    params: &WhisperParams,
) -> Run {
    makepad_voice::set_accel_enabled(accel);
    let backend = makepad_voice::accel_backend_name();
    // Drop anything a previous pass left behind.
    let _ = makepad_voice::take_token_trace();

    let mut state = WhisperState::new(model);
    let t0 = std::time::Instant::now();
    let segments = state.transcribe(model, samples, params);
    let seconds = t0.elapsed().as_secs_f64();

    Run {
        label,
        backend,
        seconds,
        tokens: makepad_voice::take_token_trace(),
        segments,
    }
}

fn print_run(run: &Run) {
    println!(
        "--- {} (backend={}, {:.2}s, {} tokens, {} segments)",
        run.label,
        run.backend,
        run.seconds,
        run.tokens.len(),
        run.segments.len()
    );
    println!("    tokens: {:?}", run.tokens);
    for seg in &run.segments {
        println!(
            "    [{:>8} --> {:>8} ms] {}",
            seg.start_ms, seg.end_ms, seg.text
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model_path = args
        .next()
        .unwrap_or_else(|| "ggml-large-v3-turbo.bin".into());
    let wav_path = args
        .next()
        .unwrap_or_else(|| "local/whisper.cpp/samples/jfk.wav".into());
    let max_sec: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);

    if std::env::var_os("MAKEPAD_VOICE_TOKEN_TRACE").is_none() {
        eprintln!(
            "note: MAKEPAD_VOICE_TOKEN_TRACE is not set — token ids will be empty \
             and only timestamps/text are compared"
        );
    }

    eprintln!("model: {model_path}");
    let model = WhisperModel::load_file(&model_path).expect("failed to load model");
    let mut samples = read_wav_pcm_f32(&wav_path);
    if max_sec > 0.0 {
        samples.truncate((max_sec * 16000.0) as usize);
    }
    eprintln!(
        "audio: {wav_path} ({} samples, {:.1}s)",
        samples.len(),
        samples.len() as f64 / 16000.0
    );

    let params = WhisperParams::default();
    assert_eq!(params.temperature, 0.0, "parity requires greedy decoding");

    // CPU first: it is the ground truth, and it also warms the page cache so
    // the accelerated timing is not distorted by first-touch faults.
    let cpu = run_once("cpu", false, &model, &samples, &params);
    let gpu = run_once("accel", true, &model, &samples, &params);

    print_run(&cpu);
    print_run(&gpu);

    if gpu.backend == "cpu" {
        eprintln!(
            "FAIL(setup): no accelerator engaged — both passes ran on the CPU, \
             so this run proves nothing about GPU parity"
        );
        std::process::exit(2);
    }

    let mut failures = Vec::new();

    if cpu.tokens != gpu.tokens {
        let first = cpu
            .tokens
            .iter()
            .zip(&gpu.tokens)
            .position(|(a, b)| a != b)
            .unwrap_or(cpu.tokens.len().min(gpu.tokens.len()));
        failures.push(format!(
            "token ids diverge at index {first} (cpu={:?} accel={:?}, lengths {} vs {})",
            cpu.tokens.get(first),
            gpu.tokens.get(first),
            cpu.tokens.len(),
            gpu.tokens.len()
        ));
    }

    if cpu.segments.len() != gpu.segments.len() {
        failures.push(format!(
            "segment count differs: cpu={} accel={}",
            cpu.segments.len(),
            gpu.segments.len()
        ));
    } else {
        // One mel frame = 10 ms.
        const FRAME_MS: i64 = 10;
        for (i, (a, b)) in cpu.segments.iter().zip(&gpu.segments).enumerate() {
            if (a.start_ms - b.start_ms).abs() > FRAME_MS
                || (a.end_ms - b.end_ms).abs() > FRAME_MS
            {
                failures.push(format!(
                    "segment {i} timestamps differ by more than one frame: \
                     cpu=[{},{}] accel=[{},{}]",
                    a.start_ms, a.end_ms, b.start_ms, b.end_ms
                ));
            }
            if a.text != b.text {
                failures.push(format!(
                    "segment {i} text differs:\n  cpu  : {:?}\n  accel: {:?}",
                    a.text, b.text
                ));
            }
        }
    }

    if failures.is_empty() {
        println!(
            "PASS: {} backend is token-identical to the CPU path \
             ({} tokens, {} segments); {:.2}s vs {:.2}s cpu ({:.2}x)",
            gpu.backend,
            cpu.tokens.len(),
            cpu.segments.len(),
            gpu.seconds,
            cpu.seconds,
            cpu.seconds / gpu.seconds.max(f64::MIN_POSITIVE)
        );
    } else {
        for failure in &failures {
            println!("FAIL: {failure}");
        }
        std::process::exit(1);
    }
}
