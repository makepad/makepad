//! The in-process broker: the [`crate::ScoreChatBroker`] seam over the
//! session engine and the hub's providers (aicore P8). The store's chat
//! routes are gone; a score session is one provider conversation on this
//! machine, driven exactly like the broker drove it — same DTO event pages,
//! so the engine and every hermetic test upstream of the seam are untouched.
//!
//! Sessions are toolless (`ToolsOff`): score generation is prompt → MusicXML
//! extraction; the engine's repair loop lives above this seam.

use crate::{BrokerError, ScoreChatBroker};
use makepad_ai_hub::chat_wire::{Locality, ProviderKind};
use makepad_ai_hub::discovery;
use makepad_ai_hub::providers::provider::ChatProvider;
use makepad_ai_hub::providers::qwen::{FleetQwenChatProvider, HttpFleetTransport};
use makepad_asset_chat::session::{CancelFlag, ExecCtx, Session, ToolExecutor};
use makepad_asset_chat::tools::{ContentToolCall, ToolDef};
use makepad_asset_chat::wire::{
    ChatEventBody, ProviderAvailability, ToolOutcome,
};
use makepad_asset_client::dto::{
    ChatEventBodyDto, ChatEventDto, ChatEventsPageDto, ChatProviderDto, ChatProviderKind,
    ChatProviderLocality, ChatProviderStateDto, ChatSessionId,
};
use makepad_asset_client::{ChatCreateRequest, ChatSendRequest};
use std::collections::HashMap;
use std::sync::Mutex;

/// No tools: a refusal for anything a model tries.
struct ToolsOff;

impl ToolExecutor for ToolsOff {
    fn capability_doc(&mut self) -> String {
        "No tools are available in this session; answer directly.".to_string()
    }
    fn tool_definitions(&mut self) -> Vec<ToolDef> {
        Vec::new()
    }
    fn client_executes(&mut self, _call: &ContentToolCall) -> bool {
        false
    }
    fn execute(
        &mut self,
        _call: &ContentToolCall,
        _ctx: &ExecCtx,
        _progress: &mut dyn FnMut(u16, &str),
        _cancel: &CancelFlag,
    ) -> ToolOutcome {
        ToolOutcome::Unavailable { reason: "score sessions run without tools".to_string() }
    }
}

fn make_provider(kind: ChatProviderKind) -> Box<dyn ChatProvider> {
    match kind {
        ChatProviderKind::FleetQwen => {
            let bases: Vec<String> = discovery::start_listener()
                .nodes()
                .into_iter()
                .map(|n| n.base_url)
                .collect();
            Box::new(FleetQwenChatProvider::new(HttpFleetTransport, bases))
        }
        ChatProviderKind::OpenAi => Box::new(makepad_asset_chat::openai::from_env()),
        ChatProviderKind::Grok => Box::new(makepad_asset_chat::grok::from_env()),
        ChatProviderKind::ClaudeCli => {
            Box::new(makepad_asset_chat::claude::ClaudeCodeChatProvider::new(None))
        }
        ChatProviderKind::CodexCli => {
            Box::new(makepad_asset_chat::codex_cli::CodexCliChatProvider::new(None))
        }
        ChatProviderKind::GrokCli => {
            Box::new(makepad_asset_chat::grok_cli::GrokCliChatProvider::new(None))
        }
    }
}

fn wire_kind(kind: ChatProviderKind) -> ProviderKind {
    match kind {
        ChatProviderKind::FleetQwen => ProviderKind::FleetQwen,
        ChatProviderKind::OpenAi => ProviderKind::OpenAi,
        ChatProviderKind::Grok => ProviderKind::Grok,
        ChatProviderKind::ClaudeCli => ProviderKind::ClaudeCli,
        ChatProviderKind::CodexCli => ProviderKind::CodexCli,
        ChatProviderKind::GrokCli => ProviderKind::GrokCli,
    }
}

fn body_dto(body: ChatEventBody) -> ChatEventBodyDto {
    match body {
        ChatEventBody::Delta { text, .. } => ChatEventBodyDto::Delta { text, serving: None },
        ChatEventBody::ToolCall { id, name, args } => {
            ChatEventBodyDto::ToolCall { id, name, args }
        }
        ChatEventBody::ToolProgress { id, permille, note } => {
            ChatEventBodyDto::ToolProgress { id, permille, note }
        }
        ChatEventBody::ToolResult { id, outcome } => ChatEventBodyDto::ToolResult {
            id,
            outcome: crate::broker_outcome_dto(&outcome),
        },
        ChatEventBody::Done => ChatEventBodyDto::Done,
        ChatEventBody::Cancelled => ChatEventBodyDto::Cancelled,
        ChatEventBody::Error { code, message } => ChatEventBodyDto::Error { code, message },
    }
}

struct LiveSession {
    session: Session,
    /// The whole event log, DTO-shaped, indexed from 1 like the broker's.
    events: Vec<ChatEventDto>,
}

/// One in-process broker: sessions keyed exactly like the wire's.
#[derive(Default)]
pub struct LocalBroker {
    sessions: Mutex<HashMap<String, LiveSession>>,
}

impl LocalBroker {
    pub fn new() -> LocalBroker {
        LocalBroker::default()
    }

    fn pump(&self, id: &ChatSessionId) {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(live) = sessions.get_mut(&id.to_string()) {
            let mut tools = ToolsOff;
            live.session.pump(&mut tools);
            for event in live.session.drain_events() {
                let seq = live.events.len() as u64 + 1;
                live.events.push(ChatEventDto { seq, body: body_dto(event.body) });
            }
        }
    }
}

impl ScoreChatBroker for LocalBroker {
    fn providers(&self) -> Result<Vec<ChatProviderDto>, BrokerError> {
        Ok([
            ChatProviderKind::FleetQwen,
            ChatProviderKind::OpenAi,
            ChatProviderKind::Grok,
            ChatProviderKind::ClaudeCli,
            ChatProviderKind::CodexCli,
            ChatProviderKind::GrokCli,
        ]
        .into_iter()
        .map(|kind| {
            let locality = if wire_kind(kind).locality() == Locality::Cloud {
                ChatProviderLocality::Cloud
            } else {
                ChatProviderLocality::Local
            };
            let state = match make_provider(kind).availability() {
                ProviderAvailability::Available { model, .. } => {
                    ChatProviderStateDto::Available { model }
                }
                ProviderAvailability::Unavailable { reason } => {
                    ChatProviderStateDto::Unavailable { reason }
                }
            };
            ChatProviderDto { kind, locality, state }
        })
        .collect())
    }

    fn create(&self, request: &ChatCreateRequest) -> Result<ChatSessionId, BrokerError> {
        let provider = make_provider(request.provider);
        if let ProviderAvailability::Unavailable { reason } =
            make_provider(request.provider).availability()
        {
            return Err(BrokerError::new(reason));
        }
        let session = Session::new("score", provider);
        let id = ChatSessionId::parse(session.id().as_str())
            .ok_or_else(|| BrokerError::new("session id shape"))?;
        self.sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), LiveSession { session, events: Vec::new() });
        Ok(id)
    }

    fn send(
        &self,
        session: &ChatSessionId,
        request: &ChatSendRequest,
    ) -> Result<u64, BrokerError> {
        let mut sessions = self.sessions.lock().unwrap();
        let live = sessions
            .get_mut(&session.to_string())
            .ok_or_else(|| BrokerError::new("no such session"))?;
        let mut tools = ToolsOff;
        live.session
            .send_with_context(
                &request.text,
                &[],
                request.dynamic_context.as_deref().unwrap_or(""),
                &mut tools,
            )
            .map_err(|refusal| BrokerError::new(format!("{refusal:?}")))
    }

    fn events(
        &self,
        session: &ChatSessionId,
        after: u64,
        _wait_ms: u64,
        limit: u32,
    ) -> Result<ChatEventsPageDto, BrokerError> {
        self.pump(session);
        let sessions = self.sessions.lock().unwrap();
        let live = sessions
            .get(&session.to_string())
            .ok_or_else(|| BrokerError::new("no such session"))?;
        let events: Vec<ChatEventDto> = live
            .events
            .iter()
            .filter(|e| e.seq > after)
            .take(limit.max(1) as usize)
            .cloned()
            .collect();
        let cursor = events.last().map(|e| e.seq).unwrap_or(after);
        Ok(ChatEventsPageDto { events, cursor })
    }

    fn cancel(&self, session: &ChatSessionId) -> Result<(), BrokerError> {
        let sessions = self.sessions.lock().unwrap();
        if let Some(live) = sessions.get(&session.to_string()) {
            live.session.cancel_flag().cancel();
        }
        Ok(())
    }

    fn retire(&self, session: &ChatSessionId) -> Result<(), BrokerError> {
        self.sessions.lock().unwrap().remove(&session.to_string());
        Ok(())
    }
}
