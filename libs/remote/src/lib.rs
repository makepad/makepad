//! makepad-remote: the one remote protocol of makepad apps and Relax (relax_network/DESIGN.md).
//!
//! Layers, bottom up: [`frame`] (length-prefixed frames), [`conn`] (channels with credit flow
//! control and priorities over any carrier), [`hello`] (version and capability handshake on
//! channel 0), [`endpoint`] (the service registry a process serves), the services ([`cmd`],
//! [`watch`], [`stream`], with the replay [`ring`] behind the last two), the [`carrier`]s
//! (Unix socket, stdio, TCP with TLS), the blocking client ([`link`]) and the command line
//! front end for an app's remote commands ([`app`], with the `frames` service records in [`frames`]). The CI bridge of the
//! same name is built from `main.rs` and keeps its own wire until it moves onto this one.

// Diagnostics never panic: std's print macros panic when a write fails (ENOSPC on the session
// log in a full tmpfs /tmp aborted a fleet Sandbox inside a lazy compile, 2026-10-23), so in this
// crate every print is a write whose error is dropped. Test builds keep std's captured macros.
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprintln {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_all(b"\n"); }};
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_fmt(format_args!("{}\n", format_args!($($t)+))); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprint {
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_fmt(format_args!($($t)+)); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! println {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_all(b"\n"); }};
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_fmt(format_args!("{}\n", format_args!($($t)+))); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! print {
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_fmt(format_args!($($t)+)); }};
}

pub mod app;
pub mod carrier;
pub mod cmd;
pub mod conn;
pub mod discovery;
pub mod endpoint;
pub mod frame;
pub mod frames;
pub mod group;
pub mod hello;
pub mod hosts;
pub mod hub;
pub mod lines;
pub mod pair;
pub mod program;
pub mod link;
pub mod mcp;
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
