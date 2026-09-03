//! The dev-only `testpattern` path (`FLOW_GEN_BASE_URL=testpattern`): the
//! hub's `testpattern` image model served in-process on a private fleet
//! name, and a chat seam that streams a deterministic paragraph — so the
//! whole picture (tokens, progress, the picture landing) can be exercised
//! with no fleet on the LAN. Never used unless the knob is set.

use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::peer_serve::PeerOptions;
use makepad_ai_hub::registry::{Domain, ModelSpec, Registry};
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_flow::engine::executors::chat::ChatSeam;
use makepad_flow::engine::{ChatEvent, ChatTurn};
use std::time::Instant;

/// A fleet name no frontend listens for, so the LAN beacon this service
/// sends never reaches the user's `gen` fleet pickers.
const FLEET: &str = "flow-testpattern";
const WORDS_PER_SECOND: f64 = 18.0;
const FIRST_TOKEN_SECS: f64 = 0.4;

/// Start the hub service with the `testpattern` image model on a loopback
/// port; returns its base URL. The service has no stop message: its
/// threads belong to this process and end with it, as in the engine tests.
pub fn start_service_url() -> Result<String, String> {
    let cache_dir = std::env::temp_dir().join(format!(
        "makepad-flow-ui-testpattern-{}",
        std::process::id()
    ));
    let downloader = Downloader::new("http://127.0.0.1:1", None).map_err(|error| error.to_string())?;
    let handle = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir,
        registry: Registry {
            models: vec![ModelSpec {
                id: "testpattern".to_string(),
                domain: Domain::Image,
                backend: "testpattern".to_string(),
                available: true,
                gated: false,
                vram_gb: Some(0.0),
                min_vram_gb: None,
                min_compute_cap: None,
                note: None,
                license: None,
                files: Vec::new(),
            }],
        },
        downloader,
        peer: PeerOptions {
            serve: Some(false),
            sources: Some(Vec::new()),
            ..Default::default()
        },
        fleet: FLEET.to_string(),
    })
    .map_err(|error| error.to_string())?;
    let url = format!("http://{}", handle.addr);
    std::mem::forget(handle);
    Ok(url)
}

/// Streams one vivid paragraph built from the prompt, word by word.
pub struct TestpatternChat;

impl ChatSeam for TestpatternChat {
    fn start_turn(
        &self,
        _system: &str,
        prompt: &str,
        _model: &str,
        _max_tokens: Option<u32>,
        _thinking: Option<bool>,
    ) -> Result<Box<dyn ChatTurn>, String> {
        let subject = prompt.trim().trim_end_matches('.');
        let subject = if subject.is_empty() { "an empty scene" } else { subject };
        let text = format!(
            "{subject}. Late light rakes across the scene from low on the left, warm and long, \
             while the sky above cools to violet; a 35 mm lens sits close and wide, so the \
             foreground looms and the horizon falls away. Surfaces keep their grain — wet stone, \
             brushed metal, worn paint — and a thin haze softens the far edges. The mood is quiet \
             and expectant, a held breath before the last of the light goes."
        );
        Ok(Box::new(Turn {
            words: text.split_inclusive(' ').map(str::to_string).collect(),
            next: 0,
            started: Instant::now(),
            done: false,
            cancelled: false,
        }))
    }
}

struct Turn {
    words: Vec<String>,
    next: usize,
    started: Instant,
    done: bool,
    cancelled: bool,
}

impl ChatTurn for Turn {
    fn poll(&mut self) -> Vec<ChatEvent> {
        if self.done {
            return Vec::new();
        }
        if self.cancelled {
            self.done = true;
            return vec![ChatEvent::Failed("cancelled".to_string())];
        }
        let elapsed = (self.started.elapsed().as_secs_f64() - FIRST_TOKEN_SECS).max(0.0);
        let due = ((elapsed * WORDS_PER_SECOND) as usize).min(self.words.len());
        let mut out = Vec::new();
        while self.next < due {
            out.push(ChatEvent::Delta(self.words[self.next].clone()));
            self.next += 1;
        }
        if self.next >= self.words.len() {
            self.done = true;
            out.push(ChatEvent::Done {
                text: self.words.concat(),
            });
        }
        out
    }

    fn cancel(&mut self) {
        self.cancelled = true;
    }
}
