//! Antigravity account limits for Gemini 4 Neon. The CLI's read-only print-mode commands
//! (`/usage` and optional `/credits` for the plan name) send no model prompt and spend no
//! quota. The reader opens no credential or keychain store. Each command runs in its own
//! short-lived detached process, so no idle language server stays resident between polls.

use crate::usage::{Reader, Reading, Stuck};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{sync::atomic::Ordering, time::Instant};

pub struct AgyReader {
    /// The hub's own empty folder, never a user workspace.
    cwd: PathBuf,
    blocked: Option<Stuck>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    start_error: Option<String>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    program: Option<PathBuf>,
}

impl AgyReader {
    pub fn new(cwd: PathBuf) -> AgyReader {
        AgyReader {
            cwd,
            blocked: None,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            start_error: None,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            program: None,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn failed_start(&mut self, error: String) -> Reading {
        self.start_error = Some(error.clone());
        Reading::failed(None, error)
    }
}

/// Explicit CLI override, then PATH and the usual installation directories.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn find_agy() -> Option<PathBuf> {
    makepad_ai_hub::providers::cli::find_cli("AGY_CLI_PATH", "agy", &["~/.local/bin/agy"])
}

impl Reader for AgyReader {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn read(&mut self, stop: &AtomicBool) -> Reading {
        use native::{run_agy, RunError};
        if stop.load(Ordering::Relaxed) {
            return Reading::failed(None, native::CANCELLED);
        }
        if let Some(stuck) = self.blocked {
            return Reading { stuck: Some(stuck), ..Reading::failed(None, native::SIGNED_OUT) };
        }
        if let Some(error) = &self.start_error {
            return Reading::failed(None, error.clone());
        }
        let Ok(cwd) = crate::usage::private_dir(&self.cwd) else {
            return self.failed_start("Cannot create the Antigravity usage folder".into());
        };
        let Some(program) = self.program.clone().or_else(find_agy) else {
            return self.failed_start("Antigravity CLI not found (set AGY_CLI_PATH or put agy on PATH)".into());
        };
        self.program = Some(program.clone());
        let deadline = Instant::now() + native::POLL_TIMEOUT;
        let usage = ["-p", "/usage", "--output-format", "json", "--print-timeout", "20s", "--log-file", "/dev/null"];
        let out = match run_agy(&program, &cwd, &usage, native::USAGE_TIMEOUT, stop) {
            Ok(out) => out,
            Err(RunError::Spawn(error)) => {
                self.program = None;
                return self.failed_start(error);
            }
            Err(RunError::Transient(error)) => return Reading::failed(None, error),
        };
        if out.status != Some(0) {
            return match classify_exit(out.status, &out.stdout, &out.stderr) {
                ExitFailure::SignedOut => {
                    self.blocked = Some(Stuck::Login);
                    Reading { stuck: self.blocked, ..Reading::failed(None, native::SIGNED_OUT) }
                }
                ExitFailure::UnsupportedApiKey => self.failed_start(native::API_KEY_UNSUPPORTED.into()),
                ExitFailure::NoQuotaSummary => Reading::failed(None, quota::NO_QUOTA),
                ExitFailure::Transient(Some(code)) => Reading::failed(None, format!("Antigravity usage query exited with code {code}")),
                ExitFailure::Transient(None) => Reading::failed(None, "Antigravity usage query ended without an exit code"),
            };
        }
        let mut reading = parse_usage(&out.stdout);
        if reading.error.is_none() && !stop.load(Ordering::Relaxed) {
            let credits = ["-p", "/credits", "--output-format", "json", "--print-timeout", "10s", "--log-file", "/dev/null"];
            let timeout = native::CREDITS_TIMEOUT.min(deadline.saturating_duration_since(Instant::now()));
            if let Ok(out) = run_agy(&program, &cwd, &credits, timeout, stop) {
                if out.status == Some(0) {
                    reading.plan = parse_credits_plan(&out.stdout);
                }
            }
        }
        reading
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn read(&mut self, _stop: &AtomicBool) -> Reading {
        let _ = (&self.cwd, &self.blocked);
        Reading::failed(None, "Reading Antigravity usage needs macOS or Linux")
    }

    fn reconnect(&mut self) {
        self.blocked = None;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.start_error = None;
        }
    }

    fn pid(&self) -> Option<u32> {
        None
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) use quota::{classify_exit, parse_credits_plan, parse_usage, ExitFailure};

#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod quota {
    use crate::usage::{account_email, parse_reset_at, plan_name, push_window, truncate, Limit, Reading, UsageWindow, MAX_OUTPUT_BYTES, MAX_UNIX};
    use makepad_strict_json::{parse_depth, Value};

    const MAX_GROUPS: usize = 32;
    const MAX_BUCKETS: usize = 64;
    const SHARED: &[&str] = &["all models", "overall", "account", "default"];
    pub(super) const NO_QUOTA: &str = "Antigravity reports no quota summary for this account";

    fn contains_any(value: &str, phrases: &[&str]) -> bool {
        let lower = value.to_ascii_lowercase();
        phrases.iter().any(|phrase| lower.contains(phrase))
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum ExitFailure {
        SignedOut,
        UnsupportedApiKey,
        NoQuotaSummary,
        Transient(Option<i32>),
    }

    pub(crate) fn classify_exit(code: Option<i32>, stdout: &str, stderr: &str) -> ExitFailure {
        let mentions = |phrases: &[&str]| contains_any(stderr, phrases) || contains_any(stdout, phrases);
        if code == Some(2) || mentions(&["gemini api key"]) {
            ExitFailure::UnsupportedApiKey
        } else if mentions(&["authentication required", "not logged in", "logged out", "run 'agy' to log in"]) {
            ExitFailure::SignedOut
        } else if mentions(&["no quota summary"]) {
            ExitFailure::NoQuotaSummary
        } else {
            ExitFailure::Transient(code)
        }
    }

    fn command_data(value: &Value) -> Result<&Value, &'static str> {
        const FAILED: &str = "Antigravity usage query failed";
        // The error first: a failed envelope's status says only that it failed.
        match value.get("error") {
            None | Some(Value::Null) => {}
            Some(Value::Str(s)) if s.is_empty() => {}
            Some(Value::Obj(fields)) if fields.is_empty() => {}
            Some(Value::Arr(items)) if items.is_empty() => {}
            Some(error) => {
                let message = error.as_str().or_else(|| error.get("message").and_then(Value::as_str));
                return Err(if message.is_some_and(|s| contains_any(s, &["no quota summary"])) { NO_QUOTA } else { FAILED });
            }
        }
        if value.get("status").and_then(Value::as_str).is_some_and(|s| s != "SUCCESS") {
            return Err(FAILED);
        }
        Ok(value.get("command").and_then(|c| c.get("data")).unwrap_or(value))
    }

    pub(crate) fn parse_credits_plan(raw: &str) -> Option<String> {
        if raw.len() > MAX_OUTPUT_BYTES {
            return None;
        }
        let value = parse_depth(raw.as_bytes(), 16).ok()?;
        plan_name(command_data(&value).ok()?.get("plan_name").and_then(Value::as_str))
    }

    fn field<'a>(object: &'a Value, key: &str) -> &'a str {
        object.get(key).and_then(Value::as_str).unwrap_or("").trim()
    }

    struct Bucket<'a> {
        index: usize,
        id: &'a str,
        mentions_neon: bool,
        shared: bool,
        lists_models: bool,
        disabled: bool,
        period: Limit,
        window: UsageWindow,
    }

    impl<'a> Bucket<'a> {
        fn parse(group: &Value, bucket: &'a Value, index: usize) -> Result<Self, &'static str> {
            if !matches!(bucket, Value::Obj(_)) {
                return Err("Antigravity returned an invalid quota bucket");
            }
            let disabled = match bucket.get("disabled") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(flag)) => *flag,
                _ => return Err("Antigravity returned an invalid disabled flag"),
            };
            let used_percent = match bucket.get("remaining_fraction") {
                None | Some(Value::Null) => None,
                Some(Value::Int(n @ (0 | 1))) => Some(((1 - n) * 100) as f64),
                Some(Value::F64(f)) if f.is_finite() && (-1e-6..=1.0 + 1e-6).contains(f) => Some((1.0 - f.clamp(0.0, 1.0)) * 100.0),
                _ => return Err("Antigravity returned an invalid remaining_fraction"),
            };
            let (reset_at, reset_text) = match bucket.get("reset_time") {
                None | Some(Value::Null) => (None, None),
                Some(Value::Str(s)) if s.is_empty() => (None, None),
                Some(Value::Str(s)) if !s.chars().any(char::is_control) => match parse_reset_at(s) {
                    Some(0) => (None, None),
                    Some(t) if t <= MAX_UNIX => (Some(t), None),
                    Some(_) => return Err("Antigravity returned an invalid reset_time"),
                    None => (None, Some(truncate(s, 160))),
                },
                _ => return Err("Antigravity returned an invalid reset_time"),
            };
            let group_name = field(group, "name");
            let group_description = field(group, "description");
            let id = field(bucket, "id");
            let name = field(bucket, "name");
            let description = field(bucket, "description");
            let mentions_neon = [group_name, group_description, id, name, description].iter().any(|s| contains_any(s, &["neon"]));
            // A group may explicitly list the models it covers, even under a generic account label.
            let shared = [group_name, id, name].iter().any(|s| contains_any(s, SHARED));
            let lists_models = [group_description, description].iter().any(|s| contains_any(s, &["models within this group:"]));
            let window = field(bucket, "window");
            let period = if window.is_empty() { name } else { window };
            let period = if contains_any(period, &["5h", "5-hour", "5 hour", "session"]) {
                Limit::Session
            } else if contains_any(period, &["7d", "7-day", "7 day", "week"]) {
                Limit::Week
            } else {
                Limit::Neon
            };
            let label = match (group_name, name) {
                ("", "") => if id.is_empty() { "Antigravity" } else { id }.to_string(),
                ("", name) | (name, "") => name.to_string(),
                (group, name) => format!("{} · {}", truncate(group, 80), truncate(name, 80)),
            };
            let label = if window.is_empty() { truncate(&label, 120) } else { format!("{} · {}", truncate(&label, 80), truncate(window, 32)) };
            let window = UsageWindow { name: label, limit: None, used_percent, reset_at, reset_text };
            Ok(Self { index, id, mentions_neon, shared, lists_models, disabled, period, window })
        }

        fn more_constrained_than(&self, other: &Self) -> bool {
            let a = &self.window;
            let b = &other.window;
            a.used_percent > b.used_percent || (a.used_percent == b.used_percent && a.reset_at.unwrap_or(u64::MAX) < b.reset_at.unwrap_or(u64::MAX))
        }

        fn push(mut self, windows: &mut Vec<UsageWindow>) {
            let taken = |name: &str| windows.iter().any(|w| w.name == name);
            if taken(&self.window.name) {
                let base = self.window.name;
                let mut name = if self.id.is_empty() { base.clone() } else { format!("{base} · {}", truncate(self.id, 80)) };
                let mut suffix = self.index;
                while taken(&name) {
                    name = format!("{base} #{suffix}");
                    suffix += 1;
                }
                self.window.name = name;
            }
            push_window(windows, self.window);
        }
    }

    fn buckets(data: &Value) -> Result<Vec<Bucket<'_>>, &'static str> {
        let groups = data.get("groups").and_then(Value::as_arr).ok_or("Antigravity returned no quota groups")?;
        if groups.len() > MAX_GROUPS {
            return Err("Antigravity returned too many quota groups");
        }
        let mut parsed = Vec::new();
        for group in groups {
            if !matches!(group, Value::Obj(_)) {
                return Err("Antigravity returned an invalid quota group");
            }
            let Some(items) = group.get("buckets") else { continue };
            let items = items.as_arr().ok_or("Antigravity returned invalid quota buckets")?;
            if parsed.len() + items.len() > MAX_BUCKETS {
                return Err("Antigravity returned too many quota buckets");
            }
            for item in items {
                parsed.push(Bucket::parse(group, item, parsed.len())?);
            }
        }
        Ok(parsed)
    }

    pub(crate) fn parse_usage(raw: &str) -> Reading {
        if raw.len() > MAX_OUTPUT_BYTES {
            return Reading::failed(None, "Antigravity usage response exceeded its size limit");
        }
        let value = match parse_depth(raw.as_bytes(), 16) {
            Ok(value) => value,
            Err(e) => return Reading::failed(None, format!("Invalid Antigravity usage JSON: {e}")),
        };
        let data = match command_data(&value) {
            Ok(data) => data,
            Err(e) => return Reading::failed(None, e),
        };
        let account = account_email(data.get("email").and_then(Value::as_str));
        let mut buckets = match buckets(data) {
            Ok(buckets) => buckets,
            Err(e) => return Reading::failed(None, e),
        };
        let explicit_neon = buckets.iter().any(|b| b.mentions_neon);
        buckets.retain(|b| !b.disabled);
        if explicit_neon && !buckets.iter().any(|b| b.mentions_neon) {
            return Reading::failed(account, "Gemini 4 Neon quota is disabled on this account");
        }
        let shared_only = !explicit_neon && buckets.len() > 1;
        // A model list that does not name Neon is another pool's, even as the only bucket.
        if !explicit_neon && (buckets.iter().any(|b| b.lists_models) || (shared_only && !buckets.iter().any(|b| b.shared))) {
            return Reading::failed(account, "Antigravity returned no Gemini 4 Neon quota bucket");
        }

        // Choose the winner of each period before the display cap or a duplicate name can hide it.
        for period in [Limit::Session, Limit::Week, Limit::Neon] {
            let mut best: Option<usize> = None;
            for (index, b) in buckets.iter().enumerate() {
                let eligible = if explicit_neon { b.mentions_neon } else { !shared_only || b.shared };
                if eligible && b.period == period && best.is_none_or(|i| b.more_constrained_than(&buckets[i])) {
                    best = Some(index);
                }
            }
            if let Some(i) = best {
                buckets[i].window.limit = Some(period);
            }
        }
        if buckets.is_empty() {
            return Reading::failed(account, "Antigravity returned no usable quota windows");
        }
        buckets.sort_by_key(|b| b.window.limit.is_none());
        let mut reading = Reading { account, ..Default::default() };
        for bucket in buckets {
            bucket.push(&mut reading.windows);
        }
        reading
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native {
    use super::*;
    use crate::usage::MAX_OUTPUT_BYTES;
    use std::io::{self, Read};
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    pub(super) const SIGNED_OUT: &str = "Not signed in to Antigravity on this machine";
    pub(super) const API_KEY_UNSUPPORTED: &str = "Antigravity usage is not available with a Gemini API key";
    pub(super) const CANCELLED: &str = "Antigravity usage query cancelled";
    pub(super) const USAGE_TIMEOUT: Duration = Duration::from_secs(25);
    pub(super) const CREDITS_TIMEOUT: Duration = Duration::from_secs(12);
    pub(super) const POLL_TIMEOUT: Duration = Duration::from_secs(35);
    const INTERVAL: Duration = Duration::from_millis(20);
    const MAX_STDERR_BYTES: usize = 16 * 1024;

    extern "C" {
        fn setsid() -> i32;
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
    }

    pub(super) struct RunOutput {
        pub status: Option<i32>,
        pub stdout: String,
        pub stderr: String,
    }

    pub(super) enum RunError {
        Spawn(String),
        Transient(String),
    }

    struct OwnedChild(Child);

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                // SAFETY: setsid created this group and its leader is still our unreaped child.
                unsafe { kill(-(self.0.id() as i32), 15) };
                let deadline = Instant::now() + Duration::from_millis(400);
                while Instant::now() < deadline {
                    match self.0.try_wait() {
                        Ok(Some(_)) => return,
                        Ok(None) => std::thread::sleep(INTERVAL),
                        Err(_) => break,
                    }
                }
                if matches!(self.0.try_wait(), Ok(None)) {
                    unsafe { kill(-(self.0.id() as i32), 9) };
                    let _ = self.0.kill();
                }
            }
            let _ = self.0.wait();
        }
    }

    struct Pipe<R> {
        pipe: R,
        bytes: Vec<u8>,
        eof: bool,
        truncated: bool,
    }

    impl<R: Read + AsRawFd> Pipe<R> {
        fn new(pipe: R) -> Result<Self, RunError> {
            #[cfg(target_os = "macos")]
            const O_NONBLOCK: i32 = 0x0004;
            #[cfg(target_os = "linux")]
            const O_NONBLOCK: i32 = 0x0800;
            let fd = pipe.as_raw_fd();
            // SAFETY: the pipe owns fd. F_GETFL and F_SETFL read and write its status flags.
            unsafe {
                let flags = fcntl(fd, 3);
                if flags < 0 || fcntl(fd, 4, flags | O_NONBLOCK) < 0 {
                    return Err(RunError::Transient(format!("Cannot configure Antigravity usage pipes: {}", io::Error::last_os_error())));
                }
            }
            Ok(Self { pipe, bytes: Vec::new(), eof: false, truncated: false })
        }

        /// Take one chunk per pipe per turn so flooding either stream cannot starve the other or cancellation.
        fn drain_chunk(&mut self, limit: usize) -> Result<bool, RunError> {
            if self.eof {
                return Ok(false);
            }
            let mut chunk = [0; 8192];
            match self.pipe.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    Ok(false)
                }
                Ok(n) => {
                    let keep = n.min(limit - self.bytes.len());
                    self.bytes.extend_from_slice(&chunk[..keep]);
                    self.truncated |= keep < n;
                    Ok(true)
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) => Ok(false),
                Err(e) => Err(RunError::Transient(format!("Cannot read Antigravity usage pipes: {e}"))),
            }
        }

        /// The answer: strict UTF-8 (a capped stdout never gets here).
        fn text(self) -> Result<String, RunError> {
            String::from_utf8(self.bytes).map_err(|_| RunError::Transient("Antigravity returned non-UTF-8 output".into()))
        }

        /// Diagnostics, only scanned for ASCII phrases: any bytes, a capped last character included.
        fn lossy(self) -> String {
            String::from_utf8_lossy(&self.bytes).into_owned()
        }
    }

    pub(super) fn run_agy(program: &Path, cwd: &Path, args: &[&str], timeout: Duration, stop: &AtomicBool) -> Result<RunOutput, RunError> {
        let deadline = Instant::now() + timeout;
        let check = || {
            if stop.load(Ordering::Relaxed) {
                Err(RunError::Transient(CANCELLED.into()))
            } else if Instant::now() >= deadline {
                Err(RunError::Transient(format!("Antigravity usage query timed out after {} seconds", timeout.as_secs())))
            } else {
                Ok(())
            }
        };
        check()?;
        let mut cmd = Command::new(program);
        cmd.args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "1").env("NO_COLOR", "1").env("CLICOLOR", "0");
        // SAFETY: setsid is async-signal-safe. A new session detaches the controlling tty as well as the process group.
        unsafe {
            cmd.pre_exec(|| if setsid() < 0 { Err(io::Error::last_os_error()) } else { Ok(()) });
        }
        let mut child = OwnedChild(cmd.spawn().map_err(|e| RunError::Spawn(format!("Cannot start installed Antigravity CLI: {e}")))?);
        let mut stdout = Pipe::new(child.0.stdout.take().ok_or_else(|| RunError::Transient("Antigravity usage stdout is unavailable".into()))?)?;
        let mut stderr = Pipe::new(child.0.stderr.take().ok_or_else(|| RunError::Transient("Antigravity usage stderr is unavailable".into()))?)?;
        loop {
            check()?;
            let stdout_read = stdout.drain_chunk(MAX_OUTPUT_BYTES)?;
            if stdout.truncated {
                return Err(RunError::Transient("Antigravity usage response exceeded its size limit".into()));
            }
            let stderr_read = stderr.drain_chunk(MAX_STDERR_BYTES)?;
            if stdout.eof && stderr.eof {
                if let Some(status) = child.0.try_wait().map_err(|e| RunError::Transient(format!("Cannot wait for Antigravity CLI: {e}")))? {
                    return Ok(RunOutput { status: status.code(), stdout: stdout.text()?, stderr: stderr.lossy() });
                }
            }
            if !stdout_read && !stderr_read {
                std::thread::sleep(INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{Limit, MAX_OUTPUT_BYTES};

    const LIVE_USAGE: &str = r#"{
        "conversation_id":"","status":"SUCCESS",
        "response":"Gemini Models\tWeekly Limit Remaining\t96%\t2026-12-04T11:33:29Z\nGemini Models\tFive Hour Limit Remaining\t85%\t2026-11-27T16:33:29Z\nClaude and GPT models\tWeekly Limit Remaining\t100%\t1970-01-01T00:00:00Z\nClaude and GPT models\tFive Hour Limit Remaining\t100%\t1970-01-01T00:00:00Z\n",
        "duration_seconds":0,"num_turns":0,
        "usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":0},
        "command":{"name":"usage","data":{
            "description":"Within each group, models share a weekly limit and a 5-hour limit. Quota is consumed proportionally to the cost of the tokens. Thus, limits will last longer with shorter tasks or using more cost-effective models. The 5-hour limit smooths out aggregate demand to fairly distribute global capacity across all users, while your weekly limit is tied directly to your individual tier.",
            "groups":[
                {"name":"Gemini Models","description":"Models within this group: Gemini 4 Neon, Gemini 4 Carbon, Gemini 4 Argon, Gemini 4 Hydrogen, Orchestrator (Neon), Orchestrator (Carbon), Orchestrator (Argon), Gemini 3.8 Flash","buckets":[
                    {"id":"gemini-weekly","name":"Weekly Limit Remaining","description":"You have used some of your weekly limit, it will fully refresh in 6 days, 23 hours.","window":"weekly","remaining_fraction":0.9627236127853394,"reset_time":"2026-12-04T11:33:29Z"},
                    {"id":"gemini-5h","name":"Five Hour Limit Remaining","description":"You have used some of your 5-hour limit, it will fully refresh in 4 hours, 11 minutes.","window":"5h","remaining_fraction":0.8544204235076904,"reset_time":"2026-11-27T16:33:29Z"}
                ]},
                {"name":"Claude and GPT models","description":"Models within this group: Claude Opus 5.5, Claude Sonnet 5.5, GPT-OSS 120B","buckets":[
                    {"id":"3p-weekly","name":"Weekly Limit Remaining","window":"weekly","remaining_fraction":1,"reset_time":"1970-01-01T00:00:00Z"},
                    {"id":"3p-5h","name":"Five Hour Limit Remaining","window":"5h","remaining_fraction":1,"reset_time":"1970-01-01T00:00:00Z"}
                ]}
            ]}}
    }"#;
    const LIVE_CREDITS: &str = r#"{"conversation_id":"","status":"SUCCESS","response":"Remaining credits\t0\n","duration_seconds":0,"num_turns":0,"usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":0},"command":{"name":"credits","data":{"remaining_credits":0}}}"#;

    #[test]
    fn parses_captured_live_fixture() {
        let reading = parse_usage(LIVE_USAGE);
        assert_eq!(reading.error, None, "captured usage must parse");
        assert_eq!(reading.account, None, "the CLI did not report an account");
        assert_eq!(reading.windows.len(), 4, "all four active buckets survive");
        for (limit, remaining, reset) in [(Limit::Week, 0.9627236127853394, 1796384009), (Limit::Session, 0.8544204235076904, 1795797209)] {
            let window = reading.window(limit).expect("Gemini group description must select both Neon periods");
            assert!((window.used_percent.unwrap() - (1.0 - remaining) * 100.0).abs() < 1e-10, "full-precision used share for {limit:?}");
            assert_eq!(window.reset_at, Some(reset), "UTC reset for {limit:?}");
        }
        for name in ["Weekly Limit Remaining · weekly", "Five Hour Limit Remaining · 5h"] {
            let window = reading.windows.iter().find(|w| w.name == format!("Claude and GPT models · {name}")).expect("third-party bucket retained");
            assert_eq!((window.limit, window.used_percent), (None, Some(0.0)), "unused third-party quota stays unassigned");
            assert_eq!((window.reset_at, &window.reset_text), (None, &None), "protobuf epoch means no scheduled reset");
        }
        assert_eq!(reading.reset_credits, None, "Antigravity has no free full resets");
        assert_eq!(parse_credits_plan(LIVE_CREDITS), None, "the live credit balance has no plan name");
        let credits = r#"{"status":"SUCCESS","command":{"data":{"plan_name":"Google AI Ultra","remaining_credits":300,"upgrade_uri":"unused"}}}"#;
        assert_eq!(parse_credits_plan(credits).as_deref(), Some("Google AI Ultra"), "keep only the explicit plan name");
        for raw in [credits.replace("SUCCESS", "FAILED"), credits.replace(r#""command""#, r#""error":"failed","command""#), " ".repeat(MAX_OUTPUT_BYTES+1) + credits, "invalid".into()] {
            assert_eq!(parse_credits_plan(&raw), None, "a failed credits response supplies no plan");
        }
    }

    fn neon_buckets(items: &str) -> Reading {
        parse_usage(&format!(r#"{{"groups":[{{"name":"Gemini 4 Neon","buckets":[{items}]}}]}}"#))
    }

    #[test]
    fn classifies_neon_session_week_and_unlabelled_buckets() {
        let reading = parse_usage(r#"{"groups":[
            {"name":"Gemini 4 Neon","buckets":[{"window":"5 hours","remaining_fraction":0.75,"reset_time":"2026-11-27T18:00:00Z"},
                {"window":"Weekly","remaining_fraction":0.4,"reset_time":"2026-12-02T00:00:00Z"}]},
            {"name":"Gemini 3.8 Flash","buckets":[{"remaining_fraction":1.0}]}]}"#);
        for (period, used, reset) in [(Limit::Session, 25.0, 1795802400), (Limit::Week, 60.0, 1796169600)] {
            let window = reading.window(period).expect("both named periods have meters");
            assert_eq!((window.used_percent, window.reset_at), (Some(used), Some(reset)), "correct share and reset for {period:?}");
        }
        assert!(reading.windows.iter().any(|w| w.name == "Gemini 3.8 Flash" && w.limit.is_none()), "the Flash pool stays separate");

        let single = neon_buckets(r#"{"window":"","remaining_fraction":0.85}"#);
        let used = single.window(Limit::Neon).expect("an unlabelled Neon period stays Neon").used_percent.unwrap();
        assert!((used - 15.0).abs() < 1e-10, "remaining 0.85 means 15 percent used");

        let ties = r#"{"id":"duplicate","remaining_fraction":0.8},
            {"id":"duplicate","remaining_fraction":0.2,"reset_time":"2026-12-02T00:00:00Z"},
            {"id":"duplicate","remaining_fraction":0.2,"reset_time":"2026-11-27T18:00:00Z"},
            {"id":"duplicate","remaining_fraction":0.2,"reset_time":"2026-11-27T18:00:00Z"},
            {"disabled":true,"remaining_fraction":0}"#;
        let reading = neon_buckets(ties);
        let winner = reading.window(Limit::Neon).expect("colliding labels must not overwrite the selected limit");
        assert_eq!((winner.used_percent, winner.reset_at), (Some(80.0), Some(1795802400)), "highest use, then earliest reset wins");
        assert_eq!(reading.windows.len(), 4, "distinct active buckets survive even with identical IDs and names");
        let crowded = neon_buckets(&format!("{},{ties}", vec!["{}"; 12].join(",")));
        assert_eq!(crowded.window(Limit::Neon), Some(winner), "a full display keeps the winning meter");

        let ambiguous = r#"{"email":"user@example.com","groups":[{"name":"Gemini 3.8 Flash","description":"This account has quota","buckets":[{}]},{"name":"Claude","buckets":[{}]}]}"#;
        let reading = parse_usage(ambiguous);
        assert_eq!(reading.error.as_deref(), Some("Antigravity returned no Gemini 4 Neon quota bucket"), "multiple model pools give no Neon meter");
        assert_eq!(reading.account.as_deref(), Some("user@example.com"), "keep the reported identity when the account has no Neon bucket");
        assert!(reading.windows.is_empty(), "ambiguous quotas publish no windows");
        let disabled = neon_buckets(r#"{"disabled":true}"#);
        assert!(disabled.error.as_deref().unwrap().contains("disabled") && disabled.windows.is_empty(), "all Neon buckets disabled means no meter");

        let shared = parse_usage(r#"{"groups":[{"name":"All Models","buckets":[{"name":"weekly"},{"name":"Session"}]}]}"#);
        assert!(shared.window(Limit::Week).is_some() && shared.window(Limit::Session).is_some(), "explicitly shared pools may name periods on the bucket");
        let mixed = r#"{"groups":[{"name":"Overall","buckets":[{"window":"week"}]},{"description":"additional quota","buckets":[{"window":"daily"}]}]}"#;
        let reading = parse_usage(mixed);
        assert!(reading.window(Limit::Week).is_some() && reading.window(Limit::Neon).is_none(), "only a marked shared pool supplies a limit");
        let roster = parse_usage(&mixed.replace("additional quota", "Models within this group: Gemini 3.8 Flash"));
        assert!(roster.error.is_some() && roster.windows.is_empty(), "an explicit model list excluding Neon makes a shared fallback ambiguous");
        let single = parse_usage(r#"{"groups":[{"buckets":[{"disabled":true},{"window":"daily"}]}]}"#);
        assert!(single.window(Limit::Neon).is_some(), "one active pool can supply an unlabelled Neon limit");
        let other = parse_usage(r#"{"groups":[{"description":"Models within this group: Claude Opus 5.5","buckets":[{"window":"5h"}]}]}"#);
        assert!(other.error.is_some() && other.windows.is_empty(), "a lone bucket whose model list leaves out Neon is not Neon's");
    }

    #[test]
    fn never_invents_limits_on_malformed_or_amount_only_payloads() {
        for item in [r#"{"remaining_fraction":1.5}"#, r#"{"remaining_fraction":-0.2}"#, r#"{"remaining_fraction":"0.5"}"#,
            r#"{"reset_time":12345}"#, r#"{"reset_time":"tomorrow\n"}"#, r#"{"reset_time":"9999-12-31T23:59:59-01:00"}"#, r#"{"disabled":1}"#, "null"] {
            let reading = neon_buckets(item);
            assert!(reading.error.is_some() && reading.windows.is_empty(), "malformed bucket must fail the entire reading: {item}");
        }
        for raw in [
            r#"{"status":"ERROR","error":"server detail"}"#.into(), r#"{"error":{"message":"server detail"}}"#.into(), "non-JSON".into(),
            " ".repeat(MAX_OUTPUT_BYTES+1), r#"{"groups":[]}"#.into(), r#"{"groups":[null]}"#.into(), r#"{"groups":[{"buckets":null}]}"#.into(),
            format!(r#"{{"groups":[{}]}}"#, vec!["{}"; 33].join(",")),
            format!(r#"{{"groups":[{{"buckets":[{}]}}]}}"#, vec!["{}"; 65].join(",")),
        ] {
            let reading = parse_usage(&raw);
            assert!(reading.error.is_some() && reading.windows.is_empty(), "invalid envelope or shape must have no windows");
            assert!(!reading.error.unwrap().contains("server detail"), "discard the server's error prose");
        }
        let failed = parse_usage(r#"{"status":"ERROR","error":"no quota summary is available for this account"}"#);
        assert_eq!(failed.error.as_deref(), Some(quota::NO_QUOTA), "a failed envelope still says it has no quota summary");
        let reading = neon_buckets(r#"{"id":"amount","remaining_fraction":null,"remaining_amount":42,"reset_time":"2026-11-27T18:00:00Z"},
            {"id":"epoch","window":"5h","reset_time":"1970-01-01T00:00:00Z"},{"id":"text","window":"week","reset_time":"at midnight"}"#);
        assert_eq!(reading.error, None, "amount-only and text-only windows are valid");
        let amount = reading.window(Limit::Neon).expect("amount-only Neon window");
        assert_eq!((amount.used_percent, amount.reset_at), (None, Some(1795802400)), "amount alone supplies no percent");
        let epoch = reading.window(Limit::Session).expect("epoch reset window");
        assert_eq!((epoch.reset_at, &epoch.reset_text), (None, &None), "zero timestamp carries no reset");
        let text = reading.window(Limit::Week).expect("printable reset window");
        assert_eq!((text.reset_at, text.reset_text.as_deref()), (None, Some("at midnight")), "retain only the printable reset text");
    }

    #[test]
    fn classifies_exit_failures_safely() {
        for (code, stdout, stderr, expected) in [
            (Some(1), "", "Error: authentication required. Run 'agy' to log in, then retry.", ExitFailure::SignedOut),
            (Some(2), "", "/usage is not available with a Gemini API key", ExitFailure::UnsupportedApiKey),
            (Some(2), "", "logged out", ExitFailure::UnsupportedApiKey),
            (Some(1), "GEMINI API KEY", "", ExitFailure::UnsupportedApiKey),
            (Some(1), "LOGGED OUT", "", ExitFailure::SignedOut),
            (Some(1), "", "no quota summary is available for this account", ExitFailure::NoQuotaSummary),
            (Some(1), "", "user@example.com: private test detail", ExitFailure::Transient(Some(1))),
            (None, "", "", ExitFailure::Transient(None)),
        ] {
            assert_eq!(classify_exit(code, stdout, stderr), expected, "exit classification for {expected:?}");
        }
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn one_shot_runner_handles_success_signed_out_api_key_cancel_and_reconnect() {
        use std::{fs, os::unix::fs::PermissionsExt, time::{SystemTime, UNIX_EPOCH}};

        struct TempDir(PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let dir = std::env::temp_dir().join(format!("aihub-agy-usage-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&dir).unwrap();
        let _cleanup = TempDir(dir.clone());
        let script = dir.join("fixture");
        fs::write(&script, r#"#!/bin/sh
set -eu
printf '%s\n' "$2" >> "$0.calls"
test "$#" -eq 8
case "$2" in /usage) timeout=20s ;; /credits) timeout=10s ;; *) exit 3 ;; esac
test "$*" = "-p $2 --output-format json --print-timeout $timeout --log-file /dev/null"
test "$AGY_CLI_DISABLE_AUTO_UPDATE/$NO_COLOR/$CLICOLOR" = "1/1/0"
test . -ef "$0.cwd"
if ( : </dev/tty ) 2>/dev/null; then exit 4; fi
if IFS= read -r input; then exit 5; fi
IFS= read -r mode < "$0.mode"
case "$2/$mode" in
    /usage/login) printf "%s\n" "Authentication required. Run 'agy' to log in" >&2; exit 1 ;;
    /usage/key) exit 2 ;;
    /usage/transient) printf '%s\n' 'user@example.com: private test detail' >&2; exit 7 ;;
    /usage/oversize) cat "$0.oversize"; exit 0 ;;
    /usage/invalid) printf '\377'; exit 0 ;;
    /usage/*) cat "$0.stderr" >&2; printf '%s\n' '{"groups":[{"name":"Neon","buckets":[{"remaining_fraction":0.25}]}]}' ;;
    /credits/no-plan) exit 1 ;;
    /credits/*) printf '%s\n' '{"plan_name":"Google AI Ultra","remaining_credits":42}' ;;
esac
"#).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        // Fill the stderr pipe beyond its cap, including a UTF-8 character crossing the boundary.
        fs::write(script.with_extension("stderr"), format!("{}é{}", "x".repeat(16 * 1024 - 1), "y".repeat(64 * 1024))).unwrap();
        fs::write(script.with_extension("oversize"), vec![b'x'; MAX_OUTPUT_BYTES + 1]).unwrap();
        let mode = |name: &str| fs::write(script.with_extension("mode"), format!("{name}\n")).unwrap();
        let log = script.with_extension("calls");
        let calls = || fs::read_to_string(&log).unwrap();
        let mut reader = AgyReader::new(script.with_extension("cwd"));
        reader.program = Some(script.clone());
        mode("ok");
        let cancelled = reader.read(&AtomicBool::new(true));
        assert_eq!(cancelled.error.as_deref(), Some(native::CANCELLED), "pre-cancelled read returns immediately");
        assert!(!log.exists(), "pre-cancelled read starts no command");

        let stop = AtomicBool::new(false);
        let reading = reader.read(&stop);
        assert_eq!(reading.error, None, "usage survives a full stderr pipe");
        assert_eq!(reading.window(Limit::Neon).unwrap().used_percent, Some(75.0), "usage command supplies the meter");
        assert_eq!(reading.plan.as_deref(), Some("Google AI Ultra"), "credits command supplies the plan");
        assert_eq!(reading.reset_credits, None, "credits cannot grant resets");
        assert_eq!(calls(), "/usage\n/credits\n", "exactly one usage then one credits command");
        assert_eq!(reader.pid(), None, "one-shot commands leave no warm CLI");

        for (name, message, stuck) in [("login", native::SIGNED_OUT, Some(Stuck::Login)), ("key", native::API_KEY_UNSUPPORTED, None)] {
            mode(name);
            let previous = calls();
            let first = reader.read(&stop);
            assert_eq!(first.error.as_deref(), Some(message), "fixed error for {name}");
            assert_eq!(first.stuck, stuck, "only sign-out blocks on login");
            assert_eq!(reader.start_error.as_deref(), stuck.is_none().then_some(message), "API-key mode latches a start error");
            assert_eq!(calls(), previous.clone() + "/usage\n", "failed usage skips credits");
            mode("ok");
            assert_eq!(reader.read(&stop), first, "another poll reports the same latched failure");
            assert_eq!(calls(), previous.clone() + "/usage\n", "a latched failure never respawns");
            reader.reconnect();
            assert_eq!(reader.program.as_ref(), Some(&script), "Reconnect keeps the resolved fixture executable");
            assert_eq!(reader.read(&stop).error, None, "Reconnect permits a fresh successful poll");
            assert_eq!(calls(), previous + "/usage\n/usage\n/credits\n", "Reconnect invokes both commands");
        }

        for (name, message) in [("transient", "Antigravity usage query exited with code 7"),
            ("oversize", "Antigravity usage response exceeded its size limit"), ("invalid", "Antigravity returned non-UTF-8 output")] {
            mode(name);
            let reading = reader.read(&stop);
            assert_eq!(reading.error.as_deref(), Some(message), "transient error discards raw CLI output");
            assert_eq!(reader.start_error, None, "transient {name} retries on the next poll");
        }
        mode("no-plan");
        let reading = reader.read(&stop);
        assert_eq!((reading.error, reading.plan), (None, None), "a credits failure preserves the usage reading");
        assert_eq!(reading.windows.len(), 1, "the quota survives a credits failure");
        reader.program = Some(dir.join("missing"));
        let failure = reader.read(&stop);
        assert!(failure.error.is_some() && reader.start_error == failure.error, "a spawn error is latched");
        assert_eq!(reader.program, None, "a spawn failure allows fresh discovery after Reconnect");
    }
}
