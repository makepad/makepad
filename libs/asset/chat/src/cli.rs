//! Shared plumbing for the headless-CLI providers (`claude`, `codex`,
//! `grok`): find the executable, spawn ONE process per turn in its own
//! process group with an empty working directory, stream its stdout and
//! stderr lines back without ever blocking the caller, and kill the whole
//! group on cancel.
//!
//! Why CLIs at all: they are the vendors' own logged-in clients. The broker
//! host authenticates each once (`claude login`, `codex login`, `grok`) and
//! from then on NO API key exists anywhere in our stack — not in server
//! config, not on the wire, not on a game client. A host without a CLI
//! reports that provider `Unavailable` and nothing else changes; a game
//! client never learns more than "available" or the reason it is not.
//!
//! The three providers differ only in argv and in the line protocol they
//! print; `claude` and `grok` both speak the Anthropic Messages stream
//! format ([`crate::claude::parse_stream_line`]), `codex` speaks its own
//! item/turn events ([`crate::codex_cli`]).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};

enum IoLine {
    Out(String),
    Err(String),
    Exit,
}

/// Resolve a CLI executable. An explicit `env_override` path wins and, when
/// it is not a file, nothing else is tried — a wrong override must fail
/// loudly, not fall back to whatever is on `$PATH`. Otherwise `$PATH`, then
/// the vendors' usual install dirs (`extra` may use a leading `~`).
pub fn find_cli(env_override: &str, name: &str, extra: &[&str]) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env_override) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates: Vec<PathBuf> =
        extra.iter().map(|e| PathBuf::from(e.replacen('~', &home, 1))).collect();
    candidates.push(PathBuf::from(&home).join(".local/bin").join(name));
    candidates.push(PathBuf::from("/usr/local/bin").join(name));
    candidates.push(PathBuf::from("/opt/homebrew/bin").join(name));
    candidates.into_iter().find(|p| p.is_file())
}

/// An empty, PRIVATE working directory for ONE CLI turn. Every CLI here
/// has file tools of some kind; pointing them at an empty directory means
/// there is nothing to read and nothing of ours to touch. Per-turn and
/// mode 0700: turns never see each other's leftovers, and no other user on
/// the host can read a prompt file staged inside it. The dir is handed to
/// [`CliTurn::spawn`], which removes it when the turn is reaped or killed.
pub fn turn_dir(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join("makepad-asset-chat")
        .join(format!("{name}-{}-{n}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

/// The command for one CLI turn: `env_clear` plus the minimal allowlist
/// each vendor CLI was VERIFIED to need (2026-08-26, macOS: claude needs
/// HOME + USER/LOGNAME for its keychain credentials; codex and grok need
/// only HOME for `~/.codex` / `~/.grok`; all need PATH). Everything else —
/// tokens, cloud credentials, proxy settings, our own MAKEPAD_* config —
/// never reaches the child. TERM is pinned to `dumb` so no CLI tries to
/// render.
pub fn cli_command(exe: &std::path::Path, cwd: &std::path::Path) -> Command {
    let mut command = Command::new(exe);
    command.current_dir(cwd);
    command.env_clear();
    #[cfg(unix)]
    const KEEP: &[&str] = &["HOME", "PATH", "USER", "LOGNAME"];
    // Windows processes need the system set to start at all; keep the
    // narrow standard set the loaders and the CLIs' config dirs rely on.
    #[cfg(not(unix))]
    const KEEP: &[&str] = &[
        "PATH", "SYSTEMROOT", "SYSTEMDRIVE", "WINDIR", "COMSPEC", "USERPROFILE", "APPDATA",
        "LOCALAPPDATA", "TEMP", "TMP", "USERNAME", "HOMEDRIVE", "HOMEPATH",
    ];
    for key in KEEP {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    command.env("TERM", "dumb");
    command
}

/// How much CLI stderr / vendor error text is kept in the SERVER log.
const LOG_DETAIL_BYTES: usize = 500;

/// Map a CLI failure to one of three FIXED public categories and log the
/// bounded raw detail server-side. Raw CLI stderr and vendor `result`
/// strings never cross the wire: they can carry env fragments, tokens,
/// paths, or prompt text, and no denylist proves they do not.
///
/// - auth-smelling detail → `authentication required` (the operator must
///   log the CLI in on the broker host);
/// - a process that died without a protocol result → `<what> CLI exited`;
/// - anything else the vendor reported → `provider unavailable`.
pub fn categorize_cli_error(what: &str, detail: &str, exited_without_result: bool) -> String {
    let detail = detail.trim();
    if !detail.is_empty() {
        let mut bounded = String::new();
        for c in detail.chars() {
            if c.is_control() {
                continue;
            }
            if bounded.len() + c.len_utf8() > LOG_DETAIL_BYTES {
                break;
            }
            bounded.push(c);
        }
        // Server-side only: the broker host's log, never a wire payload.
        eprintln!("chat {what} CLI error: {bounded}");
    }
    let lower = detail.to_ascii_lowercase();
    let auth = ["not logged in", "logged out", "login", "log in", "auth", "credential",
        "api key", "api_key", "unauthorized", "401", "forbidden"]
        .iter()
        .any(|m| lower.contains(m));
    if auth {
        "authentication required".to_string()
    } else if exited_without_result {
        format!("{what} CLI exited")
    } else {
        "provider unavailable".to_string()
    }
}

/// One in-flight CLI process and its line streams.
pub struct CliTurn {
    child: Child,
    rx: Receiver<IoLine>,
    /// Bounded stderr; server-side log material only (see
    /// [`categorize_cli_error`]), never a wire payload.
    pub stderr_tail: String,
    /// The protocol reported its end; later lines are ignored.
    pub finished: bool,
    /// The per-turn 0700 working directory, removed when the turn is
    /// reaped or killed.
    turn_dir: Option<PathBuf>,
}

/// What [`CliTurn::drain`] hands back: protocol lines, and whether stdout
/// closed (the process is gone or going).
pub struct Drained {
    pub lines: Vec<String>,
    pub exited: bool,
}

impl CliTurn {
    /// Spawn `command` with piped stdio in a fresh process group. A `stdin`
    /// prompt is written from a helper thread and the pipe closed after it
    /// (the CLIs that read the prompt from stdin need the EOF to start).
    /// `turn_dir` is the per-turn working directory to remove when the
    /// turn ends, however it ends.
    pub fn spawn(
        mut command: Command,
        stdin: Option<String>,
        what: &str,
        turn_dir: Option<PathBuf>,
    ) -> Result<CliTurn, String> {
        command
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            // Own process group so cancel can address CLI descendants.
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                if let Some(dir) = &turn_dir {
                    let _ = std::fs::remove_dir_all(dir);
                }
                return Err(format!("failed to start {what}: {e}"));
            }
        };
        let mut child = child;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            std::thread::spawn(move || {
                let _ = pipe.write_all(text.as_bytes());
            });
        }
        let (tx, rx) = channel();
        // Only the stdout reader signals Exit: stderr can hit EOF before the
        // final stdout result line is forwarded, and an early Exit would
        // mark the turn errored and drop the real result.
        spawn_reader(child.stdout.take(), tx.clone(), IoLine::Out, true);
        spawn_reader(child.stderr.take(), tx, IoLine::Err, false);
        Ok(CliTurn { child, rx, stderr_tail: String::new(), finished: false, turn_dir })
    }

    /// Non-blocking: everything that arrived since the last call.
    pub fn drain(&mut self) -> Drained {
        let mut out = Drained { lines: Vec::new(), exited: false };
        while let Ok(line) = self.rx.try_recv() {
            match line {
                IoLine::Out(line) => {
                    if !self.finished && !line.trim().is_empty() {
                        out.lines.push(line);
                    }
                }
                IoLine::Err(line) => {
                    if self.stderr_tail.len() < 4096 {
                        self.stderr_tail.push_str(&line);
                        self.stderr_tail.push('\n');
                    }
                }
                IoLine::Exit => out.exited = true,
            }
        }
        out
    }

    /// The PUBLIC error for a process that closed stdout without a result
    /// line: a fixed category; the stderr tail goes to the server log only.
    pub fn exit_error(&self, what: &str) -> String {
        categorize_cli_error(what, &self.stderr_tail, true)
    }

    /// Reap a finished process and remove its turn directory.
    pub fn wait(mut self) {
        let _ = self.child.wait();
        self.cleanup();
    }

    /// Kill the process and everything it spawned (the group we created),
    /// then remove its turn directory.
    pub fn kill_group(mut self) {
        let pid = self.child.id();
        let _ = self.child.kill();
        // std has no group kill; /bin/kill does.
        #[cfg(unix)]
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.wait();
        self.cleanup();
    }

    fn cleanup(&mut self) {
        if let Some(dir) = self.turn_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn spawn_reader<R: std::io::Read + Send + 'static>(
    stream: Option<R>,
    tx: Sender<IoLine>,
    wrap: fn(String) -> IoLine,
    signal_exit: bool,
) {
    let Some(stream) = stream else {
        return;
    };
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(wrap(line)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        if signal_exit {
            let _ = tx.send(IoLine::Exit);
        }
    });
}

/// Quote as a TOML basic string (for `codex --config key=value`, whose
/// value is parsed as TOML rather than raw text).
pub fn toml_basic_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_basic_string_escapes_what_toml_needs() {
        assert_eq!(toml_basic_string("plain"), "\"plain\"");
        assert_eq!(toml_basic_string("a \"q\" \\ b\nc\t"), "\"a \\\"q\\\" \\\\ b\\nc\\t\"");
        assert_eq!(toml_basic_string("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn cli_commands_start_from_a_cleared_env() {
        let cwd = std::env::temp_dir();
        let command = cli_command(std::path::Path::new("/bin/echo"), &cwd);
        let mut keys: Vec<String> = Vec::new();
        for (key, value) in command.get_envs() {
            let key = key.to_string_lossy().to_string();
            assert!(value.is_some(), "{key} must be set, not merely removed");
            keys.push(key);
        }
        // env_clear means get_envs lists EXACTLY what the child will see.
        #[cfg(unix)]
        for key in &keys {
            assert!(
                matches!(key.as_str(), "HOME" | "PATH" | "USER" | "LOGNAME" | "TERM"),
                "unexpected env var {key} would leak into the CLI"
            );
        }
        assert!(keys.iter().any(|k| k == "TERM"));
        // Nothing secret-shaped survives even if the parent holds it.
        assert!(!keys.iter().any(|k| k.contains("KEY") || k.contains("TOKEN")));
    }

    #[test]
    fn cli_errors_map_to_fixed_public_categories() {
        // Auth-smelling detail, however spelled, is the auth category.
        for detail in [
            "Not logged in · Please run /login",
            "error: 401 Unauthorized",
            "missing API key",
        ] {
            assert_eq!(categorize_cli_error("grok", detail, false), "authentication required");
            assert_eq!(categorize_cli_error("grok", detail, true), "authentication required");
        }
        // A death without a result is named as such; the stderr detail
        // itself never appears in the public string.
        let public = categorize_cli_error("codex", "GH_TOKEN=abc panic at src/main.rs:1", true);
        assert_eq!(public, "codex CLI exited");
        // Any other vendor-reported error is the generic category.
        assert_eq!(
            categorize_cli_error("Claude Code", "quota exceeded for org acme", false),
            "provider unavailable"
        );
        assert_eq!(categorize_cli_error("codex", "", true), "codex CLI exited");
    }

    #[test]
    fn turn_dirs_are_fresh_and_private() {
        let a = turn_dir("test");
        let b = turn_dir("test");
        assert_ne!(a, b, "every turn gets its own directory");
        assert!(a.is_dir() && b.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "turn dir must be private (0700)");
        }
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn a_wrong_override_never_falls_back() {
        // The override names a directory, not a file: nothing on PATH may
        // be substituted for it.
        std::env::set_var("MAKEPAD_TEST_CLI_OVERRIDE", std::env::temp_dir());
        assert!(find_cli("MAKEPAD_TEST_CLI_OVERRIDE", "sh", &[]).is_none());
        std::env::remove_var("MAKEPAD_TEST_CLI_OVERRIDE");
        // Without it, a PATH binary is found.
        assert!(find_cli("MAKEPAD_TEST_CLI_OVERRIDE_UNSET", "sh", &[]).is_some());
    }
}
