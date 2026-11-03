//! The wire of the hub's `kernel` domain: compute programs (`makepad_compute_program`) a fleet
//! client runs on a node (`crate::program_serve` is the node side, `crate::program_client` the
//! client side over `crate::kernel_client`'s stream).
//!
//! ```text
//! GET /kernel          -> KernelInfoJson
//! GET /kernel/stream   websocket of binary frames: kind u8, word u64 (little-endian), payload;
//!                      the kinds and payloads are `makepad_compute_program::wire`'s
//! ```
//!
//! Fleet machine credentials only (a paired phone has none of this domain).

use makepad_micro_serde::*;

/// A client and node speak the domain only at the same version.
pub const KERNEL_API_VERSION: u32 = 2;

/// `GET /kernel`.
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct KernelInfoJson {
    pub api: u32,
    /// The node's logical cores (its program workers).
    pub cores: u32,
    /// `x86_64`, `aarch64`.
    pub arch: String,
    /// `native` or `interp`.
    pub backend: String,
    /// Program sessions open.
    pub programs: u32,
}
