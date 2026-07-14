//! Transcribe a WAV with Whisper and score it against the text it should say.
//!
//! The other half of the round-trip harness: `roundtrip.rs` synthesizes through
//! a `Speaker`, this scores audio that came from anywhere — the ONNX reference,
//! the Rust graph, a file on disk.
//!
//!     cargo run --release --manifest-path libs/tts/Cargo.toml \
//!         --example score_wav -- kokoro_ref_16k.wav "Escape the Gummer, a squishy purple blob."

use makepad_voice::{VoiceTranscribeParams, VoiceTranscriber};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: score_wav <wav> [expected text]");
    let expected: String = args.collect::<Vec<_>>().join(" ");

    let (samples, rate) = read_wav(&path);
    println!("wav      : {path} ({} samples @ {rate} Hz)", samples.len());
    if rate != 16_000 {
        println!("warning  : Whisper expects 16 kHz");
    }

    let mut transcriber = VoiceTranscriber::from_makepad_env();
    let params = VoiceTranscribeParams::default();
    if let Err(err) = transcriber.preload(&params) {
        println!("preload failed: {err:?}");
        return;
    }

    let heard = match transcriber.transcribe(&samples, &params) {
        Ok(segments) => segments
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" "),
        Err(err) => {
            println!("transcribe failed: {err:?}");
            return;
        }
    };
    println!("heard    : {}", heard.trim());

    if !expected.is_empty() {
        println!("expected : {expected}");
        let (errors, words) = word_error(&expected, &heard);
        println!(
            "WER      : {:.1}%  ({errors}/{words} words)",
            100.0 * errors as f32 / words.max(1) as f32
        );
    }
}

/// 16-bit mono PCM WAV.
fn read_wav(path: &str) -> (Vec<f32>, u32) {
    let bytes = std::fs::read(path).expect("cannot read wav");
    let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let at = bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("no data chunk")
        + 8;
    let samples = bytes[at..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0)
        .collect();
    (samples, rate)
}

fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .flat_map(|w| match w.parse::<u64>() {
            Ok(n) => makepad_tts::g2p::spell_number(n)
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>(),
            Err(_) => vec![w.to_string()],
        })
        .collect()
}

fn word_error(said: &str, heard: &str) -> (usize, usize) {
    let reference = normalize(said);
    let hypothesis = normalize(heard);
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    let mut current = vec![0usize; hypothesis.len() + 1];
    for (i, want) in reference.iter().enumerate() {
        current[0] = i + 1;
        for (j, got) in hypothesis.iter().enumerate() {
            current[j + 1] = (previous[j] + usize::from(want != got))
                .min(current[j] + 1)
                .min(previous[j + 1] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    (previous[hypothesis.len()], reference.len())
}
