//! The master's inbox: every record that matters (sev err / warn / req), from every machine, until
//! the user clears it. Kept in `~/.makepad/devhub/inbox.jsonl`, append-only (records, `seen`
//! and `clear` lines), compacted at start (an entry's first record and a `fold` line with its
//! count and last time); bounded to [`MAX_ENTRIES`].
//!
//! Repeats (same machine, pid, kind and line apart from its numbers, within [`DEDUP_MS`]) fold into one
//! entry with a count, so a JIT hitching every second is one line, not hundreds.

use crate::proto::{wall_ms, Record};
use makepad_remote::json_str;
use std::io::Write;
use std::path::PathBuf;

pub const MAX_ENTRIES: usize = 2000;
pub const DEDUP_MS: u64 = 30_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub record: Record,
    pub count: u32,
    pub last_ms: u64,
    pub seen: bool,
}

pub struct Inbox {
    path: Option<PathBuf>,
    pub entries: Vec<Entry>,
    hot_cleared: std::collections::HashSet<String>,
}

fn fold_key(r: &Record) -> (String, u32, String, String) {
    // the line without its numbers: `hitch 840 ms` and `hitch 900 ms` are the same trouble
    let head: String = r.line.chars().filter(|c| !c.is_ascii_digit()).take(120).collect();
    (r.machine.clone(), r.pid, r.kind.clone(), head)
}

impl Inbox {
    /// An inbox in memory only (tests).
    pub fn memory() -> Inbox {
        Inbox { path: None, entries: Vec::new(), hot_cleared: Default::default() }
    }

    /// Loads and compacts `path` (missing: empty).
    pub fn open(path: PathBuf) -> Inbox {
        let mut inbox = Inbox { path: None, entries: Vec::new(), hot_cleared: Default::default() };
        for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
            if let Some(id) = line.strip_prefix("jit-clear ") {
                inbox.hot_cleared.insert(id.to_string());
            } else if let Some(id) = line.strip_prefix("clear ") {
                inbox.clear(id.trim());
            } else if let Some(id) = line.strip_prefix("seen ") {
                inbox.mark_seen(id.trim());
            } else if let Some(fold) = line.strip_prefix("fold ") {
                inbox.fold(fold);
            } else if let Some(r) = line.strip_prefix("rec ").and_then(Record::parse) {
                inbox.add(r);
            }
        }
        inbox.path = Some(path);
        inbox.rewrite();
        inbox
    }

    fn append(&self, line: &str) {
        let Some(path) = &self.path else { return };
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }

    fn rewrite(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut text = String::new();
        for id in &self.hot_cleared { text.push_str(&format!("jit-clear {id}\n")); }
        for e in &self.entries {
            text.push_str("rec ");
            text.push_str(&e.record.to_json());
            text.push('\n');
            if e.count > 1 || e.last_ms != e.record.at_ms {
                text.push_str(&format!("fold {} {} {}\n", e.record.id, e.count, e.last_ms));
            }
            if e.seen {
                text.push_str(&format!("seen {}\n", e.record.id));
            }
        }
        let tmp = path.with_extension("jsonl.new");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// A compacted entry's repeats: `<id> <count> <last ms>`.
    fn fold(&mut self, line: &str) {
        let mut w = line.split_whitespace();
        let (Some(id), Some(count), Some(last)) = (w.next(), w.next().and_then(|c| c.parse().ok()), w.next().and_then(|l| l.parse().ok())) else { return };
        if let Some(e) = self.entries.iter_mut().find(|e| e.record.id == id) {
            (e.count, e.last_ms) = (count, last);
        }
    }

    /// Takes a record; false when it is not inbox material (info).
    pub fn add(&mut self, r: Record) -> bool {
        if !r.sev.inbox() {
            return false;
        }
        if self.hot_cleared.contains(&r.id) { return false; }
        if self.entries.iter().any(|e| e.record.id == r.id) {
            return false;
        }
        let key = fold_key(&r);
        self.append(&format!("rec {}", r.to_json()));
        if let Some(e) = self.entries.iter_mut().rev().find(|e| !matches!(r.kind.as_str(), "patch_failed" | "jit_error") && fold_key(&e.record) == key && r.at_ms.saturating_sub(e.last_ms) < DEDUP_MS) {
            e.count += 1;
            e.last_ms = r.at_ms;
            e.seen = false;
            return true;
        }
        let at = r.at_ms;
        self.entries.push(Entry { record: r, count: 1, last_ms: at, seen: false });
        while self.entries.len() > MAX_ENTRIES {
            let Some(i) = self.entries.iter().position(|e| !matches!(e.record.kind.as_str(), "patch_failed" | "jit_error")) else { break };
            self.entries.remove(i);
        }
        true
    }

    pub fn unseen(&self) -> usize {
        self.entries.iter().filter(|e| !e.seen).count()
    }

    /// The unseen entries by severity: `{"unseen","err","warn","req"}` (the control panel's strip).
    pub fn counts(&self) -> String {
        let n = |sev: crate::proto::Sev| self.entries.iter().filter(|e| !e.seen && e.record.sev == sev).count();
        format!("{{\"unseen\":{},\"err\":{},\"warn\":{},\"req\":{}}}", self.unseen(), n(crate::proto::Sev::Err), n(crate::proto::Sev::Warn), n(crate::proto::Sev::Req))
    }

    pub fn mark_seen(&mut self, id: &str) {
        let mut hit = false;
        for e in self.entries.iter_mut().filter(|e| id == "all" || e.record.id == id) {
            e.seen = true;
            hit = true;
        }
        if hit && self.path.is_some() {
            self.append(&format!("seen {id}"));
        }
    }

    /// Clears one entry, or `all`.
    pub fn clear(&mut self, id: &str) -> usize {
        let before = self.entries.len();
        for e in self.entries.iter().filter(|e| (id == "all" || e.record.id == id) && matches!(e.record.kind.as_str(), "patch_failed" | "jit_error")) {
            self.hot_cleared.insert(e.record.id.clone());
        }
        self.entries.retain(|e| !(id == "all" || e.record.id == id));
        let n = before - self.entries.len();
        if n > 0 && self.path.is_some() {
            self.append(&format!("clear {id}"));
        }
        n
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.record.id == id)
    }

    /// One line per entry, newest first: `{"id","count","seen","line"(the stream line)}`.
    pub fn list(&self, all: bool, out: &mut Vec<String>) {
        for e in self.entries.iter().rev().filter(|e| all || !e.seen) {
            out.push(format!(
                "{{\"id\":{},\"count\":{},\"seen\":{},\"last_ms\":{},\"line\":{}}}",
                json_str(&e.record.id),
                e.count,
                e.seen,
                e.last_ms,
                json_str(&e.record.stream_line())
            ));
        }
    }

    pub fn now() -> u64 {
        wall_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{hub_record, Sev};

    #[test]
    fn repeats_fold_info_stays_out_and_clear_persists() {
        let dir = std::env::temp_dir().join(format!("devhub-inbox-test-{}", std::process::id()));
        let path = dir.join("inbox.jsonl");
        let mut ib = Inbox::open(path.clone());
        let h = |line: &str| hub_record("node-a", "foo", 7, "hitch", Sev::Warn, line.to_string(), String::new());
        assert!(ib.add(h("hitch 840 ms (typical 8.0 ms)")));
        assert!(ib.add(h("hitch 900 ms (typical 8.0 ms)")));
        assert!(!ib.add(hub_record("node-a", "", 0, "hub", Sev::Info, "hub: up".into(), String::new())));
        assert_eq!(ib.entries.len(), 1);
        assert_eq!(ib.entries[0].count, 2);
        let crash = hub_record("node-a", "foo", 7, "crash", Sev::Err, "crash: ended".into(), String::new());
        let id = crash.id.clone();
        ib.add(crash);
        assert_eq!(ib.unseen(), 2);
        ib.mark_seen(&id);
        assert_eq!(ib.unseen(), 1);
        let first = ib.entries[0].record.id.clone();
        assert_eq!(ib.clear(&first), 1);
        let again = Inbox::open(path.clone());
        assert_eq!(again.entries.len(), 1);
        assert!(again.entries[0].seen);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    /// An app restarted in place keeps its pid and counts its events from seq 1 again: its
    /// records are kept, while the same event heard again (a hub that follows the JIT again) is
    /// not. A restarted run's panics were dropped as seen ones (11-06).
    #[test]
    fn a_run_started_again_in_place_is_heard() {
        let panic = |seq: u64, clock: u64| {
            let ev = format!("{{\"event\":\"panic\",\"seq\":{seq},\"pid\":4242,\"app\":\"makepad-foo\",\"at_ms\":{clock},\"message\":\"boom\",\"location\":\"src/a.rs:10\",\"thread\":\"main\"}}");
            crate::proto::from_jit_event("box-a", "makepad-foo", 4242, &ev, makepad_devhub_conf::HITCH_MS).unwrap()
        };
        let mut ib = Inbox::memory();
        assert!(ib.add(panic(1, 5120)));
        assert!(!ib.add(panic(1, 5120)), "the same event heard again");
        assert!(ib.add(panic(1, 830)), "the run started again in place");
        assert_eq!(ib.entries.iter().map(|e| e.count).sum::<u32>(), 2);
    }

    /// An entry's count and last time outlive a compaction (each master start compacts): the
    /// compaction wrote its first record once per repeat, and the next start dropped the copies
    /// by their id, so every count read 1 after two starts (11-06).
    #[test]
    fn counts_outlive_compactions() {
        let dir = std::env::temp_dir().join(format!("devhub-inbox-fold-{}", std::process::id()));
        let path = dir.join("inbox.jsonl");
        let _ = std::fs::remove_dir_all(&dir);
        let h = |line: &str| hub_record("box-a", "foo", 7, "hitch", Sev::Warn, line.to_string(), String::new());
        let mut ib = Inbox::open(path.clone());
        for ms in [840, 900, 910] {
            assert!(ib.add(h(&format!("hitch {ms} ms (typical 8.0 ms)"))));
        }
        let last = ib.entries[0].last_ms;
        drop(ib);
        for _ in 0..3 {
            let ib = Inbox::open(path.clone());
            assert_eq!((ib.entries.len(), ib.entries[0].count, ib.entries[0].last_ms), (1, 3, last));
        }
        // a repeat after the compactions still folds into it
        let mut ib = Inbox::open(path.clone());
        assert!(ib.add(h("hitch 950 ms (typical 8.0 ms)")));
        drop(ib);
        let ib = Inbox::open(path.clone());
        assert_eq!((ib.entries.len(), ib.entries[0].count), (1, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
