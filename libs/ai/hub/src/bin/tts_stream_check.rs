//! Time to first sound: speak a text through a `TtsSession` with `stream` on and
//! print when each clause chunk and the final take arrive.
//!
//! ```text
//! cargo run --release --features tts,stt --bin tts-stream-check -- [--voice bm_daniel] [--remote] [text...]
//! ```
//! `--remote` forces the node path (no local weights are used).

use makepad_ai_hub::hub::AiHub;
use makepad_ai_hub::speech::{SpeechReach, TtsConfig, TtsEngine, TtsEvent};
use std::time::{Duration, Instant};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut voice = None;
    let mut remote = false;
    let mut text_parts = Vec::new();
    while !args.is_empty() {
        match args.remove(0).as_str() {
            "--voice" => voice = Some(args.remove(0)),
            "--remote" => remote = true,
            other => text_parts.push(other.to_string()),
        }
    }
    let text = if text_parts.is_empty() {
        "Hello there, how are you doing today? I am doing well, thank you for asking. The weather is nice.".to_string()
    } else {
        text_parts.join(" ")
    };
    let hub = AiHub::in_process();
    let tts = hub.start_tts(TtsConfig {
        engine: TtsEngine::Kokoro,
        reach: if remote { SpeechReach::Lan } else { SpeechReach::Local },
        voice,
        stream: true,
        ..TtsConfig::default()
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match tts.recv_timeout(Duration::from_millis(50)) {
            Some(TtsEvent::Ready(info)) => {
                println!("engine: {} remote={:?}", info.engine, info.remote);
                break;
            }
            Some(TtsEvent::Failed(why)) => panic!("tts failed: {why}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "no engine");
    }
    // One warm-up utterance so the numbers are not the cold load.
    let warm = tts.say("Warm up.");
    loop {
        if let Some(TtsEvent::Audio { utterance, .. }) = tts.recv_timeout(Duration::from_millis(50)) {
            if utterance == warm {
                break;
            }
        }
    }
    let started = Instant::now();
    let id = tts.say(text.clone());
    let mut first = None;
    let mut chunks = 0;
    loop {
        match tts.recv_timeout(Duration::from_millis(5)) {
            Some(TtsEvent::AudioChunk { utterance, index, audio }) if utterance == id => {
                let at = started.elapsed().as_secs_f64();
                first.get_or_insert(at);
                chunks += 1;
                println!("chunk {index}: {:.2}s of audio at {at:.3}s", audio.duration_secs());
            }
            Some(TtsEvent::Audio { utterance, audio, .. }) if utterance == id => {
                println!(
                    "final: {:.2}s of audio at {:.3}s; first sound at {:.3}s ({} chunks)",
                    audio.duration_secs(),
                    started.elapsed().as_secs_f64(),
                    first.unwrap_or(f64::NAN),
                    chunks
                );
                break;
            }
            Some(TtsEvent::Error { message, .. }) => panic!("{message}"),
            _ => {}
        }
        assert!(started.elapsed() < Duration::from_secs(120), "timeout");
    }
}
