//! The master's inbox: every record that matters (sev err / warn / req), from every machine, until
//! the user clears it. Kept in `~/.makepad/devhub/inbox.jsonl`, append-only (records, `seen`
//! and `clear` lines), compacted at start; bounded to [`MAX_ENTRIES`].
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
}

fn fold_key(r: &Record) -> (String, u32, String, String) {
    // the line without its numbers: `hitch 840 ms` and `hitch 900 ms` are the same trouble
    let head: String = r.line.chars().filter(|c| !c.is_ascii_digit()).take(120).collect();
    (r.machine.clone(), r.pid, r.kind.clone(), head)
}

impl Inbox {
    /// An inbox in memory only (tests).
    pub fn memory() -> Inbox {
        Inbox { path: None, entries: Vec::new() }
    }

    /// Loads and compacts `path` (missing: empty).
    pub fn open(path: PathBuf) -> Inbox {
        let mut inbox = Inbox { path: None, entries: Vec::new() };
        for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
            if let Some(id) = line.strip_prefix("clear ") {
                inbox.clear(id.trim());
            } else if let Some(id) = line.strip_prefix("seen ") {
                inbox.mark_seen(id.trim());
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
        for e in &self.entries {
            for _ in 0..e.count {
                text.push_str("rec ");
                text.push_str(&e.record.to_json());
                text.push('\n');
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

    /// Takes a record; false when it is not inbox material (info).
    pub fn add(&mut self, r: Record) -> bool {
        if !r.sev.inbox() {
            return false;
        }
        if self.entries.iter().any(|e| e.record.id == r.id) {
            return false;
        }
        let key = fold_key(&r);
        self.append(&format!("rec {}", r.to_json()));
        if let Some(e) = self.entries.iter_mut().rev().find(|e| fold_key(&e.record) == key && r.at_ms.saturating_sub(e.last_ms) < DEDUP_MS) {
            e.count += 1;
            e.last_ms = r.at_ms;
            e.seen = false;
            return true;
        }
        let at = r.at_ms;
        self.entries.push(Entry { record: r, count: 1, last_ms: at, seen: false });
        if self.entries.len() > MAX_ENTRIES {
            let drop = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..drop);
        }
        true
    }

    pub fn unseen(&self) -> usize {
        self.entries.iter().filter(|e| !e.seen).count()
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
        let h = |line: &str| hub_record("217", "sandbox", 7, "hitch", Sev::Warn, line.to_string(), String::new());
        assert!(ib.add(h("hitch 840 ms (typical 8.0 ms)")));
        assert!(ib.add(h("hitch 900 ms (typical 8.0 ms)")));
        assert!(!ib.add(hub_record("217", "", 0, "hub", Sev::Info, "hub: up".into(), String::new())));
        assert_eq!(ib.entries.len(), 1);
        assert_eq!(ib.entries[0].count, 2);
        let crash = hub_record("217", "sandbox", 7, "crash", Sev::Err, "crash: ended".into(), String::new());
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
}
