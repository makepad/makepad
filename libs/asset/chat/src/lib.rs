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
//! - **Mutation is closed and typed.** There is no raw job enqueue, publish,
//!   or alias tool. Game sessions get one closed `content.generate` entry
//!   point for three publishable pipeline families; transform work uses
//!   `operation.create` over exact [`makepad_asset_data::AssetRevisionId`]s
//!   already bound to the session. The server pins exact inputs, validates
//!   rights, and workers publish NEW immutable revisions; sources are never
//!   mutated.
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
pub mod claude {
    pub use makepad_ai_hub::providers::claude::*;
}
pub mod cli {
    pub use makepad_ai_hub::providers::cli::*;
}
pub mod codex_cli {
    pub use makepad_ai_hub::providers::codex_cli::*;
}
pub mod context;
pub mod dispatch;
pub mod fleet_discovery;
pub mod fleet_http {
    pub use makepad_ai_hub::providers::fleet_http::*;
}
pub mod grok {
    pub use makepad_ai_hub::providers::grok::*;
    use makepad_ai_hub::providers::responses::{
        ApiKey, BlockingTransport, ResponsesConfig, ResponsesTransport,
    };

    pub fn from_env() -> GrokChatProvider<BlockingTransport> {
        makepad_ai_hub::providers::grok::from_env(Some(crate::tools::native_tools_payload()))
    }

    pub fn new(
        api_key: ApiKey,
        model: impl Into<String>,
    ) -> GrokChatProvider<BlockingTransport> {
        makepad_ai_hub::providers::grok::new(
            api_key,
            model,
            Some(crate::tools::native_tools_payload()),
        )
    }

    pub fn with_transport<T: ResponsesTransport>(
        config: ResponsesConfig,
        transport: T,
    ) -> Result<GrokChatProvider<T>, String> {
        makepad_ai_hub::providers::grok::with_transport(
            config,
            transport,
            Some(crate::tools::native_tools_payload()),
        )
    }
}
pub mod grok_cli {
    pub use makepad_ai_hub::providers::grok_cli::*;
}
pub mod openai {
    pub use makepad_ai_hub::providers::openai::*;
    use makepad_ai_hub::providers::responses::{
        ApiKey, BlockingTransport, ResponsesConfig, ResponsesTransport,
    };

    pub fn from_env() -> OpenAiChatProvider<BlockingTransport> {
        makepad_ai_hub::providers::openai::from_env(Some(crate::tools::native_tools_payload()))
    }

    pub fn new(
        api_key: ApiKey,
        model: impl Into<String>,
    ) -> OpenAiChatProvider<BlockingTransport> {
        makepad_ai_hub::providers::openai::new(
            api_key,
            model,
            Some(crate::tools::native_tools_payload()),
        )
    }

    pub fn with_transport<T: ResponsesTransport>(
        config: ResponsesConfig,
        transport: T,
    ) -> Result<OpenAiChatProvider<T>, String> {
        makepad_ai_hub::providers::openai::with_transport(
            config,
            transport,
            Some(crate::tools::native_tools_payload()),
        )
    }
}
pub mod provider {
    pub use makepad_ai_hub::providers::provider::*;
}
pub mod qwen {
    pub use makepad_ai_hub::providers::qwen::*;
}
pub mod responses {
    pub use makepad_ai_hub::providers::responses::*;
}
pub mod session;
pub mod toolcall;
pub mod tools;
pub mod transcript;
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
    canonicalize_json, encode_args, AliasExpectArg, ConsultTask, ContentGenerateKind,
    ContentToolCall, GenerateThen, InspectTarget, OperationInputArg, PublicationArg, ToolDef,
};
pub use wire::{
    AttachmentBinding, ChatEvent, ChatEventBody, ChatMessage, ChatRole, ProviderAvailability,
    ProviderKind, ToolOutcome,
};
