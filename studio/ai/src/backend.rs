use crate::types::*;
use makepad_widgets2::*;
use std::collections::HashMap;

/// Identifies an in-flight request
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub LiveId);

impl RequestId {
    pub fn new() -> Self {
        Self(LiveId::unique())
    }
}

/// Configuration for an AI backend
#[derive(Clone, Debug)]
pub enum BackendConfig {
    Claude {
        api_key: Option<String>,
        oauth_token: Option<String>,
        model: String,
    },
    OpenAI {
        api_key: String,
        model: String,
        base_url: Option<String>,
        reasoning_effort: Option<String>,
    },
    Gemini {
        api_key: String,
        model: String,
    },
}

impl BackendConfig {
    pub fn name(&self) -> &'static str {
        match self {
            BackendConfig::Claude { .. } => "Claude",
            BackendConfig::OpenAI { .. } => "OpenAI",
            BackendConfig::Gemini { .. } => "Gemini",
        }
    }

    pub fn model(&self) -> &str {
        match self {
            BackendConfig::Claude { model, .. } => model,
            BackendConfig::OpenAI { model, .. } => model,
            BackendConfig::Gemini { model, .. } => model,
        }
    }
}

/// Events emitted by backends during streaming
#[derive(Clone, Debug)]
pub enum AiEvent {
    StreamDelta {
        request_id: RequestId,
        delta: StreamDelta,
    },
    Complete {
        request_id: RequestId,
        response: AiResponse,
    },
    Error {
        request_id: RequestId,
        error: String,
    },
}

/// Trait for AI backend implementations
pub trait AiBackend {
    fn send_request(&mut self, cx: &mut Cx, request: AiRequest) -> RequestId;
    fn cancel_request(&mut self, cx: &mut Cx, request_id: RequestId);
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<AiEvent>;
    fn config(&self) -> &BackendConfig;
}

/// Manages multiple AI backends
pub struct AiManager {
    backends: HashMap<String, Box<dyn AiBackend>>,
    active_backend: Option<String>,
}

impl Default for AiManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AiManager {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
            active_backend: None,
        }
    }

    pub fn add_backend(&mut self, name: &str, backend: Box<dyn AiBackend>) {
        self.backends.insert(name.to_string(), backend);
        if self.active_backend.is_none() {
            self.active_backend = Some(name.to_string());
        }
    }

    pub fn set_active(&mut self, name: &str) {
        if self.backends.contains_key(name) {
            self.active_backend = Some(name.to_string());
        }
    }

    pub fn active_backend_name(&self) -> Option<&str> {
        self.active_backend.as_deref()
    }

    pub fn backend_names(&self) -> Vec<&str> {
        self.backends.keys().map(|s| s.as_str()).collect()
    }

    pub fn send_request(&mut self, cx: &mut Cx, request: AiRequest) -> Option<RequestId> {
        let name = self.active_backend.as_ref()?;
        let backend = self.backends.get_mut(name)?;
        Some(backend.send_request(cx, request))
    }

    pub fn cancel_request(&mut self, cx: &mut Cx, request_id: RequestId) {
        for backend in self.backends.values_mut() {
            backend.cancel_request(cx, request_id);
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<AiEvent> {
        let mut all_events = vec![];
        for backend in self.backends.values_mut() {
            all_events.extend(backend.handle_event(cx, event));
        }
        all_events
    }
}
