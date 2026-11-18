//! Claude Code's account limits, from the `/usage` screen of one interactive terminal the
//! worker keeps for the app's lifetime (ported from Scope). The terminal runs with no tools,
//! no MCP servers and no hooks, in the hub's own empty folder; only slash commands are typed,
//! once its prompt shows, so no model prompt is ever sent. The account comes from the CLI's
//! identity projection (`claude auth status --json`: its email and plan tier, nothing else
//! kept); no credential file is read.

#[cfg(any(target_os = "macos", target_os = "linux", test))]
use crate::usage::{account_email, local_reset_at, plan_name, push_window, strip_ansi, truncate, Limit, UsageWindow, MAX_OUTPUT_BYTES};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::usage::{local_zone_name, now};
use crate::usage::{Reader, Reading, Stuck};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::sync::atomic::Ordering;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", target_os = "linux", test))]
const MAX_IDENTITY_BYTES: usize = 16 * 1024;

pub struct ClaudeReader {
    /// The folder the terminal runs in (the hub's own, empty).
    cwd: PathBuf,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    session: Option<ClaudeSession>,
    /// The account the terminal was started for.
    account: Option<String>,
    /// The terminal needs the person: no read starts another until Reconnect (or another login).
    blocked: Option<Stuck>,
}

impl ClaudeReader {
    pub fn new(cwd: PathBuf) -> ClaudeReader {
        ClaudeReader {
            cwd,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            session: None,
            account: None,
            blocked: None,
        }
    }
}

impl Reader for ClaudeReader {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn read(&mut self, stop: &AtomicBool) -> Reading {
        let identity = match identity(stop) {
            Ok(identity) => identity,
            Err(e) => return Reading::failed(None, e),
        };
        if identity.email != self.account {
            // Another account (or signed out): the terminal holds the old login. It ends once;
            // the read below starts one for the account now.
            self.session = None;
            self.blocked = None;
            self.account = identity.email.clone();
        }
        let Some(email) = identity.email.clone() else {
            return Reading::failed(None, "Not signed in to Claude Code on this machine");
        };
        let failed = |e: String| Reading { plan: identity.plan.clone(), ..Reading::failed(Some(email.clone()), e) };
        if let Some(stuck) = self.blocked {
            return Reading { stuck: Some(stuck), ..failed(stuck_text(stuck).into()) };
        }
        if self.session.is_none() {
            let cwd = match crate::usage::private_dir(&self.cwd) {
                Ok(cwd) => cwd,
                Err(e) => return failed(e),
            };
            match ClaudeSession::start(&cwd) {
                Ok(session) => self.session = Some(session),
                Err(e) => return failed(e),
            }
        }
        let observed_at = now();
        let mut reading = match self.session.as_mut().map(|s| s.query(stop)) {
            Some(Ok(screen)) => parse_claude_usage(&screen, observed_at, local_zone_name().as_deref()),
            Some(Err(QueryError { message, stuck })) => {
                if stuck == Some(Stuck::Ended) {
                    self.session = None;
                }
                self.blocked = stuck;
                Reading { stuck, ..failed(message) }
            }
            None => failed("no terminal".into()),
        };
        // The read is the account's only if the CLI is still signed in as it afterwards.
        let after = identity_or_none(stop);
        if after.as_ref().and_then(|i| i.email.as_ref()) != Some(&email) {
            reading.windows.clear();
            reading.error = Some("The account changed during the read".into());
            reading.account = after.and_then(|i| i.email);
            return reading;
        }
        reading.account = Some(email);
        reading.plan = identity.plan;
        reading
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn read(&mut self, _stop: &AtomicBool) -> Reading {
        let _ = (&self.cwd, &self.account, &self.blocked);
        Reading::failed(None, "Reading Claude's usage needs macOS or Linux")
    }

    fn reconnect(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.session = None;
        }
        self.blocked = None;
    }

    fn pid(&self) -> Option<u32> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.session.as_ref().map(|s| s.pty.child_pid())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        None
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn stuck_text(stuck: Stuck) -> &'static str {
    match stuck {
        Stuck::Login => "Stuck: waiting for a login",
        Stuck::Ended => "The CLI ended",
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
#[derive(Clone, Debug, Default, PartialEq)]
struct Identity {
    email: Option<String>,
    plan: Option<String>,
}

/// Only `loggedIn`, `email` and `planTier` of the CLI's status are kept.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn parse_identity(raw: &[u8]) -> Option<Identity> {
    if raw.len() > MAX_IDENTITY_BYTES {
        return None;
    }
    let value = makepad_strict_json::parse_depth(raw, 8).ok()?;
    let plan = plan_name(value.get("planTier").or_else(|| value.get("subscriptionType")).and_then(|v| v.as_str()));
    if value.get("loggedIn").and_then(|v| v.as_bool()) != Some(true) {
        return Some(Identity::default());
    }
    Some(Identity { email: account_email(value.get("email").and_then(|v| v.as_str())), plan })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn identity_or_none(stop: &AtomicBool) -> Option<Identity> {
    identity(stop).ok()
}

/// `claude auth status --json` in its own process group, bounded to 5 s and 16 KiB.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn identity(stop: &AtomicBool) -> Result<Identity, String> {
    use std::io::{self, Read};
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    use std::process::{Child, Command, Stdio};
    extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
    }
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                // this fresh child leads its own process group, never anyone else's
                unsafe {
                    kill(-(self.0.id() as i32), 9);
                }
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let exe = makepad_ai_hub::providers::claude::find_cli().ok_or("Claude Code CLI not found (set CLAUDE_CODE_PATH or put claude on PATH)")?;
    let mut child = OwnedChild(
        Command::new(&exe)
            .args(["auth", "status", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("Cannot start {}: {e}", exe.display()))?,
    );
    let mut output = child.0.stdout.take().ok_or("no output")?;
    #[cfg(target_os = "macos")]
    const O_NONBLOCK: i32 = 0x0004;
    #[cfg(target_os = "linux")]
    const O_NONBLOCK: i32 = 0x0800;
    let fd = output.as_raw_fd();
    // SAFETY: the pipe stays owned here; F_GETFL/F_SETFL take integers.
    unsafe {
        let flags = fcntl(fd, 3);
        if flags < 0 || fcntl(fd, 4, flags | O_NONBLOCK) < 0 {
            return Err("Cannot read the Claude CLI's status".into());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut raw = Vec::new();
    let mut eof = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("Usage reading stopped".into());
        }
        if Instant::now() >= deadline {
            return Err("`claude auth status` did not answer in 5 s".into());
        }
        let mut buf = [0u8; 4096];
        match output.read(&mut buf) {
            Ok(0) => eof = true,
            Ok(n) => {
                if raw.len() + n > MAX_IDENTITY_BYTES {
                    return Err("`claude auth status` said too much".into());
                }
                raw.extend_from_slice(&buf[..n]);
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
        if eof {
            if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
                // signed out: the CLI says `loggedIn: false` (and may exit non-zero)
                return parse_identity(&raw).ok_or_else(|| format!("`claude auth status` gave no status (exit {status})"));
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct QueryError {
    message: String,
    stuck: Option<Stuck>,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl From<String> for QueryError {
    fn from(message: String) -> QueryError {
        QueryError { message, stuck: None }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct ClaudeSession {
    pty: makepad_agents::unix::Pty,
    terminal: makepad_agents::term::Terminal,
    stream: makepad_agents::term::stream::Stream,
    ready: bool,
    /// A `/usage` panel may still be open (a read that did not finish).
    panel_open: bool,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl ClaudeSession {
    fn start(cwd: &std::path::Path) -> Result<ClaudeSession, String> {
        use makepad_agents::{
            term::{stream::Stream, Terminal},
            unix::Pty,
        };
        use std::ffi::{OsStr, OsString};
        let exe = makepad_ai_hub::providers::claude::find_cli().ok_or("Claude Code CLI not found (set CLAUDE_CODE_PATH or put claude on PATH)")?;
        // A status-only terminal: no model tools, no MCP servers, no hooks, the person's own
        // settings only; screen-reader mode keeps its screen plain text.
        let args: Vec<OsString> = ["--tools", "", "--strict-mcp-config", "--mcp-config", r#"{"mcpServers":{}}"#, "--settings", r#"{"disableAllHooks":true}"#, "--setting-sources", "user", "--ax-screen-reader"]
            .into_iter()
            .map(OsString::from)
            .collect();
        let env: [(&OsStr, &OsStr); 2] = [(OsStr::new("NO_COLOR"), OsStr::new("1")), (OsStr::new("CLICOLOR"), OsStr::new("0"))];
        let pty = Pty::spawn_with_env(exe.as_os_str(), &args, cwd, 160, 64, &env).map_err(|e| format!("Cannot start the Claude usage terminal: {e}"))?;
        Ok(ClaudeSession { pty, terminal: Terminal::new(160, 64), stream: Stream::new(), ready: false, panel_open: false })
    }

    /// Bytes waiting on the PTY (its master is non-blocking).
    fn try_read(&mut self) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut buffer = [0u8; 16384];
        match self.pty.read(&mut buffer) {
            Ok(n) if n > 0 => Some(buffer[..n].to_vec()),
            _ => None,
        }
    }

    fn try_write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        std::io::Write::write(&mut self.pty, bytes)
    }

    fn child_exited(&mut self) -> bool {
        !matches!(self.pty.try_wait(), Ok(None))
    }

    fn screen(&self) -> String {
        let screen = self.terminal.screen();
        (screen.total_rows().saturating_sub(64)..screen.total_rows()).filter_map(|i| screen.row_virtual(i)).map(|row| row.text()).collect::<Vec<_>>().join("\n")
    }

    fn send(&mut self, mut bytes: &[u8], stop: &AtomicBool, deadline: Instant) -> Result<(), String> {
        while !bytes.is_empty() {
            if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
                return Err("Claude usage input cancelled or timed out".into());
            }
            match self.try_write(bytes) {
                Ok(n) if n > 0 => bytes = &bytes[n..],
                Ok(_) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    fn query(&mut self, stop: &AtomicBool) -> Result<String, QueryError> {
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut total = 0;
        let mut last_output = Instant::now();
        let mut submitted = false;
        let mut submitted_at = Instant::now();
        // the trust question answered: `y`, then Enter while it still asks
        let (mut trusted, mut confirmed) = (false, false);
        if self.panel_open {
            // a panel a timed-out read left open
            self.send(b"\x1b", stop, deadline)?;
            self.panel_open = false;
        }
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err("Usage reading stopped".to_string().into());
            }
            if Instant::now() >= deadline {
                return Err("Claude did not answer /usage in 25 s".to_string().into());
            }
            for _ in 0..32 {
                let Some(chunk) = self.try_read() else { break };
                total += chunk.len();
                if total > MAX_OUTPUT_BYTES {
                    return Err("Claude's /usage exceeded its output budget".to_string().into());
                }
                self.stream.process(&chunk, &mut self.terminal);
                last_output = Instant::now();
            }
            let replies = self.terminal.take_outbound();
            if replies.len() > 8192 {
                return Err("Claude terminal reply budget exceeded".to_string().into());
            }
            self.send(&replies, stop, deadline)?;
            if self.child_exited() {
                return Err(QueryError { message: "The Claude usage terminal ended".into(), stuck: Some(Stuck::Ended) });
            }
            let screen = self.screen();
            let settled = last_output.elapsed() > Duration::from_millis(400);
            if settled && login_screen(&screen) {
                return Err(QueryError { message: "Claude waits for a login".into(), stuck: Some(Stuck::Login) });
            }
            if !submitted {
                // The terminal runs in the hub's own empty folder with no tools: trusting it
                // gives the CLI nothing to read or run. Only typed while the question is the
                // screen's last thing, so nothing reaches the prompt.
                if settled && trust_question(&screen) && last_line(&screen).to_ascii_lowercase().contains("enter to confirm") {
                    if !trusted {
                        self.send(b"y", stop, deadline)?;
                        trusted = true;
                    } else if !confirmed {
                        self.send(b"\r", stop, deadline)?;
                        confirmed = true;
                    }
                    last_output = Instant::now();
                    continue;
                }
                let prompt = screen.lines().any(|line| matches!(line.trim(), "$" | "❯" | ">"));
                if (self.ready || prompt) && last_output.elapsed() > Duration::from_millis(250) {
                    self.send(b"/usage\r", stop, deadline)?;
                    self.panel_open = true;
                    submitted = true;
                    submitted_at = Instant::now();
                    self.ready = false;
                    last_output = Instant::now();
                }
            } else if submitted_at.elapsed() > Duration::from_millis(1500) && settled && !screen.contains("Refreshing…") {
                // Done once a week shows (a plan without a session limit has none), on the
                // screen after this read's own command, never an earlier panel.
                let fresh = after_command(&screen);
                let parsed = parse_claude_usage(fresh, 0, None);
                if parsed.error.is_none() && parsed.window(Limit::Week).is_some() {
                    let fresh = fresh.to_string();
                    self.send(b"\x1b", stop, deadline)?;
                    self.panel_open = false;
                    self.ready = true;
                    return Ok(fresh);
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for ClaudeSession {
    fn drop(&mut self) {
        // Escape closes /usage, /exit ends this terminal; the PTY's own cleanup ends its
        // process group after that.
        let _ = self.try_write(b"\x1b");
        std::thread::sleep(Duration::from_millis(60));
        let _ = self.try_write(b"/exit\r");
        let deadline = Instant::now() + Duration::from_millis(750);
        while !self.child_exited() && Instant::now() < deadline {
            for _ in 0..8 {
                if self.try_read().is_none() {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// The screen's last non-empty line.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn last_line(screen: &str) -> &str {
    screen.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

/// The screen after the last line that typed `/usage` (`you: /usage`, `> /usage`): what this
/// read's command showed.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn after_command(screen: &str) -> &str {
    let mut at = 0;
    let mut offset = 0;
    for line in screen.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end().ends_with("/usage") {
            at = offset;
        }
    }
    &screen[at..]
}

/// The screen's last `n` lines: what the CLI shows now, not its scrollback.
/// The last `n` lines of what the screen shows: the blank rows under a short screen (a question
/// at the top of the 64-row terminal) are not its last lines.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn tail(screen: &str, n: usize) -> String {
    let lines: Vec<&str> = screen.trim_end().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The CLI asks whether to trust its folder (in the screen's last lines).
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn trust_question(screen: &str) -> bool {
    let lower = tail(screen, 16).to_ascii_lowercase();
    lower.contains("trust this folder") || lower.contains("do you trust the files")
}

/// A login screen: the CLI waits for the person (in the screen's last lines).
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn login_screen(screen: &str) -> bool {
    let lower = tail(screen, 20).to_ascii_lowercase();
    ["select login method", "please run /login", "paste code here", "not logged in", "invalid api key", "oauth error", "login expired", "sign in to continue"].iter().any(|s| lower.contains(s))
}

/// The limits on Claude's `/usage` screen. Only labelled percentages count; cost and token
/// analytics never become limits.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub fn parse_claude_usage(raw: &str, observed_at: u64, local_zone: Option<&str>) -> Reading {
    let text = strip_ansi(raw);
    let mut reading = Reading::default();
    if text.len() > MAX_OUTPUT_BYTES {
        reading.error = Some("Usage output exceeds 256 KiB".into());
        return reading;
    }
    let mut current: Option<UsageWindow> = None;
    for line in text.lines() {
        let line = line.trim().trim_matches(|c: char| matches!(c, '│' | '┃' | '║')).trim();
        let lower = line.to_ascii_lowercase();
        let heading = if lower.contains("current session") || lower.contains("5-hour") || lower.contains("5 hour") || lower.starts_with("5h ") {
            Some(("5-hour", Some(Limit::Session)))
        } else if lower.contains("current week") || lower.contains("7-day") || lower.contains("7 day") || lower.contains("weekly") {
            Some(if lower.contains("sonnet") {
                ("7-day Sonnet", None)
            } else if lower.contains("opus") {
                ("7-day Opus", None)
            } else if lower.contains("fable") {
                ("7-day Fable", Some(Limit::Fable))
            } else {
                ("7-day", Some(Limit::Week))
            })
        } else {
            None
        };
        if let Some((name, limit)) = heading {
            if let Some(row) = current.take() {
                push_text_window(&mut reading, row);
            }
            current = Some(UsageWindow { name: name.into(), limit, ..Default::default() });
        }
        let Some(row) = &mut current else { continue };
        if let Some(mark) = lower.rfind('%') {
            let before = &lower[..mark];
            let start = before.char_indices().rev().find(|(_, c)| !(c.is_ascii_digit() || *c == '.')).map(|(i, c)| i + c.len_utf8()).unwrap_or(0);
            let prefix = before[..start].chars().next_back();
            let malformed = prefix.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.' | '_'));
            if let Ok(p) = before[start..].parse::<f64>() {
                if !malformed && p.is_finite() && (0.0..=100.0).contains(&p) {
                    let after = lower[mark + 1..].trim_start();
                    if after.starts_with("left") || after.starts_with("remaining") {
                        row.used_percent = Some(100.0 - p);
                    } else if after.starts_with("used") {
                        row.used_percent = Some(p);
                    }
                }
            }
        }
        if let Some(start) = lower.find("reset") {
            let reset = line[start..].trim();
            row.reset_text = Some(truncate(reset, 256));
            row.reset_at = if observed_at > 0 { local_reset_at(reset, observed_at, local_zone) } else { None };
        }
    }
    if let Some(row) = current {
        push_text_window(&mut reading, row);
    }
    if reading.windows.is_empty() {
        reading.error = Some("Claude's /usage showed no limits we recognise; none are inferred".into());
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("stale") && lower.contains("cache") {
        reading.error = Some("Claude reported stale cached usage".into());
        reading.windows.clear();
    }
    reading
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn push_text_window(reading: &mut Reading, row: UsageWindow) {
    if row.used_percent.is_some() {
        push_window(&mut reading.windows, row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI 2.1.325 on a plan without a session limit: the week of all models and Fable's, the
    /// cost block's "Session" heading is no limit.
    #[test]
    fn the_usage_screen_gives_the_week_and_fables_week_and_ignores_cost_analytics() {
        let screen = "[Screen Reader Mode: on via flag]\nClaude Code v2.1.325\n$Settings  Status   Config   Usage   Stats\nSession\nTotal cost:            $0.0000\nUsage:                 0 input, 0 output, 0 cache read, 0 cache write\nCurrent week (all models)\n52% 52% used\nResets Nov 23 at 5pm (UTC)\nWhat's contributing to your usage?\n76% of your usage was at >150k context\nCurrent week (Fable)\n21% 21% used\nResets Nov 23 at 5pm (UTC)\nUsage credits are off · /usage-credits to turn them on\nEsc to cancel";
        let observed = crate::usage::parse_reset_at("2026-11-17T10:00:00Z").unwrap();
        let r = parse_claude_usage(screen, observed, Some("Etc/UTC"));
        assert_eq!(r.error, None);
        assert_eq!(r.window(Limit::Session), None, "the cost block is no session limit");
        let (week, fable) = (r.window(Limit::Week).unwrap(), r.window(Limit::Fable).unwrap());
        assert_eq!((week.used_percent, fable.used_percent), (Some(52.0), Some(21.0)));
        assert_eq!(week.reset_at, crate::usage::parse_reset_at("2026-11-23T17:00:00Z"));
        assert_eq!(week.reset_text.as_deref(), Some("Resets Nov 23 at 5pm (UTC)"));
    }

    #[test]
    fn a_session_limit_and_left_percentages_read_as_used() {
        let r = parse_claude_usage("\x1b[32mCurrent session\x1b[0m\n████45% used\nResets 7pm (Europe/Amsterdam)\nCurrent week (all models)\n20% left\nResets Sep 12 at 6am\n", 99, Some("America/New_York"));
        assert_eq!(r.window(Limit::Session).unwrap().used_percent, Some(45.0));
        assert_eq!(r.window(Limit::Session).unwrap().reset_at, None, "another zone's clock is never guessed");
        assert_eq!(r.window(Limit::Week).unwrap().used_percent, Some(80.0));
    }

    #[test]
    fn cost_reports_and_malformed_numbers_are_no_limits() {
        for raw in [r#"{"total_cost":50,"tokens":10000}"#, "zsh: command not found: claude", "Current session\n-5% used", "Current session\n1e2% used", "Current week (all models)\n150% used"] {
            let r = parse_claude_usage(raw, 99, None);
            assert!(r.error.is_some() && r.windows.is_empty(), "{raw}");
        }
    }

    /// Only the signed-in email and plan tier are kept; a signed-out CLI has no account.
    #[test]
    fn identity_keeps_only_a_valid_signed_in_email_and_plan() {
        let id = parse_identity(br#"{"loggedIn":true,"email":"user@example.com","orgId":"not kept","planTier":"max_20x","subscriptionType":"max"}"#).unwrap();
        assert_eq!(id, Identity { email: Some("user@example.com".into()), plan: Some("max_20x".into()) });
        for raw in [r#"{"loggedIn":false,"email":"user@example.com"}"#, r#"{"email":"user@example.com"}"#, r#"{"loggedIn":true,"email":"user@example.com\nx"}"#] {
            assert_eq!(parse_identity(raw.as_bytes()).unwrap().email, None, "{raw}");
        }
        assert_eq!(parse_identity(b"not json"), None);
    }

    /// A second read's screen still holds the first panel above its own command.
    #[test]
    fn a_read_parses_only_what_its_own_command_showed() {
        let screen = "you: /usage\nCurrent week (all models)\n30% used\nUsage credits are off · /usage-credits to turn them on\n$\nyou: /usage\nRefreshing…";
        assert_eq!(after_command(screen), "Refreshing…");
        assert!(parse_claude_usage(after_command(screen), 0, None).window(Limit::Week).is_none(), "the old panel's 30% is not this read");
        assert_eq!(last_line("Enter y/n:\nEnter to confirm · Esc to cancel\n\n"), "Enter to confirm · Esc to cancel");
    }

    #[test]
    fn login_and_trust_screens_are_recognised() {
        assert!(login_screen("Select login method:\n❯ 1. Claude account with subscription"));
        assert!(trust_question("Quick safety check: Is this a project you created or one you trust?\ny. Yes, I trust this folder\nn. No, exit"));
        assert!(!login_screen("Current week (all models)\n52% used") && !trust_question("$"));
        // as the terminal renders them: the question at the top, blank rows to the bottom
        let rows = |text: &str| format!("{text}{}", "\n".repeat(60));
        let trust = rows("[Screen Reader Mode: on via flag]\nPermission Required: Accessing workspace:\n/tmp/aihub/usage-terminal\nQuick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take\n a moment to review what's in this folder first.\nClaude Code'll be able to read, edit, and execute files here.\nSecurity guide\ny. Yes, I trust this folder\nn. No, exit\nEnter y/n:\nEnter to confirm \u{b7} Esc to cancel");
        assert!(trust_question(&trust) && last_line(&trust).to_ascii_lowercase().contains("enter to confirm"), "a 64-row trust screen is the question");
        assert!(login_screen(&rows("Select login method:\n\u{276f} 1. Claude account with subscription")), "a 64-row login screen is a login");
    }
}
