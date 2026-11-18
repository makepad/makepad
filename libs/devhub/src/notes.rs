//! What devhub learns for an agent after its calls returned: a post-land bucket red that names a
//! landing of the agent's (bucket.rs). Such a red reached the lane that landed it only through
//! main's check-in, half an hour on (11-11). A note waits in `<home>/agent-notes.jsonl`, across
//! master restarts, until the agent's CLI next speaks to the master (`agent-seen`: each of its
//! devhub calls, and each minute of a wait such as `job wait`), whose answer says it on that
//! call's stderr, once. A red's note that a later run passed before the agent's next call is not
//! said: several lanes heard of a red an hour after a later landing had made it pass (11-17).

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
    write(home, agent, line, "")
}

/// Leaves a bucket red's `line` for `agent`'s next devhub call: units `keys`, red in a run that
/// started at `after` (wall ms). [`take_open`] drops it once each of them passed since.
pub fn leave_red(home: &Path, agent: &str, line: &str, after: u64, keys: &[String]) -> std::io::Result<()> {
    let keys: Vec<String> = keys.iter().map(|k| json_str(k)).collect();
    write(home, agent, line, &format!(",\"after\":{after},\"keys\":[{}]", keys.join(",")))
}

fn write(home: &Path, agent: &str, line: &str, more: &str) -> std::io::Result<()> {
    let _one = FILE.lock().unwrap();
    std::fs::create_dir_all(home)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path(home))?;
    writeln!(file, "{{\"agent\":{},\"ms\":{},\"line\":{}{more}}}", json_str(agent), wall_ms(), json_str(line))
}

/// The notes left for `agent`, oldest first, with when each was left; they leave the file, the
/// other agents' stay.
pub fn take(home: &Path, agent: &str) -> Vec<(u64, String)> {
    take_open(home, agent, &|_, _| false)
}

/// [`take`], less a red's note whose units `passed` says passed since its run started (`passed(after,
/// keys)`): it leaves the file unsaid.
pub fn take_open(home: &Path, agent: &str, passed: &dyn Fn(u64, &[String]) -> bool) -> Vec<(u64, String)> {
    let _one = FILE.lock().unwrap();
    let file = path(home);
    let Ok(text) = std::fs::read_to_string(&file) else { return Vec::new() };
    let (mut mine, mut rest, mut taken) = (Vec::new(), String::new(), false);
    for l in text.lines().filter(|l| !l.is_empty()) {
        let v = makepad_strict_json::parse(l.as_bytes()).ok();
        let v = v.as_ref().filter(|v| v.get("agent").and_then(|a| a.as_str()) == Some(agent));
        let note = v.and_then(|v| Some((v.get("ms")?.as_u64()?, v.get("line")?.as_str()?.to_string())));
        let red = v.and_then(|v| Some((v.get("after")?.as_u64()?, v.get("keys")?.as_arr()?.iter().filter_map(|k| k.as_str().map(str::to_string)).collect::<Vec<_>>())));
        match note {
            Some(note) => {
                taken = true;
                if !red.is_some_and(|(after, keys)| !keys.is_empty() && passed(after, &keys)) {
                    mine.push(note);
                }
            }
            None => {
                rest.push_str(l);
                rest.push('\n');
            }
        }
    }
    if taken {
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
        // a red's note: unsaid once each of its units passed in a run after the red's
        let keys = |k: &[&str]| k.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        leave_red(&home, "AGENT-A", "bucket tests: #1 turned test a red", 100, &keys(&["test a"])).unwrap();
        leave_red(&home, "AGENT-A", "bucket tests: #2 turned test a red; test b red", 100, &keys(&["test a", "test b"])).unwrap();
        leave_red(&home, "AGENT-A", "bucket tests: #3 turned test c red", 300, &keys(&["test c"])).unwrap();
        let passed = |after: u64, keys: &[String]| after < 200 && keys.iter().all(|k| k == "test a");
        let open: Vec<String> = take_open(&home, "AGENT-A", &passed).into_iter().map(|(_, l)| l).collect();
        assert_eq!(open, ["bucket tests: #2 turned test a red; test b red", "bucket tests: #3 turned test c red"], "a red passed since is unsaid; one with a unit still red is said");
        assert!(take(&home, "AGENT-A").is_empty(), "the unsaid note left the file too");
        let _ = std::fs::remove_dir_all(&home);
    }
}
