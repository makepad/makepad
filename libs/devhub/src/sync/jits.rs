//! This machine's JITs as sync sees them: which run from the synced checkout, what their live
//! patcher did with a sync's edits (the JIT's `makepad-run-live-<pid>.jsonl`, its `edit`
//! records), whether they still run a `makepad` binary a build replaced, and the session tick a
//! JIT reports (for coordinated patches).
//!
//! The registry is read here without listing through `discovery::apps` (which deletes the files
//! of ended processes: the backbone's crash detection needs them).

use makepad_remote::discovery;
use makepad_remote::link::Client;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Jit {
    pub pid: u32,
    pub app: String,
}

fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists() || makepad_remote::relay::proc_start(pid).is_some()
}

/// The JITs announced in the registry whose process lives.
pub fn list() -> Vec<Jit> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(discovery::registry_dir()) else { return out };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("makepad-remote-").and_then(|r| r.strip_suffix(".json")).and_then(|p| p.parse::<u32>().ok()) else { continue };
        if !alive(pid) {
            continue;
        }
        let Some(app) = std::fs::read_to_string(e.path()).ok().and_then(|t| discovery::parse(&t)) else { continue };
        if app.text("run").is_some() || app.serves("cmd") {
            out.push(Jit { pid, app: app.app });
        }
    }
    out.sort_by_key(|j| j.pid);
    out
}

/// Whether `pid` announced itself (its discovery file exists).
pub fn announced(pid: u32) -> Option<String> {
    let f = discovery::registry_dir().join(format!("makepad-remote-{pid}.json"));
    std::fs::read_to_string(f).ok().and_then(|t| discovery::parse(&t)).map(|a| a.app)
}

fn environ(pid: u32, key: &str) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    raw.split(|b| *b == 0).find_map(|kv| std::str::from_utf8(kv).ok()?.strip_prefix(key)?.strip_prefix('=').map(str::to_string))
}

/// The process's working directory (Linux).
pub fn cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// The JIT runs from inside `checkout` (the launchers start `cargo run` there).
pub fn runs_from(pid: u32, checkout: &Path) -> bool {
    cwd(pid).is_some_and(|c| c.starts_with(checkout))
}

/// Its executable was replaced on disk (a build rewrote the `makepad` binary it runs).
pub fn exe_replaced(pid: u32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|p| p.to_string_lossy().ends_with(" (deleted)"))
}

/// Where the JIT writes its live-edit records.
pub fn live_log(pid: u32) -> PathBuf {
    if let Some(p) = environ(pid, "MAKEPAD_RUN_LIVE_LOG").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let tmp = environ(pid, "TMPDIR").filter(|t| !t.is_empty()).unwrap_or_else(|| "/tmp".into());
    Path::new(&tmp).join(format!("makepad-run-live-{pid}.jsonl"))
}

pub fn log_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// What one live edit did: the `edit` record's rung, files, fns and total ms (or a restart
/// requirement).
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub rung: String,
    pub files: u64,
    pub fns: u64,
    pub ms: f64,
    pub reasons: String,
    pub raw: String,
}

/// The edit records written to `path` after byte `from`.
pub fn edits_since(path: &Path, from: u64) -> (Vec<Edit>, u64) {
    let Ok(bytes) = std::fs::read(path) else { return (Vec::new(), from) };
    let start = (from as usize).min(bytes.len());
    let text = String::from_utf8_lossy(&bytes[start..]);
    // only whole lines: a record being written is read next time
    let whole = text.rfind('\n').map_or(0, |i| i + 1);
    let mut out = Vec::new();
    for line in text[..whole].lines() {
        let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
        let num = |k: &str| match v.get(k) {
            Some(makepad_strict_json::Value::Int(i)) => *i as f64,
            Some(makepad_strict_json::Value::F64(f)) => *f,
            Some(makepad_strict_json::Value::Arr(a)) => a.len() as f64,
            _ => 0.0,
        };
        if v.get("restart_required").and_then(|x| x.as_bool()) == Some(true) {
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            out.push(Edit { rung: "restart".into(), files: 0, fns: 0, ms: 0.0, reasons: crate::proto::clean(&msg, 200), raw: crate::proto::clean(line, 2000) });
            continue;
        }
        if v.get("event").and_then(|e| e.as_str()) != Some("edit") {
            continue;
        }
        let ms = v.get("ms").map_or(0.0, |m| match m.get("total") {
            Some(makepad_strict_json::Value::Int(i)) => *i as f64,
            Some(makepad_strict_json::Value::F64(f)) => *f,
            _ => 0.0,
        });
        let reasons = v.get("reasons").and_then(|r| r.as_arr()).map(|r| r.iter().filter_map(|x| x.as_str()).take(2).collect::<Vec<_>>().join("; ")).unwrap_or_default();
        out.push(Edit {
            rung: v.get("rung").and_then(|r| r.as_str()).unwrap_or("?").to_string(),
            files: num("files") as u64,
            fns: num("fns") as u64,
            ms,
            reasons: crate::proto::clean(&reasons, 200),
            raw: crate::proto::clean(line, 2000),
        });
    }
    (out, start as u64 + whole as u64)
}

/// Reads the session tick a JIT reports: `cmd` is an inspect command line whose answer carries
/// it, either an app command (`"tick":N`) or an injected probe's read (`probe read P<n> n=1`: the
/// latest `"emit":"N"`). One connection for every read (coordinated patches poll it).
pub struct Ticker {
    pid: u32,
    cmd: String,
    client: Client,
}

impl Ticker {
    pub fn new(pid: u32, cmd: &str) -> Result<Ticker, String> {
        let client = Client::connect(&pid.to_string()).map_err(|e| format!("JIT {pid}: {e}"))?;
        client.set_timeout(Some(Duration::from_secs(5)));
        Ok(Ticker { pid, cmd: cmd.to_string(), client })
    }

    pub fn read(&self) -> Result<u64, String> {
        let pid = self.pid;
        let lines = self.client.cmd(&self.cmd).map_err(|e| format!("JIT {pid}: {e}"))?;
        let mut found = None;
        for l in lines {
            let l = l.map_err(|e| format!("JIT {pid}: {e}"))?;
            if l.starts_with("{\"err\":") {
                return Err(format!("JIT {pid}: {}", crate::proto::clean(&l, 200)));
            }
            // a probe's read lists its emits newest last
            if let Some(t) = tick_of(&l) {
                found = Some(t);
            }
        }
        found.ok_or_else(|| format!("JIT {pid}: `{}` answered no tick", self.cmd))
    }
}

pub fn tick_of(line: &str) -> Option<u64> {
    let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
    if let Some(t) = v.get("tick").and_then(|t| t.as_u64()) {
        return Some(t);
    }
    // a probe emits its value's Debug text: a number, or a string of one
    v.get("emit").and_then(|e| e.as_str()).and_then(|s| s.trim().trim_matches('"').parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_from_app_commands_and_probes() {
        assert_eq!(tick_of(r#"{"tick":1234,"session":"ab"}"#), Some(1234));
        assert_eq!(tick_of(r#"{"emit":"5678","ago_ms":3,"line":12}"#), Some(5678));
        assert_eq!(tick_of(r#"{"emit":"\"4185\"","ago_ms":7,"line":1}"#), Some(4185));
        assert_eq!(tick_of(r#"{"emit":"hello","ago_ms":3}"#), None);
    }

    #[test]
    fn edit_records_are_read_whole() {
        let p = std::env::temp_dir().join(format!("devhub-sync-live-{}.jsonl", std::process::id()));
        std::fs::write(&p, "{\"event\":\"edit\",\"rung\":\"patch\",\"files\":[\"a.rs\"],\"fns\":3,\"ms\":{\"total\":42.5},\"reasons\":[]}\n{\"restart_required\":true,\"message\":\"the Cargo graph changed\"}\n{\"event\":\"ed").unwrap();
        let (e, at) = edits_since(&p, 0);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].rung.as_str(), e[0].files, e[0].fns, e[0].ms), ("patch", 1, 3, 42.5));
        assert_eq!(e[1].rung, "restart");
        let (again, _) = edits_since(&p, at);
        assert!(again.is_empty());
        let _ = std::fs::remove_file(&p);
    }
}
