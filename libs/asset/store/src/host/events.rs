//! Bounded catalog event journal + long-poll wakeup hub.
//!
//! One in-memory, monotonic journal of committed catalog changes: asset
//! publish/quarantine, alias set/clear, annotation set/clear, and the game
//! equivalents. Routes append AFTER the core mutation has committed — and
//! only from inside the state-thread closure that performed it, so journal
//! order equals commit order by construction (the state thread serializes
//! every mutation).
//!
//! Consumers (games, the VJ app, the asset store) long-poll
//! `GET /v1/events` with a resume cursor. Everything is bounded:
//!
//! - the journal is a ring of at most `cap` events; readers whose cursor
//!   points before the retained window get `gap: true` and must resync
//!   their catalog view (never a silent hole),
//! - the cursor carries a per-process random epoch, so a restarted server
//!   (fresh, empty journal) can never alias a stale cursor onto new
//!   sequence numbers — an epoch mismatch is a `gap` too,
//! - waiting is condvar-sliced (250 ms) against a wall-clock deadline and a
//!   shutdown flag, so shutdown never blocks behind a parked long-poll,
//! - concurrent waiters are capped; over the cap a poll degrades to an
//!   immediate empty response (the client just polls again) instead of
//!   parking one more thread.
//!
//! The journal mutex is only ever held for O(len) scans and O(1) appends —
//! never across a wait, never across I/O, and never inside a core catalog
//! transaction's wait path.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Closed event-kind vocabulary. Additions require a protocol version bump —
/// clients refuse unknown kinds rather than misrender them.
pub const KIND_ASSET_PUBLISHED: &str = "asset_published";
pub const KIND_ASSET_QUARANTINED: &str = "asset_quarantined";
pub const KIND_ALIAS_SET: &str = "alias_set";
pub const KIND_ALIAS_CLEARED: &str = "alias_cleared";
pub const KIND_ANNOTATION_SET: &str = "annotation_set";
pub const KIND_ANNOTATION_CLEARED: &str = "annotation_cleared";
pub const KIND_GAME_PUBLISHED: &str = "game_published";
pub const KIND_GAME_QUARANTINED: &str = "game_quarantined";
pub const KIND_GAME_ALIAS_SET: &str = "game_alias_set";
pub const KIND_GAME_ALIAS_CLEARED: &str = "game_alias_cleared";

/// Condvar wait slice; mirrors the connection loop's idle-slice stop checks.
const WAIT_SLICE_MS: u64 = 250;

/// One committed catalog change. Identity fields are pre-rendered display
/// spellings (`ast_…`, `arev_…`, `gam_…`, `grev_…`), which are exactly what
/// travels on the wire.
#[derive(Clone, Debug)]
pub struct CatalogEvent {
    pub seq: u64,
    pub kind: &'static str,
    pub namespace: String,
    pub asset_id: Option<String>,
    pub revision: Option<String>,
    pub game_id: Option<String>,
    pub game_revision: Option<String>,
    pub alias: Option<String>,
    /// Lowercase content kind (`video`, `audio`, …) when the transport knew
    /// it at emit time (from the asset's annotation); `None` otherwise.
    pub content_kind: Option<&'static str>,
    pub ts_ms: u64,
}

/// Everything but the sequence number; the journal assigns `seq` at append.
#[derive(Clone, Debug)]
pub struct EventBody {
    pub kind: &'static str,
    pub namespace: String,
    pub asset_id: Option<String>,
    pub revision: Option<String>,
    pub game_id: Option<String>,
    pub game_revision: Option<String>,
    pub alias: Option<String>,
    pub content_kind: Option<&'static str>,
    pub ts_ms: u64,
}

impl EventBody {
    pub fn asset(kind: &'static str, ns: &str, asset_id: String, ts_ms: u64) -> EventBody {
        EventBody {
            kind,
            namespace: ns.to_string(),
            asset_id: Some(asset_id),
            revision: None,
            game_id: None,
            game_revision: None,
            alias: None,
            content_kind: None,
            ts_ms,
        }
    }

    pub fn game(kind: &'static str, ns: &str, game_id: String, ts_ms: u64) -> EventBody {
        EventBody {
            kind,
            namespace: ns.to_string(),
            asset_id: None,
            revision: None,
            game_id: Some(game_id),
            game_revision: None,
            alias: None,
            content_kind: None,
            ts_ms,
        }
    }

    pub fn with_revision(mut self, rev: String) -> EventBody {
        self.revision = Some(rev);
        self
    }

    pub fn with_game_revision(mut self, rev: String) -> EventBody {
        self.game_revision = Some(rev);
        self
    }

    pub fn with_alias(mut self, alias: String) -> EventBody {
        self.alias = Some(alias);
        self
    }

    pub fn with_content_kind(mut self, kind: Option<&'static str>) -> EventBody {
        self.content_kind = kind;
        self
    }
}

/// A parsed resume cursor: journal epoch + highest sequence already seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventCursor {
    pub epoch: [u8; 8],
    pub seq: u64,
}

impl EventCursor {
    pub fn render(&self) -> String {
        format!("{}-{}", super::util::to_hex(&self.epoch), self.seq)
    }

    /// Strict parse of `<16 lowercase hex>-<decimal seq>`; anything else is
    /// `None` (routes map that to a 400).
    pub fn parse(text: &str) -> Option<EventCursor> {
        let (epoch_hex, seq_text) = text.split_once('-')?;
        let epoch = super::util::from_hex_exact::<8>(epoch_hex)?;
        if seq_text.is_empty()
            || seq_text.len() > 20
            || !seq_text.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let seq: u64 = seq_text.parse().ok()?;
        Some(EventCursor { epoch, seq })
    }
}

/// What one poll of the journal produced.
#[derive(Clone, Debug)]
pub struct EventsPoll {
    pub events: Vec<CatalogEvent>,
    /// Resume cursor covering everything scanned (matching or not), so a
    /// kind-filtered subscriber still advances past foreign events.
    pub cursor: EventCursor,
    /// The caller's cursor fell outside the retained window (or belonged to
    /// another journal epoch/process life): events were lost and the client
    /// must resync its catalog view from scratch.
    pub gap: bool,
}

struct Journal {
    /// Retained events; `seq` values are contiguous.
    buf: VecDeque<CatalogEvent>,
    /// Sequence the NEXT event will get; first event of a journal gets 1.
    next_seq: u64,
}

impl Journal {
    /// Sequence of the oldest retained event; `next_seq` when empty.
    fn first_seq(&self) -> u64 {
        self.next_seq - self.buf.len() as u64
    }
}

pub struct EventHub {
    epoch: [u8; 8],
    cap: usize,
    max_waiters: usize,
    journal: Mutex<Journal>,
    wake: Condvar,
    stopped: AtomicBool,
    waiters: AtomicUsize,
}

impl EventHub {
    pub fn new(epoch: [u8; 8], cap: usize, max_waiters: usize) -> EventHub {
        EventHub {
            epoch,
            cap: cap.max(1),
            max_waiters: max_waiters.max(1),
            journal: Mutex::new(Journal { buf: VecDeque::new(), next_seq: 1 }),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
            waiters: AtomicUsize::new(0),
        }
    }

    /// Append one committed event and wake every parked long-poll. Called
    /// from state-thread closures AFTER the core mutation succeeded; the
    /// lock is held for O(1).
    pub fn publish(&self, body: EventBody) {
        {
            let mut j = self.journal.lock().unwrap();
            let seq = j.next_seq;
            j.next_seq += 1;
            j.buf.push_back(CatalogEvent {
                seq,
                kind: body.kind,
                namespace: body.namespace,
                asset_id: body.asset_id,
                revision: body.revision,
                game_id: body.game_id,
                game_revision: body.game_revision,
                alias: body.alias,
                content_kind: body.content_kind,
                ts_ms: body.ts_ms,
            });
            while j.buf.len() > self.cap {
                j.buf.pop_front();
            }
        }
        self.wake.notify_all();
    }

    /// The resume cursor a fresh (cursor-less) subscriber starts from: the
    /// current tail. The subscriber then loads its catalog view once and
    /// receives only events committed after this point.
    pub fn tail_cursor(&self) -> EventCursor {
        let j = self.journal.lock().unwrap();
        EventCursor { epoch: self.epoch, seq: j.next_seq - 1 }
    }

    /// One non-blocking poll: events with `seq > cursor.seq`, kind-filtered,
    /// at most `limit`. See [`EventsPoll::cursor`] for the advance rule.
    pub fn poll_after(
        &self,
        cursor: EventCursor,
        content_kind: Option<&str>,
        limit: usize,
    ) -> EventsPoll {
        let j = self.journal.lock().unwrap();
        // Another epoch's cursor, or a sequence this journal never issued:
        // the client's view is unanchored — force a resync at the tail.
        if cursor.epoch != self.epoch || cursor.seq >= j.next_seq {
            return EventsPoll {
                events: Vec::new(),
                cursor: EventCursor { epoch: self.epoch, seq: j.next_seq - 1 },
                gap: true,
            };
        }
        // Events (cursor.seq, first_seq) have been evicted: the client
        // missed changes it can never receive.
        if cursor.seq + 1 < j.first_seq() {
            return EventsPoll {
                events: Vec::new(),
                cursor: EventCursor { epoch: self.epoch, seq: j.next_seq - 1 },
                gap: true,
            };
        }
        let mut events = Vec::new();
        let mut scanned_to = cursor.seq;
        for ev in j.buf.iter() {
            if ev.seq <= cursor.seq {
                continue;
            }
            let matches = match (content_kind, ev.content_kind) {
                // No filter: everything matches.
                (None, _) => true,
                // Filtered, event kind known: exact match only.
                (Some(want), Some(have)) => want == have,
                // Filtered, event kind unknown: include. A refresh is
                // idempotent; silently dropping a possibly-matching change
                // is not.
                (Some(_), None) => true,
            };
            if matches {
                if events.len() >= limit {
                    // Batch full: stop BEFORE consuming this event so the
                    // next poll resumes exactly here.
                    break;
                }
                events.push(ev.clone());
            }
            scanned_to = ev.seq;
        }
        EventsPoll {
            events,
            cursor: EventCursor { epoch: self.epoch, seq: scanned_to },
            gap: false,
        }
    }

    /// Park until an event newer than `after_seq` exists, the deadline
    /// passes, the hub shuts down, or the waiter cap is hit (returns
    /// immediately). Returns true when new events may exist.
    pub fn wait_beyond(&self, after_seq: u64, deadline: Instant) -> bool {
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        // Cap check-and-claim; over the cap the poll degrades to an
        // immediate empty response instead of parking one more thread.
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
                Err(cur) => claimed = cur,
            }
        }
        let mut woke = false;
        let mut j = self.journal.lock().unwrap();
        loop {
            if j.next_seq - 1 > after_seq {
                woke = true;
                break;
            }
            if self.stopped.load(Ordering::SeqCst) {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let slice = (deadline - now).min(Duration::from_millis(WAIT_SLICE_MS));
            let (guard, _timeout) = self.wake.wait_timeout(j, slice).unwrap();
            j = guard;
        }
        drop(j);
        self.waiters.fetch_sub(1, Ordering::SeqCst);
        woke
    }

    /// Wake every parked poll and refuse future waits. Idempotent; called
    /// before connection threads are joined so shutdown never blocks behind
    /// a long-poll deadline.
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn body(kind: &'static str, ns: &str, ck: Option<&'static str>) -> EventBody {
        EventBody::asset(kind, ns, format!("ast_{ns}"), 5).with_content_kind(ck)
    }

    #[test]
    fn cursor_render_parse_roundtrip_and_strictness() {
        let c = EventCursor { epoch: [0xab; 8], seq: 42 };
        let text = c.render();
        assert_eq!(text, format!("{}-42", "ab".repeat(8)));
        assert_eq!(EventCursor::parse(&text), Some(c));
        for bad in [
            "",
            "abab-1",                          // short epoch
            &format!("{}-", "ab".repeat(8)),   // empty seq
            &format!("{}-x1", "ab".repeat(8)), // non-digit seq
            &format!("{}-1", "AB".repeat(8)),  // uppercase epoch
            &"ab".repeat(8),                   // no separator
            &format!("{}-123456789012345678901", "ab".repeat(8)), // seq too long
        ] {
            assert_eq!(EventCursor::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn sequences_are_monotonic_and_retention_gaps_reported() {
        let hub = EventHub::new([1; 8], 4, 4);
        let start = hub.tail_cursor();
        assert_eq!(start.seq, 0);
        for i in 0..10u64 {
            hub.publish(body(KIND_ASSET_PUBLISHED, &format!("n{i}"), None));
        }
        // Ring cap 4: only seqs 7..=10 retained; the start cursor is out.
        let poll = hub.poll_after(start, None, 100);
        assert!(poll.gap);
        assert_eq!(poll.cursor.seq, 10);
        // Resuming from the gap cursor is clean.
        let poll = hub.poll_after(poll.cursor, None, 100);
        assert!(!poll.gap);
        assert!(poll.events.is_empty());
        // A cursor still inside the window replays exactly the missing tail.
        let mid = EventCursor { epoch: [1; 8], seq: 8 };
        let poll = hub.poll_after(mid, None, 100);
        assert!(!poll.gap);
        assert_eq!(poll.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![9, 10]);
    }

    #[test]
    fn epoch_mismatch_and_future_cursor_force_resync() {
        let hub = EventHub::new([2; 8], 8, 4);
        hub.publish(body(KIND_ALIAS_SET, "ns", None));
        let foreign = EventCursor { epoch: [9; 8], seq: 1 };
        assert!(hub.poll_after(foreign, None, 10).gap);
        let future = EventCursor { epoch: [2; 8], seq: 99 };
        assert!(hub.poll_after(future, None, 10).gap);
    }

    #[test]
    fn kind_filter_matches_exact_and_includes_unknown() {
        let hub = EventHub::new([3; 8], 16, 4);
        hub.publish(body(KIND_ASSET_PUBLISHED, "a", Some("video")));
        hub.publish(body(KIND_ASSET_PUBLISHED, "b", Some("mesh")));
        hub.publish(body(KIND_ASSET_PUBLISHED, "c", None));
        let start = EventCursor { epoch: [3; 8], seq: 0 };
        let poll = hub.poll_after(start, Some("video"), 10);
        let kinds: Vec<_> = poll.events.iter().map(|e| e.namespace.as_str()).collect();
        assert_eq!(kinds, vec!["a", "c"]);
        // Cursor advanced past the filtered-out mesh event too.
        assert_eq!(poll.cursor.seq, 3);
    }

    #[test]
    fn batch_limit_stops_before_dropping() {
        let hub = EventHub::new([4; 8], 16, 4);
        for i in 0..5u64 {
            hub.publish(body(KIND_ASSET_PUBLISHED, &format!("n{i}"), None));
        }
        let start = EventCursor { epoch: [4; 8], seq: 0 };
        let poll = hub.poll_after(start, None, 2);
        assert_eq!(poll.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(poll.cursor.seq, 2);
        let rest = hub.poll_after(poll.cursor, None, 100);
        assert_eq!(rest.events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
    }

    #[test]
    fn wait_wakes_on_publish_and_respects_shutdown_and_cap() {
        let hub = Arc::new(EventHub::new([5; 8], 16, 1));
        // Waker thread publishes shortly after the wait parks.
        let hub_pub = hub.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            hub_pub.publish(body(KIND_ALIAS_SET, "ns", None));
        });
        let woke = hub.wait_beyond(0, Instant::now() + Duration::from_secs(5));
        assert!(woke);
        t.join().unwrap();

        // Cap 1: a first parked waiter makes a second degrade immediately.
        let hub_a = hub.clone();
        let parked = std::thread::spawn(move || {
            hub_a.wait_beyond(1, Instant::now() + Duration::from_millis(600))
        });
        std::thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        let woke = hub.wait_beyond(1, Instant::now() + Duration::from_secs(5));
        assert!(!woke);
        assert!(start.elapsed() < Duration::from_millis(400), "over-cap wait must not park");
        let _ = parked.join().unwrap();

        // Shutdown wakes parked waiters promptly.
        let hub_b = hub.clone();
        let parked = std::thread::spawn(move || {
            hub_b.wait_beyond(1, Instant::now() + Duration::from_secs(30))
        });
        std::thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        hub.shutdown();
        assert!(!parked.join().unwrap());
        assert!(start.elapsed() < Duration::from_secs(2));
        // After shutdown nothing waits.
        assert!(!hub.wait_beyond(0, Instant::now() + Duration::from_secs(30)));
    }
}
