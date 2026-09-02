//! Portable session configuration; static execution is a later feature lane.

use crate::error::{ClientError, ClientResult};
use crate::location::{ApiEndpoints, BaseUrl, ClientLocation, ClientMode};
use makepad_asset_data::AssetKind;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub struct CatalogSubscriberConfig {
    pub kind: Option<AssetKind>,
    pub wait_ms: u64,
    pub batch_limit: u32,
    pub channel_capacity: usize,
    pub retry_min_ms: u64,
    pub retry_max_ms: u64,
}

impl CatalogSubscriberConfig {
    pub fn default_v1() -> Self {
        Self {
            kind: None,
            wait_ms: 10_000,
            batch_limit: 128,
            channel_capacity: 16,
            retry_min_ms: 250,
            retry_max_ms: 10_000,
        }
    }

    fn validate(&self) -> ClientResult<()> {
        if self.wait_ms == 0
            || self.wait_ms > crate::wire::MAX_EVENT_WAIT_MS
            || self.batch_limit == 0
            || self.batch_limit > crate::wire::MAX_EVENT_BATCH
            || self.channel_capacity == 0
            || self.channel_capacity > 4096
            || self.retry_min_ms == 0
            || self.retry_max_ms < self.retry_min_ms
            || self.retry_max_ms > 60_000
        {
            return Err(ClientError::InvalidInput {
                what: "subscriber configuration",
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub fast_workers: usize,
    pub bulk_workers: usize,
    pub fast_blob_max_bytes: u64,
    pub fast_batch_max_items: usize,
}

impl RuntimeConfig {
    pub fn default_v1() -> Self {
        Self {
            fast_workers: 4,
            bulk_workers: 2,
            fast_blob_max_bytes: 512 * 1024,
            fast_batch_max_items: 16,
        }
    }

    fn validate(&self) -> ClientResult<()> {
        if self.fast_workers == 0
            || self.bulk_workers == 0
            || self.fast_workers + self.bulk_workers > 64
            || self.fast_batch_max_items == 0
            || self.fast_batch_max_items > crate::wire::MAX_BLOB_BATCH_ITEMS
        {
            return Err(ClientError::InvalidInput {
                what: "runtime configuration",
            });
        }
        Ok(())
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::default_v1()
    }
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub location: Option<ClientLocation>,
    pub endpoints: Option<ApiEndpoints>,
    pub server_id: Option<[u8; 16]>,
    pub token: Option<String>,
    pub cache_parent: PathBuf,
    pub catalog_cache_leaf: String,
    pub media_lanes: Vec<String>,
    pub discovery_port: u16,
    pub discovery_wait_ms: u64,
    pub subscriber: CatalogSubscriberConfig,
    pub catalog_runtime: RuntimeConfig,
    pub retry_min_ms: u64,
    pub retry_max_ms: u64,
}

impl SessionConfig {
    pub fn new(cache_parent: impl Into<PathBuf>) -> Self {
        Self {
            location: None,
            endpoints: None,
            server_id: None,
            token: None,
            cache_parent: cache_parent.into(),
            catalog_cache_leaf: "cache-catalog".to_string(),
            media_lanes: vec!["cache-media".to_string()],
            discovery_port: crate::wire::DEFAULT_DISCOVERY_PORT,
            discovery_wait_ms: 4_000,
            subscriber: CatalogSubscriberConfig::default_v1(),
            catalog_runtime: RuntimeConfig::default_v1(),
            retry_min_ms: 1_000,
            retry_max_ms: 10_000,
        }
    }

    pub fn static_site(base_url: BaseUrl) -> Self {
        let mut config = Self::new(PathBuf::new());
        config.location = Some(ClientLocation::StaticSite(base_url));
        config
    }

    fn validate(&self) -> ClientResult<()> {
        self.subscriber.validate()?;
        self.catalog_runtime.validate()?;
        if matches!(self.location, Some(ClientLocation::StaticSite(_))) && self.token.is_some() {
            return Err(ClientError::InvalidInput { what: "static site bearer token" });
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Discovering,
    Connecting { server: String },
    Connected { server: String },
    Retrying { error: String, in_secs: u64 },
}

pub enum SessionMsg {
    Status(SessionStatus),
}

pub struct SessionConnector;

impl SessionConnector {
    pub fn start(config: SessionConfig) -> ClientResult<Self> {
        config.validate()?;
        let mode = config
            .location
            .as_ref()
            .map(ClientLocation::mode)
            .unwrap_or(ClientMode::Native);
        Err(ClientError::Unavailable { capability: "static_site_session", mode })
    }

    pub fn poll(&mut self) -> Vec<SessionMsg> {
        Vec::new()
    }

    pub fn stop(&self) {}
}
