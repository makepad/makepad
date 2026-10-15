//! makepad-remote: the one remote protocol of makepad apps and Relax (relax_network/DESIGN.md).
//!
//! Layers, bottom up: [`frame`] (length-prefixed frames), [`conn`] (channels with credit flow
//! control and priorities over any carrier), [`hello`] (version and capability handshake on
//! channel 0), [`endpoint`] (the service registry a process serves), the services ([`cmd`],
//! [`watch`], [`stream`], with the replay [`ring`] behind the last two), the [`carrier`]s
//! (Unix socket, stdio, TCP with TLS), the blocking client ([`link`]) and the command line
//! front end for an app's remote commands ([`app`], with the `frames` service records in [`frames`]). The CI bridge of the
//! same name is built from `main.rs` and keeps its own wire until it moves onto this one.

pub mod app;
pub mod carrier;
pub mod cmd;
pub mod conn;
pub mod discovery;
pub mod endpoint;
pub mod frame;
pub mod frames;
pub mod hello;
pub mod hosts;
#[cfg(unix)]
pub mod hub;
pub mod lines;
pub mod pair;
pub mod program;
pub mod link;
pub mod mcp;
#[cfg(unix)]
pub mod relay;
pub mod ring;
pub mod stream;
pub mod watch;

#[cfg(all(test, unix))]
mod tests;
#[cfg(all(test, unix))]
mod hub_tests;

pub use cmd::{json_str, now_ms, Out, Req};
pub use conn::{Channel, ChannelTx, Conn, Event, Hooks};
pub use endpoint::Endpoint;
pub use link::Client;
