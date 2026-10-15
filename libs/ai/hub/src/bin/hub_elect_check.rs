//! Which hub does this machine's client election pick for chat, speech-to-text
//! and text-to-speech? Prints the elected node per capability (no audio device).
//!
//! ```text
//! MAKEPAD_AI_HUB=10.0.0.149:8123 cargo run --release --features llm,stt,tts --bin hub-elect-check
//! ```

use makepad_ai_hub::hub::AiHub;
use makepad_ai_hub::hub_chat::{HubChatConfig, HubChatSession};
use makepad_ai_hub::local_llm::{ChatEvent, LocalLlmConfig};
use makepad_ai_hub::speech::{SpeechReach, SttConfig, SttEngine, SttEvent, TtsConfig, TtsEngine, TtsEvent};
use std::time::{Duration, Instant};

fn main() {
    println!("pinned: {:?}", std::env::var("MAKEPAD_AI_HUB").ok());
    {
        let discovery = makepad_ai_hub::discovery::start_listener();
        std::thread::sleep(Duration::from_millis(4500));
        let mut bases: Vec<String> = discovery.nodes().into_iter().map(|n| n.base_url).collect();
        makepad_ai_hub::hub_order::order(&mut bases);
        for b in &bases {
            println!("  node {b}  tier {}", makepad_ai_hub::hub_order::tier(b));
        }
    }
    let chat = HubChatSession::start(HubChatConfig {
        llm: LocalLlmConfig::new(std::path::PathBuf::new()),
        preferred_model: Some("qwen3.8-27b".into()),
        max_tokens: Some(8),
        thinking: Some(false),
        system_prompt: "Reply with one word.".into(),
        tools: Vec::new(),
        wake: None,
    });
    chat.send_user_turn("Say ok.".into());
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut route = None;
    while Instant::now() < deadline && route.is_none() {
        for event in chat.poll() {
            if let ChatEvent::Failed(why) = event {
                println!("chat: FAILED {why}");
                route = Some("failed".to_string());
            }
        }
        route = route.or_else(|| chat.route().map(|r| r.label()));
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("chat elected: {}", route.unwrap_or_else(|| "none".into()));

    let hub = AiHub::in_process();
    let stt = hub.start_stt(SttConfig { engine: SttEngine::Whisper, reach: SpeechReach::Lan, ..SttConfig::default() });
    let tts = hub.start_tts(TtsConfig { engine: TtsEngine::Kokoro, reach: SpeechReach::Lan, ..TtsConfig::default() });
    let deadline = Instant::now() + Duration::from_secs(120);
    let (mut stt_done, mut tts_done) = (false, false);
    while Instant::now() < deadline && !(stt_done && tts_done) {
        for event in stt.poll() {
            match event {
                SttEvent::Ready(info) => {
                    println!("stt elected: {} remote={:?}", info.engine, info.remote);
                    stt_done = true;
                }
                SttEvent::Failed(why) => {
                    println!("stt: FAILED {why}");
                    stt_done = true;
                }
                _ => {}
            }
        }
        for event in tts.poll() {
            match event {
                TtsEvent::Ready(info) => {
                    println!("tts elected: {} remote={:?}", info.engine, info.remote);
                    tts_done = true;
                }
                TtsEvent::Failed(why) => {
                    println!("tts: FAILED {why}");
                    tts_done = true;
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
