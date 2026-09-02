//! FULLY-LOCAL MODE: the VJ hosting its own Asset Server.
//!
//! The mechanics live in [`makepad_app_asset_server::embed`] and are shared
//! with the sandbox: attach whenever a real store answers (health probe,
//! UDP beacon, the main store's server.lock), otherwise bring up the SAME
//! server crate in-process — loopback only, no beacon, the ai-content
//! library publisher riding along, rooted in the user's main library when
//! one exists. `VJ_ASSET_EMBED` / `VJ_ASSET_ROOT` / `VJ_ASSET_PORT` steer
//! it; an explicit `VJ_ASSET_SERVER` pin always means attach.

use makepad_asset_client::SessionConfig;

#[cfg(not(target_arch = "wasm32"))]
use makepad_app_asset_server::embed;

#[cfg(not(target_arch = "wasm32"))]
pub use makepad_app_asset_server::embed::{LocalStore, StoreMode};

/// The web build has no native host to own. Its catalog comes from the
/// static-site session; browser-local publication is handled by the embedded
/// store service rather than pretending a socket server exists.
#[cfg(target_arch = "wasm32")]
pub struct LocalStore;

/// What `resolve` decided, and why — the `note` is written straight into the
/// VJ's status line, because "which store am I on" is the first thing that
/// matters when content does not show up.
pub struct Resolved {
    pub config: SessionConfig,
    #[cfg(not(target_arch = "wasm32"))]
    pub mode: StoreMode,
    pub local: Option<LocalStore>,
    pub note: String,
}

/// Decide between attaching and hosting, and produce a session config that
/// is ready to hand to `SessionConnector::start`.
///
/// `base` is the VJ's existing env-derived config (see
/// [`crate::service::session_config_from_env`]) — everything about cache
/// roots, lanes and token conventions is already resolved there, and this
/// only overrides the three fields that say WHICH server.
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve(base: SessionConfig) -> Resolved {
    let pinned = std::env::var("VJ_ASSET_SERVER")
        .ok()
        .is_some_and(|s| !s.trim().is_empty());
    let resolved = embed::resolve("VJ", "vjassets", pinned, base.endpoints);
    let mut config = base;
    if let Some(local) = &resolved.local {
        config.endpoints = Some(local.endpoints());
        config.server_id = Some(local.server_id());
        config.token = Some(local.token().to_string());
    }
    Resolved {
        config,
        mode: resolved.mode,
        local: resolved.local,
        note: resolved.note,
    }
}

#[cfg(target_arch = "wasm32")]
pub fn resolve(base: SessionConfig) -> Resolved {
    Resolved {
        config: base,
        local: None,
        note: "web catalog at https://makepad.nl/vj/store/".to_string(),
    }
}
