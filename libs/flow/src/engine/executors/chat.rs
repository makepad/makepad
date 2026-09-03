use super::{string_param, Executor, Poll};
use crate::{Node, Value};
use std::collections::VecDeque;
#[cfg(feature = "hub-chat")]
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub trait ChatSeam: Send + Sync {
    fn start_turn(
        &self,
        system: &str,
        prompt: &str,
        model: &str,
    ) -> Result<Box<dyn ChatTurn>, String>;
}

pub trait ChatTurn {
    fn poll(&mut self) -> Vec<ChatEvent>;
    fn cancel(&mut self);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatEvent {
    Delta(String),
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
    ) -> Result<Box<dyn ChatTurn>, String> {
        use makepad_ai_hub::hub::{AiHub, ChatConfig};
        use makepad_ai_hub::local_llm::LocalLlmConfig;
        let path = if model.is_empty() {
            self.model_path.clone()
        } else {
            PathBuf::from(model)
        };
        let session = AiHub::in_process().start_local_chat(ChatConfig {
            llm: LocalLlmConfig::new(path),
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
                HubEvent::Loading { .. } | HubEvent::Ready { .. } => {}
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
    ) -> Result<Box<dyn ChatTurn>, String> {
        Err("makepad-flow was built without hub-chat".to_string())
    }
}
