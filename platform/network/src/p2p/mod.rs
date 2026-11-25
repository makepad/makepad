//! Peer-to-peer reach: two machines behind home or office firewalls opening a direct UDP path to
//! each other, with no relay. App-neutral pieces, each usable alone:
//!
//! - [`stun`]: STUN binding messages (RFC 5389), shared with `makepad-webrtc`.
//! - [`portmap`]: asking the router to open a port: PCP (RFC 6887), NAT-PMP (RFC 6886) and
//!   UPnP-IGD. Routers are on the local network, so every send goes through [`crate::lan`]. The
//!   first way to learn our public address; a router behind another NAT reports a private one.
//! - [`reflect`]: "what address do you see me at" (a STUN binding), asked of the peer once one side
//!   is reachable, else of public STUN servers; from two destinations it also tells how the NAT
//!   maps the socket. The answering side, for a peer that reflects.
//! - [`discover`] selects the invite address from router and reflector findings and assesses the direct path.
//! - [`words`]: an address and a key commitment as plain English words people read to each other.
//! - [`punch`]: the simultaneous open (hole punching), sans-IO: probes, retries, keepalives, and a
//!   plain answer when no direct path can exist.
//! - [`natsim`]: a deterministic in-process internet with NATs of every kind, for tests that drive
//!   the whole open without sockets or wall-clock time.
//!
//! The sans-IO pieces take the time as milliseconds from an origin the caller picks, so the same
//! code runs on a socket thread and in the simulator.

pub mod discover;
pub mod natsim;
pub mod portmap;
pub mod punch;
pub mod reflect;
pub mod stream;
pub mod stun;
pub mod words;
