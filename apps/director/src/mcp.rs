//! The loopback MCP endpoint for the AI lanes. The server itself is shared
//! with every app's Claude Desktop connection and lives in the AI services
//! layer; Director binds its lanes' persisted tokens to it.

pub use makepad_ai_services::mcp::server::*;
