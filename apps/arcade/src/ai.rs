//! The AI shell: which agent backend to talk to, and how an utterance gets
//! there (game.md §"AI tiers").
//!
//! Two tiers, one surface. The local judge — when this device can run one —
//! decides whether the mic even heard a request; the cloud agent is the only
//! thing that writes code. Where there is no local compute, the text box IS
//! the gate and typing goes straight through.

use crate::capability::{Capabilities, Tier};
use crate::library::{classify_deterministic, Action, Librarian, Library, Manifest};
use crate::pairing::{has_key, load_key, Provider};
use makepad_ai::agent::{Agent, StatelessBackendAdapter};
use makepad_ai::backend::BackendConfig;
use makepad_ai::backends::claude::ClaudeBackend;
use makepad_ai::backends::claude_code::ClaudeCodeAgent;
use makepad_ai::backends::gemini::GeminiBackend;
use makepad_ai::backends::openai::OpenAiBackend;
use makepad_converse::filter::{FilterDecision, PassthroughFilter, TranscriptFilter};
use makepad_converse::pipeline::ConversePipeline;

/// The system prompt is the whole game-authoring contract: the agent writes
/// splash against the `game.*` verbs and nothing else.
pub fn system_prompt(api: &str) -> String {
    format!(
        "You build small 3D games for kids in the Makepad Arcade engine.\n\
         You edit ONE file: game.splash. Write splash script that calls the \
         `game.*` verbs below — the engine owns physics, vehicles, characters, \
         AI brains and race logic, so never reimplement those.\n\
         Keep games short and readable. Prefer engine blocks (game.car, \
         game.character, game.plane, game.wander/chase/patrol, game.checkpoint/\
         race) over hand-written movement.\n\n\
         Verbs:\n{api}"
    )
}

/// Which backend this device should use, and why — the reason is worth showing
/// in settings, because "no key" and "no CLI" need different fixes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    Ready(Provider),
    /// A direct-API provider is selected but has no key yet: this device can
    /// still play and join, and in a hosted room it can still ASK the host's
    /// agent — it just cannot create on its own.
    NeedsKey(Provider),
    /// Nothing configured at all.
    None,
}

/// Prefer a CLI agent when one exists (it runs its own tool loop and needs no
/// key); otherwise fall back to whichever direct API has a key.
pub fn choose_backend(preferred: Option<Provider>) -> BackendChoice {
    if let Some(p) = preferred {
        if p == Provider::ClaudeCode && !ClaudeCodeAgent::is_available() {
            // Asked for the CLI but it is not installed — say so rather than
            // silently using something else.
            return BackendChoice::None;
        }
        return if has_key(p) {
            BackendChoice::Ready(p)
        } else {
            BackendChoice::NeedsKey(p)
        };
    }
    if ClaudeCodeAgent::is_available() {
        return BackendChoice::Ready(Provider::ClaudeCode);
    }
    for p in [Provider::Anthropic, Provider::OpenAi, Provider::Gemini] {
        if has_key(p) {
            return BackendChoice::Ready(p);
        }
    }
    BackendChoice::None
}

/// Default models per provider: small edits should not cost flagship prices
/// (game.md: "default small edits to cheaper models").
pub fn default_model(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode => "claude-sonnet-5",
        Provider::Anthropic => "claude-sonnet-5",
        Provider::OpenAi => "gpt-5",
        Provider::Gemini => "gemini-2.5-flash",
    }
}

pub fn build_agent(provider: Provider, model: Option<String>) -> Option<Box<dyn Agent>> {
    let model = model.unwrap_or_else(|| default_model(provider).to_string());
    Some(match provider {
        // The CLI runs its own tool loop, so it is an Agent directly.
        Provider::ClaudeCode => Box::new(ClaudeCodeAgent::new()) as Box<dyn Agent>,
        // The HTTP backends are stateless; the adapter owns the session and
        // executes tool calls on our side (the Quest/mobile path).
        Provider::Anthropic => Box::new(StatelessBackendAdapter::new(Box::new(
            ClaudeBackend::new(BackendConfig::Claude {
                api_key: Some(load_key(provider)?),
                oauth_token: None,
                model,
            }),
        ))) as Box<dyn Agent>,
        Provider::OpenAi => Box::new(StatelessBackendAdapter::new(Box::new(
            OpenAiBackend::new(BackendConfig::OpenAI {
                api_key: load_key(provider)?,
                model,
                base_url: None,
                reasoning_effort: None,
            }),
        ))) as Box<dyn Agent>,
        Provider::Gemini => Box::new(StatelessBackendAdapter::new(Box::new(
            GeminiBackend::new(BackendConfig::Gemini {
                api_key: load_key(provider)?,
                model,
            }),
        ))) as Box<dyn Agent>,
    })
}

/// Build the conversational pipeline for this device's tier.
///
/// The filter is constructed ON the worker thread (a local LLM session is not
/// Send), which is why this takes capabilities rather than a built filter.
pub fn build_pipeline(
    agent: Box<dyn Agent>,
    caps: &Capabilities,
    voice: &str,
) -> ConversePipeline {
    let tier = caps.tier();
    #[cfg(feature = "local-llm")]
    let qwen = caps.qwen_model.clone();
    let make_filter = move || -> Box<dyn TranscriptFilter> {
        #[cfg(feature = "local-llm")]
        {
            if matches!(tier, Tier::Voice) {
                if let Some(path) = qwen {
                    if let Some(f) = makepad_converse::qwen_filter::QwenFilter::new(&path) {
                        return Box::new(f);
                    }
                }
            }
        }
        // No judge available: every transcript passes. Safe because in this
        // tier the mic is push-to-talk, so the human is the gate.
        let _ = tier;
        Box::new(PassthroughFilter)
    };
    ConversePipeline::new(agent, make_filter, voice)
}

/// Route one utterance. Voice goes through the pipeline's filter; typed text
/// bypasses it (typing IS the gate) — but BOTH go through the librarian first,
/// so "play the racing game" never costs a cloud call.
pub struct Router {
    pub tier: Tier,
}

impl Router {
    /// Returns the action; `Action::AskAgent` means it must reach the cloud.
    pub fn route(
        &self,
        utterance: &str,
        library: &Library,
        current: Option<&Manifest>,
        librarian: Option<&mut dyn Librarian>,
    ) -> Action {
        crate::library::route(utterance, library, current, librarian)
    }
}

/// A transcript filter that defers to the deterministic librarian: anything
/// the librarian can answer locally is SKIPped from the agent's point of view,
/// because it never needs to reach the cloud at all.
pub struct LibrarianGate {
    pub library: Library,
    pub current: Option<Manifest>,
}

impl TranscriptFilter for LibrarianGate {
    fn judge(&mut self, utterance: &str, _recent_dialog: &[String]) -> FilterDecision {
        match classify_deterministic(utterance, &self.library, self.current.as_ref()) {
            Action::AskAgent(text) => FilterDecision::Forward { instruction: text },
            // Handled on-device; the cloud never sees it.
            other => FilterDecision::Drop {
                reason: format!("handled locally: {other:?}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_key_is_reported_not_silently_swapped() {
        let _guard = crate::pairing::tests::KEY_STORE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Point the key store at an empty dir so nothing is configured.
        let dir = std::env::temp_dir().join(format!("arcade-ai-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ARCADE_CONFIG_DIR", &dir);
        for var in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GOOGLE_API_KEY"] {
            std::env::remove_var(var);
        }
        assert_eq!(
            choose_backend(Some(Provider::Anthropic)),
            BackendChoice::NeedsKey(Provider::Anthropic)
        );
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
        assert_eq!(
            choose_backend(Some(Provider::Anthropic)),
            BackendChoice::Ready(Provider::Anthropic)
        );
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::fs::remove_dir_all(&dir).ok();
        std::env::remove_var("ARCADE_CONFIG_DIR");
    }

    #[test]
    fn the_librarian_gate_keeps_local_requests_off_the_wire() {
        let library = Library {
            games: vec![crate::library::GameEntry {
                dir: std::path::PathBuf::from("/g/racing"),
                manifest: Manifest {
                    name: "racing".into(),
                    description: "cars on a track".into(),
                    players: 4,
                    knobs: vec![],
                },
            }],
        };
        let mut gate = LibrarianGate { library, current: None };
        assert!(matches!(
            gate.judge("play racing", &[]),
            FilterDecision::Drop { .. }
        ));
        assert!(matches!(
            gate.judge("restart", &[]),
            FilterDecision::Drop { .. }
        ));
        match gate.judge("add a giant ramp over the lake", &[]) {
            FilterDecision::Forward { instruction } => assert!(instruction.contains("ramp")),
            other => panic!("creative work must reach the agent, got {other:?}"),
        }
    }

    #[test]
    fn system_prompt_carries_the_verb_table() {
        let p = system_prompt(&makepad_game_script::api_text());
        assert!(p.contains("game.car"), "prompt must list the blocks");
        assert!(p.contains("game.checkpoint"));
    }
}

#[cfg(test)]
mod pair_endpoint_tests {
    use crate::pair_server::{PairEvent, PairServer};
    use crate::pairing::{load_key, PairError, Pairing, Provider};
    use std::io::Write;
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    /// Drive the real /pair endpoint over HTTP (not a browser): the right code
    /// stores a key, the wrong one stores nothing.
    ///
    /// The server only answers while we poll it, so the client must not wait
    /// for a response — write, then pump until the event lands.
    #[test]
    fn pair_endpoint_accepts_the_right_code_and_refuses_the_wrong_one() {
        let _guard = crate::pairing::tests::KEY_STORE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("arcade-pair-http-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ARCADE_CONFIG_DIR", &dir);
        std::env::remove_var("OPENAI_API_KEY");

        // The server cannot report its bound port back (the app shows a fixed
        // one), so claim a free port first and hand it over.
        let port = {
            let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            probe.local_addr().unwrap().port()
        };
        let mut server = PairServer::start(Provider::OpenAi, 4242, port).expect("pair server");
        let code = server.pairing.code.clone();
        std::thread::sleep(Duration::from_millis(300));

        // Hold the connection open across the pump: dropping it early races
        // the server's body read.
        let conn = post(port, "code=0000&key=sk-should-not-store");
        let events = pump(&mut server);
        drop(conn);
        assert_eq!(
            events,
            vec![PairEvent::Rejected(PairError::WrongCode)],
            "wrong code must be rejected"
        );
        assert_eq!(load_key(Provider::OpenAi), None, "a wrong code stored a key");

        let conn = post(port, &format!("code={code}&key=sk-correct-value"));
        let events = pump(&mut server);
        drop(conn);
        assert_eq!(events, vec![PairEvent::Stored(Provider::OpenAi)]);
        assert_eq!(
            load_key(Provider::OpenAi).as_deref(),
            Some("sk-correct-value")
        );

        std::fs::remove_dir_all(&dir).ok();
        std::env::remove_var("ARCADE_CONFIG_DIR");
    }

    /// Write the request and hand back the live socket; the response needs our
    /// own poll loop, so the caller must hold this until after pumping.
    fn post(port: u16, body: &str) -> TcpStream {
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_write_timeout(Some(Duration::from_secs(2))).ok();
        // Headers and body go in SEPARATE writes on purpose: the server's
        // header parser wraps the socket in a BufReader, which swallows any
        // body bytes that arrive in the same segment (platform bug, noted in
        // the M4 report) — browsers happen to split them, so this mirrors a
        // real client rather than papering over it.
        let head = format!(
            "POST /pair HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).unwrap();
        s.flush().unwrap();
        std::thread::sleep(Duration::from_millis(120));
        s.write_all(body.as_bytes()).unwrap();
        s.flush().unwrap();
        s
    }

    fn pump(server: &mut PairServer) -> Vec<PairEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = server.poll();
            if !events.is_empty() {
                return events;
            }
            if Instant::now() > deadline {
                return events;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_pairing_page_is_served_and_self_contained() {
        let p = Pairing::new(Provider::Anthropic, 7);
        let html = p.page_html();
        // No external requests: a key must never transit a third party.
        assert!(!html.contains("http://") && !html.contains("https://"), "{html}");
        assert!(html.contains("action=/pair"));
    }
}
