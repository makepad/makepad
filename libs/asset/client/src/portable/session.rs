//! Poll-driven static session connector. It performs no discovery and starts
//! no worker thread.

use crate::client::{CacheBudgets, ClientConfig};
use crate::error::{ClientError, ClientResult};
use crate::location::{ApiEndpoints, BaseUrl, ClientLocation};
use crate::runtime::{ClientRuntime, RuntimeConfig};
use crate::subscriber::{CatalogSubscriber, CatalogSubscriberConfig};
use std::collections::VecDeque;
use std::path::PathBuf;

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
    pub cache: CacheBudgets,
}

impl SessionConfig {
    pub fn new(cache_parent: impl Into<PathBuf>) -> Self {
        Self {
            location: None, endpoints: None, server_id: None, token: None,
            cache_parent: cache_parent.into(), catalog_cache_leaf: "cache-catalog".into(),
            media_lanes: vec!["cache-media".into()], discovery_port: crate::wire::DEFAULT_DISCOVERY_PORT,
            discovery_wait_ms: 4_000, subscriber: CatalogSubscriberConfig::default_v1(),
            catalog_runtime: RuntimeConfig::default_v1(), retry_min_ms: 1_000,
            retry_max_ms: 10_000, cache: CacheBudgets::default_v1(),
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
        self.cache.validate()?;
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
        if !matches!(self.location, Some(ClientLocation::StaticSite(_))) {
            return Err(ClientError::Unavailable {
                capability: "native_session", mode: crate::location::ClientMode::StaticWeb,
            });
        }
        Ok(())
    }
}

pub struct SessionHandles {
    pub catalog: ClientRuntime,
    pub media: Vec<ClientRuntime>,
    pub subscriber: CatalogSubscriber,
    pub server_label: String,
    pub server_id: [u8; 16],
    pub location: ClientLocation,
    pub endpoints: Option<ApiEndpoints>,
    pub token: Option<String>,
}

impl SessionHandles {
    pub fn native_endpoints(&self) -> ClientResult<ApiEndpoints> {
        Err(ClientError::Unavailable {
            capability: "native_endpoints", mode: crate::location::ClientMode::StaticWeb,
        })
    }
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
    Retrying { error: String, in_secs: u64 },
}

pub enum SessionMsg {
    Status(SessionStatus),
    Up(Box<SessionHandles>),
}

pub struct SessionConnector {
    runtime: Option<ClientRuntime>,
    messages: VecDeque<SessionMsg>,
    stopped: bool,
}

impl SessionConnector {
    pub fn start(config: SessionConfig) -> ClientResult<Self> {
        config.validate()?;
        let ClientLocation::StaticSite(base) = config.location.clone().unwrap() else { unreachable!() };
        let mut client = ClientConfig::static_site(base.clone());
        client.cache = config.cache;
        let store = crate::static_store::StaticStore::platform(base.clone(), client.cache.max_ram_bytes)?;
        let runtime = ClientRuntime::start_static_store(store, config.catalog_runtime)?;
        let mut messages = VecDeque::new();
        messages.push_back(SessionMsg::Status(SessionStatus::Connecting { server: base.to_string() }));
        Ok(Self { runtime: Some(runtime), messages, stopped: false })
    }

    pub fn poll(&mut self) -> Vec<SessionMsg> {
        let mut out: Vec<_> = self.messages.drain(..).collect();
        if self.stopped { return out; }
        if let Some(runtime) = &mut self.runtime {
            let _ = runtime.poll();
            if let Some(error) = runtime.connect_error().cloned() {
                out.push(SessionMsg::Status(SessionStatus::Retrying {
                    error: error.to_string(),
                    in_secs: 0,
                }));
                self.runtime = None;
            } else if runtime.is_ready() {
                let runtime = self.runtime.take().unwrap();
                let location = runtime.location().unwrap();
                let server_id = runtime.server_id().unwrap();
                let label = location.to_string();
                out.push(SessionMsg::Up(Box::new(SessionHandles {
                    catalog: runtime, media: Vec::new(), subscriber: CatalogSubscriber::null(),
                    server_label: label.clone(), server_id, location, endpoints: None, token: None,
                })));
                out.push(SessionMsg::Status(SessionStatus::Connected { server: label }));
            }
        }
        out
    }

    pub fn stop(&mut self) { self.stopped = true; self.runtime = None; }
}
