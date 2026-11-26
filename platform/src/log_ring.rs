//! Every line this process logs, kept where anything in the app can read it.
//!
//! The single sink in `log.rs` fills this on every `log!` and `error!`. It
//! used to fill a ring inside the remote control surface, which meant an app
//! could not show its own log unless it had been started for automation.

use crate::log::LogLevel;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// Lines kept. Enough to cover the minute before something went wrong,
/// small enough that an app logging in a loop cannot grow memory.
pub const CAP: usize = 2000;

#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub seq: u64,
    pub level: LogLevel,
    pub text: String,
}

/// What one read since a cursor found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogRead {
    /// The newest line's sequence number, to carry back as the next cursor.
    pub newest: u64,
    /// Lines after the cursor that the ring no longer keeps: older than its oldest line. Their
    /// text went to the process's terminal output (its stderr) like every line; only this view
    /// of the log has lost them. A reader that needs every line after its cursor has a gap.
    pub dropped: u64,
    /// The lines after the cursor that are kept, oldest first (the newest `max` of them).
    pub lines: Vec<LogLine>,
}

#[derive(Default)]
struct Ring {
    next_seq: u64,
    lines: VecDeque<LogLine>,
}

impl Ring {
    fn push(&mut self, level: LogLevel, text: String) {
        self.next_seq += 1;
        let seq = self.next_seq;
        self.lines.push_back(LogLine { seq, level, text });
        while self.lines.len() > CAP {
            self.lines.pop_front();
        }
    }

    fn read_since(&self, cursor: u64, max: usize) -> LogRead {
        let mut lines: Vec<LogLine> =
            self.lines.iter().filter(|line| line.seq > cursor).cloned().collect();
        if lines.len() > max {
            lines = lines.split_off(lines.len() - max);
        }
        // the lines numbered past the cursor and before the oldest one kept
        let oldest = self.lines.front().map_or(self.next_seq + 1, |line| line.seq);
        let dropped = oldest.saturating_sub(cursor.saturating_add(1));
        LogRead { newest: self.next_seq, dropped, lines }
    }
}

fn ring() -> &'static Mutex<Ring> {
    static RING: OnceLock<Mutex<Ring>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(Ring::default()))
}

/// Keep one line. Called from the log sink, on whatever thread logged.
pub fn push(level: LogLevel, text: String) {
    let Ok(mut ring) = ring().lock() else { return };
    ring.push(level, text);
}

/// Lines newer than `cursor`, oldest first, at most `max` of them — the
/// newest `max` when there are more — with the newest line's sequence number
/// and how many lines after `cursor` the ring no longer keeps ([`LogRead`]).
pub fn read_since(cursor: u64, max: usize) -> LogRead {
    let Ok(ring) = ring().lock() else {
        return LogRead { newest: cursor, ..LogRead::default() };
    };
    ring.read_since(cursor, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every test in this process logs through the one process-wide ring, so
    // the ring's own rules are checked on a ring of each test's own: read from
    // the shared one, a line another test logged meanwhile turned up in the
    // middle (2 lines asked, 3 read back, 11-24).

    #[test]
    fn lines_come_back_oldest_first_with_rising_sequence_numbers() {
        let mut ring = Ring::default();
        ring.push(LogLevel::Log, "first".into());
        ring.push(LogLevel::Error, "second".into());
        let LogRead { newest, dropped, lines } = ring.read_since(0, 10);
        assert_eq!(newest, 2);
        assert_eq!(dropped, 0, "nothing forgotten yet");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[0].seq, 1);
        assert_eq!(lines[1].text, "second");
        assert_eq!(lines[1].seq, 2);
        assert_eq!(lines[1].level, LogLevel::Error);
    }

    #[test]
    fn a_cursor_asks_only_for_what_it_has_not_seen() {
        let mut ring = Ring::default();
        ring.push(LogLevel::Log, "one".into());
        ring.push(LogLevel::Log, "two".into());
        let newest = ring.read_since(0, 10).newest;
        ring.push(LogLevel::Log, "three".into());
        let fresh = ring.read_since(newest, 10).lines;
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].text, "three");
    }

    #[test]
    fn a_reader_asking_for_a_few_gets_the_newest_few() {
        let mut ring = Ring::default();
        for n in 0..10 {
            ring.push(LogLevel::Log, format!("line {n}"));
        }
        let read = ring.read_since(0, 3);
        assert_eq!(read.dropped, 0, "a short answer the reader asked for is no gap");
        let tail = read.lines;
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].text, "line 7");
        assert_eq!(tail[2].text, "line 9", "newest last, the way a tail reads");
    }

    #[test]
    fn the_ring_forgets_the_oldest_rather_than_growing_without_end() {
        let mut ring = Ring::default();
        for n in 0..(CAP + 50) {
            ring.push(LogLevel::Log, format!("ring-test {n}"));
        }
        let LogRead { newest, dropped, lines: all } = ring.read_since(0, CAP * 2);
        assert_eq!(newest as usize, CAP + 50);
        assert_eq!(dropped, 50, "a reader from the start learns the fifty it can no longer see");
        assert_eq!(all.len(), CAP, "the ring holds its newest {CAP} lines");
        let ours: Vec<usize> = all.iter().filter_map(|l| l.text.strip_prefix("ring-test ")?.parse().ok()).collect();
        assert_eq!(ours.last(), Some(&(CAP + 49)));
        assert!(ours.windows(2).all(|w| w[1] == w[0] + 1), "in order, none lost from the middle");
        assert_eq!(ours[0], 50, "the oldest fifty are gone");
    }

    #[test]
    fn a_line_the_app_logs_reaches_the_ring_with_no_remote_surface_running() {
        crate::Cx::init_log();
        crate::log::log_with_level_makepad_platform(
            "widgets/src/thing.rs",
            41,
            8,
            41,
            20,
            "the thing happened".into(),
            LogLevel::Warning,
        );
        // Native logging is asynchronous, and other tests may log too.
        // Wait for this record, without requiring a remote surface or an
        // otherwise empty process-wide ring.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let lines = loop {
            let lines: Vec<_> = read_since(0, CAP).lines
                .into_iter()
                .filter(|line| {
                    line.text.contains("widgets/src/thing.rs")
                        && line.text.contains("the thing happened")
                })
                .collect();
            if !lines.is_empty() || std::time::Instant::now() >= deadline {
                break lines;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].level, LogLevel::Warning);
        assert!(
            lines[0].text.contains("the thing happened")
                && lines[0].text.contains("widgets/src/thing.rs"),
            "the line carries where it came from: {}",
            lines[0].text
        );
    }

    #[test]
    fn a_cursor_older_than_the_forgotten_lines_still_answers() {
        let mut ring = Ring::default();
        for n in 0..(CAP + 10) {
            ring.push(LogLevel::Log, format!("line {n}"));
        }
        let read = ring.read_since(1, CAP * 2);
        assert_eq!(read.lines.len(), CAP, "never a panic, never an empty answer");
        assert_eq!(read.dropped, 9, "lines 2..=10 were forgotten after the cursor");
        assert_eq!(ring.read_since(10, 1).dropped, 0, "a cursor at the last forgotten line misses none");
        assert_eq!(ring.read_since(CAP as u64 + 10, 5).dropped, 0, "a cursor at the newest line misses none");
        assert_eq!(ring.read_since(u64::MAX, 0), LogRead { newest: CAP as u64 + 10, dropped: 0, lines: Vec::new() },
            "a cursor past the newest line asks only for the next cursor");
    }
}
