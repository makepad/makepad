use makepad_micro_serde::JsonValue;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const WAIT_SLICE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventCursor {
    pub epoch: u64,
    pub seq: u64,
}

impl EventCursor {
    pub fn render(self) -> String {
        format!("{:016x}-{}", self.epoch, self.seq)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (epoch, seq) = text.split_once('-')?;
        if epoch.len() != 16
            || !epoch.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || seq.is_empty()
            || seq.len() > 20
            || !seq.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        Some(Self {
            epoch: u64::from_str_radix(epoch, 16).ok()?,
            seq: seq.parse().ok()?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct FlowEvent {
    pub seq: u64,
    pub topic: String,
    pub kind: String,
    pub payload: JsonValue,
}

impl FlowEvent {
    pub(crate) fn wire_value(&self) -> JsonValue {
        let mut fields = match self.payload.clone() {
            JsonValue::Object(fields) => fields,
            payload => HashMap::from([("payload".to_string(), payload)]),
        };
        fields.insert("seq".to_string(), JsonValue::U64(self.seq));
        fields.insert("topic".to_string(), JsonValue::String(self.topic.clone()));
        fields.insert("kind".to_string(), JsonValue::String(self.kind.clone()));
        JsonValue::Object(fields)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EventPoll {
    pub events: Vec<FlowEvent>,
    pub cursor: EventCursor,
    pub gap: bool,
}

struct Journal {
    entries: VecDeque<FlowEvent>,
    next_seq: u64,
}

impl Journal {
    fn first_seq(&self) -> u64 {
        self.next_seq - self.entries.len() as u64
    }
}

pub struct EventHub {
    epoch: u64,
    cap: usize,
    max_waiters: usize,
    journal: Mutex<Journal>,
    wake: Condvar,
    stopped: AtomicBool,
    waiters: AtomicUsize,
}

impl EventHub {
    pub fn new(epoch: u64, cap: usize, max_waiters: usize) -> Self {
        Self {
            epoch,
            cap: cap.max(1),
            max_waiters: max_waiters.max(1),
            journal: Mutex::new(Journal { entries: VecDeque::new(), next_seq: 1 }),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
            waiters: AtomicUsize::new(0),
        }
    }

    pub fn publish(&self, topic: &str, kind: &str, payload: JsonValue) {
        let mut journal = self.journal.lock().unwrap_or_else(|poison| poison.into_inner());
        let seq = journal.next_seq;
        journal.next_seq = journal.next_seq.saturating_add(1);
        journal.entries.push_back(FlowEvent {
            seq,
            topic: topic.to_string(),
            kind: kind.to_string(),
            payload,
        });
        while journal.entries.len() > self.cap {
            journal.entries.pop_front();
        }
        drop(journal);
        self.wake.notify_all();
    }

    pub fn tail_cursor(&self) -> EventCursor {
        let journal = self.journal.lock().unwrap_or_else(|poison| poison.into_inner());
        EventCursor { epoch: self.epoch, seq: journal.next_seq - 1 }
    }

    pub(crate) fn poll_after(&self, cursor: EventCursor, topic: Option<&str>, limit: usize) -> EventPoll {
        let journal = self.journal.lock().unwrap_or_else(|poison| poison.into_inner());
        if cursor.epoch != self.epoch || cursor.seq >= journal.next_seq {
            return EventPoll {
                events: Vec::new(),
                cursor: EventCursor { epoch: self.epoch, seq: journal.next_seq - 1 },
                gap: true,
            };
        }
        if cursor.seq.saturating_add(1) < journal.first_seq() {
            return EventPoll {
                events: Vec::new(),
                cursor: EventCursor { epoch: self.epoch, seq: journal.next_seq - 1 },
                gap: true,
            };
        }
        let mut events = Vec::new();
        let mut scanned_to = cursor.seq;
        for event in journal.entries.iter().filter(|event| event.seq > cursor.seq) {
            if topic.is_none_or(|topic| event.topic == topic) {
                if events.len() >= limit {
                    break;
                }
                events.push(event.clone());
            }
            scanned_to = event.seq;
        }
        EventPoll {
            events,
            cursor: EventCursor { epoch: self.epoch, seq: scanned_to },
            gap: false,
        }
    }

    /// Wait on the connection thread. `false` also covers the waiter cap.
    pub fn wait_beyond(&self, after_seq: u64, deadline: Instant) -> bool {
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        let mut claimed = self.waiters.load(Ordering::SeqCst);
        loop {
            if claimed >= self.max_waiters {
                return false;
            }
            match self.waiters.compare_exchange(
                claimed,
                claimed + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(current) => claimed = current,
            }
        }
        let mut woke = false;
        let mut journal = self.journal.lock().unwrap_or_else(|poison| poison.into_inner());
        loop {
            if journal.next_seq - 1 > after_seq {
                woke = true;
                break;
            }
            if self.stopped.load(Ordering::SeqCst) || Instant::now() >= deadline {
                break;
            }
            let slice = deadline.saturating_duration_since(Instant::now()).min(WAIT_SLICE);
            journal = self
                .wake
                .wait_timeout(journal, slice)
                .unwrap_or_else(|poison| poison.into_inner())
                .0;
        }
        drop(journal);
        self.waiters.fetch_sub(1, Ordering::SeqCst);
        woke
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }
}
