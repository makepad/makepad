//! The bodies the borrow checker found clean in an earlier run of a program, under any Relax
//! build: one hash of each body's path and text, 16 bytes a body, appended to
//! `~/.relax/seen-bodies/<program's crate>-<its identity>.bin` (the identity: the crate's name, its
//! root file and whether it is a test build). Only the background checker's order reads it:
//! after a start (a restart in place among them) a body whose path and text no run checked clean
//! is checked ahead of the startup's other checks (lazy.rs `RANK_EDITED`), so the error in the
//! body the person just edited comes back in seconds. The verdict cache keys every body by the
//! Relax build and its crates' interfaces, so after a Relax update or an interface edit every
//! body misses it, and the edited one waited behind the startup's thousands: a per-frame body's
//! borrow error came 228 s after its app's start, and 208 s after a restart that applied an edit
//! (W14431, 11-19). A program with no file yet (its first run here) ranks nothing first, and a
//! body an earlier run left unchecked (it quit before the background pass ended) ranks first
//! once more, until a run checks it clean. A lost, torn or stale file only changes the order.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

pub(crate) struct SeenBodies {
    path: PathBuf,
    /// the file had bodies when it was opened: an absent body is an edit, not a first run
    known: bool,
    seen: HashSet<u128>,
    /// seen this run, not written yet ([`Self::flush`])
    added: Vec<u128>,
}

impl SeenBodies {
    /// The index of program `name` (its crate's name and identity) in `dir` (`~/.relax/seen-bodies`).
    pub(crate) fn open(dir: PathBuf, name: &str) -> Self {
        let file: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
        let path = dir.join(format!("{file}.bin"));
        // (a torn last record of a write cut short is left out)
        let seen: HashSet<u128> = std::fs::read(&path).map(|b| b.chunks_exact(16).map(|r| u128::from_le_bytes(r.try_into().unwrap())).collect()).unwrap_or_default();
        Self { path, known: !seen.is_empty(), seen, added: Vec::new() }
    }

    /// Body `key` ([`key`]) was not checked clean by an earlier run of a program this index knows.
    pub(crate) fn edited(&self, key: u128) -> bool {
        self.known && !self.seen.contains(&key)
    }

    /// Body `key` was checked clean (or its clean verdict reused).
    pub(crate) fn clean(&mut self, key: u128) {
        if self.seen.insert(key) {
            self.added.push(key);
        }
    }

    /// Appends the bodies seen since the last flush, in one write (two runs of a program append
    /// whole records side by side).
    pub(crate) fn flush(&mut self) -> std::io::Result<()> {
        if self.added.is_empty() {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        // a write cut short (a run killed mid-flush) left a torn record: filled up to a whole one
        // (a key no body has), so these records start on a record's edge, as every reader reads them
        let torn = (file.metadata()?.len() % 16) as usize;
        let mut bytes = vec![0u8; if torn == 0 { 0 } else { 16 - torn }];
        bytes.extend(self.added.iter().flat_map(|k| k.to_le_bytes()));
        file.write_all(&bytes)?;
        self.added.clear();
        Ok(())
    }
}

/// The key of a body: its definition's path and its text (`items::body_hash`, without its
/// place in the file: an edit above it is no edit of it).
pub(crate) fn key(path: &str, body: u128) -> u128 {
    let mut h = crate::sip::Sip128::new();
    h.str("seen-body");
    h.str(path);
    h.u128(body);
    h.finish128()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first run ranks nothing first and keeps what it checked clean; the next run of the
    /// program finds those bodies seen and every other one (an edit, or a body no run checked
    /// clean) edited; a torn last record is left out and the next run's records are read whole
    /// after it, and another program's index is its own.
    #[test]
    fn a_program_knows_the_bodies_an_earlier_run_checked_clean() {
        let dir = std::env::temp_dir().join(format!("relax-seen-bodies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b, edited_b) = (key("app::a", 1), key("app::b", 2), key("app::b", 3));
        let mut first = SeenBodies::open(dir.clone(), "my-app");
        assert!(!first.edited(a), "no file yet: nothing ranks first");
        first.clean(a);
        first.clean(b);
        first.clean(a);
        first.flush().unwrap();
        assert_eq!(std::fs::metadata(dir.join("my-app.bin")).unwrap().len(), 32, "each body once");
        let mut next = SeenBodies::open(dir.clone(), "my-app");
        assert!(!next.edited(a) && !next.edited(b), "checked clean before: in their place");
        assert!(next.edited(edited_b), "the same fn with another text: first");
        assert!(next.edited(key("app::c", 1)), "a body no run checked clean: first");
        assert_ne!(key("app::a", 1), key("app::c", 1), "the path is part of the key");
        next.clean(edited_b);
        next.flush().unwrap();
        // a write cut short: its torn record is left out
        std::fs::OpenOptions::new().append(true).open(dir.join("my-app.bin")).unwrap().write_all(&[7; 5]).unwrap();
        let mut again = SeenBodies::open(dir.clone(), "my-app");
        assert!(!again.edited(edited_b) && !again.edited(a), "both runs' bodies are kept");
        let c = key("app::c", 4);
        again.clean(c);
        again.flush().unwrap();
        let after = SeenBodies::open(dir.clone(), "my-app");
        assert!(!after.edited(c) && !after.edited(a) && !after.edited(edited_b), "a run after the torn write is read whole");
        assert!(after.edited(key("app::d", 5)));
        assert!(!SeenBodies::open(dir.clone(), "other").edited(edited_b), "another program has no file: nothing first");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
