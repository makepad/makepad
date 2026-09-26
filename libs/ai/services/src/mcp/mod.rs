//! MCP on loopback.
//!
//! - [`server`] — the streamable-HTTP JSON-RPC endpoint and its bearer
//!   tokens. Director serves its lanes' tools with it; an app serves its
//!   own through [`host`].
//! - [`host`] — an app's tools offered to Claude Desktop while the F10
//!   panel's provider is "Claude Desktop": the server over the panel's
//!   registry, and the discovery file the app's `--mcp` relay finds.
//! - [`mcpb`] — the Claude Desktop extension that installs that relay.

pub mod host;
pub mod mcpb;
pub mod server;
