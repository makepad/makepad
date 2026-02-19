use makepad_voice::{WhisperModel, WhisperParams, WhisperState};
use std::io::{Read, Seek, SeekFrom};

fn read_wav_pcm_f32(path: &str) -> Vec<f32> {
    let mut f = std::fs::File::open(path).expect("failed to open wav");

    // Read RIFF header (12 bytes)
    let mut riff_header = [0u8; 12];
    f.read_exact(&mut riff_header)
        .expect("failed to read RIFF header");
    assert_eq!(&riff_header[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&riff_header[8..12], b"WAVE", "not a WAVE file");

    let mut channels = 1u16;
    let mut sample_rate = 16000u32;
    let mut bits_per_sample = 16u16;
    let mut audio_data = Vec::new();

    // Parse chunks
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
            // Skip unknown chunks
            f.seek(SeekFrom::Current(chunk_size as i64))
                .expect("failed to skip chunk");
        }
    }

    eprintln!(
        "wav: {} Hz, {} ch, {} bit",
        sample_rate, channels, bits_per_sample
    );
    assert_eq!(sample_rate, 16000, "expected 16kHz");
    assert_eq!(bits_per_sample, 16, "expected 16-bit PCM");

    // Convert i16 -> f32
    let n_samples = audio_data.len() / 2;
    let mut samples = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let s = i16::from_le_bytes([audio_data[i * 2], audio_data[i * 2 + 1]]);
        samples.push(s as f32 / 32768.0);
    }

    // If stereo, take left channel only
    if channels == 2 {
        samples = samples.iter().step_by(2).copied().collect();
    }

    eprintln!(
        "wav: {} samples ({:.1}s)",
        samples.len(),
        samples.len() as f64 / 16000.0
    );
    samples
}

fn main() {
    let model_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "local/ggml-large-v3-turbo.bin".into());
    let wav_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "local/whisper.cpp/samples/jfk.wav".into());

    eprintln!("loading model: {}", model_path);
    if let Ok(threads) = std::env::var("MAKEPAD_VOICE_THREADS") {
        eprintln!("using MAKEPAD_VOICE_THREADS={}", threads);
    }
    let t0 = std::time::Instant::now();
    let model = WhisperModel::load_file(&model_path).expect("failed to load model");
    eprintln!("model loaded in {:.1}s", t0.elapsed().as_secs_f64());

    let max_sec: f64 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let max_tokens: Option<usize> = std::env::args()
        .nth(4)
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            std::env::var("MAKEPAD_VOICE_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
        });

    eprintln!("loading audio: {}", wav_path);
    let mut samples = read_wav_pcm_f32(&wav_path);
    if max_sec > 0.0 {
        let max_samples = (max_sec * 16000.0) as usize;
        samples.truncate(max_samples);
        eprintln!("truncated to {:.1}s ({} samples)", max_sec, samples.len());
    }

    let mut state = WhisperState::new(&model);
    let mut params = WhisperParams::default();
    if let Some(max_tokens) = max_tokens {
        params.max_tokens = max_tokens;
        eprintln!("using max_tokens={}", max_tokens);
    }

    eprintln!("transcribing...");
    makepad_voice::reset_profiling();
    let t0 = std::time::Instant::now();
    let segments = state.transcribe(&model, &samples, &params);
    let total = t0.elapsed().as_secs_f64();
    eprintln!("transcription done in {:.1}s", total);
    makepad_voice::print_profiling();

    for seg in &segments {
        let t0 = seg.start_ms as f64 / 1000.0;
        let t1 = seg.end_ms as f64 / 1000.0;
        println!("[{:.2} --> {:.2}]  {}", t0, t1, seg.text);
    }
}
