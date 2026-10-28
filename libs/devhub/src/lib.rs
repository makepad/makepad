//! devhub: one place to observe every JIT (`makepad run`) on the LAN.
//!
//! One app, two roles ([`node`] on each Linux WM, [`master`] on the user's Mac); the design is in
//! the lane's DESIGN.md (local/agent_state/devhub). Everything rides on makepad-remote (frames,
//! channels, the relay's `fwd` for full inspect, the hub's `route`); how hubs trust each other is
//! behind [`trust::Trust`].

// Output never panics: a CLI piped into `head` gets EPIPE (std's print macros would abort the
// command). Test builds keep std's captured macros.
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

pub mod proto;
pub mod conf;
pub mod procinfo;
pub mod os;
pub mod embed;
pub mod inbox;
pub mod local;
pub mod node;
pub mod master;
pub mod cli;
pub mod sync;
pub mod cluster;
pub mod trust;
pub mod tray_glyph;
pub mod explorer;
pub mod top;
pub mod lease;
pub mod jitcache;
