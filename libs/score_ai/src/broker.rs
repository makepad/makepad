use makepad_asset_client::{
    ChatCreateRequest, ChatEventBodyDto, ChatEventsPageDto, ChatProviderDto, ChatSendRequest,
    ChatSessionId,
};
use std::fmt;

/// Transport error kept independent from the asset client's concrete error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerError {
    pub message: String,
}

impl BrokerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for BrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BrokerError {}

/// Narrow broker seam used by the worker and by hermetic tests.
///
/// Frontends should pass an implementation to a background worker. The
/// worker re-checks provider locality immediately before creating a session,
/// so a stale or permissive UI cannot bypass a local-only lock.
pub trait ScoreChatBroker {
    fn providers(&self) -> Result<Vec<ChatProviderDto>, BrokerError>;

    fn create(&self, request: &ChatCreateRequest) -> Result<ChatSessionId, BrokerError>;

    fn send(
        &self,
        session: &ChatSessionId,
        request: &ChatSendRequest,
    ) -> Result<u64, BrokerError>;

    fn events(
        &self,
        session: &ChatSessionId,
        after: u64,
        wait_ms: u64,
        limit: u32,
    ) -> Result<ChatEventsPageDto, BrokerError>;

    fn cancel(&self, session: &ChatSessionId) -> Result<(), BrokerError>;

    fn retire(&self, session: &ChatSessionId) -> Result<(), BrokerError>;
}

/// True for terminal broker events. Useful to broker adapters that relay raw
/// progress without running the full generation engine.
pub fn is_terminal_chat_event(body: &ChatEventBodyDto) -> bool {
    matches!(
        body,
        ChatEventBodyDto::Done
            | ChatEventBodyDto::Cancelled
            | ChatEventBodyDto::Error { .. }
    )
}

/// Wire outcome → DTO outcome (same five shapes), for local brokers.
pub fn broker_outcome_dto(
    outcome: &makepad_asset_chat::wire::ToolOutcome,
) -> makepad_asset_client::dto::ChatToolOutcomeDto {
    use makepad_asset_chat::wire::ToolOutcome as W;
    use makepad_asset_client::dto::ChatToolOutcomeDto as D;
    match outcome {
        W::Ok { value } => D::Ok { value: value.clone() },
        W::Failed { message } => D::Failed { message: message.clone() },
        W::Refused { what } => D::Refused { what: what.clone() },
        W::Denied { what } => D::Denied { what: what.clone() },
        W::Unavailable { reason } => D::Unavailable { reason: reason.clone() },
    }
}
