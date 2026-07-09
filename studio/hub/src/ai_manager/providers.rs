mod chatgpt;
mod openai_compatible;

use self::chatgpt::CHATGPT_BACKEND;
use self::openai_compatible::OPENAI_COMPATIBLE_BACKEND;
use super::{
    AiBackendConfig, AssistantTurn, ConversationItem, OpenAiStreamToolCallDelta, ParsedSkill,
    ParsedWorkflow, StreamingTurnState,
};
use makepad_network::HttpRequest;
use makepad_studio_protocol::hub_protocol::ActiveWorkflowState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AiProviderKind {
    OpenAiCompatible,
    ChatGpt,
}

impl AiProviderKind {
    pub(super) fn backend(self) -> &'static dyn AiProviderBackend {
        match self {
            AiProviderKind::OpenAiCompatible => &OPENAI_COMPATIBLE_BACKEND,
            AiProviderKind::ChatGpt => &CHATGPT_BACKEND,
        }
    }
}

impl AiBackendConfig {
    pub(super) fn provider_kind(&self) -> AiProviderKind {
        if self.chatgpt.is_some() {
            AiProviderKind::ChatGpt
        } else {
            AiProviderKind::OpenAiCompatible
        }
    }

    pub(super) fn provider_backend(&self) -> &'static dyn AiProviderBackend {
        self.provider_kind().backend()
    }
}

pub(super) struct ProviderStreamDeltas {
    pub(super) thinking_delta: String,
    pub(super) assistant_delta: String,
    pub(super) finish_reason: Option<String>,
    pub(super) tool_call_deltas: Vec<OpenAiStreamToolCallDelta>,
    pub(super) saw_done: bool,
}

impl ProviderStreamDeltas {
    fn empty() -> Self {
        Self {
            thinking_delta: String::new(),
            assistant_delta: String::new(),
            finish_reason: None,
            tool_call_deltas: Vec::new(),
            saw_done: false,
        }
    }
}

pub(super) trait AiProviderBackend {
    #[allow(clippy::too_many_arguments)]
    fn build_http_request(
        &self,
        backend: &AiBackendConfig,
        mount: &str,
        root_path: &str,
        history: &[ConversationItem],
        role: Option<&str>,
        task: Option<&str>,
        skills: &[ParsedSkill],
        workflows: &[ParsedWorkflow],
        active_workflow: Option<&ActiveWorkflowState>,
    ) -> Result<HttpRequest, String>;

    fn response_is_stream(&self) -> bool {
        false
    }

    fn extract_assistant_turn(&self, body: &str) -> Result<AssistantTurn, String>;

    fn process_stream_events(
        &self,
        events: Vec<String>,
        stream: &mut StreamingTurnState,
    ) -> Result<ProviderStreamDeltas, String>;
}
