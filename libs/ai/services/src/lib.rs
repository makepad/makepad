//! The AI services layer: one conversation, many apps.
//!
//! - [`wire`] — the manifest, the tools, the calls and their results, and
//!   the JSON envelope a hosted transport carries them in.
//! - [`port`] — what an app opens to expose its service: hosted by the
//!   window manager, or in-process to a chat panel the app embeds.
//! - `engine` (feature `engine`) — what a host runs: the registry of
//!   connected services, the router that sends each call to its owner and
//!   gates the risky ones, and the session over the hub's providers.
//!
//! Design of record: `local/agent_state/aichat/DESIGN.md`.

pub mod port;
pub mod state;
pub mod wire;

#[cfg(feature = "engine")]
pub mod engine;

pub use port::{AiServicePort, PortEvent, ServiceLink, ServiceLinkHost};
pub use state::{
    EngineState, Entry, ProviderChoice, ProviderRow, ServiceInfo, Status, ToolEntry, ToolStatus,
};
pub use wire::{
    api_name, canonical_name, split_name, Risk, ServiceCall, ServiceContext, ServiceDown,
    ServiceManifest, ServiceUp, ToolDef, ToolOutcome, ToolResult,
};
