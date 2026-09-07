//! Journal-before-forward intake.
//!
//! Every observation the stamping layer produces is recorded here first
//! and only then forwarded to the consumer. Forwarding is bounded: the
//! consumer's callback returns `false` when it cannot admit an event right
//! now, the event stays in the backlog behind a forwarding cursor, and the
//! next observation (or an explicit [`Journal::pump`]) retries in order.
//! Forwarded records stay retained as history as far as the byte budget
//! allows, so a consumer can [`Journal::retained`] the recent past.
//!
//! When the unforwarded backlog alone exceeds the budget the oldest
//! unforwarded records are dropped and a sticky [`Gap`] is recorded: the
//! consumer reads it through [`Journal::gap`] and the watcher also emits a
//! `RescanRequired { reason: JournalGap }` per root in-band, so a gap is
//! never silent. The gap stays until [`Journal::clear_gap`].
//!
//! An optional spool file receives one line per record (tab separated:
//! seq, epoch, wall-clock milliseconds, kind, mount, path, and the old
//! name for a rename) before the record is forwarded.

use crate::{FileSystemEvent, FileSystemEventKind};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

pub type ForwardFn = Arc<dyn Fn(&FileSystemEvent) -> bool + Send + Sync + 'static>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalConfig {
    /// Byte budget for retained records: the unforwarded backlog first,
    /// then forwarded history as far as the budget allows.
    pub ring_bytes: usize,
    /// Append-only spool file written before forwarding, if any.
    pub spool: Option<PathBuf>,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            ring_bytes: 8 * 1024 * 1024,
            spool: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapReason {
    /// The unforwarded backlog outgrew the byte budget: the oldest
    /// unforwarded records were dropped before the consumer admitted them.
    AdmissionOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    /// The first and last sequence numbers dropped (inclusive; a gap that
    /// grows across several overflows keeps its first and extends its last).
    pub first_seq: u64,
    pub last_seq: u64,
    pub dropped: u64,
    pub at: Instant,
    pub reason: GapReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct JournalStats {
    pub recorded: u64,
    pub forwarded: u64,
    pub dropped: u64,
    /// Records recorded but not yet admitted by the consumer.
    pub backlog: usize,
    /// Records currently retained (backlog plus history).
    pub retained: usize,
    pub retained_bytes: usize,
    pub gap: Option<Gap>,
}

struct State {
    config: JournalConfig,
    ring: VecDeque<(FileSystemEvent, usize)>,
    ring_bytes: usize,
    /// Index into `ring` of the next record to forward.
    cursor: usize,
    recorded: u64,
    forwarded: u64,
    dropped: u64,
    gap: Option<Gap>,
    spool: Option<std::io::BufWriter<std::fs::File>>,
    forward: ForwardFn,
}

/// A cloneable handle onto one watcher's journal.
#[derive(Clone)]
pub struct Journal {
    inner: Arc<Mutex<State>>,
}

fn record_size(event: &FileSystemEvent) -> usize {
    let from = match &event.kind {
        FileSystemEventKind::Renamed { from } => from.as_os_str().len(),
        FileSystemEventKind::RescanRequired { root, .. } => root.as_os_str().len(),
        _ => 0,
    };
    std::mem::size_of::<FileSystemEvent>()
        + std::mem::size_of::<usize>()
        + event.path.as_os_str().len()
        + event.mount.len()
        + from
}

fn spool_line(event: &FileSystemEvent) -> String {
    let wall_ms = event
        .wall_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let (kind, extra) = match &event.kind {
        FileSystemEventKind::Changed => ("changed", None),
        FileSystemEventKind::Created => ("created", None),
        FileSystemEventKind::Removed => ("removed", None),
        FileSystemEventKind::Renamed { from } => ("renamed", Some(from.display().to_string())),
        FileSystemEventKind::RescanRequired { reason, .. } => ("rescan", Some(format!("{reason:?}"))),
    };
    let mut line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        event.seq,
        event.epoch,
        wall_ms,
        kind,
        event.mount,
        event.path.display()
    );
    if let Some(extra) = extra {
        line.push('\t');
        line.push_str(&extra);
    }
    line.push('\n');
    line
}

impl State {
    fn record(&mut self, event: FileSystemEvent) {
        if let Some(spool) = self.spool.as_mut() {
            let line = spool_line(&event);
            let _ = spool.write_all(line.as_bytes());
            let _ = spool.flush();
        }
        let size = record_size(&event);
        self.ring.push_back((event, size));
        self.ring_bytes += size;
        self.recorded += 1;
    }

    fn forward(&mut self) -> usize {
        let mut count = 0;
        while self.cursor < self.ring.len() {
            let accepted = (self.forward)(&self.ring[self.cursor].0);
            if !accepted {
                break;
            }
            self.cursor += 1;
            self.forwarded += 1;
            count += 1;
        }
        count
    }

    /// Evict forwarded history over budget, then drop unforwarded records
    /// if the backlog alone is over budget. Returns whether a gap opened.
    fn trim(&mut self) -> bool {
        let budget = self.config.ring_bytes;
        while self.ring_bytes > budget && self.cursor > 0 {
            if let Some((_, size)) = self.ring.pop_front() {
                self.ring_bytes -= size;
                self.cursor -= 1;
            }
        }
        if self.ring_bytes <= budget || self.ring.len() <= 1 {
            return false;
        }
        let mut first = None;
        let mut last = 0;
        let mut dropped = 0u64;
        while self.ring_bytes > budget && self.ring.len() > 1 {
            let Some((event, size)) = self.ring.pop_front() else {
                break;
            };
            self.ring_bytes -= size;
            first.get_or_insert(event.seq);
            last = event.seq;
            dropped += 1;
        }
        let Some(first) = first else {
            return false;
        };
        self.dropped += dropped;
        let at = Instant::now();
        self.gap = Some(match self.gap {
            Some(old) => Gap {
                first_seq: old.first_seq,
                last_seq: last,
                dropped: old.dropped + dropped,
                at,
                reason: GapReason::AdmissionOverflow,
            },
            None => Gap {
                first_seq: first,
                last_seq: last,
                dropped,
                at,
                reason: GapReason::AdmissionOverflow,
            },
        });
        true
    }

    fn stats(&self) -> JournalStats {
        JournalStats {
            recorded: self.recorded,
            forwarded: self.forwarded,
            dropped: self.dropped,
            backlog: self.ring.len() - self.cursor,
            retained: self.ring.len(),
            retained_bytes: self.ring_bytes,
            gap: self.gap,
        }
    }
}

impl Journal {
    pub(crate) fn new(config: JournalConfig, forward: ForwardFn) -> Result<Self, String> {
        let spool = match &config.spool {
            Some(path) => {
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|err| format!("cannot open journal spool {}: {err}", path.display()))?;
                Some(std::io::BufWriter::new(file))
            }
            None => None,
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(State {
                config,
                ring: VecDeque::new(),
                ring_bytes: 0,
                cursor: 0,
                recorded: 0,
                forwarded: 0,
                dropped: 0,
                gap: None,
                spool,
                forward,
            })),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record, then forward as far as the consumer admits, then trim to
    /// budget. Returns whether a gap opened during this admission.
    pub(crate) fn admit(&self, event: FileSystemEvent) -> bool {
        let mut state = self.lock();
        state.record(event);
        state.forward();
        state.trim()
    }

    /// Retry forwarding the backlog in order; returns how many records the
    /// consumer admitted. The consumer's callback must not call back into
    /// the journal.
    pub fn pump(&self) -> usize {
        let mut state = self.lock();
        let count = state.forward();
        state.trim();
        count
    }

    pub fn stats(&self) -> JournalStats {
        self.lock().stats()
    }

    pub fn backlog(&self) -> usize {
        let state = self.lock();
        state.ring.len() - state.cursor
    }

    /// The sticky gap, if admission ever overflowed since the last clear.
    pub fn gap(&self) -> Option<Gap> {
        self.lock().gap
    }

    pub fn clear_gap(&self) {
        self.lock().gap = None;
    }

    /// Retained records (history and backlog) with `seq >= since_seq`.
    pub fn retained(&self, since_seq: u64) -> Vec<FileSystemEvent> {
        self.lock()
            .ring
            .iter()
            .filter(|(event, _)| event.seq >= since_seq)
            .map(|(event, _)| event.clone())
            .collect()
    }

    pub fn flush(&self) {
        if let Some(spool) = self.lock().spool.as_mut() {
            let _ = spool.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn event(seq: u64, name: &str) -> FileSystemEvent {
        FileSystemEvent {
            mount: "m".into(),
            path: PathBuf::from(format!("/w/{name}")),
            kind: FileSystemEventKind::Changed,
            seq,
            epoch: 0,
            received_at: Instant::now(),
            wall_time: SystemTime::now(),
            is_dir: Some(false),
        }
    }

    #[test]
    fn records_before_forwarding_and_retries_the_backlog_in_order() {
        let accept = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let forward: ForwardFn = {
            let accept = Arc::clone(&accept);
            let seen = Arc::clone(&seen);
            Arc::new(move |event: &FileSystemEvent| {
                if accept.load(Ordering::Relaxed) {
                    seen.lock().unwrap().push(event.seq);
                    true
                } else {
                    false
                }
            })
        };
        let journal = Journal::new(JournalConfig::default(), forward).unwrap();
        for seq in 1..=5 {
            assert!(!journal.admit(event(seq, "a.rs")));
        }
        let stats = journal.stats();
        assert_eq!((stats.recorded, stats.forwarded, stats.backlog), (5, 0, 5));
        assert!(seen.lock().unwrap().is_empty(), "nothing forwarded while refused");
        accept.store(true, Ordering::Relaxed);
        assert_eq!(journal.pump(), 5);
        assert_eq!(*seen.lock().unwrap(), vec![1, 2, 3, 4, 5]);
        assert_eq!(journal.backlog(), 0);
        // History stays retained for replay.
        assert_eq!(journal.retained(4).len(), 2);
        assert!(journal.gap().is_none());
    }

    #[test]
    fn backlog_over_budget_drops_oldest_and_records_a_sticky_gap() {
        let forward: ForwardFn = Arc::new(|_: &FileSystemEvent| false);
        let budget = record_size(&event(1, "x.rs")) * 3;
        let journal = Journal::new(
            JournalConfig {
                ring_bytes: budget,
                spool: None,
            },
            forward,
        )
        .unwrap();
        for seq in 1..=3 {
            assert!(!journal.admit(event(seq, "x.rs")));
        }
        assert!(journal.admit(event(4, "x.rs")), "the fourth record overflows");
        let gap = journal.gap().expect("sticky gap");
        assert_eq!((gap.first_seq, gap.last_seq, gap.dropped), (1, 1, 1));
        assert!(journal.admit(event(5, "x.rs")));
        let gap = journal.gap().unwrap();
        assert_eq!((gap.first_seq, gap.last_seq, gap.dropped), (1, 2, 2));
        assert_eq!(journal.stats().dropped, 2);
        journal.clear_gap();
        assert!(journal.gap().is_none());
        assert_eq!(journal.retained(0).iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
    }

    #[test]
    fn forwarded_history_is_evicted_before_any_backlog() {
        let count = Arc::new(AtomicUsize::new(0));
        let forward: ForwardFn = {
            let count = Arc::clone(&count);
            Arc::new(move |_: &FileSystemEvent| {
                // Admit the first two, refuse afterwards.
                count.fetch_add(1, Ordering::Relaxed) < 2
            })
        };
        let budget = record_size(&event(1, "y.rs")) * 3;
        let journal = Journal::new(
            JournalConfig {
                ring_bytes: budget,
                spool: None,
            },
            forward,
        )
        .unwrap();
        assert!(!journal.admit(event(1, "y.rs")));
        assert!(!journal.admit(event(2, "y.rs")));
        assert!(!journal.admit(event(3, "y.rs")));
        assert!(!journal.admit(event(4, "y.rs")), "history 1 evicted, no gap");
        assert!(!journal.admit(event(5, "y.rs")), "history 2 evicted, no gap");
        assert_eq!(journal.stats().backlog, 3);
        assert!(journal.gap().is_none());
        assert!(journal.admit(event(6, "y.rs")), "backlog alone over budget");
        assert_eq!(journal.gap().unwrap().first_seq, 3);
    }

    #[test]
    fn spool_receives_one_line_per_record_before_forwarding() {
        let dir = std::env::temp_dir().join(format!(
            "fswatch-journal-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let spool = dir.join("spool.log");
        let forward: ForwardFn = Arc::new(|_: &FileSystemEvent| true);
        let journal = Journal::new(
            JournalConfig {
                ring_bytes: 1024,
                spool: Some(spool.clone()),
            },
            forward,
        )
        .unwrap();
        journal.admit(event(1, "s.rs"));
        let mut renamed = event(2, "t.rs");
        renamed.kind = FileSystemEventKind::Renamed {
            from: PathBuf::from("/w/s.rs"),
        };
        journal.admit(renamed);
        journal.flush();
        let text = std::fs::read_to_string(&spool).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("1\t0\t"));
        assert!(lines[0].contains("\tchanged\tm\t/w/s.rs"));
        assert!(lines[1].ends_with("\trenamed\tm\t/w/t.rs\t/w/s.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
