//! What the serving box does with the generation session's ACTUAL system
//! text. Ignored by default; it needs a live fleet box.
//!
//! ```text
//! MAKEPAD_FLEET_BASE=http://10.0.0.165:8123 \
//! cargo test -p makepad-asset-chat --test live_fleet_probe -- --ignored --nocapture
//! ```
//!
//! It answers one question a model's self-report cannot: given the tool
//! table we render, does the box emit `image.generate` for "make me a
//! picture of a unicorn"?

use makepad_asset_chat::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_asset_chat::qwen::{FleetQwenChatProvider, HttpFleetTransport};
use makepad_asset_chat::toolcall;
use makepad_asset_chat::tools;
use makepad_asset_chat::wire::{ChatMessage, ChatRole};
use std::time::{Duration, Instant};

fn run(label: &str, capabilities: &str) -> String {
    run_with_history(label, capabilities, Vec::new())
}

fn run_with_history(
    label: &str,
    capabilities: &str,
    history: Vec<ChatMessage>,
) -> String {
    let base = std::env::var("MAKEPAD_FLEET_BASE").expect("MAKEPAD_FLEET_BASE");
    let mut provider = FleetQwenChatProvider::new(HttpFleetTransport, vec![base]);
    let system = toolcall::render_system(&tools::definitions(), capabilities);
    let mut messages = history;
    messages.push(ChatMessage::new(ChatRole::User, "make me a picture of a unicorn"));
    let input = TurnInput {
        system,
        messages,
        tools_enabled: true,
    };
    provider.begin_turn(&input).expect("begin turn");
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut text = String::new();
    while Instant::now() < deadline {
        for ev in provider.poll() {
            match ev {
                ProviderEvent::Delta(t) => text.push_str(&t),
                ProviderEvent::Done { text: t } => {
                    let out = if t.is_empty() { text.clone() } else { t };
                    println!("=== {label} ===\n{out}\n");
                    return out;
                }
                ProviderEvent::Error(e) => panic!("{label}: provider error {e}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{label}: timed out; partial={text}");
}

#[test]
#[ignore = "needs a live fleet box (MAKEPAD_FLEET_BASE)"]
fn the_box_calls_image_generate_from_our_tool_table() {
    let out = run("bare table", "");
    assert!(
        out.contains("image.generate"),
        "the box ignored the advertised tool table"
    );
}

/// The failure the user hit: a worker-less server, a few turns of ordinary
/// chat first, and the model answers "I don't have an image-generation tool
/// available in this session" instead of calling the tool that is sitting in
/// its own list. The capability doc's wall of UNAVAILABLE is what did it —
/// so the generation profile now says, before that list, that the generate
/// tools are a different path and are live.
#[test]
#[ignore = "needs a live fleet box (MAKEPAD_FLEET_BASE)"]
fn an_offline_operation_worker_does_not_silence_the_generate_tools() {
    // The real assembled context of a `gen` session on a server whose
    // operation worker is down.
    let dynamic = format!(
        "LIVE STORE right now (kind count): mesh 4004, image 271, audio 245\n\n{}{}",
        "The *.generate tools (image, video, audio, speech, music, mesh, world, \
         character) run on the CONNECTED APP's fleet and are AVAILABLE in this \
         session. The registered operations below are a separate, optional path \
         for deriving from an existing asset; their worker status never applies \
         to the generate tools.\n",
        "Registered operations (for operation.create):\n\
         - mesh.from_image.v1 [UNAVAILABLE: the worker is currently offline]: image to mesh\n\
         - depth.from_image.v1 [UNAVAILABLE: the worker is currently offline]: image to depth\n\
         Operations are created in namespace 'gen'. Inputs must be exact revisions \
         bound to this session.\n",
    );
    let caps = makepad_asset_chat::context::assemble(
        makepad_asset_chat::context::ClientProfile::Gen,
        &dynamic,
    );
    // The same warm-up the user's window had before it refused.
    let history = vec![
        ChatMessage::new(ChatRole::User, "hi qwen"),
        ChatMessage::new(ChatRole::Assistant, "Hi! I'm Qwen. How can I help you today?"),
        ChatMessage::new(ChatRole::User, "test"),
        ChatMessage::new(
            ChatRole::Assistant,
            "All good — I'm here and responsive. What can I do for you?",
        ),
    ];
    let out = run_with_history("worker offline, warmed-up chat", &caps, history);
    assert!(
        out.contains("image.generate"),
        "an offline operation worker talked the model out of generating"
    );
}
