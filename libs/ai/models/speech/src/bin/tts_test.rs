//! Speak a sentence with the Kokoro engine and write it to a WAV.
//!
//!     cargo run --release --manifest-path libs/ai/models/speech/Cargo.toml --bin tts_test -- "Hello there" [voice.mkvoice] [out.wav]
//!
//! Weights resolve through the working directory or next to the executable
//! (`kokoro-v1_0.mktts`, `bm_daniel.mkvoice`).

use makepad_ai_speech::kokoro::{self, KokoroSpeaker};
use makepad_ai_speech::SpeechAudio;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let text = args.get(0).cloned().unwrap_or_else(|| "Hello from Kokoro.".to_string());
    let out = args.get(2).cloned().unwrap_or_else(|| "tts_test.wav".to_string());

    let Some(model) = kokoro::model_path_if_present() else {
        eprintln!("kokoro weights not found (put {} in the cwd)", kokoro::DEFAULT_MODEL_PATH);
        std::process::exit(1);
    };
    let voice = match args.get(1) {
        Some(name) => kokoro::named_voice_path_if_present(name),
        None => kokoro::voice_path_if_present(),
    };
    let Some(voice) = voice else {
        eprintln!("voice pack not found");
        std::process::exit(1);
    };
    let started = std::time::Instant::now();
    let mut speaker = match KokoroSpeaker::load_with_voice(&model, &voice) {
        Ok(speaker) => speaker,
        Err(err) => {
            eprintln!("load failed: {err:?}");
            std::process::exit(1);
        }
    };
    eprintln!("loaded {model} + {voice} in {:.2}s", started.elapsed().as_secs_f64());

    let started = std::time::Instant::now();
    let audio = match speaker.synthesize(&text) {
        Ok(audio) => audio,
        Err(err) => {
            eprintln!("synthesis failed: {err:?}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "rendered {:.2}s of audio in {:.2}s",
        audio.duration_secs(),
        started.elapsed().as_secs_f64()
    );
    std::fs::write(&out, wav_pcm16(&audio)).expect("write wav");
    println!("wrote {out}");
}

fn wav_pcm16(audio: &SpeechAudio) -> Vec<u8> {
    let data_len = (audio.samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&audio.sample_rate.to_le_bytes());
    out.extend_from_slice(&(audio.sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in &audio.samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}
