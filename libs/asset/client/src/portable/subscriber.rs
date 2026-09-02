//! Portable catalog subscription vocabulary and inert handle.

use crate::dto::CatalogEventDto;
use crate::error::ClientError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEventCursor {
    server_id: [u8; 16],
    token: String,
}

impl CatalogEventCursor {
    pub fn server_id(&self) -> &[u8; 16] {
        &self.server_id
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
    pub fn poll(&mut self) -> Vec<CatalogSubscriptionEvent> {
        Vec::new()
    }

    pub fn shutdown(self) {}
}
