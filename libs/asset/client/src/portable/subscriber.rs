//! Immutable static snapshots have no event side channel.

use crate::client::CatalogEventCursor;
use crate::dto::CatalogEventDto;
use crate::error::{ClientError, ClientResult};
use makepad_asset_data::AssetKind;

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
        Self { kind: None, wait_ms: 10_000, batch_limit: 128, channel_capacity: 16,
            retry_min_ms: 250, retry_max_ms: 10_000 }
    }

    pub fn validate(&self) -> ClientResult<()> {
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
            return Err(ClientError::InvalidInput { what: "subscriber configuration" });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogSubscriptionEvent {
    Ready { cursor: CatalogEventCursor },
    Events { events: Vec<CatalogEventDto>, cursor: CatalogEventCursor },
    ResyncRequired { cursor: CatalogEventCursor },
    Retry { error: ClientError, retry_in_ms: u64 },
}

pub struct CatalogSubscriber;

impl CatalogSubscriber {
    pub fn null() -> Self { Self }
    pub fn poll(&mut self) -> Vec<CatalogSubscriptionEvent> { Vec::new() }
    pub fn request_stop(&self) {}
    pub fn shutdown(self) {}
}
