//! Relax's names as devhub reads them: its variables (`RELAX_*`) and its message prefix
//! (`relax:` and `relax <cmd>:`) in a JIT's output.

use std::path::{Path, PathBuf};

const PREFIX: &str = "RELAX_";

/// Whether `key` is Relax's variable `suffix` (`VFS` for `RELAX_VFS`).
pub fn is(key: &str, suffix: &str) -> bool {
    key.strip_prefix(PREFIX) == Some(suffix)
}

/// Relax's variable `suffix` (`RELAX_VFS` for `VFS`).
pub fn var(suffix: &str) -> String {
    format!("{PREFIX}{suffix}")
}

/// The text after Relax's message prefix in a line of a JIT's output: `word` is "" for the
/// bare `relax:`, else the command (`live` for `relax live:`).
pub fn after<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let r = line.strip_prefix("relax")?;
    let r = if word.is_empty() { r } else { r.strip_prefix(' ')?.strip_prefix(word)? };
    r.strip_prefix(':')
}

/// Whether `line` is Relax's message `word` (see [`after`]) whose text starts with `text`.
pub fn says(line: &str, word: &str, text: &str) -> bool {
    after(line, word).is_some_and(|r| r.trim_start().starts_with(text))
}

/// Where a JIT of process `pid` writes its live-edit records, under `tmp`, when its environment
/// names none.
pub fn live_log(tmp: &Path, pid: u32) -> PathBuf {
    tmp.join(format!("relax-live-{pid}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relax_names_and_its_message_prefix_read() {
        assert!(is("RELAX_EDITS", "EDITS") && !is("RELAX_EDITS_X", "EDITS") && !is("MAKEPAD_HIDE_WINDOWS", "HIDE_WINDOWS"), "a variable is Relax's by its prefix");
        assert_eq!(var("VFS"), "RELAX_VFS");
        assert_eq!(after("relax live: restarting", "live"), Some(" restarting"));
        assert!(says("relax: POISONED: x", "", "POISONED:"));
        assert!(after("relax check: 0 errors", "").is_none() && after("relaxed: x", "").is_none() && after("makepad devhub: x", "live").is_none(), "only the prefix itself");
        assert_eq!(live_log(Path::new("/t"), 7), Path::new("/t/relax-live-7.jsonl"));
    }
}
