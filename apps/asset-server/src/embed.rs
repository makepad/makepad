//! FULLY-LOCAL MODE, shared: an app hosting its own Asset Server.
//!
//! The VJ and the sandbox are thin clients over a store somebody else runs —
//! normally asset-ui's embedded one, or the standalone host. That stays the
//! default and still wins whenever it is actually reachable. This module is
//! the shared answer to "and if it is not?": the app brings up [`Host`] in
//! its own process, on 127.0.0.1 only, rooted in the user's main library
//! when one exists on disk (a private seed root only when none does), with
//! the ai-content library publisher riding along.
//!
//! The thin-client law does not bend for this. Hosting changes WHERE the
//! store runs, never who owns the content: the app still browses, publishes
//! and fetches over HTTP, through the same `AssetClient`, with the same
//! catalog-event subscription. Nothing durable moves into the app.
//!
//! ## Loopback, deliberately
//!
//! The embedded host binds 127.0.0.1 for both planes (port 0 = OS-assigned,
//! or pinned by `<PREFIX>_ASSET_PORT`) and runs NO discovery beacon: an app
//! hosting for itself has no business accepting connections from the
//! network, and announcing the store would invite other apps onto a private
//! instance of the user's library. That is also what makes reference
//! imports safe to enable here (see `makepad_asset_store::blobrefs`).
//!
//! ## Choosing
//!
//! `<PREFIX>_ASSET_EMBED` decides, `auto` by default:
//!
//! - `never`  — attach only; if nothing is reachable, behave as before.
//! - `auto`   — attach if an external store ANSWERS, else host.
//! - `always` — host, regardless of what else is running.
//!
//! "Answers" is a real probe, not a guess: `GET /v1/health` against the
//! endpoints the caller advertises, failing that a short listen for a UDP
//! beacon, failing that the main store root's `server.lock` — a held lock
//! is proof the user's library is hosted RIGHT NOW whatever its ports, and
//! self-hosting then would silently split their library in two.

use crate::{Host, HostConfig};
use makepad_asset_client::ApiEndpoints;
use makepad_asset_store::BlobRefPolicy;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How the app got its store this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreMode {
    /// Attached to a store some other process runs (the usual case).
    Attached,
    /// Hosting the store in this process, on loopback.
    Hosting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedPolicy {
    Never,
    Auto,
    Always,
}

/// `<PREFIX>_ASSET_EMBED`, `auto` when unset or unrecognised.
pub fn embed_policy(prefix: &str) -> EmbedPolicy {
    match std::env::var(format!("{prefix}_ASSET_EMBED"))
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

/// The checkout this binary was built in — every default root hangs off it.
fn checkout_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Where asset-ui keeps the user's main store.
pub fn main_store_root() -> PathBuf {
    checkout_root().join("local/asset-ui/asset-server")
}

/// The ai-content library this checkout generates into — the same default
/// the standalone asset-server and asset-ui use.
fn library_root() -> PathBuf {
    checkout_root().join("local/ai_content_library")
}

/// The root the app hosts over when it hosts.
///
/// `<PREFIX>_ASSET_ROOT` pins it. Otherwise the user's MAIN library —
/// asset-ui's store root — whenever it holds a catalog: hosting only happens
/// after the probe found nobody serving, and `AssetServer::start` takes the
/// same `server.lock` the daemon would, so losing the race resolves to
/// "attach instead", never to two servers over one WAL. The private seed
/// root is only for a machine with no main library at all — self-hosting a
/// fresh empty root next to a full library reads as "the app lost my
/// content".
pub fn default_store_root(prefix: &str, seed_dir: &str) -> PathBuf {
    if let Ok(root) = std::env::var(format!("{prefix}_ASSET_ROOT")) {
        return PathBuf::from(root);
    }
    let main = main_store_root();
    if main.join("catalog.sqlite3").exists() {
        return main;
    }
    checkout_root().join("local").join(seed_dir)
}

/// Does something on the other end of `addr` answer `GET /v1/health` like an
/// Asset Server?
///
/// Deliberately minimal and deliberately SHORT: this runs on the startup
/// path, and its only job is to tell "the user's server is up" from "that
/// port file is stale". A 400 ms budget is generous for loopback and
/// invisible to a human. Anything unexpected reads as "no" — the cost of a
/// false negative is one extra local store; the cost of a false positive is
/// an app that never connects.
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
/// This is the second chance for "a server is running": HTTP ports are
/// ephemeral and `listen` files go stale every launch, but a beacon is
/// live. One beacon period is 2 s, so we wait a little over one.
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
pub fn external_store_reachable(hinted: Option<ApiEndpoints>) -> bool {
    if let Some(endpoints) = hinted {
        if health_answers(endpoints.control) {
            return true;
        }
    }
    // Ports move every launch; the beacon does not.
    if beacon_heard(2_400) {
        return true;
    }
    // THE USER'S MAIN STORE IS ALIVE BUT NOT ANSWERING YET (a succession
    // handover, a stale `listen` file, a beacon missed by a hair): the
    // lock holder is proof it exists. Self-hosting here would SILENTLY
    // put this app on a private empty store — "no content in the grid" —
    // so treat a held lock as reachable and let the attach path keep
    // discovering; it retries on its own.
    main_store_lock_held()
}

/// True when another process holds the main (asset-ui) store's server
/// lock — i.e. the user's library is hosted right now, whatever its ports.
fn main_store_lock_held() -> bool {
    let root = main_store_root();
    let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .open(root.join("server.lock"))
    else {
        return false;
    };
    // The SAME advisory lock the store takes (File::try_lock): if we can
    // take it, nobody serves that root; the file drop releases it.
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(_) => true,
    }
}

/// The in-process host (server + library publisher), held for as long as
/// the app runs.
///
/// Dropping it stops the publisher first and the server last, so the field
/// holding this must be declared AFTER anything that talks to the store —
/// the same drop-order discipline asset-ui's `AssetStore` uses.
pub struct LocalStore {
    host: Host,
    root: PathBuf,
}

impl LocalStore {
    pub fn endpoints(&self) -> ApiEndpoints {
        self.host.endpoints()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn control_addr(&self) -> SocketAddr {
        self.host.endpoints().control
    }

    pub fn data_addr(&self) -> SocketAddr {
        self.host.endpoints().data
    }

    pub fn server_id(&self) -> [u8; 16] {
        self.host.server_id()
    }

    pub fn token(&self) -> &str {
        self.host.token()
    }

    pub fn publisher_running(&self) -> bool {
        self.host.publisher_running()
    }
}

/// What [`resolve`] decided, and why — the `note` belongs in the app's
/// status line, because "which store am I on" is the first thing that
/// matters when content does not show up.
pub struct Resolved {
    pub mode: StoreMode,
    /// The embedded host, when hosting. The caller owns it for the life of
    /// the app and points its client at [`LocalStore::endpoints`] with
    /// [`LocalStore::server_id`] and [`LocalStore::token`].
    pub local: Option<LocalStore>,
    pub note: String,
}

/// Decide between attaching and hosting.
///
/// `pinned` is the caller saying "an explicit server was named" — naming a
/// server is not something you do by accident, so it always means attach.
/// `hinted` is where the caller last knew a server to live, for the health
/// probe.
pub fn resolve(
    prefix: &str,
    seed_dir: &str,
    pinned: bool,
    hinted: Option<ApiEndpoints>,
) -> Resolved {
    let policy = embed_policy(prefix);

    if pinned || policy == EmbedPolicy::Never {
        let note = if pinned {
            format!("asset server pinned by {prefix}_ASSET_SERVER")
        } else {
            format!("attach-only ({prefix}_ASSET_EMBED=never)")
        };
        return Resolved { mode: StoreMode::Attached, local: None, note };
    }

    if policy == EmbedPolicy::Auto && external_store_reachable(hinted) {
        return Resolved {
            mode: StoreMode::Attached,
            local: None,
            note: "attached to the running asset server".to_string(),
        };
    }

    let root = default_store_root(prefix, seed_dir);
    match host_at(&root, prefix) {
        Ok(local) => {
            let note = format!(
                "local store on {} over {} · library publisher {}",
                local.control_addr(),
                root.file_name().and_then(|n| n.to_str()).unwrap_or("store"),
                if local.publisher_running() { "on" } else { "off" }
            );
            Resolved { mode: StoreMode::Hosting, local: Some(local), note }
        }
        Err(error) => {
            // Hosting failed — most often because another process already
            // holds this root's lock. Say so and fall back to the attach
            // path, which keeps retrying discovery on its own.
            Resolved {
                mode: StoreMode::Attached,
                local: None,
                note: format!("local store unavailable ({error}); attaching instead"),
            }
        }
    }
}

/// Bring up the in-process host on loopback.
///
/// This is the standalone asset-server's own [`Host`] — catalog + CAS plus
/// the ai-content LIBRARY PUBLISHER, so a self-hosted app sees the same
/// `local/ai_content_library` rows asset-ui and the standalone server
/// publish, not a bare seed store. Two deployment defaults are overridden,
/// deliberately, and both stay: LOOPBACK ONLY, and NO discovery beacon.
fn host_at(root: &Path, prefix: &str) -> Result<LocalStore, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("create store root: {e}"))?;
    let port: u16 = std::env::var(format!("{prefix}_ASSET_PORT"))
        .ok()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(0);
    let mut cfg = HostConfig::new(root.to_path_buf());
    cfg.control_addr = SocketAddr::from(([127, 0, 0, 1], port));
    // The data plane always takes an OS-assigned port: pinning one would
    // only create a second thing to collide.
    cfg.data_addr = SocketAddr::from(([127, 0, 0, 1], 0));
    cfg.beacon = false;
    // The user's generation library, when this checkout has one: the
    // publisher keeps turning it into catalog rows exactly as the
    // standalone server would.
    let library = library_root();
    cfg.library = library.join("index.json").exists().then_some(library);
    // Reference imports ON: the whole point of the local mode is pointing
    // the store at content that stays where it is. Loopback-only and no
    // prefix restriction, which is exactly the privilege this process
    // already has over the user's own files.
    cfg.blob_refs = BlobRefPolicy::local_host();
    cfg.log = true;
    let host = Host::start(&cfg).map_err(|e| format!("{e}"))?;
    if host.token().is_empty() {
        return Err("admin token file empty".to_string());
    }
    Ok(LocalStore { host, root: root.to_path_buf() })
}
