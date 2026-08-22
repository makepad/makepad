//! Shared test scaffolding: fixture databases built by the system `sqlite3`
//! CLI, and helpers that turn CLI output into our [`Value`] model so results
//! can be compared value-by-value rather than as formatted text.
//!
//! Every helper degrades gracefully when `sqlite3` is missing: tests call
//! [`have_sqlite3`] and skip.

#![allow(dead_code)]

use makepad_sqlite::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Tests in one binary run in parallel threads, and two `sqlite3` processes
/// opening the same WAL database at once can collide while one of them
/// rebuilds the shared-memory index ("database is locked"). CLI calls are
/// serialized here so the comparison harness never reports that as a diff.
fn cli_gate() -> &'static Mutex<()> {
    static GATE: OnceLock<Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(()))
}

pub fn have_sqlite3() -> bool {
    Command::new("sqlite3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A unique scratch directory that is removed when the guard drops.
pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn new(tag: &str) -> Scratch {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "makepad-sqlite-{tag}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch { dir }
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run SQL through the `sqlite3` CLI and return stdout. Panics on CLI failure
/// so a broken fixture is never mistaken for a passing test.
pub fn sqlite3(db: &Path, sql: &str) -> String {
    let _guard = cli_gate().lock().unwrap_or_else(|e| e.into_inner());
    let mut child = Command::new("sqlite3")
        .arg(db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sqlite3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(sql.as_bytes())
        .expect("write sql");
    let out = child.wait_with_output().expect("wait sqlite3");
    assert!(
        out.status.success(),
        "sqlite3 failed:\n{}\nfor SQL:\n{sql}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Query one column per row and return the parsed values (uses `quote()` so
/// blobs, NULLs and text survive the round trip).
pub fn sqlite3_column(db: &Path, sql: &str) -> Vec<Value> {
    let out = sqlite3(db, &format!(".mode list\n.headers off\n{sql}\n"));
    out.lines().map(parse_quoted).collect()
}

/// Parse one `quote()` rendering into a value.
pub fn parse_quoted(s: &str) -> Value {
    let t = s.trim();
    if t == "NULL" {
        return Value::Null;
    }
    if (t.starts_with("X'") || t.starts_with("x'")) && t.ends_with('\'') {
        let hex = &t[2..t.len() - 1];
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let hb = hex.as_bytes();
        let mut i = 0;
        while i + 1 < hb.len() {
            bytes.push(u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0));
            i += 2;
        }
        return Value::Blob(bytes);
    }
    if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 {
        return Value::Text(t[1..t.len() - 1].replace("''", "'"));
    }
    if let Ok(i) = t.parse::<i64>() {
        return Value::Integer(i);
    }
    if let Ok(f) = t.parse::<f64>() {
        return Value::Real(f);
    }
    Value::Text(t.to_string())
}

/// Render a value the way `sqlite3`'s `quote()` does, for text comparisons.
pub fn quote(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => {
            let s = makepad_sqlite::value::format_real(*f);
            if s.contains('.') || s.contains('e') || s.contains("Inf") {
                s
            } else {
                format!("{s}.0")
            }
        }
        Value::Text(t) => format!("'{}'", t.replace('\'', "''")),
        Value::Blob(b) => {
            let mut s = String::from("X'");
            for byte in b {
                s.push_str(&format!("{byte:02X}"));
            }
            s.push('\'');
            s
        }
    }
}

/// Build a database with the CLI (fully checkpointed: no WAL left behind).
pub fn build_db(dir: &Path, name: &str, sql: &str) -> PathBuf {
    let path = dir.join(name);
    sqlite3(&path, sql);
    path
}

/// Build a database whose newest rows live only in its `-wal` file: the CLI is
/// killed before it can checkpoint, exactly like a crashed writer.
pub fn build_wal_db(dir: &Path, name: &str, setup_sql: &str, wal_sql: &str) -> PathBuf {
    let path = dir.join(name);
    sqlite3(&path, &format!("PRAGMA journal_mode=WAL;\n{setup_sql}\n"));
    let mut child = Command::new("sqlite3")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sqlite3");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    stdin
        .write_all(
            format!("PRAGMA wal_autocheckpoint=0;\n{wal_sql}\nSELECT 'READY-MARKER';\n").as_bytes(),
        )
        .expect("write sql");
    stdin.flush().expect("flush");
    let mut reader = BufReader::new(stdout);
    let mut saw_ready = false;
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        if line.contains("READY-MARKER") {
            saw_ready = true;
            break;
        }
        line.clear();
    }
    // SIGKILL: no checkpoint runs, so the frames stay in the -wal file.
    let _ = child.kill();
    let _ = child.wait();
    assert!(saw_ready, "sqlite3 never reached the WAL marker");
    path
}
