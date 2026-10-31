//! The fleet: credentials, the TLS front and the LAN bind rule, shared by
//! every service a fleet machine exposes (the AI hub, the asset store's sync
//! listener). Operator guide: tools/fleet-identity.md.
//!
//! - [`id`]: fleet identity: per-machine keys and root-signed credentials,
//!   certificate-bound proofs, rights ([`id::require`]), signed
//!   announcements, revocation; this process as a fleet client.
//! - [`front`]: the TLS front, the only listener a service exposes off
//!   loopback: it authenticates every request, applies the service's route
//!   policy, and proxies to the service's loopback HTTP server.
//! - [`lan`]: which addresses a fleet listener may bind (private LAN only).

#[cfg(not(target_arch = "wasm32"))]
pub mod id;
#[cfg(target_arch = "wasm32")]
#[path = "id_portable.rs"]
pub mod id;
#[cfg(not(target_arch = "wasm32"))]
pub mod front;
#[cfg(target_arch = "wasm32")]
#[path = "front_portable.rs"]
pub mod front;
pub mod lan;

/// `MAKEPAD_HOME`, else `~/.makepad` (`%USERPROFILE%\.makepad` on Windows),
/// else `<temp>/.makepad`. An empty `MAKEPAD_HOME` counts as unset.
pub fn makepad_home() -> std::path::PathBuf {
    if let Some(home) = std::env::var_os("MAKEPAD_HOME").filter(|home| !home.is_empty()) {
        return std::path::PathBuf::from(home);
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".makepad")
}
