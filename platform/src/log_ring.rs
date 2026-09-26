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

#[derive(Default)]
struct Ring {
    next_seq: u64,
    lines: VecDeque<LogLine>,
}

fn ring() -> &'static Mutex<Ring> {
    static RING: OnceLock<Mutex<Ring>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(Ring::default()))
}

/// Keep one line. Called from the log sink, on whatever thread logged.
pub fn push(level: LogLevel, text: String) {
    let Ok(mut ring) = ring().lock() else { return };
    ring.next_seq += 1;
    let seq = ring.next_seq;
    ring.lines.push_back(LogLine { seq, level, text });
    while ring.lines.len() > CAP {
        ring.lines.pop_front();
    }
}

/// Lines newer than `cursor`, oldest first, at most `max` of them — the
/// newest `max` when there are more. The returned sequence is the newest
/// line's, to carry back as the next cursor.
pub fn read_since(cursor: u64, max: usize) -> (u64, Vec<LogLine>) {
    let Ok(ring) = ring().lock() else {
        return (cursor, Vec::new());
    };
    let mut lines: Vec<LogLine> =
        ring.lines.iter().filter(|line| line.seq > cursor).cloned().collect();
    if lines.len() > max {
        lines = lines.split_off(lines.len() - max);
    }
    (ring.next_seq, lines)
}

/// Empty the ring. Tests share one process, so each starts from nothing.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    if let Ok(mut ring) = ring().lock() {
        ring.lines.clear();
        ring.next_seq = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One ring, one process, and tests run at once: they take turns.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    #[test]
    fn lines_come_back_oldest_first_with_rising_sequence_numbers() {
        let _serial = serial();
        reset_for_test();
        push(LogLevel::Log, "first".into());
        push(LogLevel::Error, "second".into());
        let (newest, lines) = read_since(0, 10);
        assert_eq!(newest, 2);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[0].seq, 1);
        assert_eq!(lines[1].text, "second");
        assert_eq!(lines[1].level, LogLevel::Error);
    }

    #[test]
    fn a_cursor_asks_only_for_what_it_has_not_seen() {
        let _serial = serial();
        reset_for_test();
        push(LogLevel::Log, "one".into());
        push(LogLevel::Log, "two".into());
        let (newest, _) = read_since(0, 10);
        push(LogLevel::Log, "three".into());
        let (_, fresh) = read_since(newest, 10);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].text, "three");
    }

    #[test]
    fn a_reader_asking_for_a_few_gets_the_newest_few() {
        let _serial = serial();
        reset_for_test();
        for n in 0..10 {
            push(LogLevel::Log, format!("line {n}"));
        }
        let (_, tail) = read_since(0, 3);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].text, "line 7");
        assert_eq!(tail[2].text, "line 9", "newest last, the way a tail reads");
    }

    #[test]
    fn the_ring_forgets_the_oldest_rather_than_growing_without_end() {
        let _serial = serial();
        reset_for_test();
        for n in 0..(CAP + 50) {
            push(LogLevel::Log, format!("line {n}"));
        }
        let (newest, all) = read_since(0, CAP * 2);
        assert_eq!(newest as usize, CAP + 50);
        assert_eq!(all.len(), CAP);
        assert_eq!(all[0].text, "line 50", "the oldest fifty are gone");
    }

    #[test]
    fn a_line_the_app_logs_reaches_the_ring_with_no_remote_surface_running() {
        let _serial = serial();
        reset_for_test();
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
            let (_, lines) = read_since(0, CAP);
            let lines: Vec<_> = lines
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
        let _serial = serial();
        reset_for_test();
        for n in 0..(CAP + 10) {
            push(LogLevel::Log, format!("line {n}"));
        }
        let (_, lines) = read_since(1, CAP * 2);
        assert_eq!(lines.len(), CAP, "never a panic, never an empty answer");
    }
}
