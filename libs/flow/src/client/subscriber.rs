use super::{ClientError, ClientResult, FlowClient};
use crate::Event;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct FlowSubscriberConfig {
    pub wait_ms: u64,
    pub limit: u32,
    pub topic: Option<String>,
}

impl Default for FlowSubscriberConfig {
    fn default() -> Self {
        Self {
            wait_ms: 10_000,
            limit: 128,
            topic: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SubscriptionEvent {
    Ready,
    Events(Vec<Event>),
    ResyncRequired,
    Retry { in_secs: u64 },
}

pub struct FlowSubscriber {
    rx: Receiver<SubscriptionEvent>,
    stopping: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl FlowSubscriber {
    pub fn start(
        client: Arc<Mutex<FlowClient>>,
        config: FlowSubscriberConfig,
    ) -> ClientResult<Self> {
        if config.wait_ms == 0 || config.wait_ms > 30_000 {
            return Err(ClientError::Protocol(
                "subscriber wait_ms is out of range".into(),
            ));
        }
        if config.limit == 0 || config.limit > 4096 {
            return Err(ClientError::Protocol(
                "subscriber limit is out of range".into(),
            ));
        }
        let lane = client
            .lock()
            .map_err(|_| ClientError::Protocol("flow client lock poisoned".into()))?
            .subscription_lane();
        let (tx, rx) = sync_channel(16);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stop = stopping.clone();
        let join = std::thread::Builder::new()
            .name("flow-events-subscriber".into())
            .spawn(move || worker(lane, config, tx, worker_stop))
            .map_err(|error| ClientError::Io {
                op: "spawn flow subscriber",
                kind: error.kind(),
            })?;
        Ok(Self {
            rx,
            stopping,
            join: Some(join),
        })
    }

    pub fn poll(&self) -> Vec<SubscriptionEvent> {
        let mut events = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return events,
            }
        }
    }

    pub fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    pub fn shutdown(mut self) {
        self.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for FlowSubscriber {
    fn drop(&mut self) {
        self.request_stop();
        self.join.take();
    }
}

fn worker(
    client: FlowClient,
    config: FlowSubscriberConfig,
    tx: SyncSender<SubscriptionEvent>,
    stopping: Arc<AtomicBool>,
) {
    let mut cursor: Option<String> = None;
    let mut ready = false;
    let mut retry_ms = 1_000u64;
    while !stopping.load(Ordering::Acquire) {
        match client.events(
            cursor.as_deref(),
            config.wait_ms,
            config.limit,
            config.topic.as_deref(),
        ) {
            Ok(page) => {
                retry_ms = 1_000;
                if page.gap {
                    ready = true;
                    if !send(&tx, SubscriptionEvent::ResyncRequired, &stopping) {
                        return;
                    }
                } else {
                    if !ready {
                        ready = true;
                        if !send(&tx, SubscriptionEvent::Ready, &stopping) {
                            return;
                        }
                    }
                    if !page.events.is_empty()
                        && !send(&tx, SubscriptionEvent::Events(page.events), &stopping)
                    {
                        return;
                    }
                }
                // Advance only after every notification for this page is in
                // the bounded channel.
                cursor = Some(page.cursor);
            }
            Err(_) => {
                if !send(
                    &tx,
                    SubscriptionEvent::Retry {
                        in_secs: retry_ms.div_ceil(1_000),
                    },
                    &stopping,
                ) {
                    return;
                }
                if !sleep_interruptible(retry_ms, &stopping) {
                    return;
                }
                retry_ms = retry_ms.saturating_mul(2).min(10_000);
            }
        }
    }
}

fn send(
    tx: &SyncSender<SubscriptionEvent>,
    mut event: SubscriptionEvent,
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

fn sleep_interruptible(milliseconds: u64, stopping: &AtomicBool) -> bool {
    let mut left = milliseconds;
    while left > 0 {
        if stopping.load(Ordering::Acquire) {
            return false;
        }
        let slice = left.min(50);
        std::thread::sleep(Duration::from_millis(slice));
        left -= slice;
    }
    true
}
