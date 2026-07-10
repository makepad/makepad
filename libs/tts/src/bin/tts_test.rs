//! Synthesize a sentence and write `tts_test.wav`.
//!
//!     cargo run --bin tts_test -- "Hi! I make games with you."

use std::io::Write;

use makepad_tts::{SpeechAudio, Speaker};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let text = if args.is_empty() {
        "Hi! I make games with you.".to_string()
    } else {
        args.join(" ")
    };

    let mut speaker = Speaker::from_makepad_env();
    println!("backend : {:?}", speaker.kind());
    println!("text    : {text:?}");

    let started = std::time::Instant::now();
    match speaker.synthesize(&text) {
        Ok(audio) => {
            let elapsed = started.elapsed().as_secs_f32();
            let peak = audio.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            println!(
                "samples : {} @ {} Hz ({:.2}s audio)",
                audio.samples.len(),
                audio.sample_rate,
                audio.duration_secs()
            );
            println!("peak    : {peak:.4}");
            println!(
                "synth   : {:.0} ms ({:.1}x realtime)",
                elapsed * 1000.0,
                audio.duration_secs() / elapsed.max(1e-6)
            );
            match write_wav("tts_test.wav", &audio) {
                Ok(()) => println!("wrote   : tts_test.wav"),
                Err(err) => println!("wav err : {err}"),
            }
        }
        Err(err) => println!("error   : {err:?}"),
    }
}

/// Minimal 16-bit mono WAV writer.
fn write_wav(path: &str, audio: &SpeechAudio) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    let data_len = (audio.samples.len() * 2) as u32;
    let rate = audio.sample_rate;

    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // fmt chunk size
    file.write_all(&1u16.to_le_bytes())?; // PCM
    file.write_all(&1u16.to_le_bytes())?; // mono
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&(rate * 2).to_le_bytes())?; // byte rate
    file.write_all(&2u16.to_le_bytes())?; // block align
    file.write_all(&16u16.to_le_bytes())?; // bits per sample
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;

    for sample in &audio.samples {
        let clamped = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        file.write_all(&clamped.to_le_bytes())?;
    }
    Ok(())
}
