//! The AI services layer: one conversation, many apps.
//!
//! - [`wire`] — the manifest, the tools, the calls and their results, and
//!   the JSON envelope a hosted transport carries them in.
//! - [`port`] — what an app opens to expose its service: hosted by the
//!   window manager, or in-process to a chat panel the app embeds.
//! - `engine` (feature `engine`) — what a host runs: the registry of
//!   connected services, the router that sends each call to its owner and
//!   gates the risky ones, and the session over the hub's providers.
//! - `app_service` — the process's `app` service on its makepad-remote
//!   endpoint: the in-process services' tools for a local agent.
//! - [`mcp`] — the Claude Desktop bundle writer. The bundle runs
//!   `makepad-remote mcp` to reach the app's tools through its `app` service.
//!
//! Design of record: `local/agent_state/aichat/DESIGN.md`.

#[cfg(all(feature = "platform", not(any(target_arch = "wasm32", target_os = "android"))))]
pub mod app_service;
pub mod card;
#[cfg(feature = "platform")]
pub mod engine;
pub mod mcp;
#[cfg(feature = "platform")]
pub mod port;
pub mod progress_eta;
pub mod provider;
pub mod state;
pub mod wire;

#[cfg(feature = "platform")]
pub use engine::{EngineCore, EngineEvent, Model, ModelEvent, ServiceRegistry, ToolDefinition};
#[cfg(feature = "platform")]
pub use port::{AiServicePort, PortEvent, ServiceLink, ServiceLinkHost};
pub use state::{
    EngineState, Entry, EventEntry, ProviderChoice, ProviderRow, ServiceInfo, Status, Suggestion,
    Target, ToolEntry, ToolStatus,
};
pub use card::{Card, CardValues, Field};
pub use wire::{
    api_name, AppLine, AppRequest, CardLayout, FieldLabel, canonical_name, split_name, Disposition, EndpointId, HostedDown, HostedUp,
    InstanceMeta, Message, Risk, ServiceCall, ServiceContext, ServiceDown, ServiceManifest,
    ServiceUp, SubscriptionRequest, ToolDef, ToolOutcome, ToolResult, TopicDef,
};
