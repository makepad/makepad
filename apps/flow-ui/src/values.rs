//! The value cache: bytes by digest, in RAM under an LRU budget, fetched over
//! the data plane on worker threads and posted back through a channel the
//! UI drains per frame. Nothing here touches the disk (thin-client law).

use makepad_flow::client::{ClientError, FlowClient};
use makepad_flow::ValueBytes;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

pub const DEFAULT_BUDGET: usize = 64 * 1024 * 1024;

pub enum ValueArrival {
    Ready(ValueBytes),
    Failed { digest: String, error: ClientError },
}

pub struct ValueCache {
    budget: usize,
    entries: HashMap<String, (ValueBytes, u64)>,
    bytes: usize,
    tick: u64,
    pending: HashSet<String>,
    sender: Sender<ValueArrival>,
    receiver: Receiver<ValueArrival>,
}

impl Default for ValueCache {
    fn default() -> Self {
        Self::new(DEFAULT_BUDGET)
    }
}

impl ValueCache {
    pub fn new(budget: usize) -> Self {
        let (sender, receiver) = channel();
        Self {
            budget,
            entries: HashMap::new(),
            bytes: 0,
            tick: 0,
            pending: HashSet::new(),
            sender,
            receiver,
        }
    }

    pub fn get(&mut self, digest: &str) -> Option<ValueBytes> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.get_mut(digest)?;
        entry.1 = tick;
        Some(entry.0.clone())
    }

    pub fn contains(&self, digest: &str) -> bool {
        self.entries.contains_key(digest)
    }

    pub fn insert(&mut self, value: ValueBytes) {
        self.pending.remove(&value.digest);
        if self.entries.contains_key(&value.digest) {
            return;
        }
        self.tick += 1;
        self.bytes = self.bytes.saturating_add(value.bytes.len());
        self.entries
            .insert(value.digest.clone(), (value, self.tick));
        while self.bytes > self.budget && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, touched))| *touched)
                .map(|(digest, _)| digest.clone())
            else {
                break;
            };
            if let Some((value, _)) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(value.bytes.len());
            }
        }
    }

    /// Fetch a value on a worker thread unless it is cached or in flight.
    /// Returns whether a fetch was started.
    pub fn request(&mut self, digest: &str, client: Arc<Mutex<FlowClient>>) -> bool {
        if self.entries.contains_key(digest) || !self.pending.insert(digest.to_string()) {
            return false;
        }
        let digest = digest.to_string();
        let sender = self.sender.clone();
        std::thread::Builder::new()
            .name("flow-ui-value".into())
            .spawn(move || {
                let result = client
                    .lock()
                    .map_err(|_| ClientError::Protocol("flow client lock poisoned".into()))
                    .and_then(|client| client.value(&digest));
                let arrival = match result {
                    Ok(value) => ValueArrival::Ready(value),
                    Err(error) => ValueArrival::Failed { digest, error },
                };
                let _ = sender.send(arrival);
                SignalToUI::set_ui_signal();
            })
            .is_ok()
    }

    /// Everything the workers delivered since the last drain; arrivals are
    /// stored before they are returned, so callers only need the digests.
    pub fn drain(&mut self) -> Vec<Result<String, (String, ClientError)>> {
        let mut out = Vec::new();
        loop {
            match self.receiver.try_recv() {
                Ok(ValueArrival::Ready(value)) => {
                    let digest = value.digest.clone();
                    self.insert(value);
                    out.push(Ok(digest));
                }
                Ok(ValueArrival::Failed { digest, error }) => {
                    self.pending.remove(&digest);
                    out.push(Err((digest, error)));
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return out,
            }
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(digest: &str, size: usize) -> ValueBytes {
        ValueBytes {
            digest: digest.to_string(),
            content_type: "application/octet-stream".into(),
            bytes: vec![0u8; size].into(),
        }
    }

    #[test]
    fn lru_evicts_the_least_recently_touched() {
        let mut cache = ValueCache::new(10);
        cache.insert(value("a", 4));
        cache.insert(value("b", 4));
        assert!(cache.get("a").is_some());
        cache.insert(value("c", 4));
        assert!(cache.contains("a"));
        assert!(!cache.contains("b"));
        assert!(cache.contains("c"));
        assert_eq!(cache.bytes(), 8);
    }
}
