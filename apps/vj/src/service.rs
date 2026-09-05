//! VJ-specific configuration mapping onto the SHARED connection lifecycle
//! (`makepad_asset_client::session`). Everything network-shaped —
//! discovery, connect/retry, identity pinning, runtime + subscriber
//! construction — lives in the client crate; this file only resolves the
//! VJ's env/token conventions and lane layout:
//!
//! - `VJ_ASSET_SERVER=ip:controlport:dataport` — pin HTTP planes;
//!   otherwise UDP-discover the local asset-ui / ai-content server
//!   (HTTP ports are ephemeral and change every launch),
//! - `VJ_ASSET_SERVER_ID=<32 hex>` — pin the beacon identity; default
//!   is `<local-root>/server-id` so we never attach a random LAN peer,
//! - `VJ_ASSET_TOKEN=mpat_…` — bearer token; then
//!   `VJ_ASSET_ADMIN_TOKEN_FILE`, then that same root's `admin-token`,
//!   then the legacy `~/.makepad-vj/asset-server.token`,
//! - `VJ_ASSET_CACHE=<dir>` — the root for EVERYTHING the VJ owns, not
//!   only the caches: marks, found loops, settings, the last-open surface
//!   and the token all sit under it. Default is the checkout's
//!   `local/vj`. See `data_root`.

#[cfg(not(target_arch = "wasm32"))]
use crate::lanes;
use makepad_asset_client::SessionConfig;
#[cfg(not(target_arch = "wasm32"))]
use makepad_asset_client::ApiEndpoints;
#[cfg(not(target_arch = "wasm32"))]
use std::net::{IpAddr, SocketAddr};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
fn from_hex16(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 32 {
        return None;
    }
    let value = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = (value(bytes[i * 2])? << 4) | value(bytes[i * 2 + 1])?;
    }
    Some(out)
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_server_spec(spec: &str) -> Option<ApiEndpoints> {
    let mut parts = spec.trim().split(':');
    let ip: IpAddr = parts.next()?.parse().ok()?;
    let control: u16 = parts.next()?.parse().ok()?;
    let data: u16 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ApiEndpoints {
        control: SocketAddr::new(ip, control),
        data: SocketAddr::new(ip, data),
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn read_trimmed(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// A local Asset Server already running under asset-ui / ai-content.
/// Token + listen come from the same catalog root so we never pair a
/// leftover `~/.makepad-vj` token with a newer server.
#[cfg(not(target_arch = "wasm32"))]
struct LocalAssetDb {
    endpoints: Option<ApiEndpoints>,
    server_id: Option<[u8; 16]>,
    token: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
fn attach_local_asset_db(home: &Path) -> Option<LocalAssetDb> {
    // Checkout-local Asset UI first; leftover $HOME trees are a fallback
    // for checkouts that never moved.
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/asset-ui/asset-server");
    let mut roots = vec![checkout];
    for leaf in [".makepad-asset-ai", ".makepad-ai-content"] {
        roots.push(home.join(leaf).join("asset-server"));
    }
    for root in roots {
        let token = read_trimmed(&root.join("admin-token"));
        let server_id = read_trimmed(&root.join("server-id")).and_then(|t| from_hex16(&t));
        let endpoints = read_trimmed(&root.join("listen"))
            .and_then(|t| parse_server_spec(t.lines().next().unwrap_or("")));
        if token.is_none() && endpoints.is_none() && server_id.is_none() {
            continue;
        }
        if token.is_some() || endpoints.is_some() {
            return Some(LocalAssetDb {
                endpoints,
                server_id,
                token,
            });
        }
    }
    None
}

/// Where the VJ keeps everything it owns: caches, marks, settings, tokens.
///
/// ONE answer, for every one of them. Some paths used to read
/// `$VJ_ASSET_CACHE` and some went straight to the checkout, so pointing
/// that variable somewhere else moved half the store and left the other
/// half where it was — the marks, the found loops and the last-open
/// surface stayed behind while their caches moved.
pub fn data_root() -> PathBuf {
    data_root_from(std::env::var("VJ_ASSET_CACHE").ok().as_deref())
}

/// The choice on its own, so it can be tested without touching the
/// environment every test in the process shares.
pub fn data_root_from(chosen: Option<&str>) -> PathBuf {
    match chosen {
        // An empty variable is not a choice. It used to become an empty
        // path, which quietly writes the whole store into the working
        // directory.
        Some(dir) if !dir.trim().is_empty() => PathBuf::from(dir.trim()),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/vj"),
    }
}

/// The VJ's session config from environment conventions.
#[cfg(not(target_arch = "wasm32"))]
pub fn session_config_from_env() -> SessionConfig {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let cache_parent = data_root();
    let local = attach_local_asset_db(&home);

    let mut config = SessionConfig::new(cache_parent.clone());
    // The catalog runtime carries EVERY small request the grids make:
    // listings, a detail and a manifest per tile, and every thumbnail blob.
    // The shared default of four workers is sized for an app that browses;
    // this one fills a wall of pads, so the lane scales with the machine.
    // (The bulk lane keeps its default — big transfers, not many small
    // ones.) The politeness valve is upstream, in how many resolves the
    // browse models will have in flight during a set.
    config.catalog_runtime.fast_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(4, 12);
    config.media_lanes = lanes::lane_leaves();
    assert_eq!(config.media_lanes.len(), lanes::LANE_COUNT);
    // Ports in `listen` are a same-machine hint only. Asset-ui binds
    // `:0` every launch, so a stale file would 401/refuse forever if we
    // treated it as authoritative. Pin the stable server-id + token and
    // let UDP discovery resolve the current planes. An explicit
    // VJ_ASSET_SERVER still wins; the session connector falls back to
    // discovery if that hint is dead.
    config.endpoints = std::env::var("VJ_ASSET_SERVER")
        .ok()
        .and_then(|spec| parse_server_spec(&spec))
        .or_else(|| local.as_ref().and_then(|db| db.endpoints));
    config.server_id = std::env::var("VJ_ASSET_SERVER_ID")
        .ok()
        .and_then(|t| from_hex16(t.trim()))
        .or_else(|| local.as_ref().and_then(|db| db.server_id));
    config.token = std::env::var("VJ_ASSET_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            let path = std::env::var("VJ_ASSET_ADMIN_TOKEN_FILE").ok()?;
            read_trimmed(Path::new(&path))
        })
        .or_else(|| local.as_ref().and_then(|db| db.token.clone()))
        .or_else(|| read_trimmed(&cache_parent.join("asset-server.token")));
    config
}

/// Browser sessions have no environment, home directory, or filesystem
/// cache. The static store keeps fetched content in memory; durable local
/// imports are owned separately by the app's `cx.storage`-backed store.
#[cfg(target_arch = "wasm32")]
pub fn session_config_from_env() -> SessionConfig {
    let base = makepad_asset_client::BaseUrl::parse("https://makepad.nl/vj/store/")
        .expect("valid built-in VJ static-store URL");
    let mut config = SessionConfig::static_site(base);
    // Keep the same lane indices as the native VJ. In particular, deck
    // audio is hard-routed to AUDIO_LANE (2); StaticSite's generic one-lane
    // default cannot service that request.
    config.media_lanes = crate::lanes::lane_leaves();
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_root_is_where_everything_goes() {
        assert_eq!(data_root_from(Some("F:/somewhere/else")), PathBuf::from("F:/somewhere/else"));
        assert_eq!(data_root_from(Some("  F:/padded  ")), PathBuf::from("F:/padded"));
    }

    #[test]
    fn no_choice_and_an_empty_choice_both_mean_the_checkout() {
        let fallback = data_root_from(None);
        assert!(fallback.ends_with("local/vj"), "{fallback:?}");
        // An empty variable used to become an empty path, which writes the
        // whole store into whatever directory the app happened to start in.
        assert_eq!(data_root_from(Some("")), fallback);
        assert_eq!(data_root_from(Some("   ")), fallback);
    }
}
