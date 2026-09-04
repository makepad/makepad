use super::{param, string_param, Executor, Poll};
use crate::{Literal, Node, Value};
use std::collections::VecDeque;
#[cfg(feature = "hub-chat")]
use std::path::PathBuf;
use std::time::{Duration, Instant};
#[cfg(feature = "hub-chat")]
#[path = "chat_providers.rs"]
mod chat_providers;

pub trait ChatSeam: Send + Sync {
    fn start_turn(
        &self,
        system: &str,
        prompt: &str,
        model: &str,
        max_tokens: Option<u32>,
        thinking: Option<bool>,
    ) -> Result<Box<dyn ChatTurn>, String>;
}

pub trait ChatTurn {
    fn poll(&mut self) -> Vec<ChatEvent>;
    fn cancel(&mut self);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatEvent {
    Delta(String),
    Progress { permille: u16, stage: String },
    Done { text: String },
    Failed(String),
}

pub struct ChatExecutor {
    seam: std::sync::Arc<dyn ChatSeam>,
    turn: Option<Box<dyn ChatTurn>>,
    queue: VecDeque<Poll>,
    text: String,
    pending_delta: String,
    last_delta: Instant,
}

impl ChatExecutor {
    pub fn new(seam: std::sync::Arc<dyn ChatSeam>) -> Self {
        Self {
            seam,
            turn: None,
            queue: VecDeque::new(),
            text: String::new(),
            pending_delta: String::new(),
            last_delta: Instant::now(),
        }
    }

    fn flush_delta(&mut self) {
        while !self.pending_delta.is_empty() {
            let mut end = self.pending_delta.len().min(4096);
            while !self.pending_delta.is_char_boundary(end) {
                end -= 1;
            }
            let chunk = self.pending_delta[..end].to_string();
            self.pending_delta.drain(..end);
            self.queue.push_back(Poll::Delta {
                port: "text".to_string(),
                text: chunk,
            });
        }
        self.last_delta = Instant::now();
    }
}

impl Executor for ChatExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        let prompt = inputs
            .iter()
            .find_map(|(port, value)| (port == "prompt").then_some(value.as_text()))
            .transpose()?
            .unwrap_or("");
        self.turn = Some(self.seam.start_turn(
            &string_param(node, "system"),
            prompt,
            &string_param(node, "model"),
            positive_u32_param(node, "max_tokens"),
            Some(false),
        )?);
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        if let Some(event) = self.queue.pop_front() {
            return event;
        }
        let Some(turn) = self.turn.as_mut() else {
            return Poll::Pending;
        };
        let events = turn.poll();
        for event in events {
            match event {
                ChatEvent::Progress { permille, stage } => {
                    self.queue.push_back(Poll::Progress { permille, stage });
                }
                ChatEvent::Delta(delta) => {
                    self.text.push_str(&delta);
                    self.pending_delta.push_str(&delta);
                    if self.pending_delta.len() >= 4096
                        || self.last_delta.elapsed() >= Duration::from_millis(50)
                    {
                        self.flush_delta();
                    }
                }
                ChatEvent::Done { text } => {
                    if !text.is_empty() {
                        self.text = text;
                    }
                    self.flush_delta();
                    self.queue.push_back(Poll::Done(vec![(
                        "text".to_string(),
                        Value::text(&self.text),
                    )]));
                }
                ChatEvent::Failed(error) => {
                    self.flush_delta();
                    self.queue.push_back(Poll::Failed(error));
                }
            }
        }
        if !self.pending_delta.is_empty()
            && self.last_delta.elapsed() >= Duration::from_millis(50)
        {
            self.flush_delta();
        }
        self.queue.pop_front().unwrap_or(Poll::Pending)
    }

    fn cancel(&mut self) {
        if let Some(turn) = self.turn.as_mut() {
            turn.cancel();
        }
    }
}

fn positive_u32_param(node: &Node, name: &str) -> Option<u32> {
    match param(node, name) {
        Some(Literal::Num(value))
            if value.is_finite() && *value > 0.0 && *value <= u32::MAX as f64 =>
        {
            Some(*value as u32)
        }
        _ => None,
    }
}

#[cfg(feature = "hub-chat")]
pub struct HubChat {
    pub model_path: PathBuf,
}

#[cfg(feature = "hub-chat")]
impl HubChat {
    pub fn from_env() -> Self {
        Self {
            model_path: std::env::var_os("MAKEPAD_FLOW_LLM_MODEL")
                .map(PathBuf::from)
                .unwrap_or_default(),
        }
    }
}

#[cfg(feature = "hub-chat")]
impl ChatSeam for HubChat {
    fn start_turn(
        &self,
        system: &str,
        prompt: &str,
        model: &str,
        max_tokens: Option<u32>,
        thinking: Option<bool>,
    ) -> Result<Box<dyn ChatTurn>, String> {
        if let Some(slug) = model.strip_prefix("provider:") {
            let mut provider = chat_providers::ProviderAdapter::new(slug, max_tokens)?;
            provider.begin(system, prompt)?;
            return Ok(Box::new(ProviderChatTurn { provider: Box::new(provider) }));
        }
        use makepad_ai_hub::hub_chat::{HubChatConfig, HubChatSession};
        use makepad_ai_hub::local_llm::LocalLlmConfig;
        let (path, preferred_model) = resolve_model(
            model,
            &self.model_path,
            &makepad_ai_hub::home::weights_dir(),
        );
        let session = HubChatSession::start(HubChatConfig {
            llm: LocalLlmConfig::new(path),
            preferred_model,
            max_tokens,
            thinking,
            system_prompt: system.to_string(),
            tools: Vec::new(),
            wake: None,
        });
        session.send_user_turn(prompt.to_string());
        Ok(Box::new(HubChatTurn {
            session,
            text: String::new(),
        }))
    }
}

#[cfg(feature = "hub-chat")]
struct ProviderChatTurn { provider: Box<dyn chat_providers::ProviderSession> }

#[cfg(feature = "hub-chat")]
impl ChatTurn for ProviderChatTurn {
    fn poll(&mut self) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        for event in self.provider.poll() {
            let terminal = matches!(event, makepad_ai_hub::providers::provider::ProviderEvent::Done { .. } | makepad_ai_hub::providers::provider::ProviderEvent::Error(_) | makepad_ai_hub::providers::provider::ProviderEvent::FunctionCall { .. });
            out.push(match event {
                makepad_ai_hub::providers::provider::ProviderEvent::Delta(text) => ChatEvent::Delta(text),
                makepad_ai_hub::providers::provider::ProviderEvent::Status { note, permille } => ChatEvent::Progress { permille, stage: note },
                makepad_ai_hub::providers::provider::ProviderEvent::Done { text } => ChatEvent::Done { text },
                makepad_ai_hub::providers::provider::ProviderEvent::Error(error) => ChatEvent::Failed(error),
                makepad_ai_hub::providers::provider::ProviderEvent::FunctionCall { .. } => { self.provider.cancel(); ChatEvent::Failed("flow Llm nodes do not expose tools".into()) },
                makepad_ai_hub::providers::provider::ProviderEvent::Serving(_) => ChatEvent::Progress { permille: 0, stage: "serving".into() },
            });
            if terminal { break; }
        }
        out
    }
    fn cancel(&mut self) { self.provider.cancel(); }
}

#[cfg(feature = "hub-chat")]
impl Drop for ProviderChatTurn {
    fn drop(&mut self) { self.provider.cancel(); }
}

#[cfg(feature = "hub-chat")]
fn resolve_model(model: &str, fallback: &std::path::Path, weights: &std::path::Path) -> (PathBuf, Option<String>) {
    let model = model.trim();
    if model.is_empty() {
        return (fallback.to_path_buf(), None);
    }
    let named = PathBuf::from(model);
    let candidate = if named.is_absolute() { named } else { weights.join(named) };
    if candidate.is_file() {
        (candidate, None)
    } else {
        (PathBuf::new(), Some(model.to_string()))
    }
}

#[cfg(all(test, feature = "hub-chat"))]
mod model_tests {
    use super::resolve_model;
    use std::path::PathBuf;

    #[test]
    fn empty_file_and_fleet_model_values_take_distinct_routes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("chat-model-test-{}", std::process::id()));
        let weights = root.join("weights");
        std::fs::create_dir_all(&weights).unwrap();
        let local = weights.join("local.gguf");
        std::fs::write(&local, b"fixture").unwrap();
        let fallback = root.join("fallback.gguf");

        assert_eq!(resolve_model("", &fallback, &weights), (fallback, None));
        assert_eq!(
            resolve_model("local.gguf", std::path::Path::new(""), &weights),
            (local.clone(), None)
        );
        assert_eq!(
            resolve_model(local.to_str().unwrap(), std::path::Path::new(""), &weights),
            (local, None)
        );
        assert_eq!(
            resolve_model("qwen3.8-27b", std::path::Path::new(""), &weights),
            (PathBuf::new(), Some("qwen3.8-27b".to_string()))
        );

        // A weights-relative file deliberately wins over an identically
        // spelled fleet id; callers can select the fleet id by removing or
        // renaming the colliding local file.
        let collision = weights.join("qwen3.8-27b");
        std::fs::write(&collision, b"fixture").unwrap();
        assert_eq!(
            resolve_model("qwen3.8-27b", std::path::Path::new(""), &weights),
            (collision, None)
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(feature = "hub-chat")]
struct HubChatTurn {
    session: makepad_ai_hub::hub_chat::HubChatSession,
    text: String,
}

#[cfg(feature = "hub-chat")]
impl ChatTurn for HubChatTurn {
    fn poll(&mut self) -> Vec<ChatEvent> {
        use makepad_ai_hub::local_llm::ChatEvent as HubEvent;
        let mut out = Vec::new();
        for event in self.session.poll() {
            match event {
                HubEvent::Delta(text) => {
                    self.text.push_str(&text);
                    out.push(ChatEvent::Delta(text));
                }
                HubEvent::TurnDone { .. } => out.push(ChatEvent::Done {
                    text: self.text.clone(),
                }),
                HubEvent::Failed(error) => out.push(ChatEvent::Failed(error)),
                HubEvent::ContextFull => {
                    out.push(ChatEvent::Failed("context full".to_string()))
                }
                HubEvent::Loading { phase, fraction } => out.push(loading_progress(phase, fraction)),
                HubEvent::Ready { .. } => {}
                HubEvent::ToolCall { .. } => out.push(ChatEvent::Failed(
                    "flow Llm nodes do not expose tools".to_string(),
                )),
            }
        }
        out
    }

    fn cancel(&mut self) {
        self.session.cancel();
    }
}

#[cfg(feature = "hub-chat")]
fn loading_progress(stage: String, fraction: f64) -> ChatEvent {
    ChatEvent::Progress { stage, permille: (fraction.clamp(0.0, 1.0) * 1000.0) as u16 }
}

#[cfg(feature = "hub-chat")]
pub fn provider_model_rows() -> Vec<crate::ModelInfoDto> {
    chat_providers::PROVIDER_SLUGS
        .into_iter()
        .filter_map(|slug| {
            let mut provider = chat_providers::ProviderAdapter::new(slug, None).ok()?;
            let (available, state, note) = match provider.availability() {
                makepad_ai_hub::chat_wire::ProviderAvailability::Available { model, detail } =>
                    (true, format!("available:{model}"), Some(detail)),
                makepad_ai_hub::chat_wire::ProviderAvailability::Unavailable { reason } =>
                    (false, "unavailable".to_string(), Some(reason)),
            };
            Some(crate::ModelInfoDto { id: format!("provider:{slug}"), domain: "text".into(), backend: slug.into(), node: "flow-host".into(), available, gated: false, state, vram_gb: None, note })
        })
        .collect()
}

#[cfg(all(test, feature = "hub-chat"))]
mod provider_turn_tests {
    use super::*;
    use makepad_ai_hub::chat_wire::ProviderAvailability;
    use makepad_ai_hub::providers::provider::ProviderEvent;

    struct Mock { events: Vec<ProviderEvent>, cancelled: bool, tools_enabled: Option<bool> }
    impl chat_providers::ProviderSession for Mock {
        fn begin(&mut self, _system: &str, _prompt: &str) -> Result<(), String> { self.tools_enabled = Some(false); Ok(()) }
        fn poll(&mut self) -> Vec<ProviderEvent> { std::mem::take(&mut self.events) }
        fn cancel(&mut self) { self.cancelled = true; }
        fn availability(&mut self) -> ProviderAvailability { ProviderAvailability::Available { model: "mock".into(), detail: String::new() } }
    }

    #[test]
    fn adapter_maps_events_and_stops_after_terminal_or_function_call() {
        let mut turn = ProviderChatTurn { provider: Box::new(Mock { events: vec![ProviderEvent::Delta("a".into()), ProviderEvent::FunctionCall { call_id: "x".into(), name: "tool".into(), arguments: "{}".into() }, ProviderEvent::Delta("late".into())], cancelled: false, tools_enabled: None }) };
        let events = turn.poll();
        assert!(matches!(&events[..], [ChatEvent::Delta(a), ChatEvent::Failed(e)] if a == "a" && e.contains("do not expose")));
        assert!(turn.provider.poll().is_empty());
    }

    #[test]
    fn adapter_drop_cancels_provider() {
        struct DropProbe(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl chat_providers::ProviderSession for DropProbe { fn begin(&mut self, _: &str, _: &str)->Result<(),String>{Ok(())} fn poll(&mut self)->Vec<ProviderEvent>{vec![]} fn cancel(&mut self){self.0.store(true,std::sync::atomic::Ordering::SeqCst)} fn availability(&mut self)->ProviderAvailability{ProviderAvailability::Unavailable{reason:String::new()}} }
        let flag=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe=flag.clone();
        let turn=ProviderChatTurn{provider:Box::new(DropProbe(probe))};
        drop(turn);
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }
}

#[cfg(not(feature = "hub-chat"))]
pub struct HubChat;

#[cfg(not(feature = "hub-chat"))]
impl HubChat {
    pub fn from_env() -> Self {
        Self
    }
}

#[cfg(not(feature = "hub-chat"))]
impl ChatSeam for HubChat {
    fn start_turn(
        &self,
        _system: &str,
        _prompt: &str,
        _model: &str,
        _max_tokens: Option<u32>,
        _thinking: Option<bool>,
    ) -> Result<Box<dyn ChatTurn>, String> {
        Err("makepad-flow was built without hub-chat".to_string())
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    struct UnusedSeam;
    impl ChatSeam for UnusedSeam {
        fn start_turn(&self, _: &str, _: &str, _: &str, _: Option<u32>, _: Option<bool>) -> Result<Box<dyn ChatTurn>, String> {
            unreachable!()
        }
    }
    struct ScriptedTurn(VecDeque<Vec<ChatEvent>>);
    impl ChatTurn for ScriptedTurn {
        fn poll(&mut self) -> Vec<ChatEvent> { self.0.pop_front().unwrap_or_default() }
        fn cancel(&mut self) { self.0.clear(); }
    }

    #[test]
    fn waiting_progress_then_output_is_forwarded_once() {
        let mut executor = ChatExecutor::new(std::sync::Arc::new(UnusedSeam));
        executor.turn = Some(Box::new(ScriptedTurn(VecDeque::from([
            vec![ChatEvent::Progress { permille: 0, stage: "waiting for admission: http 409: busy".into() }],
            vec![ChatEvent::Progress { permille: 250, stage: "loading model".into() }],
            vec![ChatEvent::Delta("answer".into()), ChatEvent::Done { text: "answer".into() }],
        ]))));
        assert!(matches!(executor.poll(), Poll::Progress { permille: 0, stage } if stage.contains("waiting") && stage.contains("409")));
        assert!(matches!(executor.poll(), Poll::Progress { permille: 250, stage } if stage == "loading model"));
        assert!(matches!(executor.poll(), Poll::Delta { text, .. } if text == "answer"));
        assert!(matches!(executor.poll(), Poll::Done(values) if values[0].1.as_text().unwrap() == "answer"));
        assert!(matches!(executor.poll(), Poll::Pending));
    }

    #[test]
    fn admission_error_reason_survives_progress_mapping() {
        let mut executor = ChatExecutor::new(std::sync::Arc::new(UnusedSeam));
        executor.turn = Some(Box::new(ScriptedTurn(VecDeque::from([vec![
            ChatEvent::Progress { permille: 0, stage: "waiting for admission".into() },
            ChatEvent::Failed("http 503: weights unavailable".into()),
        ]]))));
        assert!(matches!(executor.poll(), Poll::Progress { .. }));
        assert!(matches!(executor.poll(), Poll::Failed(error) if error == "http 503: weights unavailable"));
        assert!(matches!(executor.poll(), Poll::Pending));
    }

    #[cfg(feature = "hub-chat")]
    #[test]
    fn hub_loading_maps_to_bounded_flow_progress() {
        for (fraction, expected) in [(0.0, 0), (0.25, 250), (2.0, 1000), (-1.0, 0), (f64::NAN, 0)] {
            assert_eq!(loading_progress("queued".into(), fraction), ChatEvent::Progress {
                permille: expected, stage: "queued".into(),
            });
        }
    }
}
