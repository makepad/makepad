#[cfg(all(any(target_os = "macos", target_os = "ios"), not(force_whisper)))]
fn main() {
    use std::io::{Read, Seek, SeekFrom};

    let wav_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "local/whisper.cpp/samples/jfk.wav".into());

    eprintln!("loading audio: {}", wav_path);
    let samples = read_wav_pcm_f32(&wav_path);

    let params = makepad_voice::WhisperParams::default();
    eprintln!(
        "params: language='{}', translate={}",
        params.language, params.translate
    );

    // Try ensure_model first
    eprintln!("ensuring model for '{}'...", params.language);
    match makepad_voice::apple_speech::ensure_model(&params.language) {
        Ok(()) => eprintln!("model ready"),
        Err(()) => eprintln!("WARNING: ensure_model failed (may still work)"),
    }

    eprintln!(
        "transcribing {} samples ({:.2}s)...",
        samples.len(),
        samples.len() as f64 / 16000.0
    );
    let t0 = std::time::Instant::now();
    let segments = makepad_voice::apple_speech::transcribe(&samples, &params);
    let elapsed = t0.elapsed().as_secs_f64();
    eprintln!(
        "transcription done in {:.2}s, got {} segments",
        elapsed,
        segments.len()
    );

    if segments.is_empty() {
        eprintln!("WARNING: no segments returned!");
    }

    for seg in &segments {
        let t0 = seg.start_ms as f64 / 1000.0;
        let t1 = seg.end_ms as f64 / 1000.0;
        println!("[{:.2} --> {:.2}]  {}", t0, t1, seg.text);
    }

    fn read_wav_pcm_f32(path: &str) -> Vec<f32> {
        let mut f = std::fs::File::open(path).expect("failed to open wav");
        let mut riff_header = [0u8; 12];
        f.read_exact(&mut riff_header)
            .expect("failed to read RIFF header");
        assert_eq!(&riff_header[0..4], b"RIFF");
        assert_eq!(&riff_header[8..12], b"WAVE");

        let mut channels = 1u16;
        let mut _sample_rate = 16000u32;
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
                f.read_exact(&mut fmt).expect("failed to read fmt");
                channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                _sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);
            } else if chunk_id == b"data" {
                audio_data = vec![0u8; chunk_size];
                f.read_exact(&mut audio_data).expect("failed to read data");
                break;
            } else {
                f.seek(SeekFrom::Current(chunk_size as i64)).expect("skip");
            }
        }

        eprintln!(
            "wav: {} Hz, {} ch, {} bit",
            _sample_rate, channels, bits_per_sample
        );

        let n_samples = audio_data.len() / 2;
        let mut samples = Vec::with_capacity(n_samples);
        for i in 0..n_samples {
            let s = i16::from_le_bytes([audio_data[i * 2], audio_data[i * 2 + 1]]);
            samples.push(s as f32 / 32768.0);
        }
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
}

#[cfg(not(all(any(target_os = "macos", target_os = "ios"), not(force_whisper))))]
fn main() {
    eprintln!("apple-speech path unavailable on this target/config");
}
