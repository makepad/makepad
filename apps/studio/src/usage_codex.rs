//! Read-only Codex account limits and signed-in email via the installed CLI.
//! This starts no conversation and sends no model input. `resetsAt` is Unix
//! seconds; `windowDurationMins` is minutes (not inferred from primary/secondary).
//! Protocol: https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt

use crate::usage::{ProviderUsage, UsageProvider, UsageWindow};
use makepad_strict_json::{parse_depth, Value};
use std::sync::atomic::{AtomicBool, Ordering};

const SOURCE: &str = "Codex CLI app-server · account/rateLimits/read + account/read";
const MAX_RESPONSE: usize = 256 * 1024;
const MAX_RAW: usize = 16 * 1024;

fn bounded(text: &str, maximum: usize) -> String {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn unavailable(error: impl Into<String>, raw: String) -> ProviderUsage {
    ProviderUsage {
        provider: UsageProvider::Codex,
        source: SOURCE.into(),
        observed_at: 0,
        windows: Vec::new(),
        plan: None,
        account_email: None,
        raw: bounded(&raw, MAX_RAW),
        error: Some(bounded(&error.into(), 800)),
    }
}

/// Parse an RPC response or its result object. The retained raw projection
/// contains rate-limit fields only; account identifiers and auth data are omitted.
pub fn parse_rate_limits(raw: &str, observed_at: u64) -> ProviderUsage {
    if raw.len() > MAX_RESPONSE {
        return unavailable(
            "Codex usage response exceeded its size limit",
            String::new(),
        );
    }
    let value = match parse_depth(raw.as_bytes(), 16) {
        Ok(value) => value,
        Err(error) => {
            return unavailable(format!("Invalid Codex usage JSON: {error}"), String::new())
        }
    };
    if let Some(error) = value.get("error").filter(|v| !v.is_null()) {
        return unavailable(
            format!(
                "Codex usage query failed: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown app-server error")
            ),
            String::new(),
        );
    }
    let value = value.get("result").unwrap_or(&value);
    let safe_raw = Value::Obj(
        ["rateLimits", "rateLimitsByLimitId", "rateLimitResetCredits"]
            .into_iter()
            .filter_map(|key| value.get(key).map(|v| (key.to_owned(), v.clone())))
            .collect(),
    )
    .to_json();
    let mut output = ProviderUsage {
        provider: UsageProvider::Codex,
        source: SOURCE.into(),
        observed_at,
        windows: Vec::new(),
        plan: None,
        account_email: None,
        raw: bounded(&safe_raw, MAX_RAW),
        error: None,
    };
    let result = (|| -> Result<(), String> {
        let mut buckets: Vec<(&str, &Value)> = match value.get("rateLimitsByLimitId") {
            Some(Value::Obj(items)) if !items.is_empty() => items
                .iter()
                .map(|(name, bucket)| (name.as_str(), bucket))
                .collect(),
            Some(Value::Null | Value::Obj(_)) | None => vec![(
                "codex",
                value
                    .get("rateLimits")
                    .ok_or("Codex did not return rateLimits")?,
            )],
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
            let label = bucket
                .get("limitName")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(if key == "codex" { "Codex" } else { key });
            if output.plan.is_none() {
                output.plan = bucket
                    .get("planType")
                    .and_then(Value::as_str)
                    .map(|p| bounded(p, 80));
            }
            for slot in ["primary", "secondary"] {
                let Some(window) = bucket.get(slot).filter(|v| !v.is_null()) else {
                    continue;
                };
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
                    Some(Value::Int(value)) if *value >= 0 && *value <= 253_402_300_799 => {
                        Some(*value as u64)
                    }
                    Some(Value::Null) | None => None,
                    _ => {
                        return Err(
                            "Codex returned an invalid resetsAt Unix-seconds timestamp".into()
                        )
                    }
                };
                output.windows.push(UsageWindow {
                    name: format!(
                        "{} · {}",
                        bounded(label, 120),
                        duration
                            .map(duration_label)
                            .unwrap_or_else(|| slot.to_owned())
                    ),
                    scope: if key == "codex" {
                        match duration {
                            Some(300) => Some("session"),
                            Some(10080) => Some("week"),
                            _ => None,
                        }
                    } else {
                        None
                    },
                    used_percent: used,
                    remaining_percent: used.map(|v| (100.0 - v).max(0.0)),
                    reset_at,
                    reset_text: None,
                });
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
                output.windows.push(UsageWindow {
                    name: format!("{} · individual limit", bounded(label, 120)),
                    scope: None,
                    used_percent: Some((100 - remaining) as f64),
                    remaining_percent: Some(remaining as f64),
                    reset_at: Some(reset_at),
                    reset_text: None,
                });
            }
        }
        if output.windows.is_empty() {
            return Err(
                "Codex returned no quota windows; limits may be unavailable for this account"
                    .into(),
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        output.error = Some(error);
        output.observed_at = 0;
        output.windows.clear();
    }
    output
}

/// Keep only the current ChatGPT email. API-key/other account types, missing
/// identities and protocol failures never fall back to a previously seen user.
/// The account response itself (including any unknown auth fields) is discarded.
fn parse_account_email(raw: &str) -> Option<String> {
    if raw.len() > MAX_RESPONSE { return None; }
    let value = parse_depth(raw.as_bytes(), 16).ok()?;
    if value.get("error").is_some_and(|error| !error.is_null()) { return None; }
    let result = value.get("result").unwrap_or(&value);
    let account = result.get("account")?;
    if account.get("type").and_then(Value::as_str) != Some("chatgpt") { return None; }
    crate::usage::account_email(account.get("email").and_then(Value::as_str))
}

fn duration_label(minutes: u64) -> String {
    if minutes % 1440 == 0 {
        format!(
            "{} day{}",
            minutes / 1440,
            if minutes == 1440 { "" } else { "s" }
        )
    } else if minutes % 60 == 0 {
        format!("{} h", minutes / 60)
    } else {
        format!("{minutes} min")
    }
}

/// Runs only on the long-lived usage worker. Uses a private child and bounded,
/// cancellable nonblocking pipes; never connects to existing user terminals.
pub fn fetch(cancel: &AtomicBool) -> ProviderUsage {
    if cancel.load(Ordering::Relaxed) {
        return unavailable("Codex usage query cancelled", String::new());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        match native::query(cancel) {
            Ok(result) => {
                let mut usage = parse_rate_limits(&result.limits, crate::usage::now());
                usage.account_email = result.account_email;
                usage
            }
            Err(error) => unavailable(error, String::new()),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        unavailable(
            "Codex usage polling is unavailable on this platform",
            String::new(),
        )
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native {
    use super::*;
    use std::{
        io::{self, Read, Write},
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Child, ChildStdin, ChildStdout, Command, Stdio},
        time::{Duration, Instant},
    };

    extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
    }

    pub(super) struct QueryResult {
        pub limits: String,
        pub account_email: Option<String>,
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
                return Err(io::Error::last_os_error().to_string());
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
            // The command created this process group. Only signal it while
            // our direct child remains live, preventing recycled-PID targeting.
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

    fn checked_pause(cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Codex usage query cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("Codex usage query timed out after 15 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(20));
        Ok(())
    }

    fn send(
        input: &mut ChildStdin,
        mut text: &[u8],
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        while !text.is_empty() {
            if cancel.load(Ordering::Relaxed) {
                return Err("Codex usage query cancelled".into());
            }
            match input.write(text) {
                Ok(0) => return Err("Codex closed its usage input".into()),
                Ok(count) => text = &text[count..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    checked_pause(cancel, deadline)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("Cannot send Codex usage request: {error}")),
            }
        }
        Ok(())
    }

    fn response(
        output: &mut ChildStdout,
        pending: &mut Vec<u8>,
        total: &mut usize,
        id: i64,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<String, String> {
        let mut buffer = [0; 8192];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Codex usage query cancelled".into());
            }
            if Instant::now() >= deadline {
                return Err("Codex usage query timed out after 15 seconds".into());
            }
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = pending.drain(..=end).collect();
                let message = parse_depth(&line, 16)
                    .map_err(|error| format!("Invalid Codex app-server response: {error}"))?;
                if message.get("id").and_then(Value::as_i64) == Some(id) {
                    return String::from_utf8(line)
                        .map_err(|_| "Codex usage response is not UTF-8".into());
                }
            }
            match output.read(&mut buffer) {
                Ok(0) => return Err("Codex exited before returning its usage response; check CLI installation and sign-in".into()),
                Ok(count) => {
                    *total += count;
                    if *total > MAX_RESPONSE { return Err("Codex usage output exceeded 256 KiB".into()); }
                    pending.extend_from_slice(&buffer[..count]);
                },
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => checked_pause(cancel, deadline)?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
                Err(error) => return Err(format!("Cannot read Codex usage response: {error}")),
            }
        }
    }

    pub(super) fn query(cancel: &AtomicBool) -> Result<QueryResult, String> {
        query_program(
            std::ffi::OsStr::new("codex"),
            cancel,
            Duration::from_secs(15),
        )
    }

    fn query_program(
        program: &std::ffi::OsStr,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> Result<QueryResult, String> {
        let mut command = Command::new(program);
        command
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command
            .spawn()
            .map_err(|error| format!("Cannot start installed Codex CLI: {error}"))?;
        let mut server = OwnedServer { child, input: None };
        server.input = server.child.stdin.take();
        let mut output = server
            .child
            .stdout
            .take()
            .ok_or("Codex usage stdout is unavailable")?;
        let input = server
            .input
            .as_mut()
            .ok_or("Codex usage stdin is unavailable")?;
        nonblocking(input.as_raw_fd())?;
        nonblocking(output.as_raw_fd())?;
        let deadline = Instant::now() + timeout;
        let mut pending = Vec::new();
        let mut total = 0;
        send(input, b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"makepad_studio_usage\",\"version\":\"0.1.0\"}}}\n", cancel, deadline)?;
        let initialized = response(&mut output, &mut pending, &mut total, 1, cancel, deadline)?;
        let initialized = parse_depth(initialized.as_bytes(), 16).map_err(str::to_owned)?;
        if let Some(error) = initialized.get("error") {
            return Err(format!(
                "Codex usage initialization failed: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            ));
        }
        if initialized.get("result").is_none() {
            return Err("Codex usage initialization returned no result".into());
        }
        send(input, b"{\"method\":\"initialized\",\"params\":{}}\n{\"id\":2,\"method\":\"account/rateLimits/read\"}\n", cancel, deadline)?;
        let limits = response(&mut output, &mut pending, &mut total, 2, cancel, deadline)?;
        // Identity is optional and must not turn a successful quota read into
        // a failure on older CLIs, sign-out, malformed output or timeout. Read
        // sequentially so responses cannot be lost by the single-ID reader.
        let account_deadline = deadline.min(Instant::now() + Duration::from_secs(2));
        let account_email = send(input,
            b"{\"id\":3,\"method\":\"account/read\",\"params\":{\"refreshToken\":false}}\n",
            cancel, account_deadline)
            .and_then(|_| response(&mut output, &mut pending, &mut total, 3, cancel, account_deadline))
            .ok()
            .and_then(|raw| parse_account_email(&raw));
        Ok(QueryResult { limits, account_email })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{fs, os::unix::fs::PermissionsExt};

        #[test]
        fn private_stdio_query_uses_only_account_reads_and_closes_on_eof() {
            let dir = std::env::temp_dir().join(format!(
                "studio-usage-protocol-{}-{}",
                std::process::id(),
                crate::usage::now()
            ));
            fs::create_dir(&dir).unwrap();
            let script = dir.join("codex-fixture");
            fs::write(&script, r##"#!/bin/sh
set -eu
while IFS= read -r request; do
    case "$request" in
        *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
        *'"method":"initialized"'*) : ;;
        *'"method":"account/rateLimits/read"'*) printf '%s\n' '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":27,"windowDurationMins":300,"resetsAt":1789299229}}}}' ;;
        *'"id":3,"method":"account/read","params":{"refreshToken":false}'*) printf '%s\n' '{"id":3,"result":{"account":{"type":"chatgpt","email":"user@example.com","accessToken":"private-token"}}}' ;;
        *) exit 18 ;;
    esac
done
printf '%s' 'graceful EOF' > "$0.closed"
"##).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            let result = query_program(
                script.as_os_str(),
                &AtomicBool::new(false),
                Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(
                parse_rate_limits(&result.limits, 1).windows[0].used_percent,
                Some(27.0)
            );
            assert_eq!(result.account_email.as_deref(), Some("user@example.com"));
            assert!(!result.limits.contains("private-token"));
            assert_eq!(
                fs::read_to_string(dir.join("codex-fixture.closed")).unwrap(),
                "graceful EOF"
            );
            fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn identity_errors_eof_and_timeout_preserve_successful_quotas() {
            let dir = std::env::temp_dir().join(format!(
                "studio-usage-identity-failure-{}-{}", std::process::id(), crate::usage::now()
            ));
            fs::create_dir(&dir).unwrap();
            for (name, reply) in [
                ("unsupported", "printf '%s\\n' '{\"id\":3,\"error\":{\"message\":\"unsupported method\"}}'"),
                ("malformed", "printf '%s\\n' 'invalid JSON'"),
                ("eof", "exit 0"),
                ("silent", ":"),
            ] {
                let script = dir.join(name);
                let source = r##"#!/bin/sh
set -eu
while IFS= read -r request; do
    case "$request" in
        *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
        *'"method":"initialized"'*) : ;;
        *'"method":"account/rateLimits/read"'*) printf '%s\n' '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":27,"windowDurationMins":300}}}}' ;;
        *'"id":3,"method":"account/read","params":{"refreshToken":false}'*) IDENTITY_REPLY ;;
        *) exit 18 ;;
    esac
done
"##.replace("IDENTITY_REPLY", reply);
                fs::write(&script, source).unwrap();
                fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
                // Initialization and quota reads share the production budget;
                // under parallel test load a 400ms deadline could expire before
                // account/read was reached. The silent identity case still
                // exercises its separate, bounded two-second timeout.
                let result = query_program(script.as_os_str(), &AtomicBool::new(false), Duration::from_secs(15)).unwrap();
                let usage = parse_rate_limits(&result.limits, 1);
                assert!(usage.error.is_none(), "{name}");
                assert_eq!(usage.windows[0].used_percent, Some(27.0), "{name}");
                assert!(result.account_email.is_none(), "{name}");
            }
            fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_identity_keeps_only_valid_current_chatgpt_email() {
        assert_eq!(parse_account_email(r#"{"id":3,"result":{"account":{"type":"chatgpt","email":" user@example.com ","accessToken":"do-not-retain","planType":"pro"}}}"#).as_deref(), Some("user@example.com"));
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
        ] { assert!(parse_account_email(raw).is_none(), "{raw}"); }
        assert!(parse_account_email(&" ".repeat(MAX_RESPONSE + 1)).is_none());
    }

    #[test]
    fn parses_actual_window_duration_percent_and_unix_seconds() {
        let usage = parse_rate_limits(
            r#"{"id":2,"result":{"accountId":"private-account","rateLimits":{"limitId":"codex","planType":"pro","primary":{"usedPercent":97,"windowDurationMins":10080,"resetsAt":1789299229},"secondary":null}}}"#,
            123,
        );
        assert_eq!(usage.observed_at, 123);
        assert!(usage.error.is_none());
        assert_eq!(usage.plan.as_deref(), Some("pro"));
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].name, "Codex · 7 days");
        assert_eq!(usage.windows[0].used_percent, Some(97.0));
        assert_eq!(usage.windows[0].remaining_percent, Some(3.0));
        assert_eq!(usage.windows[0].reset_at, Some(1789299229));
        assert!(!usage.raw.contains("private-account"));
    }

    #[test]
    fn multiple_buckets_are_not_double_counted_or_mislabeled() {
        let usage = parse_rate_limits(
            r#"{"rateLimits":{"primary":{"usedPercent":99}},"rateLimitsByLimitId":{"spark":{"limitName":"Spark","primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":null},"secondary":{"usedPercent":4,"windowDurationMins":10080}},"codex":{"primary":{"usedPercent":23,"windowDurationMins":10080}}}}"#,
            1,
        );
        assert!(usage.error.is_none());
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].used_percent, Some(23.0));
        assert_eq!(usage.windows[0].name, "Codex · 7 days");
        assert_eq!(usage.windows[1].name, "Spark · 5 h");
        assert_eq!(usage.windows[1].reset_at, None);
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
            let usage = parse_rate_limits(raw, 123);
            assert_eq!(usage.observed_at, 0);
            assert!(usage.error.is_some());
            assert!(usage.windows.is_empty());
        }
        let usage = parse_rate_limits(
            r#"{"rateLimits":{"primary":{"usedPercent":null,"resetsAt":null}}}"#,
            123,
        );
        assert_eq!(usage.windows[0].used_percent, None);
        assert_eq!(usage.windows[0].remaining_percent, None);
        assert_eq!(usage.windows[0].name, "Codex · primary");
    }

    #[test]
    fn cancellation_never_starts_a_cli_and_overage_is_preserved() {
        let usage = fetch(&AtomicBool::new(true));
        assert_eq!(usage.observed_at, 0);
        assert!(usage.error.unwrap().contains("cancelled"));
        let usage = parse_rate_limits(
            r#"{"rateLimits":{"primary":{"usedPercent":105,"windowDurationMins":45}}}"#,
            1,
        );
        assert_eq!(usage.windows[0].used_percent, Some(105.0));
        assert_eq!(usage.windows[0].remaining_percent, Some(0.0));
        assert_eq!(usage.windows[0].name, "Codex · 45 min");
    }

    #[test]
    fn individual_limit_is_retained_when_periodic_windows_are_absent() {
        let usage = parse_rate_limits(
            r#"{"rateLimits":{"individualLimit":{"limit":"100","used":"80","remainingPercent":20,"resetsAt":1789299229}}}"#,
            1,
        );
        assert!(usage.error.is_none());
        assert_eq!(usage.windows[0].name, "Codex · individual limit");
        assert_eq!(usage.windows[0].remaining_percent, Some(20.0));
        assert_eq!(usage.windows[0].reset_at, Some(1789299229));
    }
}
