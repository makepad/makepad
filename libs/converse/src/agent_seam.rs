//! The Cx-side agent seam: session-based conversational agents in UI apps.
//!
//! Moved here from `libs/makepad_ai` when the backends became ai-hub pipes
//! (aicore P1): the TRAIT and its event/config types are an interface owned
//! by their consumers — converse's pipeline, route's dispatchers, sandbox —
//! while every backend now lives behind `makepad_ai_hub::providers`. An
//! implementation adapts a hub provider (or the in-process local engine) on
//! a worker thread and pumps `AgentEvent`s back through `handle_event`.

use makepad_widgets::*;

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
    /// Tool definitions exposed to the backend.
    pub tools: Vec<ToolDefinition>,
    /// Names of the backend's own built-in tools the session may use, e.g.
    /// `["Read", "Edit", "Bash"]`. Empty runs the session without any tools.
    pub allowed_tools: Vec<String>,
    /// Backend permission policy, e.g. `"dontAsk"`, `"acceptEdits"`, `"plan"`.
    pub permission_mode: Option<String>,
    /// Inline settings JSON handed to the backend. Prefer this over a settings
    /// file in `cwd`: workspace settings are ignored until the user has accepted
    /// the trust dialog there, whereas inline settings always apply.
    pub settings_json: Option<String>,
    /// A backend-native session id to pick up where a previous run left off.
    /// Lets a conversation survive an app restart, not just a new prompt.
    pub resume_session_id: Option<String>,
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

// --------------------------------------------------------------- messages

/// Role in a conversation
#[derive(Clone, Debug, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(MessageRole::System),
            "user" => Some(MessageRole::User),
            "assistant" => Some(MessageRole::Assistant),
            "tool" => Some(MessageRole::Tool),
            "model" => Some(MessageRole::Assistant), // Gemini uses "model"
            _ => None,
        }
    }
}

/// Content block - supports text, images, tool calls
#[derive(Clone, Debug)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

/// A message in the conversation
#[derive(Clone, Debug)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: &str) -> Self {
        Self {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn assistant(text: &str) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn system(text: &str) -> Self {
        Self {
            role: MessageRole::System,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| {
                if let ContentBlock::Text { text } = c {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Tool definition for function calling
#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: String,
}

/// Why an agent turn stopped.
#[derive(Clone, Debug, Default)]
pub enum StopReason {
    #[default]
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
}

