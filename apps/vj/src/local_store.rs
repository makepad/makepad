//! FULLY-LOCAL MODE: the VJ hosting its own Asset Server.
//!
//! The VJ has always been a thin client over a store somebody else runs —
//! normally asset-ui's embedded one. That is still the default and still
//! wins whenever it is actually reachable. What this module adds is the
//! answer to "and if it is not?": the VJ brings up the SAME server crate
//! in its own process, on 127.0.0.1 only, rooted in its own state directory,
//! and points its own client at it.
//!
//! The thin-client law does not bend for this. Hosting changes WHERE the
//! store runs, never who owns the content: the VJ still browses, publishes
//! and fetches over HTTP, through the same `AssetClient`, with the same
//! catalog-event subscription. Nothing durable moves into the app. Swap the
//! embedded server for a remote one and the rest of the VJ cannot tell.
//!
//! ## Loopback, deliberately
//!
//! `ServerConfig::new` already defaults to 127.0.0.1, and we keep it there
//! (port 0 = OS-assigned, or pinned by `VJ_ASSET_PORT`). asset-ui binds
//! `0.0.0.0` because it is meant to serve the LAN; a VJ hosting for itself
//! is not, so it does not listen on any other interface and runs no
//! discovery beacon. That is also what makes reference imports safe to
//! enable here (see [`makepad_asset_store::blobrefs`]): the ability to have
//! the store read a local path never leaves the machine.
//!
//! ## Choosing
//!
//! `VJ_ASSET_EMBED` decides, `auto` by default:
//!
//! - `never`  — attach only; if nothing is reachable, keep retrying as before.
//! - `auto`   — attach if an external store ANSWERS, else host.
//! - `always` — host, regardless of what else is running.
//!
//! An explicit `VJ_ASSET_SERVER=ip:control:data` pin always means attach:
//! naming a server is not something you do by accident.
//!
//! "Answers" is a real probe, not a guess: `GET /v1/health` against the
//! endpoints the local asset-ui root advertises, and failing that a short
//! listen for a UDP beacon. Both are cheap and both are checked before the
//! window appears, because the alternative — starting a second store while
//! the user's asset-ui is up — would split their library in two.

use makepad_asset_client::{ApiEndpoints, SessionConfig};
use makepad_asset_store::{AssetServer, BlobRefPolicy, ServerConfig};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

/// How the VJ got its store this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreMode {
    /// Attached to a store some other process runs (the usual case).
    Attached,
    /// Hosting the store in this process, on loopback.
    Hosting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EmbedPolicy {
    Never,
    Auto,
    Always,
}

fn embed_policy() -> EmbedPolicy {
    match std::env::var("VJ_ASSET_EMBED")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "never" | "off" | "0" => EmbedPolicy::Never,
        "always" | "force" | "1" => EmbedPolicy::Always,
        _ => EmbedPolicy::Auto,
    }
}

/// The VJ's own store root. Its own directory, never asset-ui's: two servers
/// over one WAL catalog is exactly what `server.lock` exists to refuse, and
/// racing the user's daemon for that lock is not a thing to do by default.
pub fn default_store_root() -> PathBuf {
    if let Ok(root) = std::env::var("VJ_ASSET_ROOT") {
        return PathBuf::from(root);
    }
    // `local/vjassets` (moved from `local/vj/asset-server` 2026-08-23): a
    // fresh default root, so a build with the new store layout simply seeds
    // a new store here and never has to migrate an old one.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/vjassets")
}

/// A locally hosted server binds loopback; reach it there. (Kept for the
/// case where a future config binds `0.0.0.0` — an unspecified address is
/// not a thing a client can connect to.)
fn localized_endpoints(server: &AssetServer) -> ApiEndpoints {
    let localize = |addr: SocketAddr| {
        if addr.ip().is_unspecified() {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
        } else {
            addr
        }
    };
    ApiEndpoints {
        control: localize(server.control_addr()),
        data: localize(server.data_addr()),
    }
}

/// Does something on the other end of `addr` answer `GET /v1/health` like an
/// Asset Server?
///
/// Deliberately minimal and deliberately SHORT: this runs on the startup
/// path, and its only job is to tell "the user's asset-ui is up" from "that
/// port file is stale". A 400 ms budget is generous for loopback and
/// invisible to a human. Anything unexpected reads as "no" — the cost of a
/// false negative is one extra local store; the cost of a false positive is
/// a VJ that never connects.
fn health_answers(addr: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));
    let req = format!(
        "GET /v1/health HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        addr
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 256];
    let mut got = 0usize;
    // One short read is enough: the status line is the first 15 bytes.
    while got < 16 {
        match stream.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(_) => break,
        }
    }
    buf[..got].starts_with(b"HTTP/1.1 200")
}

/// Listen briefly for an Asset Server beacon on the LAN discovery port.
///
/// This is the second chance for "asset-ui is running": its HTTP ports are
/// ephemeral and its `listen` file goes stale every launch, but its beacon
/// is live. One beacon period is 2 s, so we wait a little over one.
fn beacon_heard(wait_ms: u64) -> bool {
    use makepad_asset_client::discovery::DiscoveryListener;
    use makepad_asset_client::util::now_ms;
    let Ok(listener) = DiscoveryListener::start(
        makepad_asset_client::wire::DEFAULT_DISCOVERY_PORT,
        10_000,
        now_ms,
    ) else {
        return false;
    };
    let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms);
    while std::time::Instant::now() < deadline {
        if !listener.snapshot(now_ms()).is_empty() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Is an external store actually reachable right now?
fn external_store_reachable(config: &SessionConfig) -> bool {
    if let Some(endpoints) = config.endpoints {
        if health_answers(endpoints.control) {
            return true;
        }
    }
    // Ports move every asset-ui launch; the beacon does not.
    beacon_heard(2_400)
}

/// The in-process server, held for as long as the VJ runs.
///
/// Dropping it shuts the store down and joins every thread, so the field
/// holding this must be declared AFTER anything that talks to the store —
/// the same drop-order discipline asset-ui's `AssetStore` uses.
pub struct LocalStore {
    server: AssetServer,
    root: PathBuf,
}

impl LocalStore {
    pub fn endpoints(&self) -> ApiEndpoints {
        localized_endpoints(&self.server)
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn control_addr(&self) -> SocketAddr {
        self.server.control_addr()
    }

    pub fn data_addr(&self) -> SocketAddr {
        self.server.data_addr()
    }
}

/// What `resolve` decided, and why — the `note` is written straight into the
/// VJ's status line, because "which store am I on" is the first thing that
/// matters when content does not show up.
pub struct Resolved {
    pub config: SessionConfig,
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
pub fn resolve(base: SessionConfig) -> Resolved {
    let policy = embed_policy();
    let pinned = std::env::var("VJ_ASSET_SERVER")
        .ok()
        .is_some_and(|s| !s.trim().is_empty());

    if pinned || policy == EmbedPolicy::Never {
        let note = if pinned {
            "asset server pinned by VJ_ASSET_SERVER".to_string()
        } else {
            "attach-only (VJ_ASSET_EMBED=never)".to_string()
        };
        return Resolved { config: base, mode: StoreMode::Attached, local: None, note };
    }

    if policy == EmbedPolicy::Auto && external_store_reachable(&base) {
        return Resolved {
            config: base,
            mode: StoreMode::Attached,
            local: None,
            note: "attached to the running asset server".to_string(),
        };
    }

    let root = default_store_root();
    match host(&root) {
        Ok((local, token)) => {
            let mut config = base;
            config.endpoints = Some(local.endpoints());
            config.server_id = Some(local.server.server_id());
            config.token = Some(token);
            let note = format!("local store on {}", local.control_addr());
            Resolved { config, mode: StoreMode::Hosting, local: Some(local), note }
        }
        Err(error) => {
            // Hosting failed — most often because another process already
            // holds this root's lock. Say so and fall back to the attach
            // path, which keeps retrying discovery on its own.
            Resolved {
                config: base,
                mode: StoreMode::Attached,
                local: None,
                note: format!("local store unavailable ({error}); attaching instead"),
            }
        }
    }
}

/// Bring up the in-process server on loopback and read back its admin token.
fn host(root: &std::path::Path) -> Result<(LocalStore, String), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("create store root: {e}"))?;
    let port: u16 = std::env::var("VJ_ASSET_PORT")
        .ok()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(0);
    let mut cfg = ServerConfig::new(root.to_path_buf());
    // LOOPBACK ONLY. Not a default we inherit — a decision. A VJ hosting for
    // itself has no business accepting connections from the network, and
    // the reference-import route below is only safe under exactly this.
    cfg.control_addr = SocketAddr::from(([127, 0, 0, 1], port));
    // The data plane always takes an OS-assigned port: pinning one would
    // only create a second thing to collide.
    cfg.data_addr = SocketAddr::from(([127, 0, 0, 1], 0));
    cfg.bootstrap_admin = true;
    // No beacon: this store is for this process. Announcing it would invite
    // other apps onto a library that is not the user's main one.
    cfg.discovery = None;
    // Reference imports ON: the whole point of the local mode is pointing
    // the VJ at a directory of video that stays where it is. Loopback-only
    // and no prefix restriction, which is exactly the privilege this
    // process already has over the user's own files.
    cfg.blob_refs = BlobRefPolicy::local_host();
    cfg.log = true;
    let server = AssetServer::start(cfg).map_err(|e| format!("{e}"))?;
    let token = std::fs::read_to_string(root.join("admin-token"))
        .map_err(|e| format!("admin token: {e}"))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err("admin token file empty".to_string());
    }
    Ok((LocalStore { server, root: root.to_path_buf() }, token))
}
