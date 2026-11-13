//! Small std-only primitives with exactly one implementation each.
//!
//! - [`sha256`]: streaming SHA-256 with hardware acceleration where the CPU
//!   has it (content digests, download verification, run identities).
//! - [`file_name`]: what a file or folder name can't be on Windows.
//! - [`netif`]: this machine's IPv4 interfaces and gateway candidates.
//! - [`udp`]: a UDP socket bound with address and port reuse, so several
//!   processes on one machine can listen for the same LAN beacons.
//! - [`process_end`]: how a process ends: its own exit work, the stdio
//!   flushes, then `_exit`, natively and under the JIT alike.
//!
//! Nothing here knows about assets, flows, models or any application.

pub mod file_name;
pub mod netif;
pub mod process_end;
pub mod sha256;
pub mod udp;
