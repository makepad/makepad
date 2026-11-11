//! What devhub learns for an agent after its calls returned: a post-land bucket red that names a
//! landing of the agent's (bucket.rs). Such a red reached the lane that landed it only through
//! main's check-in, half an hour on (11-11). A note waits in `<home>/agent-notes.jsonl`, across
//! master restarts, until the agent's CLI next speaks to the master (`agent-seen`: each of its
//! devhub calls, and each minute of a wait such as `job wait`), whose answer says it on that
//! call's stderr, once.

use crate::proto::wall_ms;
use makepad_remote::json_str;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Leaving and taking rewrite one file: one at a time within the master.
static FILE: Mutex<()> = Mutex::new(());

fn path(home: &Path) -> PathBuf {
    home.join("agent-notes.jsonl")
}

/// Leaves `line` for `agent`'s next devhub call.
pub fn leave(home: &Path, agent: &str, line: &str) -> std::io::Result<()> {
    let _one = FILE.lock().unwrap();
    std::fs::create_dir_all(home)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path(home))?;
    writeln!(file, "{{\"agent\":{},\"ms\":{},\"line\":{}}}", json_str(agent), wall_ms(), json_str(line))
}

/// The notes left for `agent`, oldest first, with when each was left; they leave the file, the
/// other agents' stay.
pub fn take(home: &Path, agent: &str) -> Vec<(u64, String)> {
    let _one = FILE.lock().unwrap();
    let file = path(home);
    let Ok(text) = std::fs::read_to_string(&file) else { return Vec::new() };
    let (mut mine, mut rest) = (Vec::new(), String::new());
    for l in text.lines().filter(|l| !l.is_empty()) {
        let v = makepad_strict_json::parse(l.as_bytes()).ok();
        let note = v.as_ref().filter(|v| v.get("agent").and_then(|a| a.as_str()) == Some(agent))
            .and_then(|v| Some((v.get("ms")?.as_u64()?, v.get("line")?.as_str()?.to_string())));
        match note {
            Some(note) => mine.push(note),
            None => {
                rest.push_str(l);
                rest.push('\n');
            }
        }
    }
    if !mine.is_empty() {
        let new = file.with_extension("jsonl.new");
        if let Err(e) = std::fs::write(&new, rest).and_then(|_| std::fs::rename(&new, &file)) {
            eprintln!("devhub: {}: {e}", file.display());
        }
    }
    mine
}

/// A note as the CLI says a service's `say` line on its stderr (cli.rs `said_line`).
pub fn said(agent: &str, age_ms: u64, line: &str) -> String {
    let min = age_ms / 60_000;
    let age = if min == 0 { "just now".to_string() } else { format!("{min} min ago") };
    format!("{{\"say\":{},\"waiting\":false}}", json_str(&format!("devhub, for {agent} ({age}): {line}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A note is said once, to its own agent, after a master restart too (it lives in the file).
    #[test]
    fn a_note_is_taken_once_by_its_own_agent() {
        let home = std::env::temp_dir().join(format!("devhub-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        assert!(take(&home, "AGENT-A").is_empty(), "no file yet");
        leave(&home, "AGENT-A", "bucket fast FAILED one").unwrap();
        leave(&home, "AGENT-B", "bucket fast FAILED two").unwrap();
        leave(&home, "AGENT-A", "bucket tests FAILED three").unwrap();
        let mine: Vec<String> = take(&home, "AGENT-A").into_iter().map(|(_, l)| l).collect();
        assert_eq!(mine, ["bucket fast FAILED one", "bucket tests FAILED three"], "its own, oldest first");
        assert!(take(&home, "AGENT-A").is_empty(), "said once");
        let theirs = take(&home, "AGENT-B");
        assert_eq!(theirs.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["bucket fast FAILED two"], "the other agent's stayed");
        let say = makepad_strict_json::parse(said("AGENT-B", 3 * 60_000 + 5, &theirs[0].1).as_bytes()).unwrap();
        assert_eq!(say.get("say").and_then(|s| s.as_str()), Some("devhub, for AGENT-B (3 min ago): bucket fast FAILED two"));
        assert_eq!(say.get("waiting").and_then(|w| w.as_bool()), Some(false), "the CLI says a say line only with its waiting flag");
        let _ = std::fs::remove_dir_all(&home);
    }
}
