//! Chat turns through the same client apps use (`HubChatSession`), one after
//! another, each printed with what came back: an answer, or the error the
//! person would see. Used to check that a turn sent while the hub gives GPU
//! memory to the desktop (a yield, a reload, a relief) is still answered.
//!
//! ```text
//! MAKEPAD_AI_HUB=127.0.0.1:8131 hub-chat-probe <model> <turns> [gap_ms]
//! ```
//!
//! Exit code 0 when every turn ended with an answer, 1 otherwise.

use makepad_ai_hub::hub_chat::{HubChatConfig, HubChatSession};
use makepad_ai_hub::local_llm::{ChatEvent, LocalLlmConfig};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let model = args.first().cloned().unwrap_or_else(|| "qwen3.8-27b".into());
    let turns: usize = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(3);
    let gap = Duration::from_millis(args.get(2).and_then(|n| n.parse().ok()).unwrap_or(1000));
    let chat = HubChatSession::start(HubChatConfig {
        llm: LocalLlmConfig::new(std::path::PathBuf::new()),
        preferred_model: Some(model),
        max_tokens: Some(16),
        thinking: Some(false),
        system_prompt: "Reply in a few words.".into(),
        tools: Vec::new(),
        wake: None,
    });
    let start = Instant::now();
    let mut answered = 0;
    for turn in 0..turns {
        if turn > 0 {
            std::thread::sleep(gap);
        }
        println!("{:7.2}s turn {turn}: hi", start.elapsed().as_secs_f64());
        chat.send_user_turn("hi".into());
        let mut text = String::new();
        let outcome = 'wait: loop {
            for event in chat.poll() {
                match event {
                    ChatEvent::Loading { phase, .. } => println!("{:7.2}s   ... {phase}", start.elapsed().as_secs_f64()),
                    ChatEvent::Delta(d) => text.push_str(&d),
                    ChatEvent::TurnDone { .. } => break 'wait format!("answered: {:?}", text.trim()),
                    ChatEvent::Failed(why) => break 'wait format!("FAILED: {why}"),
                    ChatEvent::ContextFull => break 'wait "FAILED: context full".to_string(),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if outcome.starts_with("answered") {
            answered += 1;
        }
        println!("{:7.2}s turn {turn}: {outcome}", start.elapsed().as_secs_f64());
    }
    println!("{answered}/{turns} answered");
    std::process::exit(if answered == turns { 0 } else { 1 });
}
