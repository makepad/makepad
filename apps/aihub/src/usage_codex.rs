//! Codex account limits and signed-in email from one warm CLI app-server (ported from
//! Scope). Only account reads are sent, never a conversation or model input. Each limits
//! read is bracketed by matching identities; only their email and the quota fields survive.
//! The auth file is never opened: its modification time tells us when to replace the server
//! after a login or token refresh. A timeout keeps the server; an exit waits for Reconnect.
//!
//! The person's Reset limits spends one of the account's free resets through the same server
//! (`account/rateLimitResetCredit/consume`, the CLI's own protocol; the backend picks the
//! credit). It is sent only after an account read names the account the person confirmed for.
//! Each attempt carries an idempotency key; one whose answer did not arrive is retried with the
//! same key, so a lost answer never spends a second reset.

#[cfg(any(target_os = "macos", target_os = "linux", test))]
use crate::usage::{account_email, plan_name, push_window, truncate, Limit, ResetOutcome, UsageWindow, MAX_OUTPUT_BYTES};
use crate::usage::{Reader, Reading, Stuck};
#[cfg(any(target_os = "macos", target_os = "linux", test))]
use makepad_strict_json::{parse_depth, Value};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{sync::atomic::Ordering, time::SystemTime};

pub struct CodexReader {
    /// The hub's own empty folder, never the person's project.
    cwd: PathBuf,
    account: Option<String>,
    blocked: Option<Stuck>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    server: Option<native::CodexServer>,
    /// A failed start is reported again without another attempt until Reconnect.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    start_error: Option<String>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    program: Option<PathBuf>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    auth_file: Option<PathBuf>,
    /// Outer None: no start attempted yet; inner None: no auth-file timestamp available.
    /// Keep this after an exit too, so a new login can release the blocked reader.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    auth_modified: Option<Option<SystemTime>>,
    /// Never reuse an id, including after a timeout or Reconnect.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    last_id: i64,
    /// A reset whose answer did not arrive (the account and its idempotency key): pressed
    /// again for that account it is retried, not spent twice. A definite answer, or the
    /// account's next good reading (which shows whether it went through), ends it.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    unanswered_reset: Option<(String, String)>,
}

impl CodexReader {
    pub fn new(cwd: PathBuf) -> CodexReader {
        CodexReader {
            cwd,
            account: None,
            blocked: None,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            server: None,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            start_error: None,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            program: None,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            auth_file: std::env::var_os("CODEX_HOME")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").filter(|p| !p.is_empty()).map(|p| PathBuf::from(p).join(".codex")))
                .map(|p| p.join("auth.json")),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            auth_modified: None,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            last_id: 0,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            unanswered_reset: None,
        }
    }

    /// A new login or token refresh (the auth file's timestamp moved) replaces the server once.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn follow_login(&mut self) {
        // Metadata only: this file contains credentials and must never be read.
        let modified = self.auth_file.as_ref().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok());
        if self.auth_modified.is_some_and(|previous| previous != modified) {
            self.reconnect();
        }
        self.auth_modified = Some(modified);
    }

    /// The warm server, started on first use. A failed start is not retried until Reconnect
    /// or a changed auth timestamp.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn server(&mut self) -> Result<&mut native::CodexServer, String> {
        if let Some(error) = &self.start_error {
            return Err(error.clone());
        }
        if self.server.is_none() {
            let started = (|| {
                let cwd = crate::usage::private_dir(&self.cwd).map_err(|_| "Cannot create the Codex usage folder".to_string())?;
                let program = self
                    .program
                    .clone()
                    .or_else(makepad_ai_hub::providers::codex_cli::find_codex)
                    .ok_or("Codex CLI not found (set CODEX_CLI_PATH or put codex on PATH)")?;
                let server = native::CodexServer::start(&program, &cwd)?;
                self.program = Some(program);
                Ok::<_, String>(server)
            })();
            match started {
                Ok(server) => self.server = Some(server),
                Err(error) => {
                    self.start_error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        self.server.as_mut().ok_or_else(|| "Codex usage server is unavailable".to_string())
    }
}

impl Reader for CodexReader {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn read(&mut self, stop: &AtomicBool) -> Reading {
        if stop.load(Ordering::Relaxed) {
            return Reading::failed(self.account.clone(), "Codex usage query cancelled");
        }
        self.follow_login();
        if let Some(stuck) = self.blocked {
            return Reading { stuck: Some(stuck), ..Reading::failed(self.account.clone(), native::ENDED) };
        }
        let mut last_id = self.last_id;
        let mut account = self.account.clone();
        let result = match self.server() {
            Ok(server) => {
                let result = server.query(&mut last_id, &mut account, stop);
                let ended = server.ended() || result.as_ref().err().is_some_and(|e| e.ended);
                Ok((result, ended))
            }
            Err(error) => Err(error),
        };
        self.last_id = last_id;
        self.account = account;
        let (result, ended) = match result {
            Ok(r) => r,
            Err(error) => return Reading::failed(self.account.clone(), error),
        };
        if ended {
            self.server = None;
            self.blocked = Some(Stuck::Ended);
            return Reading { stuck: self.blocked, ..Reading::failed(self.account.clone(), native::ENDED) };
        }
        match result {
            Ok(reading) => {
                // a good reading shows whether an unanswered reset went through
                if reading.error.is_none() && self.unanswered_reset.as_ref().is_some_and(|(a, _)| reading.account.as_deref() == Some(a.as_str())) {
                    self.unanswered_reset = None;
                }
                reading
            }
            Err(error) => Reading::failed(self.account.clone(), error.message),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn read(&mut self, _stop: &AtomicBool) -> Reading {
        let _ = (&self.cwd, &self.account, &self.blocked);
        Reading::failed(None, "Reading Codex usage needs macOS or Linux")
    }

    fn reconnect(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.server = None;
            self.start_error = None;
            self.auth_modified = None;
        }
        self.account = None;
        self.blocked = None;
    }

    fn pid(&self) -> Option<u32> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.server.as_ref().map(native::CodexServer::pid)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        None
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn reset_limits(&mut self, account: &str, stop: &AtomicBool) -> Result<ResetOutcome, String> {
        if stop.load(Ordering::Relaxed) {
            return Err("Reset cancelled; nothing was spent".into());
        }
        self.follow_login();
        if self.blocked.is_some() {
            return Err(native::ENDED.into());
        }
        let key = match &self.unanswered_reset {
            Some((a, key)) if a == account => key.clone(),
            _ => native::reset_key()?,
        };
        let mut last_id = self.last_id;
        let result = self.server().map(|server| {
            let result = server.reset(&mut last_id, account, &key, stop);
            let ended = server.ended() || result.as_ref().err().is_some_and(|e| e.ended);
            (result, ended)
        });
        self.last_id = last_id;
        let (result, ended) = result?;
        if ended {
            self.server = None;
            self.blocked = Some(Stuck::Ended);
        }
        match result {
            // another account is signed in: nothing was sent
            Ok(None) => Err("Codex is signed in as another account now; nothing was spent".into()),
            // a definite answer: this attempt is over
            Ok(Some(Ok(outcome))) => {
                self.unanswered_reset = None;
                Ok(outcome)
            }
            // a refusal or a lost answer may still have spent it: the attempt keeps its key
            Ok(Some(Err(error))) => {
                self.unanswered_reset = Some((account.to_owned(), key));
                Err(format!("{error}; pressing again retries it without spending a second"))
            }
            Err(error) => {
                self.unanswered_reset = Some((account.to_owned(), key));
                Err(format!("No answer to the reset ({}); pressing again retries it without spending a second", error.message))
            }
        }
    }
}

/// The reset request's params: the attempt's idempotency key, no credit named (the backend
/// takes the next one available).
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn reset_params(key: &str) -> String {
    makepad_strict_json::obj(vec![("idempotencyKey", makepad_strict_json::s(key))]).to_json()
}

/// The reset's answer. A server error's text is never kept (it can carry private fields).
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn parse_reset(raw: &str) -> Result<ResetOutcome, String> {
    if raw.len() > MAX_OUTPUT_BYTES {
        return Err("Codex's reset answer exceeded its size limit".into());
    }
    let value = parse_depth(raw.as_bytes(), 16).map_err(|e| format!("Invalid Codex reset JSON: {e}"))?;
    if value.get("error").is_some_and(|v| !v.is_null()) {
        return Err("Codex refused the reset".into());
    }
    let value = value.get("result").unwrap_or(&value);
    match value.get("outcome").and_then(Value::as_str) {
        Some("reset") => Ok(ResetOutcome::Reset),
        Some("nothingToReset") => Ok(ResetOutcome::NothingToReset),
        Some("noCredit") => Ok(ResetOutcome::NoCredit),
        Some("alreadyRedeemed") => Ok(ResetOutcome::AlreadyRedeemed),
        _ => Err("Codex answered the reset without a known outcome".into()),
    }
}

/// Parse an RPC response or its result object. Durations, not primary/secondary positions,
/// identify session and weekly limits. Unknown fields and raw responses are discarded.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn parse_rate_limits(raw: &str) -> Reading {
    if raw.len() > MAX_OUTPUT_BYTES {
        return Reading::failed(None, "Codex usage response exceeded its size limit");
    }
    let value = match parse_depth(raw.as_bytes(), 16) {
        Ok(value) => value,
        Err(error) => return Reading::failed(None, format!("Invalid Codex usage JSON: {error}")),
    };
    if value.get("error").is_some_and(|v| !v.is_null()) {
        // Server error messages can include private fields; never retain their text.
        return Reading::failed(None, "Codex usage query failed");
    }
    let value = value.get("result").unwrap_or(&value);
    let mut output = Reading {
        reset_credits: value.get("rateLimitResetCredits").and_then(|v| v.get("availableCount")).and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()),
        ..Default::default()
    };
    let result = (|| -> Result<(), String> {
        let mut buckets: Vec<(&str, &Value)> = match value.get("rateLimitsByLimitId") {
            Some(Value::Obj(items)) if !items.is_empty() => items.iter().map(|(name, bucket)| (name.as_str(), bucket)).collect(),
            Some(Value::Null | Value::Obj(_)) | None => vec![("codex", value.get("rateLimits").ok_or("Codex did not return rateLimits")?)],
            _ => return Err("Codex rateLimitsByLimitId is not an object".into()),
        };
        if buckets.len() > 16 {
            return Err("Codex returned too many quota buckets".into());
        }
        // Keep the default account bucket first even when the JSON map order varies.
        buckets.sort_by_key(|(key, _)| (*key != "codex", *key));
        for (key, bucket) in buckets {
            if !matches!(bucket, Value::Obj(_)) {
                return Err("Codex quota bucket is not an object".into());
            }
            let label = bucket.get("limitName").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(if key == "codex" { "Codex" } else { key });
            if output.plan.is_none() {
                output.plan = plan_name(bucket.get("planType").and_then(Value::as_str));
            }
            for slot in ["primary", "secondary"] {
                let Some(window) = bucket.get(slot).filter(|v| !v.is_null()) else { continue };
                if !matches!(window, Value::Obj(_)) {
                    return Err(format!("Codex {slot} quota window is not an object"));
                }
                let used = match window.get("usedPercent") {
                    Some(Value::Int(value)) if *value >= 0 => Some(*value as f64),
                    Some(Value::F64(value)) if value.is_finite() && *value >= 0.0 => Some(*value),
                    Some(Value::Null) | None => None,
                    _ => return Err("Codex returned an invalid usedPercent".into()),
                };
                let duration = match window.get("windowDurationMins") {
                    Some(Value::Int(value)) if *value > 0 => Some(*value as u64),
                    Some(Value::Null) | None => None,
                    _ => return Err("Codex returned an invalid windowDurationMins".into()),
                };
                let reset_at = match window.get("resetsAt") {
                    Some(Value::Int(value)) if *value >= 0 && *value <= 253_402_300_799 => Some(*value as u64),
                    Some(Value::Null) | None => None,
                    _ => return Err("Codex returned an invalid resetsAt Unix-seconds timestamp".into()),
                };
                push_window(
                    &mut output.windows,
                    UsageWindow {
                        name: format!("{} · {}", truncate(label, 120), duration.map(duration_label).unwrap_or_else(|| slot.to_owned())),
                        limit: if key == "codex" {
                            match duration {
                                Some(300) => Some(Limit::Session),
                                Some(10080) => Some(Limit::Week),
                                _ => None,
                            }
                        } else {
                            None
                        },
                        used_percent: used,
                        reset_at,
                        reset_text: None,
                    },
                );
            }
            if let Some(individual) = bucket.get("individualLimit").filter(|v| !v.is_null()) {
                let remaining = individual
                    .get("remainingPercent")
                    .and_then(Value::as_i64)
                    .filter(|v| (0..=100).contains(v))
                    .ok_or("Codex returned an invalid individual remainingPercent")?;
                let reset_at = individual
                    .get("resetsAt")
                    .and_then(Value::as_u64)
                    .filter(|v| *v <= 253_402_300_799)
                    .ok_or("Codex returned an invalid individual resetsAt")?;
                push_window(
                    &mut output.windows,
                    UsageWindow {
                        name: format!("{} · individual limit", truncate(label, 120)),
                        limit: None,
                        used_percent: Some((100 - remaining) as f64),
                        reset_at: Some(reset_at),
                        reset_text: None,
                    },
                );
            }
        }
        if output.windows.is_empty() {
            return Err("Codex returned no quota windows; limits may be unavailable for this account".into());
        }
        Ok(())
    })();
    match result {
        Ok(()) => output,
        Err(error) => Reading::failed(None, error),
    }
}

/// Keep only the current ChatGPT email. Other account types, missing identities and
/// protocol failures never fall back to a previously seen account or retain auth fields.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn parse_account_email(raw: &str) -> Option<String> {
    if raw.len() > MAX_OUTPUT_BYTES {
        return None;
    }
    let value = parse_depth(raw.as_bytes(), 16).ok()?;
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return None;
    }
    let result = value.get("result").unwrap_or(&value);
    let account = result.get("account")?;
    if account.get("type").and_then(Value::as_str) != Some("chatgpt") {
        return None;
    }
    account_email(account.get("email").and_then(Value::as_str))
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn duration_label(minutes: u64) -> String {
    if minutes % 1440 == 0 {
        format!("{} day{}", minutes / 1440, if minutes == 1440 { "" } else { "s" })
    } else if minutes % 60 == 0 {
        format!("{} h", minutes / 60)
    } else {
        format!("{minutes} min")
    }
}

/// A private child on the lifetime worker, with Scope's bounded nonblocking pipes and
/// graceful process-group shutdown. No connection is made to the person's existing CLI.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native {
    use super::*;
    use std::{
        io::{self, Read, Write},
        os::{fd::AsRawFd, unix::process::CommandExt},
        path::Path,
        process::{Child, ChildStdin, ChildStdout, Command, Stdio},
        time::{Duration, Instant},
    };

    pub(super) const ENDED: &str = "Codex CLI ended; reconnect to read usage again";
    const SIGNED_OUT: &str = "Not signed in to Codex on this machine";

    extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
    }

    pub(super) struct QueryError {
        pub message: String,
        pub ended: bool,
    }

    impl From<String> for QueryError {
        fn from(message: String) -> Self {
            Self { message, ended: false }
        }
    }

    impl QueryError {
        fn ended() -> Self {
            Self { message: ENDED.into(), ended: true }
        }
    }

    fn nonblocking(fd: i32) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        const O_NONBLOCK: i32 = 0x0004;
        #[cfg(target_os = "linux")]
        const O_NONBLOCK: i32 = 0x0800;
        // SAFETY: fd remains owned by the live pipe; F_GETFL/F_SETFL take integers.
        unsafe {
            let flags = fcntl(fd, 3);
            if flags < 0 || fcntl(fd, 4, flags | O_NONBLOCK) < 0 {
                return Err(format!("Cannot configure Codex usage pipes: {}", io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    struct OwnedServer {
        child: Child,
        input: Option<ChildStdin>,
    }

    impl OwnedServer {
        fn stop(&mut self) {
            // EOF is the app-server's graceful stdio shutdown protocol.
            self.input.take();
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break,
                }
            }
            // Only signal the group this command created while our direct child is live,
            // so a recycled PID can never target another process.
            if matches!(self.child.try_wait(), Ok(None)) {
                unsafe {
                    kill(-(self.child.id() as i32), 15);
                }
                let deadline = Instant::now() + Duration::from_millis(400);
                while Instant::now() < deadline {
                    if matches!(self.child.try_wait(), Ok(Some(_))) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                if matches!(self.child.try_wait(), Ok(None)) {
                    unsafe {
                        kill(-(self.child.id() as i32), 9);
                    }
                    let _ = self.child.kill();
                }
            }
            let _ = self.child.wait();
        }
    }

    impl Drop for OwnedServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    enum Initialization {
        New,
        Waiting(i64),
        Ready,
        Failed,
    }

    pub(super) struct CodexServer {
        owned: OwnedServer,
        output: ChildStdout,
        /// Transport fragments only, never a retained response or diagnostic snapshot.
        pending: Vec<u8>,
        /// Complete a partially written request before sending a new one after a timeout.
        outgoing: Vec<u8>,
        discard_line: bool,
        initialization: Initialization,
    }

    fn check(stop: &AtomicBool, deadline: Instant) -> Result<(), QueryError> {
        if stop.load(Ordering::Relaxed) {
            return Err("Codex usage query cancelled".to_string().into());
        }
        if Instant::now() >= deadline {
            return Err("Codex usage query timed out after 15 seconds".to_string().into());
        }
        Ok(())
    }

    fn checked_pause(stop: &AtomicBool, deadline: Instant) -> Result<(), QueryError> {
        check(stop, deadline)?;
        std::thread::sleep(Duration::from_millis(20));
        Ok(())
    }

    fn next_id(last_id: &mut i64) -> Result<i64, QueryError> {
        *last_id = last_id.checked_add(1).ok_or_else(|| "Codex usage request ids exhausted".to_string())?;
        Ok(*last_id)
    }

    impl CodexServer {
        pub(super) fn start(program: &Path, cwd: &Path) -> Result<Self, String> {
            let child = Command::new(program)
                .args(["app-server", "--stdio"])
                .current_dir(cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .map_err(|error| format!("Cannot start installed Codex CLI: {error}"))?;
            let mut owned = OwnedServer { child, input: None };
            owned.input = owned.child.stdin.take();
            let output = owned.child.stdout.take().ok_or("Codex usage stdout is unavailable")?;
            let input = owned.input.as_ref().ok_or("Codex usage stdin is unavailable")?;
            nonblocking(input.as_raw_fd())?;
            nonblocking(output.as_raw_fd())?;
            Ok(Self { owned, output, pending: Vec::new(), outgoing: Vec::new(), discard_line: false, initialization: Initialization::New })
        }

        pub(super) fn pid(&self) -> u32 {
            self.owned.child.id()
        }

        pub(super) fn ended(&mut self) -> bool {
            matches!(self.owned.child.try_wait(), Ok(Some(_)))
        }

        fn flush_input(&mut self, stop: &AtomicBool, deadline: Instant) -> Result<(), QueryError> {
            while !self.outgoing.is_empty() {
                check(stop, deadline)?;
                if self.ended() {
                    return Err(QueryError::ended());
                }
                let input = self.owned.input.as_mut().ok_or_else(QueryError::ended)?;
                match input.write(&self.outgoing) {
                    Ok(0) => return Err("Codex closed its usage input".to_string().into()),
                    Ok(count) => {
                        drop(self.outgoing.drain(..count));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => checked_pause(stop, deadline)?,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(format!("Cannot send Codex usage request: {error}").into()),
                }
            }
            Ok(())
        }

        fn send(&mut self, text: &str, stop: &AtomicBool, deadline: Instant) -> Result<(), QueryError> {
            self.flush_input(stop, deadline)?;
            self.outgoing.extend_from_slice(text.as_bytes());
            self.flush_input(stop, deadline)
        }

        /// Only one request is awaited at a time. Everything already buffered after its
        /// response is unsolicited or stale, so discard it instead of keeping private data
        /// until the next poll. A partial trailing line is skipped when its remainder arrives.
        fn discard_pending(&mut self) {
            if let Some(last) = self.pending.last() {
                self.discard_line = *last != b'\n';
            }
            self.pending.clear();
        }

        fn response(&mut self, total: &mut usize, id: i64, stop: &AtomicBool, deadline: Instant) -> Result<String, QueryError> {
            let mut buffer = [0; 8192];
            loop {
                check(stop, deadline)?;
                if self.ended() {
                    return Err(QueryError::ended());
                }
                while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                    check(stop, deadline)?;
                    let line: Vec<_> = self.pending.drain(..=end).collect();
                    let message = match parse_depth(&line, 16) {
                        Ok(message) => message,
                        Err(error) => {
                            self.discard_pending();
                            return Err(format!("Invalid Codex app-server response: {error}").into());
                        }
                    };
                    // Notifications and replies to older, timed-out requests are never this read.
                    if message.get("id").and_then(Value::as_i64) == Some(id) {
                        self.discard_pending();
                        return String::from_utf8(line).map_err(|_| "Codex usage response is not UTF-8".to_string().into());
                    }
                }
                match self.output.read(&mut buffer) {
                    Ok(0) => return Err(QueryError::ended()),
                    Ok(count) => {
                        *total += count;
                        if *total > MAX_OUTPUT_BYTES {
                            self.pending.clear();
                            // Recover framing on the next read, without ever growing this buffer.
                            self.discard_line = buffer[count - 1] != b'\n';
                            return Err("Codex usage output exceeded 256 KiB".to_string().into());
                        }
                        let mut bytes = &buffer[..count];
                        if self.discard_line {
                            let Some(end) = bytes.iter().position(|b| *b == b'\n') else { continue };
                            bytes = &bytes[end + 1..];
                            self.discard_line = false;
                        }
                        self.pending.extend_from_slice(bytes);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => checked_pause(stop, deadline)?,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(format!("Cannot read Codex usage response: {error}").into()),
                }
            }
        }

        fn request(
            &mut self,
            last_id: &mut i64,
            method: &str,
            params: &str,
            total: &mut usize,
            stop: &AtomicBool,
            deadline: Instant,
        ) -> Result<String, QueryError> {
            let id = next_id(last_id)?;
            self.send(&format!("{{\"id\":{id},\"method\":\"{method}\",\"params\":{params}}}\n"), stop, deadline)?;
            self.response(total, id, stop, deadline)
        }

        /// The server initialized: the handshake is sent once per server and a timed-out one
        /// stays pending.
        fn ready(&mut self, last_id: &mut i64, total: &mut usize, stop: &AtomicBool, deadline: Instant) -> Result<(), QueryError> {
            check(stop, deadline)?;
            if self.ended() {
                return Err(QueryError::ended());
            }
            self.flush_input(stop, deadline)?;
            if matches!(self.initialization, Initialization::New) {
                let id = next_id(last_id)?;
                self.initialization = Initialization::Waiting(id);
                self.send(
                    &format!(
                        "{{\"id\":{id},\"method\":\"initialize\",\"params\":{{\"clientInfo\":{{\"name\":\"makepad_aihub_usage\",\"version\":\"0.1.0\"}}}}}}\n"
                    ),
                    stop,
                    deadline,
                )?;
            }
            if let Initialization::Waiting(id) = self.initialization {
                // A timed-out handshake stays pending; initialize is sent only once per server.
                let raw = self.response(total, id, stop, deadline)?;
                let initialized = parse_depth(raw.as_bytes(), 16).map_err(str::to_owned)?;
                if initialized.get("error").is_some_and(|v| !v.is_null()) || initialized.get("result").is_none() {
                    self.initialization = Initialization::Failed;
                } else {
                    self.initialization = Initialization::Ready;
                    self.send("{\"method\":\"initialized\",\"params\":{}}\n", stop, deadline)?;
                }
            }
            if matches!(self.initialization, Initialization::Failed) {
                return Err("Codex usage initialization failed; reconnect to try again".to_string().into());
            }
            Ok(())
        }

        pub(super) fn query(&mut self, last_id: &mut i64, account: &mut Option<String>, stop: &AtomicBool) -> Result<Reading, QueryError> {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut total = self.pending.len();
            self.ready(last_id, &mut total, stop, deadline)?;
            let before = parse_account_email(&self.request(last_id, "account/read", "{\"refreshToken\":false}", &mut total, stop, deadline)?);
            *account = before.clone();
            let Some(before) = before else {
                return Ok(Reading::failed(None, SIGNED_OUT));
            };
            let mut reading = parse_rate_limits(&self.request(last_id, "account/rateLimits/read", "{}", &mut total, stop, deadline)?);
            let after = parse_account_email(&self.request(last_id, "account/read", "{\"refreshToken\":false}", &mut total, stop, deadline)?);
            *account = after.clone();
            let Some(after) = after else {
                return Ok(Reading::failed(None, SIGNED_OUT));
            };
            if before != after {
                return Ok(Reading::failed(Some(after), "The account changed during the Codex usage read"));
            }
            reading.account = Some(after);
            Ok(reading)
        }

        /// Spend one reset of `account`'s limits with `key`, after an account read names it
        /// (None: another account is signed in, and nothing was sent).
        pub(super) fn reset(&mut self, last_id: &mut i64, account: &str, key: &str, stop: &AtomicBool) -> Result<Option<Result<ResetOutcome, String>>, QueryError> {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut total = self.pending.len();
            self.ready(last_id, &mut total, stop, deadline)?;
            let signed_in = parse_account_email(&self.request(last_id, "account/read", "{\"refreshToken\":false}", &mut total, stop, deadline)?);
            if signed_in.as_deref() != Some(account) {
                return Ok(None);
            }
            let raw = self.request(last_id, "account/rateLimitResetCredit/consume", &reset_params(key), &mut total, stop, deadline)?;
            Ok(Some(parse_reset(&raw)))
        }
    }

    /// A fresh idempotency key: a random UUID (version 4) from the OS.
    pub(super) fn reset_key() -> Result<String, String> {
        let mut b = [0u8; 16];
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("Cannot make a reset key: {e}"))?;
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_identity_keeps_only_valid_current_chatgpt_email() {
        assert_eq!(
            parse_account_email(
                r#"{"id":3,"result":{"account":{"type":"chatgpt","email":" user@example.com ","accessToken":"do-not-retain","planType":"pro"}}}"#
            )
            .as_deref(),
            Some("user@example.com")
        );
        assert_eq!(parse_account_email(r#"{"account":{"type":"chatgpt","email":"another@example.com"}}"#).as_deref(), Some("another@example.com"));
        for raw in [
            r#"{"result":{"account":null}}"#,
            r#"{"result":{"account":{"type":"apiKey","email":"user@example.com"}}}"#,
            r#"{"result":{"account":{"type":"amazonBedrock","email":"user@example.com"}}}"#,
            r#"{"result":{"email":"user@example.com"}}"#,
            r#"{"result":{"account":{"type":"chatgpt","email":null}}}"#,
            r#"{"result":{"account":{"type":"chatgpt","email":123}}}"#,
            r#"{"result":{"account":{"type":"chatgpt","email":"not-an-email"}}}"#,
            r#"{"result":{"account":{"type":"chatgpt","email":"a\nb@example.com"}}}"#,
            r#"{"error":{"message":"signed out"},"result":{"account":{"type":"chatgpt","email":"old@example.com"}}}"#,
            "invalid JSON",
        ] {
            assert!(parse_account_email(raw).is_none(), "{raw}");
        }
        assert!(parse_account_email(&" ".repeat(MAX_OUTPUT_BYTES + 1)).is_none());
    }

    #[test]
    fn parses_actual_window_duration_percent_and_unix_seconds() {
        let reading = parse_rate_limits(
            r#"{"id":2,"result":{"accountId":"do-not-retain","rateLimits":{"limitId":"codex","planType":"pro","primary":{"usedPercent":97,"windowDurationMins":10080,"resetsAt":1789299229},"secondary":null}}}"#,
        );
        assert!(reading.error.is_none());
        assert_eq!(reading.plan.as_deref(), Some("pro"));
        assert_eq!(reading.account, None);
        assert_eq!(reading.windows.len(), 1);
        assert_eq!(reading.windows[0].name, "Codex · 7 days");
        assert_eq!(reading.windows[0].limit, Some(Limit::Week));
        assert_eq!(reading.windows[0].used_percent, Some(97.0));
        assert_eq!(reading.windows[0].reset_at, Some(1789299229));
        assert!(!format!("{reading:?}").contains("do-not-retain"));
    }

    #[test]
    fn multiple_buckets_are_not_double_counted_or_mislabeled() {
        let reading = parse_rate_limits(
            r#"{"rateLimits":{"primary":{"usedPercent":99}},"rateLimitsByLimitId":{"spark":{"limitName":"Spark","primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":null},"secondary":{"usedPercent":4,"windowDurationMins":10080}},"codex":{"primary":{"usedPercent":23,"windowDurationMins":10080}}}}"#,
        );
        assert!(reading.error.is_none());
        assert_eq!(reading.windows.len(), 3);
        assert_eq!(reading.windows[0].used_percent, Some(23.0));
        assert_eq!(reading.windows[0].name, "Codex · 7 days");
        assert_eq!(reading.windows[0].limit, Some(Limit::Week));
        assert_eq!(reading.windows[1].name, "Spark · 5 h");
        assert_eq!(reading.windows[1].reset_at, None);
        assert_eq!(reading.windows[1].limit, None);
        assert_eq!(reading.windows[2].limit, None);
    }

    #[test]
    fn unavailable_malformed_and_wrong_units_never_invent_limits() {
        for raw in [
            r#"{"error":{"message":"not authenticated"}}"#,
            r#"{"rateLimits":{}}"#,
            r#"{"rateLimits":{"primary":{"usedPercent":-1}}}"#,
            r#"{"rateLimits":{"primary":{"usedPercent":1,"resetsAt":1789299229000}}}"#,
            "not json",
        ] {
            let reading = parse_rate_limits(raw);
            assert!(reading.error.is_some(), "{raw}");
            assert!(reading.windows.is_empty(), "{raw}");
        }
        let reading = parse_rate_limits(r#"{"rateLimits":{"primary":{"usedPercent":null,"resetsAt":null}}}"#);
        assert_eq!(reading.windows[0].used_percent, None);
        assert_eq!(reading.windows[0].limit, None);
        assert_eq!(reading.windows[0].name, "Codex · primary");
        for available in ["-1", "1.5", "\"3\"", "null", "4294967296"] {
            let raw = format!(r#"{{"rateLimits":{{"primary":{{"usedPercent":0}}}},"rateLimitResetCredits":{{"availableCount":{available}}}}}"#);
            let reading = parse_rate_limits(&raw);
            assert!(reading.error.is_none());
            assert_eq!(reading.reset_credits, None, "{available}");
        }
    }

    #[test]
    fn cancellation_never_starts_a_cli_and_overage_is_preserved() {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let mut reader = CodexReader::new(PathBuf::new());
            reader.auth_file = None;
            let reading = reader.read(&AtomicBool::new(true));
            assert!(reading.error.unwrap().contains("cancelled"));
            assert_eq!(reader.pid(), None);
        }
        let reading = parse_rate_limits(r#"{"rateLimits":{"primary":{"usedPercent":105,"windowDurationMins":45}}}"#);
        assert_eq!(reading.windows[0].used_percent, Some(105.0));
        assert_eq!(reading.windows[0].name, "Codex · 45 min");
        assert_eq!(reading.windows[0].limit, None);
    }

    #[test]
    fn individual_limit_is_retained_when_periodic_windows_are_absent() {
        let reading = parse_rate_limits(r#"{"rateLimits":{"individualLimit":{"limit":"100","used":"80","remainingPercent":20,"resetsAt":1789299229}}}"#);
        assert!(reading.error.is_none());
        assert_eq!(reading.windows[0].name, "Codex · individual limit");
        assert_eq!(reading.windows[0].limit, None);
        assert_eq!(reading.windows[0].used_percent, Some(80.0));
        assert_eq!(reading.windows[0].reset_at, Some(1789299229));
    }

    #[test]
    fn current_pro_shape_keeps_week_plan_and_reset_credit_count() {
        let reading = parse_rate_limits(
            r#"{"id":7,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":9,"windowDurationMins":10080,"resetsAt":1795456109},"secondary":null,"credits":{"hasCredits":true,"balance":"1.0"},"planType":"pro"},"rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":9,"windowDurationMins":10080,"resetsAt":1795456109},"secondary":null,"planType":"pro"}},"rateLimitResetCredits":{"availableCount":3,"credits":[{"id":"x","status":"available"}]}}}"#,
        );
        assert!(reading.error.is_none());
        assert_eq!(reading.windows.len(), 1);
        assert_eq!(reading.windows[0].limit, Some(Limit::Week));
        assert_eq!(reading.windows[0].used_percent, Some(9.0));
        assert_eq!(reading.windows[0].reset_at, Some(1795456109));
        assert_eq!(reading.plan.as_deref(), Some("pro"));
        assert_eq!(reading.reset_credits, Some(3));
    }

    /// The reset request names only its idempotency key; its answer is one of the four
    /// outcomes, and a refusal never keeps the server's text.
    #[test]
    fn reset_request_and_its_outcomes() {
        assert_eq!(reset_params("0b5f-4\"x"), r#"{"idempotencyKey":"0b5f-4\"x"}"#);
        for (raw, outcome) in [
            (r#"{"id":9,"result":{"outcome":"reset"}}"#, ResetOutcome::Reset),
            (r#"{"outcome":"nothingToReset"}"#, ResetOutcome::NothingToReset),
            (r#"{"id":9,"result":{"outcome":"noCredit"}}"#, ResetOutcome::NoCredit),
            (r#"{"id":9,"result":{"outcome":"alreadyRedeemed"}}"#, ResetOutcome::AlreadyRedeemed),
        ] {
            assert_eq!(parse_reset(raw), Ok(outcome), "{raw}");
        }
        for raw in [r#"{"id":9,"error":{"message":"user@example.com has no credit"}}"#, r#"{"result":{"outcome":"later"}}"#, r#"{"result":{}}"#, "not json"] {
            let error = parse_reset(raw).unwrap_err();
            assert!(!error.contains("example.com"), "{raw}: {error}");
        }
        assert!(parse_reset(&" ".repeat(MAX_OUTPUT_BYTES + 1)).is_err());
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn private_stdio_reader_stays_warm_spends_a_reset_once_and_closes_on_eof() {
        use std::{fs, os::unix::fs::PermissionsExt, time::UNIX_EPOCH};
        struct TempDir(PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let dir =
            std::env::temp_dir().join(format!("aihub-codex-usage-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&dir).unwrap();
        let _cleanup = TempDir(dir.clone());
        let script = dir.join("codex-fixture");
        fs::write(
            &script,
            r##"#!/bin/sh
set -eu
stage=initialize
last_id=0
while IFS= read -r request; do
    case "$request" in
        *'"method":"initialized"'*)
            test "$stage" = initialized
            stage=before
            continue ;;
    esac
    id=${request#*'"id":'}
    id=${id%%,*}
    case "$id" in ''|*[!0-9]*) exit 18 ;; esac
    test "$id" -gt "$last_id"
    printf '{"id":%s,"result":null}\n' "$last_id"
    last_id=$id
    printf '%s\n' '{"method":"account/updated","params":{}}'
    case "$request" in
        *'"method":"initialize"'*)
            test "$stage" = initialize
            stage=initialized
            printf '%s\n' initialize >> "$0.initialized"
            printf '{"id":%s,"result":{}}\n' "$id" ;;
        *'"method":"account/read","params":{"refreshToken":false}'*)
            case "$stage" in
                before) stage=limits ;;
                after) stage=before ;;
                *) exit 19 ;;
            esac
            printf '{"id":%s,"result":{"account":{"type":"chatgpt","email":"user@example.com","accessToken":"do-not-retain"}}}\n' "$id" ;;
        *'"method":"account/rateLimits/read"'*)
            test "$stage" = limits
            stage=after
            printf '{"id":%s,"result":{"rateLimits":{"primary":{"usedPercent":27,"windowDurationMins":300,"resetsAt":1789299229}}}}\n' "$id" ;;
        *'"method":"account/rateLimitResetCredit/consume","params":{"idempotencyKey":"'*)
            test "$stage" = limits
            stage=before
            key=${request#*'"idempotencyKey":"'}
            printf '%s\n' "${key%%'"'*}" >> "$0.keys"
            case $(wc -l < "$0.keys") in
                *1) printf '{"id":%s,"error":{"message":"backend busy for user@example.com"}}\n' "$id" ;;
                *2) printf '{"id":%s,"result":{"outcome":"alreadyRedeemed"}}\n' "$id" ;;
                *) printf '{"id":%s,"result":{"outcome":"noCredit"}}\n' "$id" ;;
            esac ;;
        *) exit 20 ;;
    esac
done
printf '%s\n' 'graceful EOF' >> "$0.closed"
"##,
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let mut reader = CodexReader::new(dir.join("usage"));
        reader.program = Some(script);
        reader.auth_file = Some(dir.join("auth.json"));
        let stop = AtomicBool::new(false);
        let first = reader.read(&stop);
        let pid = reader.pid();
        let second = reader.read(&stop);
        for reading in [&first, &second] {
            assert!(reading.error.is_none(), "{:?}", reading.error);
            assert_eq!(reading.account.as_deref(), Some("user@example.com"));
            assert_eq!(reading.windows[0].used_percent, Some(27.0));
            assert_eq!(reading.windows[0].limit, Some(Limit::Session));
            assert!(!format!("{reading:?}").contains("do-not-retain"));
        }
        assert!(pid.is_some());
        assert_eq!(reader.pid(), pid, "both reads use the same child");
        let initialized = dir.join("codex-fixture.initialized");
        let closed = dir.join("codex-fixture.closed");
        assert_eq!(fs::read_to_string(&initialized).unwrap().lines().count(), 1);
        assert!(!closed.exists(), "the warm child has not been closed");
        reader.reconnect();
        assert_eq!(reader.pid(), None);
        assert_eq!(fs::read_to_string(&closed).unwrap(), "graceful EOF\n");
        let third = reader.read(&stop);
        assert!(third.error.is_none(), "{:?}", third.error);
        assert_eq!(third.account.as_deref(), Some("user@example.com"));
        assert_eq!(fs::read_to_string(&initialized).unwrap().lines().count(), 2);
        // A reset: an account read names the account first; a refused one is pressed again
        // with the same key, and only a definite answer lets the next press make a new one.
        let refused = reader.reset_limits("user@example.com", &stop).unwrap_err();
        assert!(refused.contains("retries") && !refused.contains("example.com"), "{refused}");
        assert_eq!(reader.reset_limits("user@example.com", &stop), Ok(ResetOutcome::AlreadyRedeemed));
        assert_eq!(reader.reset_limits("user@example.com", &stop), Ok(ResetOutcome::NoCredit));
        let keys = fs::read_to_string(dir.join("codex-fixture.keys")).unwrap();
        let keys: Vec<&str> = keys.lines().collect();
        assert_eq!(keys.len(), 3);
        assert!(keys[0] == keys[1] && keys[1] != keys[2], "{keys:?}");
        assert!(keys.iter().all(|k| k.len() == 36 && k.as_bytes()[14] == b'4'), "random UUIDs: {keys:?}");
        assert!(reader.reset_limits("other@example.com", &stop).unwrap_err().contains("another account"));
        assert_eq!(fs::read_to_string(dir.join("codex-fixture.keys")).unwrap().lines().count(), 3, "another account signed in: nothing sent");
        drop(reader);
        assert_eq!(fs::read_to_string(&closed).unwrap().lines().count(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }
}
