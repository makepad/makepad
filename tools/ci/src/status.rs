//! The CI's state for other machines: `GET /ci/v1/status`, read-only JSON.
//!
//! The watcher already keeps everything in `local/ci/state.json` (saved on
//! every script change) and the last passing tip per branch in
//! `local/ci/passes.json`; this serves both, without logs or grab paths.
//! Where it listens is `status_listen` in `local/ci/ci.toml`: loopback
//! `127.0.0.1:7717` by default (reach it over ssh), `0.0.0.0:7717` for the
//! LAN, `"off"` for nothing.
use crate::process::Result;
use makepad_strict_json::{self as json, Value};
use makepad_toml_parser::parse_toml;
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

pub const VERSION: i64 = 1;
pub const DEFAULT_LISTEN: &str = "127.0.0.1:7717";
pub const ROUTE: &str = "/ci/v1/status";

fn passes_path(base: &Path) -> PathBuf {
    base.join("local/ci/passes.json")
}

/// The tip a branch last passed on (green or orange), kept across runs.
pub fn record_pass(base: &Path, branch: &str, tip: &str) -> Result<()> {
    let mut passes = read_passes(base);
    passes.retain(|(name, _)| name != branch);
    passes.push((branch.to_string(), tip.to_string()));
    let value = Value::Obj(passes.into_iter().map(|(k, v)| (k, json::s(v))).collect());
    let path = passes_path(base);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, value.to_json())
        .and_then(|_| fs::rename(&tmp, &path))
        .map_err(|e| e.to_string())
}

fn read_passes(base: &Path) -> Vec<(String, String)> {
    let Ok(bytes) = fs::read(passes_path(base)) else { return Vec::new() };
    let Ok(Value::Obj(fields)) = json::parse_depth(&bytes, 8) else { return Vec::new() };
    fields
        .into_iter()
        .filter_map(|(k, v)| v.as_str().map(|tip| (k, tip.to_string())))
        .collect()
}

/// `status_listen` from ci.toml; None when it is "off" or empty.
pub fn listen_address(base: &Path) -> Option<String> {
    let text = fs::read_to_string(base.join("local/ci/ci.toml")).unwrap_or_default();
    let configured = parse_toml(&text)
        .ok()
        .and_then(|doc| doc.root.get("status_listen").and_then(|v| v.as_str()).map(str::to_string));
    match configured.as_deref() {
        None => Some(DEFAULT_LISTEN.into()),
        Some("") | Some("off") => None,
        Some(address) => Some(address.into()),
    }
}

fn keep(value: &Value, keys: &[&str]) -> Value {
    json::obj(
        keys.iter()
            .map(|k| (*k, value.get(k).cloned().unwrap_or(Value::Null)))
            .collect(),
    )
}

/// The status document from the state files: every branch with its tip,
/// verdict, last passing tip and scripts with their steps.
pub fn status_value(base: &Path, now: u64) -> Value {
    let passes = read_passes(base);
    let (branches, note) = match fs::read(base.join("local/ci/state.json")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), "no run yet".to_string()),
        Err(e) => (Vec::new(), format!("state.json: {e}")),
        Ok(bytes) => match json::parse_depth(&bytes, 32) {
            Err(e) => (Vec::new(), format!("state.json: {e}")),
            Ok(state) => (
                state.get("branches").and_then(Value::as_arr).unwrap_or(&[]).to_vec(),
                String::new(),
            ),
        },
    };
    let branches = branches
        .iter()
        .map(|b| {
            let name = b.get("name").and_then(Value::as_str).unwrap_or("");
            let scripts = b
                .get("scripts")
                .and_then(Value::as_arr)
                .unwrap_or(&[])
                .iter()
                .map(|s| {
                    let steps = s
                        .get("steps")
                        .and_then(Value::as_arr)
                        .unwrap_or(&[])
                        .iter()
                        .map(|step| keep(step, &["name", "state", "seconds", "command", "detail"]))
                        .collect();
                    let mut script = keep(s, &["name", "verdict", "previous", "seconds", "failed_at", "detail"]);
                    if let Value::Obj(fields) = &mut script {
                        fields.push(("steps".into(), Value::Arr(steps)));
                    }
                    script
                })
                .collect();
            // A green or orange tip passed itself; a CI older than passes.json
            // records nothing else, so an earlier pass stays unknown (null).
            let passed_now = matches!(b.get("verdict").and_then(Value::as_str), Some("green" | "orange"));
            let last_pass = passes
                .iter()
                .find(|(branch, _)| branch == name)
                .map(|(_, tip)| tip.as_str())
                .or_else(|| b.get("tip").and_then(Value::as_str).filter(|t| passed_now && !t.is_empty()))
                .map(json::s)
                .unwrap_or(Value::Null);
            let mut branch = keep(b, &["name", "tip", "verdict", "finished", "detail"]);
            if let Value::Obj(fields) = &mut branch {
                fields.push(("last_pass".into(), last_pass));
                fields.push(("scripts".into(), Value::Arr(scripts)));
            }
            branch
        })
        .collect();
    json::obj(vec![
        ("version", Value::Int(VERSION)),
        ("generated", Value::Int(now as i64)),
        ("note", json::s(note)),
        ("branches", Value::Arr(branches)),
    ])
}

fn answer(stream: &mut TcpStream, base: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&request);
    let mut words = line.lines().next().unwrap_or("").split_whitespace();
    let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    let (status, body) = if method == "GET" && path.split('?').next() == Some(ROUTE) {
        ("200 OK", status_value(base, crate::report::now()).to_json())
    } else {
        ("404 Not Found", json::obj(vec![("error", json::s(format!("only GET {ROUTE}")))]).to_json())
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Serve the status on its own thread for the life of the process. A port
/// that cannot be bound is reported, never fatal: the CI runs on without it.
pub fn start(base: &Path) -> Result<Option<String>> {
    let Some(address) = listen_address(base) else { return Ok(None) };
    let listener = TcpListener::bind(&address).map_err(|e| format!("status endpoint {address}: {e}"))?;
    let base = base.to_path_buf();
    std::thread::Builder::new()
        .name("ci-status".into())
        .spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    answer(&mut stream, &base);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Some(format!("http://{address}{ROUTE}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_carries_scripts_steps_and_last_pass_without_logs() {
        let base = std::env::temp_dir().join(format!("ci-status-{}-{}", std::process::id(), crate::report::stamp()));
        fs::create_dir_all(base.join("local/ci")).unwrap();
        assert_eq!(status_value(&base, 5).get("note").and_then(Value::as_str), Some("no run yet"));
        fs::write(base.join("local/ci/state.json"), r#"{"branches":[{"name":"work","tip":"bbb","verdict":"red","finished":7,"detail":"","scripts":[{"name":"apps/calculator","verdict":"red","previous":"green","failed_at":3,"seconds":1.5,"detail":"boom","log":"huge","grabs":["/x.png"],"steps":[{"name":"apps/calculator / check other platforms","state":"failed","seconds":1.0,"command":"cargo check","detail":"error[E0425]: x","progress":""}]}]}]}"#).unwrap();
        fs::write(base.join("local/ci/state.json"), r#"{"branches":[{"name":"dev","tip":"ddd","verdict":"orange","finished":1,"detail":"","scripts":[]}]}"#).unwrap();
        let v = status_value(&base, 1);
        assert_eq!(v.get("branches").and_then(Value::as_arr).unwrap()[0].get("last_pass").and_then(Value::as_str), Some("ddd"));
        fs::write(base.join("local/ci/state.json"), r#"{"branches":[{"name":"work","tip":"bbb","verdict":"red","finished":7,"detail":"","scripts":[{"name":"apps/calculator","verdict":"red","previous":"green","failed_at":3,"seconds":1.5,"detail":"boom","log":"huge","grabs":["/x.png"],"steps":[{"name":"apps/calculator / check other platforms","state":"failed","seconds":1.0,"command":"cargo check","detail":"error[E0425]: x","progress":""}]}]}]}"#).unwrap();
        assert!(status_value(&base, 1).get("branches").and_then(Value::as_arr).unwrap()[0].get("last_pass") == Some(&Value::Null));
        record_pass(&base, "work", "aaa").unwrap();
        record_pass(&base, "dev", "ddd").unwrap();
        record_pass(&base, "work", "abc").unwrap();
        let v = status_value(&base, 9);
        let b = &v.get("branches").and_then(Value::as_arr).unwrap()[0];
        assert_eq!(b.get("last_pass").and_then(Value::as_str), Some("abc"));
        let s = &b.get("scripts").and_then(Value::as_arr).unwrap()[0];
        assert!(s.get("log").is_none() && s.get("grabs").is_none());
        let step = &s.get("steps").and_then(Value::as_arr).unwrap()[0];
        assert_eq!(step.get("detail").and_then(Value::as_str), Some("error[E0425]: x"));
        assert!(step.get("progress").is_none());
        fs::write(base.join("local/ci/ci.toml"), "status_listen = \"off\"\n").unwrap();
        assert_eq!(listen_address(&base), None);
        fs::write(base.join("local/ci/ci.toml"), "branches = [\"work\"]\n").unwrap();
        assert_eq!(listen_address(&base).as_deref(), Some(DEFAULT_LISTEN));
        fs::remove_dir_all(base).unwrap();
    }
}
