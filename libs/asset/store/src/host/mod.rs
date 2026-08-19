//! Makepad Asset Server — the standalone transport runtime over the headless
//! core (`makepad-asset-store`).
//!
//! The core stays deterministic and transport-free; this crate is where real
//! time, OS randomness, sockets, and credentials enter. It consumes the core
//! exclusively through its public API and adds:
//!
//! - a bounded, fail-closed HTTP/1.1 transport on two planes — control
//!   (auth, catalog, publication, search, jobs/workers) and data (blobs,
//!   thumbnails) with Range/ETag serving of verified bytes
//! - scoped bearer-token auth resolved against the core's grant store, with
//!   a bootstrap root admin whose secret lives only in `<root>/admin-token`
//! - a transport-owned SQLite store for job routing metadata, worker
//!   progress and result documents
//! - a broker-owned chat session actor: local fleet Qwen and/or external
//!   OpenAI/Grok, explicit provider choice, `llm.consult` so a local
//!   session can ask the external model for code/level drafts. Keys and
//!   provider URLs never appear on the wire.
//! - UDP LAN discovery (fixed 36-byte beacon, allowlisted fields only)
//! - single-process-per-root locking and clean joined shutdown
//!
//! Embedding boundary: [`server::AssetServer::start`] with a
//! [`config::ServerConfig`]; the returned handle owns every thread and
//! resource, and `shutdown()`/drop tears the whole service down. The
//! `makepad-asset-store` binary is a thin flag wrapper over exactly this.

pub mod api;
pub mod config;
pub mod discovery;
pub mod http;
pub mod json;
pub mod profiles;
pub mod server;
pub mod util;

pub(crate) mod appdb;
pub(crate) mod chat;
pub(crate) mod events;
pub(crate) mod routes;
pub(crate) mod routes_control;
pub(crate) mod routes_data;
pub(crate) mod routes_chat;
pub(crate) mod derive_recipes;
pub(crate) mod routes_import;
pub(crate) mod routes_operations;
pub(crate) mod state;

pub use config::{
    ChatConfig, ChatScript, DiscoveryConfig, ScriptedLane, ScriptedTurn, ServerConfig,
    DEFAULT_DISCOVERY_PORT,
};
pub use server::{AssetServer, LISTEN_FILE};
