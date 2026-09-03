use super::ServerError;
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
    pub log: Box<dyn Fn(&str) + Send + Sync>,
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
            log: Box::new(|line| eprintln!("[flow-server] {line}")),
        }
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

