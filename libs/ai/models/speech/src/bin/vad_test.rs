//! Run Silero VAD over a wav file and print speech segments plus timing.
//!
//! Usage: vad-test <audio_16k_mono.wav> [--probs]
//! The model resolves via MAKEPAD_VAD_MODEL or ./silero_vad.onnx.

use makepad_ai_speech::vad::{SileroVad, VAD_CHUNK_SAMPLES, VAD_SAMPLE_RATE};
use std::io::{Read, Seek, SeekFrom};

fn read_wav_pcm_f32(path: &str) -> Vec<f32> {
    let mut f = std::fs::File::open(path).expect("failed to open wav");
    let mut riff_header = [0u8; 12];
    f.read_exact(&mut riff_header).expect("failed to read RIFF header");
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
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()) as usize;
        if &chunk_header[0..4] == b"fmt " {
            let mut fmt = vec![0u8; chunk_size];
            f.read_exact(&mut fmt).expect("failed to read fmt chunk");
            channels = u16::from_le_bytes([fmt[2], fmt[3]]);
            sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);
        } else if &chunk_header[0..4] == b"data" {
            audio_data = vec![0u8; chunk_size];
            f.read_exact(&mut audio_data).expect("failed to read data chunk");
            break;
        } else {
            f.seek(SeekFrom::Current(chunk_size as i64)).expect("failed to skip chunk");
        }
    }
    assert_eq!(sample_rate as usize, VAD_SAMPLE_RATE, "expected 16kHz");
    assert_eq!(bits_per_sample, 16, "expected 16-bit PCM");
    let mut samples: Vec<f32> = audio_data
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes(pair.try_into().unwrap()) as f32 / 32768.0)
        .collect();
    if channels == 2 {
        samples = samples.iter().step_by(2).copied().collect();
    }
    samples
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(wav_path) = args.get(1) else {
        eprintln!("usage: vad-test <audio_16k_mono.wav> [--probs]");
        std::process::exit(1);
    };
    let print_probs = args.iter().any(|a| a == "--probs");

    let samples = read_wav_pcm_f32(wav_path);
    let mut vad = SileroVad::from_makepad_env().expect("vad model");

    // Simple hysteresis, mirroring silero's defaults: enter at 0.5, exit 0.35.
    let mut in_speech = false;
    let mut seg_start = 0.0f64;
    let started = std::time::Instant::now();
    let chunk_count = samples.len() / VAD_CHUNK_SAMPLES;
    for index in 0..chunk_count {
        let chunk = &samples[index * VAD_CHUNK_SAMPLES..(index + 1) * VAD_CHUNK_SAMPLES];
        let prob = vad.process_chunk(chunk);
        let time = (index * VAD_CHUNK_SAMPLES) as f64 / VAD_SAMPLE_RATE as f64;
        if print_probs {
            println!("{time:7.3}s  {prob:.3}  {}", "#".repeat((prob * 40.0) as usize));
        }
        if !in_speech && prob >= 0.5 {
            in_speech = true;
            seg_start = time;
        } else if in_speech && prob < 0.35 {
            in_speech = false;
            println!("speech {seg_start:7.3}s .. {time:7.3}s");
        }
    }
    if in_speech {
        println!(
            "speech {seg_start:7.3}s .. {:7.3}s (end)",
            chunk_count * VAD_CHUNK_SAMPLES / VAD_SAMPLE_RATE
        );
    }
    let elapsed = started.elapsed();
    println!(
        "{chunk_count} chunks ({:.1}s audio) in {:.1}ms — {:.1}us/chunk",
        (chunk_count * VAD_CHUNK_SAMPLES) as f64 / VAD_SAMPLE_RATE as f64,
        elapsed.as_secs_f64() * 1e3,
        elapsed.as_secs_f64() * 1e6 / chunk_count.max(1) as f64
    );
}
