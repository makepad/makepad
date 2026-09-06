//! Bounded background subscriber for the committed catalog event journal.
//!
//! The worker owns only a cloned authenticated [`crate::Api`], not the
//! content cache, so a long-poll never stalls blob or thumbnail work in
//! [`crate::ClientRuntime`]. Delivery is lossless and bounded: when the
//! consumer queue is full the worker stops advancing the server cursor until
//! the pending notification is accepted. Transport failures are surfaced as
//! typed events and retried with capped exponential backoff.

use crate::client::{AssetClient, CatalogEventCursor};
use crate::dto::CatalogEventDto;
use crate::error::{ClientError, ClientResult};
use crate::Api;
use makepad_asset_data::AssetKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct CatalogSubscriberConfig {
    /// Optional server-side content-kind filter. Events whose kind was not
    /// known at commit time are conservatively delivered by the server.
    pub kind: Option<AssetKind>,
    /// Duration of each server long-poll.
    pub wait_ms: u64,
    /// Maximum events requested per response.
    pub batch_limit: u32,
    /// Maximum undrained notifications held in memory.
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

    pub fn validate(&self) -> ClientResult<()> {
        if self.wait_ms == 0 || self.wait_ms > crate::wire::MAX_EVENT_WAIT_MS {
            return Err(ClientError::InvalidInput { what: "subscriber wait_ms" });
        }
        if self.batch_limit == 0 || self.batch_limit > crate::wire::MAX_EVENT_BATCH {
            return Err(ClientError::InvalidInput { what: "subscriber batch_limit" });
        }
        if self.channel_capacity == 0 || self.channel_capacity > 4096 {
            return Err(ClientError::InvalidInput { what: "subscriber channel capacity" });
        }
        if self.retry_min_ms == 0
            || self.retry_max_ms < self.retry_min_ms
            || self.retry_max_ms > 60_000
        {
            return Err(ClientError::InvalidInput { what: "subscriber retry bounds" });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogSubscriptionEvent {
    /// Initial cursor acquired. Consumers should complete their first catalog
    /// load before applying subsequent event batches.
    Ready { cursor: CatalogEventCursor },
    Events { events: Vec<CatalogEventDto>, cursor: CatalogEventCursor },
    /// Retention loss or server restart. Rebuild the visible catalog and then
    /// resume from this cursor.
    ResyncRequired { cursor: CatalogEventCursor },
    /// A poll failed. The worker retains its last delivered cursor and will
    /// retry after the reported delay.
    Retry { error: ClientError, retry_in_ms: u64 },
}

pub struct CatalogSubscriber {
    rx: Receiver<CatalogSubscriptionEvent>,
    stopping: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl CatalogSubscriber {
    /// Static snapshots are immutable, so their subscriber is deliberately
    /// silent and owns no worker thread.
    pub fn null() -> Self {
        let (_tx, rx) = sync_channel(1);
        Self {
            rx,
            stopping: Arc::new(AtomicBool::new(true)),
            join: None,
        }
    }

    pub fn start(client: &AssetClient, config: CatalogSubscriberConfig) -> ClientResult<Self> {
        config.validate()?;
        let (api, server_id) = client.subscription_parts();
        let (tx, rx) = sync_channel(config.channel_capacity);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop_worker = stopping.clone();
        let join = std::thread::Builder::new()
            .name("asset-catalog-subscriber".into())
            .spawn(move || worker(api, server_id, config, tx, stop_worker))
            .map_err(|e| ClientError::Io { op: "spawn catalog subscriber", kind: e.kind() })?;
        Ok(Self { rx, stopping, join: Some(join) })
    }

    /// Drain every currently queued notification without blocking a frame.
    pub fn poll(&mut self) -> Vec<CatalogSubscriptionEvent> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(event) => out.push(event),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Request stop without joining. Safe to call from an event/UI thread;
    /// the in-flight HTTP poll will finish under its configured deadline.
    pub fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    /// Explicitly wait for the worker. Call from an owning background thread
    /// when deterministic teardown matters.
    pub fn shutdown(mut self) {
        self.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for CatalogSubscriber {
    fn drop(&mut self) {
        // Never join a potentially parked long-poll from an arbitrary caller
        // (often the UI thread). Dropping JoinHandle detaches safely.
        self.request_stop();
        self.join.take();
    }
}

fn worker(
    api: Api,
    server_id: [u8; 16],
    config: CatalogSubscriberConfig,
    tx: SyncSender<CatalogSubscriptionEvent>,
    stopping: Arc<AtomicBool>,
) {
    let mut cursor: Option<CatalogEventCursor> = None;
    let mut announced_ready = false;
    let mut retry_ms = config.retry_min_ms;
    while !stopping.load(Ordering::Acquire) {
        let page = api.events_page(
            cursor.as_ref().map(CatalogEventCursor::token),
            config.wait_ms,
            config.batch_limit,
            config.kind,
        );
        match page {
            Ok(page) => {
                retry_ms = config.retry_min_ms;
                let next = CatalogEventCursor {
                    server_id,
                    token: page.cursor,
                };
                if page.gap {
                    if !send_lossless(
                        &tx,
                        CatalogSubscriptionEvent::ResyncRequired { cursor: next.clone() },
                        &stopping,
                    ) {
                        return;
                    }
                } else if !announced_ready {
                    announced_ready = true;
                    if !send_lossless(
                        &tx,
                        CatalogSubscriptionEvent::Ready { cursor: next.clone() },
                        &stopping,
                    ) {
                        return;
                    }
                    // Cursor-less v4 pages may carry cumulative preview
                    // announcements for a late joiner. Ready must precede
                    // them, but they must not be discarded.
                    if !page.events.is_empty()
                        && !send_lossless(
                            &tx,
                            CatalogSubscriptionEvent::Events {
                                events: page.events,
                                cursor: next.clone(),
                            },
                            &stopping,
                        )
                    {
                        return;
                    }
                } else if !page.events.is_empty()
                    && !send_lossless(
                        &tx,
                        CatalogSubscriptionEvent::Events {
                            events: page.events,
                            cursor: next.clone(),
                        },
                        &stopping,
                    )
                {
                        return;
                }
                // Advance only after the notification is durably queued.
                cursor = Some(next);
            }
            Err(error) => {
                let event = CatalogSubscriptionEvent::Retry { error, retry_in_ms: retry_ms };
                if !send_lossless(&tx, event, &stopping) {
                    return;
                }
                if !sleep_interruptible(retry_ms, &stopping) {
                    return;
                }
                retry_ms = retry_ms.saturating_mul(2).min(config.retry_max_ms);
            }
        }
    }
}

fn send_lossless(
    tx: &SyncSender<CatalogSubscriptionEvent>,
    mut event: CatalogSubscriptionEvent,
    stopping: &AtomicBool,
) -> bool {
    loop {
        if stopping.load(Ordering::Acquire) {
            return false;
        }
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn sleep_interruptible(ms: u64, stopping: &AtomicBool) -> bool {
    let mut left = ms;
    while left > 0 {
        if stopping.load(Ordering::Acquire) {
            return false;
        }
        let slice = left.min(25);
        std::thread::sleep(Duration::from_millis(slice));
        left -= slice;
    }
    !stopping.load(Ordering::Acquire)
}

impl AssetClient {
    pub fn subscribe_catalog(
        &self,
        config: CatalogSubscriberConfig,
    ) -> ClientResult<CatalogSubscriber> {
        CatalogSubscriber::start(self, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriber_config_bounds() {
        assert!(CatalogSubscriberConfig::default_v1().validate().is_ok());
        let mut c = CatalogSubscriberConfig::default_v1();
        c.channel_capacity = 0;
        assert!(c.validate().is_err());
        c = CatalogSubscriberConfig::default_v1();
        c.wait_ms = crate::wire::MAX_EVENT_WAIT_MS + 1;
        assert!(c.validate().is_err());
        c = CatalogSubscriberConfig::default_v1();
        c.retry_min_ms = 100;
        c.retry_max_ms = 99;
        assert!(c.validate().is_err());
    }
}
