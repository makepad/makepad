//! Reusable connection lifecycle: from "somewhere on the LAN there may be
//! an Asset Server" to a verified, ready-to-use session — on a worker
//! thread, with retry, typed status, and clean teardown.
//!
//! UI hosts (the VJ, AI Content, the Asset Store) share this instead of
//! re-implementing discovery/connect/retry loops. The caller supplies
//! POLICY only — explicit endpoints or discovery, an optional pinned server
//! identity, and a resolved bearer token (env/file conventions are app
//! business) — and receives [`SessionHandles`]:
//!
//! - a `catalog` [`ClientRuntime`] (search/detail/manifests/thumbnails/jobs)
//!   and a `media` [`ClientRuntime`] (large blobs) on two distinct
//!   single-owner cache roots, so one long transfer never queues behind
//!   catalog work,
//! - a [`CatalogSubscriber`] following committed catalog events.
//!
//! The connector retries forever with capped backoff until it hands over a
//! session, the handle is dropped, or [`SessionConnector::stop`] is called;
//! every wait is sliced so teardown is prompt.

use crate::api::ApiEndpoints;
use crate::client::{AssetClient, ClientConfig};
use crate::discovery::{content_client_caps, DiscoveryListener};
use crate::error::{ClientError, ClientResult};
use crate::runtime::ClientRuntime;
use crate::subscriber::{CatalogSubscriber, CatalogSubscriberConfig};
use crate::util::now_ms;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// Explicit endpoints; `None` = LAN discovery.
    pub endpoints: Option<ApiEndpoints>,
    /// Expected server identity. With discovery it selects the beacon; with
    /// explicit endpoints it pins the health handshake.
    pub server_id: Option<[u8; 16]>,
    /// Bearer token (`mpat_…`), already resolved by the caller.
    pub token: Option<String>,
    /// Parent directory for the session's cache roots.
    pub cache_parent: PathBuf,
    /// Leaf directory name of the catalog runtime's cache root.
    pub catalog_cache_leaf: String,
    /// Leaf names of the media lanes, one bounded FIFO runtime per lane on
    /// its own single-owner cache root. One lane suffices for most apps;
    /// latency-sensitive hosts (the VJ) split lanes so a long video
    /// transfer never queues in front of deck/SFX/mesh media.
    pub media_lanes: Vec<String>,
    /// UDP port discovery listens on.
    pub discovery_port: u16,
    /// How long one discovery pass waits for a usable beacon.
    pub discovery_wait_ms: u64,
    pub subscriber: CatalogSubscriberConfig,
    /// Connect retry backoff bounds.
    pub retry_min_ms: u64,
    pub retry_max_ms: u64,
}

impl SessionConfig {
    pub fn new(cache_parent: impl Into<PathBuf>) -> SessionConfig {
        SessionConfig {
            endpoints: None,
            server_id: None,
            token: None,
            cache_parent: cache_parent.into(),
            catalog_cache_leaf: "cache-catalog".to_string(),
            media_lanes: vec!["cache-media".to_string()],
            discovery_port: crate::wire::DEFAULT_DISCOVERY_PORT,
            discovery_wait_ms: 4_000,
            subscriber: CatalogSubscriberConfig::default_v1(),
            retry_min_ms: 1_000,
            retry_max_ms: 10_000,
        }
    }

    fn validate(&self) -> ClientResult<()> {
        self.subscriber.validate()?;
        if self.catalog_cache_leaf.is_empty()
            || self.media_lanes.is_empty()
            || self.media_lanes.len() > 8
        {
            return Err(ClientError::InvalidInput { what: "session cache leaves" });
        }
        for (i, lane) in self.media_lanes.iter().enumerate() {
            if lane.is_empty()
                || *lane == self.catalog_cache_leaf
                || self.media_lanes[..i].contains(lane)
            {
                return Err(ClientError::InvalidInput { what: "session cache leaves" });
            }
        }
        if self.discovery_wait_ms == 0
            || self.retry_min_ms == 0
            || self.retry_max_ms < self.retry_min_ms
            || self.retry_max_ms > 60_000
        {
            return Err(ClientError::InvalidInput { what: "session timing bounds" });
        }
        Ok(())
    }
}

/// Everything a connected app works with. `media[i]` corresponds to
/// `SessionConfig::media_lanes[i]`.
pub struct SessionHandles {
    pub catalog: ClientRuntime,
    pub media: Vec<ClientRuntime>,
    pub subscriber: CatalogSubscriber,
    pub server_label: String,
    pub server_id: [u8; 16],
    /// Control/data planes of the verified session — enough for a second
    /// client (chat broker) without re-discovering.
    pub endpoints: ApiEndpoints,
    pub token: Option<String>,
}

impl SessionHandles {
    /// Deterministic teardown: stop the subscriber's long-poll, then join
    /// every runtime worker. Call from a background/exit path — the runtime
    /// joins wait out any in-flight transfer.
    pub fn shutdown(self) {
        self.subscriber.shutdown();
        self.catalog.shutdown();
        for lane in self.media {
            lane.shutdown();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Discovering,
    Connecting { server: String },
    Connected { server: String },
    /// Honest failure + when the next attempt fires.
    Retrying { error: String, in_secs: u64 },
}

pub enum SessionMsg {
    Status(SessionStatus),
    Up(Box<SessionHandles>),
}

/// Handle to the connect worker. Dropping it (or `stop()`) makes the worker
/// exit at its next check; a session already handed over stays alive.
pub struct SessionConnector {
    rx: Receiver<SessionMsg>,
    stopping: Arc<AtomicBool>,
}

impl SessionConnector {
    pub fn start(config: SessionConfig) -> ClientResult<SessionConnector> {
        config.validate()?;
        let (tx, rx) = channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let stop_worker = stopping.clone();
        std::thread::Builder::new()
            .name("asset-session-connect".into())
            .spawn(move || {
                let mut backoff_ms = config.retry_min_ms;
                // A listen-file / env hint is tried once. Asset servers bind
                // ephemeral ports and UDP-announce the live pair; if the hint
                // is stale, fall through to discovery instead of retrying a
                // dead SocketAddr forever.
                let mut allow_hint = config.endpoints.is_some();
                loop {
                    if stop_worker.load(Ordering::Acquire) {
                        return;
                    }
                    let status = |s: SessionStatus| tx.send(SessionMsg::Status(s)).is_ok();
                    let hinted = allow_hint.then_some(config.endpoints).flatten();
                    let endpoints = match hinted {
                        Some(endpoints) => Some((endpoints, config.server_id)),
                        None => {
                            if !status(SessionStatus::Discovering) {
                                return;
                            }
                            discover(&config, &stop_worker)
                        }
                    };
                    if stop_worker.load(Ordering::Acquire) {
                        return;
                    }
                    let used_hint = hinted.is_some();
                    let attempt = endpoints
                        .ok_or_else(|| "no server discovered on the LAN".to_string())
                        .and_then(|(endpoints, pin)| {
                            let label = format!("{}", endpoints.control);
                            if !status(SessionStatus::Connecting { server: label }) {
                                return Err("session consumer closed".into());
                            }
                            connect_all(&config, endpoints, pin)
                        });
                    match attempt {
                        Ok(handles) => {
                            let label = handles.server_label.clone();
                            if tx.send(SessionMsg::Up(Box::new(handles))).is_ok() {
                                let _ = tx.send(SessionMsg::Status(SessionStatus::Connected {
                                    server: label,
                                }));
                            }
                            return;
                        }
                        Err(error) => {
                            if error == "session consumer closed" {
                                return;
                            }
                            if used_hint {
                                allow_hint = false;
                                continue;
                            }
                            let wait = backoff_ms.min(config.retry_max_ms);
                            if !status(SessionStatus::Retrying {
                                error,
                                in_secs: wait.div_ceil(1000),
                            }) {
                                return;
                            }
                            if !sleep_sliced(wait, &stop_worker) {
                                return;
                            }
                            backoff_ms = (backoff_ms * 2).min(config.retry_max_ms);
                        }
                    }
                }
            })
            .map_err(|e| ClientError::Io { op: "spawn session connector", kind: e.kind() })?;
        Ok(SessionConnector { rx, stopping })
    }

    /// Drain pending status/handover messages without blocking a frame.
    pub fn poll(&mut self) -> Vec<SessionMsg> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(msg) => out.push(msg),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Ask the worker to stop retrying. Prompt (sliced waits) and safe from
    /// any thread; a session already handed over is unaffected.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
}

impl Drop for SessionConnector {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
    }
}

fn sleep_sliced(ms: u64, stopping: &AtomicBool) -> bool {
    let mut left = ms;
    while left > 0 {
        if stopping.load(Ordering::Acquire) {
            return false;
        }
        let slice = left.min(50);
        std::thread::sleep(Duration::from_millis(slice));
        left -= slice;
    }
    !stopping.load(Ordering::Acquire)
}

/// One bounded discovery pass for a usable beacon (matching the pinned
/// identity when one is configured).
fn discover(
    config: &SessionConfig,
    stopping: &AtomicBool,
) -> Option<(ApiEndpoints, Option<[u8; 16]>)> {
    let mut listener =
        DiscoveryListener::start(config.discovery_port, 10_000, now_ms).ok()?;
    let deadline =
        std::time::Instant::now() + Duration::from_millis(config.discovery_wait_ms);
    let found = loop {
        if stopping.load(Ordering::Acquire) {
            break None;
        }
        let candidate = match config.server_id {
            Some(id) => listener.find(&id, now_ms()),
            None => listener.pick(content_client_caps(), now_ms()),
        };
        if let Some(server) = candidate {
            break Some(server);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    listener.stop();
    let server = found?;
    Some((
        ApiEndpoints {
            control: server.control_endpoint(),
            data: server.data_endpoint(),
        },
        Some(server.server_id),
    ))
}

/// Connect both runtimes (distinct single-owner cache roots) + subscriber.
fn connect_all(
    config: &SessionConfig,
    endpoints: ApiEndpoints,
    pin: Option<[u8; 16]>,
) -> Result<SessionHandles, String> {
    let client_config = |leaf: &str| {
        let mut c = ClientConfig::new(config.cache_parent.join(leaf));
        c.token = config.token.clone();
        c
    };
    let catalog_client =
        AssetClient::connect(client_config(&config.catalog_cache_leaf), endpoints, pin)
            .map_err(|e| e.to_string())?;
    let server_id = catalog_client.server_id();
    // Media lanes pin the identity the catalog connect verified.
    let mut media = Vec::with_capacity(config.media_lanes.len());
    for lane in &config.media_lanes {
        let client = AssetClient::connect(client_config(lane), endpoints, Some(server_id))
            .map_err(|e| e.to_string())?;
        media.push(ClientRuntime::start(client).map_err(|e| e.to_string())?);
    }
    let subscriber = catalog_client
        .subscribe_catalog(config.subscriber)
        .map_err(|e| e.to_string())?;
    let catalog = ClientRuntime::start(catalog_client).map_err(|e| e.to_string())?;
    Ok(SessionHandles {
        catalog,
        media,
        subscriber,
        server_label: format!("{}", endpoints.control),
        server_id,
        endpoints,
        token: config.token.clone(),
    })
}
