//! Chat + content-tool protocol for AI-driven asset authoring.
//!
//! One reusable layer between a thin streaming chat UI (AI Content today,
//! the game sandbox later) and the services that do the real work. The laws,
//! in order:
//!
//! - **Providers are explicit.** A session is bound at creation to exactly
//!   one [`wire::ProviderKind`] — fleet/local Qwen, OpenAI Responses, or
//!   xAI Grok Responses. There is no auto mode and no fallback: an
//!   unavailable provider yields a typed refusal with its honest reason
//!   ([`wire::ProviderAvailability`]), never a silent switch.
//! - **Credentials never reach the client wire.** No wire type in this crate
//!   can carry a provider secret. OpenAI/Grok keys live in process env or
//!   an explicit server-side constructor; the fleet speaks its LAN job
//!   protocol; the Asset Server sees only its own bearer token, held by
//!   the tool dispatcher on the server side.
//! - **Tools execute server-side, against the Asset Server.** The LLM emits
//!   a typed tool call ([`tools::ContentToolCall`]); the dispatcher
//!   ([`dispatch::AssetServerTools`]) resolves and verifies every referenced
//!   asset itself. Clients never relay paths, blobs, or credentials.
//! - **Mutation is typed operations only.** There is no raw job enqueue,
//!   publish, or alias tool: `operation.create` names a REGISTERED operation
//!   kind over exact [`makepad_asset_data::AssetRevisionId`]s that must
//!   already be bound to the session (attachments or revisions a prior tool
//!   returned). The server pins the exact input file, validates rights, and
//!   its atomic finalizer publishes NEW immutable revisions with exact
//!   parent lineage; source revisions are never mutated.
//! - **Availability is honest, end to end.** Operations are filtered by the
//!   server's registered types intersected with live worker availability; a
//!   missing or worker-less operation returns a structured
//!   [`wire::ToolOutcome::Unavailable`], not an error string.
//!
//! Module map: [`wire`] (bounded schema the UI and sandbox both speak),
//! [`tools`] (typed tool calls), [`toolcall`] (Fleet Qwen textual marker),
//! [`provider`] (the explicit-provider seam), [`session`] (turn/tool
//! state machine, session ids, cancellation), [`dispatch`]
//! (Asset Server execution), [`qwen`] / [`openai`] / [`grok`] (the
//! providers), [`responses`] (shared Responses API driver),
//! [`fleet_http`] (minimal bounded HTTP for the fleet wire).

pub mod catalog_sql;
pub mod context;
pub mod dispatch;
pub mod fleet_discovery;
pub mod fleet_http;
pub mod grok;
pub mod openai;
pub mod provider;
pub mod qwen;
pub mod responses;
pub mod session;
pub mod toolcall;
pub mod tools;
pub mod wire;

pub use dispatch::AssetServerTools;
pub use grok::GrokChatProvider;
pub use openai::OpenAiChatProvider;
pub use provider::{ChatProvider, ProviderEvent, TurnInput};
pub use responses::{ApiKey, ResponsesChatProvider, ResponsesConfig};
pub use session::{
    CancelFlag, ExecCtx, Origin, SendRefusal, Session, SessionId, ToolExecutor,
};
pub use tools::{
    canonicalize_json, encode_args, AliasExpectArg, ConsultTask, ContentToolCall, GenerateThen,
    InspectTarget, OperationInputArg, PublicationArg, ToolDef,
};
pub use wire::{
    AttachmentBinding, ChatEvent, ChatEventBody, ChatMessage, ChatRole, ProviderAvailability,
    ProviderKind, ToolOutcome,
};
