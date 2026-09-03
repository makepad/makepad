use super::ServerError;
use crate::engine::{NetPolicy, Seams};
use std::path::PathBuf;
use std::sync::Arc;

/// Reserved configuration shape for the later `MPFLDIS1` beacon lane.
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    pub port: u16,
    pub interval_ms: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self { port: 41872, interval_ms: 2_000 }
    }
}

/// Every operational host bound. Nothing is process-global.
pub struct FlowServerConfig {
    pub root: PathBuf,
    pub control_addr: String,
    pub data_addr: String,
    pub control_max_conns: usize,
    pub data_max_conns: usize,
    pub event_journal_cap: usize,
    pub event_max_waiters: usize,
    pub revision_ring: usize,
    pub watch_interval_ms: u64,
    pub discovery: Option<DiscoveryConfig>,
    /// Hub base URLs consulted before LAN and machine discovery. This is
    /// useful on beacon-less networks and makes the fleet seam deterministic
    /// in tests.
    pub fleet_hint: Vec<String>,
    pub log: Box<dyn Fn(&str) + Send + Sync>,
    /// A specific local LLM to pin the `chat` executor's `HubChat` seam to
    /// (§5.4); `None` lets the hub elect / falls back to
    /// `MAKEPAD_FLOW_LLM_MODEL`. Only consulted when `seams` is `None`.
    pub chat_model: Option<PathBuf>,
    /// Egress policy for the `http` executor (§5.4); default allows
    /// loopback + LAN + `*`, visible in every instance's request log.
    pub net: NetPolicy,
    /// Values (§5.5) expire this long after their last touch, once spilled
    /// past `values_ram_budget`.
    pub value_ttl_secs: u64,
    /// An idle, unpinned, non-`autostart`, non-waiting instance is dropped
    /// by the janitor after this many seconds of inactivity (§5.2).
    pub instance_ttl_secs: u64,
    /// RAM budget for `ValueStore` before values spill to `<root>/values/`.
    pub values_ram_budget: usize,
    /// How often the janitor's slow tick sweeps values/runs/instances
    /// (§5.2's "every 30 s"); a test lowers this the way it lowers
    /// `watch_interval_ms`.
    pub janitor_sweep_secs: u64,
    /// Injected seams for tests (`with_seams`); `None` builds the real
    /// `FleetGen` + `HubChat` + `HubHttp` seams at `FlowServer::start`.
    seams: Option<Seams>,
}

impl FlowServerConfig {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            control_addr: "127.0.0.1:0".to_string(),
            data_addr: "127.0.0.1:0".to_string(),
            control_max_conns: 64,
            data_max_conns: 64,
            event_journal_cap: 4_096,
            event_max_waiters: 32,
            revision_ring: 32,
            watch_interval_ms: 250,
            discovery: None,
            fleet_hint: Vec::new(),
            log: Box::new(|line| eprintln!("[flow-server] {line}")),
            chat_model: None,
            net: NetPolicy::default(),
            value_ttl_secs: 60 * 60,
            instance_ttl_secs: 24 * 60 * 60,
            values_ram_budget: 256 * 1024 * 1024,
            janitor_sweep_secs: 30,
            seams: None,
        }
    }

    /// Inject fake seams for a socket test; the real path (`FleetGen` +
    /// `HubChat` + `HubHttp`) stays the default when this is never called.
    pub fn with_seams(mut self, seams: Seams) -> Self {
        self.seams = Some(seams);
        self
    }

    pub(crate) fn seams(&self) -> Option<Seams> {
        self.seams.clone()
    }

    pub fn validate(&self) -> Result<(), ServerError> {
        if self.root.as_os_str().is_empty() {
            return Err(ServerError::InvalidConfig("root is empty"));
        }
        let _: std::net::SocketAddr = self
            .control_addr
            .parse()
            .map_err(|_| ServerError::InvalidConfig("control_addr is not a socket address"))?;
        let _: std::net::SocketAddr = self
            .data_addr
            .parse()
            .map_err(|_| ServerError::InvalidConfig("data_addr is not a socket address"))?;
        if self.control_max_conns == 0
            || self.data_max_conns == 0
            || self.control_max_conns > 1_024
            || self.data_max_conns > 1_024
        {
            return Err(ServerError::InvalidConfig("connection caps must be in 1..=1024"));
        }
        if self.event_journal_cap == 0 || self.event_journal_cap > 65_536 {
            return Err(ServerError::InvalidConfig("event_journal_cap must be in 1..=65536"));
        }
        if self.event_max_waiters == 0 || self.event_max_waiters > 1_024 {
            return Err(ServerError::InvalidConfig("event_max_waiters must be in 1..=1024"));
        }
        if self.revision_ring == 0 || self.revision_ring > 1_024 {
            return Err(ServerError::InvalidConfig("revision_ring must be in 1..=1024"));
        }
        if self.watch_interval_ms == 0 || self.watch_interval_ms > 60_000 {
            return Err(ServerError::InvalidConfig("watch_interval_ms must be in 1..=60000"));
        }
        if self.value_ttl_secs == 0 {
            return Err(ServerError::InvalidConfig("value_ttl_secs must be at least 1"));
        }
        if self.instance_ttl_secs == 0 {
            return Err(ServerError::InvalidConfig("instance_ttl_secs must be at least 1"));
        }
        if self.values_ram_budget == 0 {
            return Err(ServerError::InvalidConfig("values_ram_budget must be at least 1"));
        }
        if self.janitor_sweep_secs == 0 {
            return Err(ServerError::InvalidConfig("janitor_sweep_secs must be at least 1"));
        }
        if let Some(discovery) = &self.discovery {
            if discovery.port == 0 || discovery.interval_ms < 10 {
                return Err(ServerError::InvalidConfig("invalid discovery settings"));
            }
            // TODO(flow-discovery): spawn the MPFLDIS1 beacon in its owner lane.
        }
        Ok(())
    }
}

pub(crate) type SharedConfig = Arc<FlowServerConfig>;
