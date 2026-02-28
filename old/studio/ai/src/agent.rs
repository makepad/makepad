//! Agent abstraction for session-based AI backends like ACP
//!
//! This provides a cleaner API for coding agents that maintain conversation state.
//! Also includes an adapter to use stateless backends (OpenAI, Gemini, Claude API)
//! with the Agent interface.

use crate::backend::AiBackend;
use crate::types::*;
use makepad_widgets::*;
use std::collections::HashMap;

/// Unique identifier for an agent session
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub LiveId);

impl SessionId {
    pub fn new() -> Self {
        Self(LiveId::unique())
    }
}

/// Unique identifier for an in-flight prompt
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PromptId(pub LiveId);

impl PromptId {
    pub fn new() -> Self {
        Self(LiveId::unique())
    }
}

/// Configuration for creating a new agent session
#[derive(Clone, Debug, Default)]
pub struct SessionConfig {
    /// Working directory for the agent
    pub cwd: Option<String>,
    /// System prompt / instructions
    pub system_prompt: Option<String>,
    /// Model to use (if selectable)
    pub model: Option<String>,
}

/// Events emitted by an agent during operation
#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// Session is ready to receive prompts
    SessionReady { session_id: SessionId },

    /// Session failed to initialize
    SessionError {
        session_id: SessionId,
        error: String,
    },

    /// Streaming text from the agent
    TextDelta { prompt_id: PromptId, text: String },

    /// Agent wants to use a tool
    ToolRequest {
        prompt_id: PromptId,
        tool_use_id: String,
        tool_name: String,
        tool_input: String,
    },

    /// Agent turn complete
    TurnComplete {
        prompt_id: PromptId,
        stop_reason: StopReason,
    },

    /// Error during prompt
    PromptError { prompt_id: PromptId, error: String },
}

/// Trait for session-based AI agents (like ACP)
pub trait Agent {
    /// Create a new session with the agent
    fn create_session(&mut self, cx: &mut Cx, config: SessionConfig) -> SessionId;

    /// Send a prompt to an existing session
    /// Only sends the new user message - session maintains history
    fn send_prompt(&mut self, cx: &mut Cx, session_id: SessionId, text: &str) -> PromptId;

    /// Provide a tool result back to the agent
    fn send_tool_result(
        &mut self,
        cx: &mut Cx,
        session_id: SessionId,
        tool_use_id: &str,
        result: &str,
        is_error: bool,
    );

    /// Cancel an in-flight prompt
    fn cancel_prompt(&mut self, cx: &mut Cx, prompt_id: PromptId);

    /// Handle platform events, returns agent events
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<AgentEvent>;

    /// Check if a session is ready
    fn is_session_ready(&self, session_id: SessionId) -> bool;

    /// Whether this agent uses a stateless backend that needs history injected
    fn is_stateless(&self) -> bool {
        false
    }

    /// Inject prior conversation history into a session (for stateless backends)
    fn inject_history(&mut self, _session_id: SessionId, _messages: Vec<Message>) {}
}

/// Simple wrapper to use an Agent with automatic session management
pub struct AgentChat {
    agent: Box<dyn Agent>,
    session_id: Option<SessionId>,
    pending_prompt: Option<String>,
    current_prompt_id: Option<PromptId>,
}

impl AgentChat {
    pub fn new(agent: Box<dyn Agent>) -> Self {
        Self {
            agent,
            session_id: None,
            pending_prompt: None,
            current_prompt_id: None,
        }
    }

    /// Initialize the agent (creates session)
    pub fn init(&mut self, cx: &mut Cx, config: SessionConfig) {
        self.session_id = Some(self.agent.create_session(cx, config));
    }

    /// Send a message. If session isn't ready yet, queues it.
    pub fn send(&mut self, cx: &mut Cx, text: &str) -> Option<PromptId> {
        if let Some(session_id) = self.session_id {
            if self.agent.is_session_ready(session_id) {
                let prompt_id = self.agent.send_prompt(cx, session_id, text);
                self.current_prompt_id = Some(prompt_id);
                return Some(prompt_id);
            }
        }
        // Queue for later
        self.pending_prompt = Some(text.to_string());
        None
    }

    /// Cancel current prompt
    pub fn cancel(&mut self, cx: &mut Cx) {
        if let Some(prompt_id) = self.current_prompt_id.take() {
            self.agent.cancel_prompt(cx, prompt_id);
        }
        self.pending_prompt = None;
    }

    /// Handle events, returns filtered events for this chat
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<AgentEvent> {
        let events = self.agent.handle_event(cx, event);

        // Check if session became ready and we have a pending prompt
        if let Some(session_id) = self.session_id {
            if self.agent.is_session_ready(session_id) {
                if let Some(text) = self.pending_prompt.take() {
                    let prompt_id = self.agent.send_prompt(cx, session_id, &text);
                    self.current_prompt_id = Some(prompt_id);
                }
            }
        }

        events
    }

    /// Check if we're currently waiting for a response
    pub fn is_busy(&self) -> bool {
        self.current_prompt_id.is_some() || self.pending_prompt.is_some()
    }
}

// === Adapter for Stateless Backends ===

/// Session state for stateless backend adapter
struct AdapterSession {
    #[allow(dead_code)]
    id: SessionId,
    messages: Vec<Message>,
    system_prompt: Option<String>,
    current_prompt: Option<PromptId>,
    accumulated_text: String,
}

/// Adapter that wraps a stateless AiBackend to implement the Agent trait.
/// This allows using OpenAI, Gemini, and Claude API with the unified Agent interface.
pub struct StatelessBackendAdapter {
    backend: Box<dyn AiBackend>,
    sessions: HashMap<LiveId, AdapterSession>,
    /// Maps backend RequestId to (SessionId, PromptId)
    pending_requests: HashMap<LiveId, (SessionId, PromptId)>,
}

impl StatelessBackendAdapter {
    pub fn new(backend: Box<dyn AiBackend>) -> Self {
        Self {
            backend,
            sessions: HashMap::new(),
            pending_requests: HashMap::new(),
        }
    }
}

impl Agent for StatelessBackendAdapter {
    fn create_session(&mut self, _cx: &mut Cx, config: SessionConfig) -> SessionId {
        let session_id = SessionId::new();

        let session = AdapterSession {
            id: session_id,
            messages: Vec::new(),
            system_prompt: config.system_prompt,
            current_prompt: None,
            accumulated_text: String::new(),
        };

        self.sessions.insert(session_id.0, session);
        session_id
    }

    fn send_prompt(&mut self, cx: &mut Cx, session_id: SessionId, text: &str) -> PromptId {
        let prompt_id = PromptId::new();

        let session = match self.sessions.get_mut(&session_id.0) {
            Some(s) => s,
            None => return prompt_id,
        };

        // Add user message to history
        session.messages.push(Message::user(text));
        session.current_prompt = Some(prompt_id);
        session.accumulated_text.clear();

        // Build request with full history
        let request = AiRequest {
            messages: session.messages.clone(),
            system_prompt: session.system_prompt.clone(),
            stream: true,
            ..Default::default()
        };

        let request_id = self.backend.send_request(cx, request);
        self.pending_requests
            .insert(request_id.0, (session_id, prompt_id));

        prompt_id
    }

    fn send_tool_result(
        &mut self,
        _cx: &mut Cx,
        _session_id: SessionId,
        _tool_use_id: &str,
        _result: &str,
        _is_error: bool,
    ) {
        // TODO: Implement tool calling for stateless backends
        log!("send_tool_result not yet implemented for stateless backends");
    }

    fn cancel_prompt(&mut self, cx: &mut Cx, prompt_id: PromptId) {
        // Find and cancel
        let request_to_cancel = self
            .pending_requests
            .iter()
            .find(|(_, (_, pid))| *pid == prompt_id)
            .map(|(rid, _)| crate::backend::RequestId(*rid));

        if let Some(request_id) = request_to_cancel {
            self.backend.cancel_request(cx, request_id);
            self.pending_requests.remove(&request_id.0);
        }

        // Clear session state
        for session in self.sessions.values_mut() {
            if session.current_prompt == Some(prompt_id) {
                session.current_prompt = None;
                session.accumulated_text.clear();
                break;
            }
        }
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<AgentEvent> {
        use crate::backend::AiEvent;

        let mut agent_events = vec![];

        for ai_event in self.backend.handle_event(cx, event) {
            match ai_event {
                AiEvent::StreamDelta { request_id, delta } => {
                    if let Some((session_id, prompt_id)) =
                        self.pending_requests.get(&request_id.0).copied()
                    {
                        if let Some(session) = self.sessions.get_mut(&session_id.0) {
                            match delta {
                                StreamDelta::TextDelta { text } => {
                                    session.accumulated_text.push_str(&text);
                                    agent_events.push(AgentEvent::TextDelta { prompt_id, text });
                                }
                                StreamDelta::Error { message } => {
                                    agent_events.push(AgentEvent::PromptError {
                                        prompt_id,
                                        error: message,
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
                AiEvent::Complete {
                    request_id,
                    response,
                } => {
                    if let Some((session_id, prompt_id)) =
                        self.pending_requests.remove(&request_id.0)
                    {
                        if let Some(session) = self.sessions.get_mut(&session_id.0) {
                            // Add assistant response to history
                            session.messages.push(response.message);
                            session.current_prompt = None;

                            agent_events.push(AgentEvent::TurnComplete {
                                prompt_id,
                                stop_reason: response.stop_reason,
                            });
                        }
                    }
                }
                AiEvent::Error { request_id, error } => {
                    if let Some((_, prompt_id)) = self.pending_requests.remove(&request_id.0) {
                        agent_events.push(AgentEvent::PromptError { prompt_id, error });
                    }
                }
            }
        }

        agent_events
    }

    fn is_session_ready(&self, session_id: SessionId) -> bool {
        // Stateless sessions are always ready (no initialization needed)
        self.sessions.contains_key(&session_id.0)
    }

    fn is_stateless(&self) -> bool {
        true
    }

    fn inject_history(&mut self, session_id: SessionId, messages: Vec<Message>) {
        if let Some(session) = self.sessions.get_mut(&session_id.0) {
            // Prepend history before any existing messages
            let existing = std::mem::take(&mut session.messages);
            session.messages = messages;
            session.messages.extend(existing);
        }
    }
}
