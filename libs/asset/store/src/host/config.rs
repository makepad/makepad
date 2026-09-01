//! Explicit runtime configuration. Every operational knob is a named field
//! here; nothing is a global, an env var, or an implicit constant. Embedders
//! (the Asset Store) construct this directly; the binary maps flags onto it.

use crate::{Budgets, ServerError, ServerResult};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

/// Default UDP LAN discovery port. Deliberately distinct from the ai-content
/// fleet port (41830) and from the HTTP planes.
pub const DEFAULT_DISCOVERY_PORT: u16 = 41871;

/// Whether this server will catalogue content it does not copy, and from
/// whom (see [`crate::blobrefs`]).
///
/// Admitting a blob BY REFERENCE means handing the server a filesystem path
/// and having it read that file — so the route is, precisely, "read any file
/// this process can read, then serve it by digest". That is exactly the
/// privilege of the process itself, which is fine for an app hosting its own
/// store on loopback and is NOT fine for anything reachable off the box.
/// Hence: OFF unless an embedder turns it on, loopback-only by default, and
/// an optional prefix allowlist for embedders that want to narrow it further.
#[derive(Clone, Debug)]
pub struct BlobRefPolicy {
    /// Master switch. `false` = `POST /v1/blobs/ref` answers 404 as though
    /// the route did not exist.
    pub enabled: bool,
    /// Only accept reference admission from a loopback peer. Leave `true`
    /// unless the paths and the callers are both already trusted — a remote
    /// caller naming a server-local path is a file-read oracle.
    pub loopback_only: bool,
    /// Directory prefixes a referenced path must live under. EMPTY means "no
    /// prefix restriction", which is only sound together with
    /// `loopback_only` and a token the host process minted for itself.
    pub roots: Vec<PathBuf>,
}

impl Default for BlobRefPolicy {
    fn default() -> Self {
        Self { enabled: false, loopback_only: true, roots: Vec::new() }
    }
}

impl BlobRefPolicy {
    /// The shape an app that hosts its own store on loopback wants: on,
    /// loopback-only, no prefix restriction (the app already runs as the
    /// user who owns the files).
    pub fn local_host() -> Self {
        Self { enabled: true, loopback_only: true, roots: Vec::new() }
    }

    /// Is this path admissible under the prefix allowlist? An empty
    /// allowlist admits everything; a non-empty one admits only paths under
    /// one of its entries.
    pub fn path_allowed(&self, path: &std::path::Path) -> bool {
        if self.roots.is_empty() {
            return true;
        }
        self.roots.iter().any(|root| path.starts_with(root))
    }
}

/// UDP LAN discovery announcement config. Present = beacon on.
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    /// UDP port beacons are sent to (and listeners bind).
    pub port: u16,
    /// Beacon destination IP. The broadcast address for real LAN use;
    /// tests point it at 127.0.0.1 for deterministic loopback delivery.
    pub target_ip: IpAddr,
    /// Beacon period in milliseconds.
    pub interval_ms: u64,
    /// Advertised capability bits (see `discovery::caps`), bounded to the
    /// defined bit set. Never carries content, names, or secrets.
    pub capability_bits: u32,
}

impl DiscoveryConfig {
    pub fn lan_default() -> Self {
        Self {
            port: DEFAULT_DISCOVERY_PORT,
            target_ip: IpAddr::V4(Ipv4Addr::BROADCAST),
            interval_ms: 2000,
            capability_bits: super::discovery::caps::ALL_V1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Server root directory: CAS, catalog, transport DB, server-id,
    /// admin-token all live under here.
    pub root: PathBuf,
    /// Core admission budgets (blob/manifest/job bounds). Tests shrink these
    /// to hit refusals cheaply.
    pub budgets: Budgets,
    /// Control plane bind address (auth, catalog, publication, jobs, workers).
    pub control_addr: SocketAddr,
    /// Data plane bind address (blob upload/download, thumbnails, media).
    pub data_addr: SocketAddr,
    /// Most concurrent connections per plane (each gets one bounded thread).
    /// At capacity, new connections receive an immediate 503 and close —
    /// explicit refusal, never silent backlog starvation.
    pub control_max_conns: usize,
    pub data_max_conns: usize,
    /// Socket-level timeouts for one read/write call, milliseconds.
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
    /// Wall-clock bound for receiving one full request head once its first
    /// byte has arrived (slowloris cap).
    pub head_deadline_ms: u64,
    /// How long an idle keep-alive connection may sit waiting for its next
    /// request before the server closes it.
    pub keepalive_idle_ms: u64,
    /// Wall-clock bound for receiving one full request body, per plane.
    pub control_body_deadline_ms: u64,
    pub data_body_deadline_ms: u64,
    /// Keep-alive: most requests served on one connection.
    pub max_requests_per_conn: u32,
    /// Largest control-plane body (canonical manifests, framed game bodies).
    pub control_max_body_bytes: u64,
    /// Largest JSON body on any route (well under the control body cap).
    pub max_json_body_bytes: u64,
    /// Lifetime of the bootstrap admin token.
    pub admin_token_ttl_ms: u64,
    /// Longest token lifetime the issuance endpoint accepts.
    pub max_token_ttl_ms: u64,
    /// Period of the lease-expiry janitor.
    pub janitor_interval_ms: u64,
    /// Most catalog events retained for `/v1/events` resume; older events
    /// evict and out-of-window cursors get an explicit `gap`.
    pub event_journal_cap: usize,
    /// Longest server-side long-poll wait `/v1/events` grants one request.
    pub event_max_wait_ms: u64,
    /// Most events one `/v1/events` response may carry.
    pub event_max_batch: usize,
    /// Most connection threads allowed to park in an event wait at once;
    /// over the cap a poll returns empty immediately instead of parking.
    pub event_max_waiters: usize,
    /// Blob GC (see `crate::gc`). A run is a sequence of bounded steps, and
    /// these knobs decide how much of it any one actor performs: a request
    /// does at most `gc_max_steps_per_request` (so `POST /v1/gc` answers in
    /// bounded time however large the store is), and each janitor tick does
    /// at most `gc_janitor_steps`, which is what finishes a run nobody is
    /// polling. Zero janitor steps = GC only advances when asked.
    pub gc_max_steps_per_request: u32,
    pub gc_janitor_steps: u32,
    /// Default grace window: bytes recorded (or re-referenced by a deduped
    /// upload) inside this window of a run's start are never swept, so an
    /// upload whose manifest has not landed yet cannot be collected.
    pub gc_grace_ms: u64,
    /// Rows one mark/sweep step reads.
    pub gc_mark_batch: u32,
    pub gc_sweep_batch: u32,
    /// Assets one retention step visits.
    pub gc_retain_batch: u32,
    /// Ordered batch blob pull (`POST /v1/blobs/fetch`). Bounded so one
    /// request can never become an unbounded read: most items per batch, and
    /// most bytes one batch may stream.
    pub batch_max_items: u32,
    pub batch_max_bytes: u64,
    /// Ensure the admin principal + a fresh admin token at startup, writing
    /// the token (once, mode 0600) to `<root>/admin-token`.
    pub bootstrap_admin: bool,
    /// UDP LAN discovery beacon; None = off.
    pub discovery: Option<DiscoveryConfig>,
    /// Reference blobs (content catalogued without copying). Off by default;
    /// see [`BlobRefPolicy`] for why turning it on is a privilege decision.
    pub blob_refs: BlobRefPolicy,
    /// Log to stderr. Never logs secrets, headers, or bodies.
    pub log: bool,
}


impl ServerConfig {
    /// Local defaults: loopback planes, discovery off (explicitly opt in),
    /// no bootstrap. Embedders start from here.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            budgets: Budgets::default_v1(),
            control_addr: SocketAddr::from(([127, 0, 0, 1], 9701)),
            data_addr: SocketAddr::from(([127, 0, 0, 1], 9702)),
            control_max_conns: 64,
            data_max_conns: 64,
            read_timeout_ms: 10_000,
            write_timeout_ms: 10_000,
            head_deadline_ms: 10_000,
            keepalive_idle_ms: 30_000,
            control_body_deadline_ms: 30_000,
            data_body_deadline_ms: 600_000,
            max_requests_per_conn: 256,
            control_max_body_bytes: 4 * 1024 * 1024,
            max_json_body_bytes: 256 * 1024,
            admin_token_ttl_ms: 30 * 24 * 60 * 60 * 1000,
            max_token_ttl_ms: 400 * 24 * 60 * 60 * 1000,
            janitor_interval_ms: 5000,
            event_journal_cap: 4096,
            event_max_wait_ms: 30_000,
            event_max_batch: 256,
            event_max_waiters: 32,
            gc_max_steps_per_request: 64,
            gc_janitor_steps: 16,
            gc_grace_ms: 60 * 60 * 1000,
            gc_mark_batch: 64,
            gc_sweep_batch: 128,
            gc_retain_batch: 64,
            batch_max_items: 32,
            batch_max_bytes: 16 * 1024 * 1024,
            bootstrap_admin: false,
            discovery: None,
            blob_refs: BlobRefPolicy::default(),
            log: true,
        }
    }

    pub fn validate(&self) -> ServerResult<()> {
        self.budgets.validate()?;
        if self.control_max_conns == 0 || self.data_max_conns == 0 {
            return Err(ServerError::InvalidInput { what: "config max conns" });
        }
        if self.control_max_conns > 1024 || self.data_max_conns > 1024 {
            return Err(ServerError::InvalidInput { what: "config max conns" });
        }
        if self.read_timeout_ms == 0
            || self.write_timeout_ms == 0
            || self.head_deadline_ms == 0
            || self.keepalive_idle_ms == 0
            || self.control_body_deadline_ms == 0
            || self.data_body_deadline_ms == 0
        {
            return Err(ServerError::InvalidInput { what: "config timeouts" });
        }
        if self.max_requests_per_conn == 0 {
            return Err(ServerError::InvalidInput { what: "config max_requests_per_conn" });
        }
        if self.max_json_body_bytes == 0
            || self.max_json_body_bytes > self.control_max_body_bytes
        {
            return Err(ServerError::InvalidInput { what: "config json body bytes" });
        }
        // Framed game bodies carry two canonical documents plus framing.
        if self.control_max_body_bytes < 2 * self.budgets.max_manifest_bytes + 16 {
            return Err(ServerError::InvalidInput { what: "config control body bytes" });
        }
        if self.admin_token_ttl_ms == 0 || self.max_token_ttl_ms == 0 {
            return Err(ServerError::InvalidInput { what: "config token ttl" });
        }
        if self.janitor_interval_ms == 0 {
            return Err(ServerError::InvalidInput { what: "config janitor interval" });
        }
        if self.event_journal_cap == 0 || self.event_journal_cap > 65_536 {
            return Err(ServerError::InvalidInput { what: "config event journal cap" });
        }
        if self.event_max_wait_ms == 0 || self.event_max_wait_ms > 60_000 {
            return Err(ServerError::InvalidInput { what: "config event max wait" });
        }
        if self.event_max_batch == 0 || self.event_max_batch > 512 {
            return Err(ServerError::InvalidInput { what: "config event max batch" });
        }
        // Every waiter is also a live connection, so the connection cap
        // already dominates; this bound only needs to stand on its own.
        if self.event_max_waiters == 0 || self.event_max_waiters > 1024 {
            return Err(ServerError::InvalidInput { what: "config event max waiters" });
        }
        // GC step budgets: a request or tick must always be bounded, and a
        // batch must never be big enough to make one step a whole-store
        // transaction.
        if self.gc_max_steps_per_request == 0 || self.gc_max_steps_per_request > 4096 {
            return Err(ServerError::InvalidInput { what: "config gc steps per request" });
        }
        if self.gc_janitor_steps > 4096 {
            return Err(ServerError::InvalidInput { what: "config gc janitor steps" });
        }
        if self.gc_mark_batch == 0
            || self.gc_mark_batch > 100_000
            || self.gc_sweep_batch == 0
            || self.gc_sweep_batch > 100_000
            || self.gc_retain_batch == 0
            || self.gc_retain_batch > 100_000
        {
            return Err(ServerError::InvalidInput { what: "config gc batch" });
        }
        if self.batch_max_items == 0 || self.batch_max_items > 256 {
            return Err(ServerError::InvalidInput { what: "config batch max items" });
        }
        if self.batch_max_bytes == 0 || self.batch_max_bytes > 256 * 1024 * 1024 {
            return Err(ServerError::InvalidInput { what: "config batch max bytes" });
        }
        // A prefix allowlist that is not absolute cannot be prefix-checked
        // against the absolute paths references store: refuse it at config
        // time rather than silently admitting everything.
        if self.blob_refs.roots.len() > 64 {
            return Err(ServerError::InvalidInput { what: "config blob ref roots count" });
        }
        for root in &self.blob_refs.roots {
            if !root.is_absolute() {
                return Err(ServerError::InvalidInput { what: "config blob ref root not absolute" });
            }
        }
        if let Some(d) = &self.discovery {
            if d.port == 0 || d.interval_ms < 10 {
                return Err(ServerError::InvalidInput { what: "config discovery" });
            }
            if d.capability_bits & !super::discovery::caps::ALL_V1 != 0 {
                return Err(ServerError::InvalidInput { what: "config discovery capability bits" });
            }
            // The UDP discovery port must never collide with an HTTP plane
            // (port 0 planes are ephemeral and resolve at bind time).
            for p in [self.control_addr.port(), self.data_addr.port()] {
                if p != 0 && p == d.port {
                    return Err(ServerError::InvalidInput { what: "config discovery port collision" });
                }
            }
        }
        Ok(())
    }
}
