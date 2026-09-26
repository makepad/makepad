//! Small std-only primitives with exactly one implementation each.
//!
//! - [`sha256`]: streaming SHA-256 with hardware acceleration where the CPU
//!   has it (content digests, download verification, run identities).
//! - [`udp`]: a UDP socket bound with address and port reuse, so several
//!   processes on one machine can listen for the same LAN beacons.
//!
//! Nothing here knows about assets, flows, models or any application.

pub mod sha256;
pub mod udp;
