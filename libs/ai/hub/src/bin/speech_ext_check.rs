//! Checks the speech extensions through the sessions apps use, with real
//! models and recorded/synthesized audio only (no microphone):
//!
//! - voice per `say` (`say_with`): two voices in one TTS session,
//! - Whisper language auto-detect (`language = "auto"` -> `Transcript::language`),
//! - streaming partials (`feed` + `end_utterance` -> `SttEvent::Partial`, then `Final`).
//!
//! ```text
//! cd libs/ai/hub && cargo run --release --features stt,tts --bin speech-ext-check -- \
//!     [--voices bm_daniel,af_heart] [--text "Hello there."] [--interval-ms 500]
//! ```

use makepad_ai_hub::hub::AiHub;
use makepad_ai_hub::speech::{
    SayOptions, SpeechReach, SttConfig, SttEngine, SttEvent, TtsConfig, TtsEngine, TtsEvent, STT_SAMPLE_RATE,
};
use std::time::{Duration, Instant};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let voices: Vec<String> = arg(&args, "--voices")
        .unwrap_or_else(|| "bm_daniel,af_heart".into())
        .split(',')
        .map(str::to_string)
        .collect();
    let text = arg(&args, "--text").unwrap_or_else(|| "The quick brown fox jumps over the lazy dog near the river bank.".into());
    let zh_text = arg(&args, "--zh-text").unwrap_or_else(|| "你好，今天天气很好，我们一起去公园玩吧。".into());
    let interval: u32 = arg(&args, "--interval-ms").and_then(|v| v.parse().ok()).unwrap_or(500);
    let language = arg(&args, "--language").unwrap_or_else(|| "auto".into());

    let hub = AiHub::in_process();
    let tts = hub.start_tts(TtsConfig { engine: TtsEngine::Kokoro, reach: SpeechReach::Local, ..TtsConfig::default() });
    let stt = hub.start_stt(SttConfig {
        engine: SttEngine::Whisper,
        reach: SpeechReach::Local,
        language,
        partial_interval_ms: interval,
        ..SttConfig::default()
    });
    let deadline = Instant::now() + Duration::from_secs(600);
    let (mut tts_ready, mut stt_ready) = (false, false);
    while !(tts_ready && stt_ready) {
        for event in tts.poll() {
            match event {
                TtsEvent::Ready(info) => {
                    println!("tts: {} ({} voices)", info.engine, info.voices.len());
                    tts_ready = true;
                }
                TtsEvent::Failed(why) => fail(&format!("tts: {why}")),
                _ => {}
            }
        }
        for event in stt.poll() {
            match event {
                SttEvent::Ready(info) => {
                    println!("stt: {} partial_results={}", info.engine, info.capabilities.partial_results);
                    stt_ready = true;
                }
                SttEvent::Failed(why) => fail(&format!("stt: {why}")),
                _ => {}
            }
        }
        if Instant::now() > deadline {
            fail("engines did not come up");
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut clips = Vec::new();
    for voice in &voices {
        let started = Instant::now();
        // Mandarin voices (zf_/zm_) speak Chinese text; the rest the English one.
        let said = if voice.starts_with('z') { zh_text.clone() } else { text.clone() };
        let id = tts.say_with(said, SayOptions::voice(voice.clone()));
        let audio = wait_audio(&tts, id);
        println!("voice {voice}: {:.1}s audio in {:.1}s", audio.duration_secs(), started.elapsed().as_secs_f64());
        clips.push((voice.clone(), audio.resampled(STT_SAMPLE_RATE).samples));
    }
    if clips.len() > 1 && clips[0].1 == clips[1].1 {
        fail("two voices produced identical audio");
    }

    for (voice, pcm) in &clips {
        let started = Instant::now();
        let id = stt.transcribe(pcm.clone());
        let (heard, language) = wait_final(&stt, id);
        println!("[one shot, {voice}] language={language:?} in {:.1}s: {heard}", started.elapsed().as_secs_f64());
    }

    // Live: 250 ms chunks, as a capture callback would deliver them.
    let (voice, pcm) = &clips[0];
    let chunk = (STT_SAMPLE_RATE / 4) as usize;
    let started = Instant::now();
    let mut partials: Vec<(f64, String)> = Vec::new();
    for piece in pcm.chunks(chunk) {
        stt.feed(piece.to_vec());
        std::thread::sleep(Duration::from_millis(250));
        for event in stt.poll() {
            if let SttEvent::Partial(text) = event {
                partials.push((started.elapsed().as_secs_f64(), text));
            }
        }
    }
    let id = stt.end_utterance();
    let ended = started.elapsed().as_secs_f64();
    let (heard, language) = wait_final_collecting(&stt, id, &mut partials, started);
    println!("[live, {voice}] {} partials; speech ended at {ended:.1}s, final at {:.1}s language={language:?}", partials.len(), started.elapsed().as_secs_f64());
    for (at, text) in &partials {
        println!("  {at:5.1}s partial: {text}");
    }
    println!("  final: {heard}");
    if partials.is_empty() {
        fail("no partial arrived while speech was fed");
    }
}

fn fail(why: &str) -> ! {
    eprintln!("FAIL {why}");
    std::process::exit(1);
}

fn wait_audio(tts: &makepad_ai_hub::speech::TtsSession, id: u64) -> makepad_ai_hub::speech::SpeechAudio {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        for event in tts.poll() {
            match event {
                TtsEvent::Audio { utterance, audio, .. } if utterance == id => return audio,
                TtsEvent::Error { message, .. } => fail(&format!("tts error: {message}")),
                _ => {}
            }
        }
        if Instant::now() > deadline {
            fail("tts timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_final(stt: &makepad_ai_hub::speech::SttSession, id: u64) -> (String, Option<String>) {
    wait_final_collecting(stt, id, &mut Vec::new(), Instant::now())
}

fn wait_final_collecting(
    stt: &makepad_ai_hub::speech::SttSession,
    id: u64,
    partials: &mut Vec<(f64, String)>,
    started: Instant,
) -> (String, Option<String>) {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        for event in stt.poll() {
            match event {
                SttEvent::Final { utterance, transcript, .. } if utterance == id => return (transcript.text(), transcript.language),
                SttEvent::Partial(text) => partials.push((started.elapsed().as_secs_f64(), text)),
                SttEvent::Error { message, .. } => fail(&format!("stt error: {message}")),
                _ => {}
            }
        }
        if Instant::now() > deadline {
            fail("stt timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
