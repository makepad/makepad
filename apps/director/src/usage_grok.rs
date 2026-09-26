//! Read-only Grok account allowance via the installed CLI's agent protocol.
//! A private `grok agent --no-leader stdio` child answers `initialize` and the
//! `x.ai/billing` extension (`_x.ai/billing` on the wire). No session is
//! created and no model input is sent. This is the source of the TUI's `/usage`
//! "Usage limit" tab; `grok usage <session>` is per-session token/cost
//! accounting and is never converted into a quota.
//! `creditUsagePercent` is the used share of the credit period named by
//! `currentPeriod.type`; `currentPeriod.end` is ISO-8601 with an explicit
//! offset. The CLI exposes no read-only account identity, so the email stays
//! unknown; its credential files are never opened.
//! Observed with grok 1.0.40. Protocol: the CLI's user guide, "Agent mode".

use crate::usage::{parse_reset_at, ProviderUsage, UsageProvider, UsageWindow};
use makepad_strict_json::{parse_depth, Value};
use std::sync::atomic::{AtomicBool, Ordering};

pub const SOURCE: &str = "Grok CLI agent stdio · x.ai/billing";
const MAX_RESPONSE: usize = 256 * 1024;
const MAX_RAW: usize = 16 * 1024;

fn bounded(text: &str, maximum: usize) -> String {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn unavailable(error: impl Into<String>) -> ProviderUsage {
    ProviderUsage {
        provider: UsageProvider::Grok,
        source: SOURCE.into(),
        observed_at: 0,
        windows: Vec::new(),
        plan: None,
        account_email: None,
        quota_account_email: None,
        raw: String::new(),
        error: Some(bounded(&error.into(), 800)),
    }
}

/// An ISO-8601 period bound: its Unix seconds when the offset is explicit,
/// else the CLI's own text. Anything but a plain string is refused.
fn period_time(period: Option<&Value>, key: &str) -> Result<(Option<u64>, Option<String>), String> {
    match period.and_then(|period| period.get(key)) {
        Some(Value::Str(text)) if !text.chars().any(char::is_control) => {
            let at = parse_reset_at(text);
            Ok((at, at.is_none().then(|| bounded(text, 256))))
        }
        Some(Value::Null) | None => Ok((None, None)),
        _ => Err(format!("Grok returned an invalid period {key}")),
    }
}

/// Parse an RPC response or its result object. The retained raw projection
/// holds the credit share, its period and the tier only; balances, caps and
/// any unknown account fields are dropped.
pub fn parse_billing(raw: &str, observed_at: u64) -> ProviderUsage {
    if raw.len() > MAX_RESPONSE {
        return unavailable("Grok usage response exceeded its size limit");
    }
    let value = match parse_depth(raw.as_bytes(), 16) {
        Ok(value) => value,
        Err(error) => return unavailable(format!("Invalid Grok usage JSON: {error}")),
    };
    if let Some(error) = value.get("error").filter(|v| !v.is_null()) {
        if error.get("code").and_then(Value::as_i64) == Some(-32601) {
            return unavailable("The installed Grok CLI has no account usage query (x.ai/billing)");
        }
        let message: String = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown agent error")
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        return unavailable(format!(
            "Grok usage query failed: {}",
            bounded(&message, 240)
        ));
    }
    let value = value.get("result").unwrap_or(&value);
    let config = match value.get("config") {
        Some(config) if matches!(config, Value::Obj(_)) => config,
        _ => {
            return unavailable(
                "Grok returned no billing config; limits may be unavailable for this account",
            )
        }
    };
    let mut kept: Vec<(String, Value)> = ["creditUsagePercent", "currentPeriod"]
        .into_iter()
        .filter_map(|key| config.get(key).map(|v| (key.to_owned(), v.clone())))
        .collect();
    if let Some(tier) = value.get("subscription_tier") {
        kept.push(("subscription_tier".to_owned(), tier.clone()));
    }
    let mut output = ProviderUsage {
        provider: UsageProvider::Grok,
        source: SOURCE.into(),
        observed_at,
        windows: Vec::new(),
        plan: value
            .get("subscription_tier")
            .and_then(Value::as_str)
            .filter(|tier| !tier.is_empty() && !tier.chars().any(char::is_control))
            .map(|tier| bounded(tier, 80)),
        account_email: None,
        quota_account_email: None,
        raw: bounded(&Value::Obj(kept).to_json(), MAX_RAW),
        error: None,
    };
    let result = (|| -> Result<UsageWindow, String> {
        let used = match config.get("creditUsagePercent") {
            Some(Value::Int(value)) if *value >= 0 => Some(*value as f64),
            Some(Value::F64(value)) if value.is_finite() && *value >= 0.0 => Some(*value),
            Some(Value::Null) | None => None,
            _ => return Err("Grok returned an invalid creditUsagePercent".into()),
        };
        let period = match config.get("currentPeriod") {
            Some(period) if matches!(period, Value::Obj(_)) => Some(period),
            Some(Value::Null) | None => None,
            _ => return Err("Grok currentPeriod is not an object".into()),
        };
        let kind = match period.and_then(|period| period.get("type")) {
            Some(Value::Str(kind)) => Some(kind.as_str()),
            Some(Value::Null) | None => None,
            _ => return Err("Grok returned an invalid period type".into()),
        };
        let (start_at, _) = period_time(period, "start")?;
        let (reset_at, reset_text) = period_time(period, "end")?;
        if used.is_none() && kind.is_none() && reset_at.is_none() && reset_text.is_none() {
            return Err(
                "Grok returned no credit usage; limits may be unavailable for this account".into(),
            );
        }
        // Only the period the CLI names decides the scope; an unnamed or
        // unknown period stays unscoped instead of being shown as a week.
        let (mut label, mut scope) = match kind {
            Some("USAGE_PERIOD_TYPE_WEEKLY") => ("weekly".to_owned(), Some("week")),
            Some("USAGE_PERIOD_TYPE_MONTHLY") => ("monthly".to_owned(), Some("month")),
            Some(kind) => (
                kind.strip_prefix("USAGE_PERIOD_TYPE_")
                    .filter(|name| {
                        (1..=40).contains(&name.len())
                            && name.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
                    })
                    .map(|name| name.to_ascii_lowercase().replace('_', " "))
                    .unwrap_or_else(|| "unknown period".to_owned()),
                None,
            ),
            None => ("unknown period".to_owned(), None),
        };
        // A named period whose own bounds span another length is not shown
        // under that name's scope.
        if let (Some(start), Some(end)) = (start_at, reset_at) {
            let days = end.saturating_sub(start) / 86_400;
            let plausible = match scope {
                Some("week") => (6..=8).contains(&days),
                Some("month") => (27..=32).contains(&days),
                _ => true,
            };
            if !plausible {
                label = format!("{label}, spanning {days} days");
                scope = None;
            }
        }
        Ok(UsageWindow {
            name: format!("Grok credits · {label}"),
            scope,
            used_percent: used,
            remaining_percent: used.map(|v| (100.0 - v).max(0.0)),
            reset_at,
            reset_text,
        })
    })();
    match result {
        Ok(window) => output.windows.push(window),
        Err(error) => {
            output.error = Some(error);
            output.observed_at = 0;
        }
    }
    output
}

/// Runs only on the long-lived usage worker. Uses a private child and bounded,
/// cancellable nonblocking pipes; never connects to existing user terminals,
/// sessions or a shared leader.
pub fn fetch(cancel: &AtomicBool) -> ProviderUsage {
    if cancel.load(Ordering::Relaxed) {
        return unavailable("Grok usage query cancelled");
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        match native::query(cancel) {
            Ok(billing) => parse_billing(&billing, crate::usage::now()),
            Err(error) => unavailable(error),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        unavailable("Grok usage polling is unavailable on this platform")
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

    const TIMEOUT: &str = "Grok usage query timed out after 20 seconds";

    extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
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

    struct OwnedAgent {
        child: Child,
        input: Option<ChildStdin>,
    }
    impl OwnedAgent {
        fn stop(&mut self) {
            // EOF on stdin ends the stdio agent (observed with grok 1.0.40).
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
    impl Drop for OwnedAgent {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn checked_pause(cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Grok usage query cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err(TIMEOUT.into());
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
                return Err("Grok usage query cancelled".into());
            }
            match input.write(text) {
                Ok(0) => return Err("Grok closed its usage input".into()),
                Ok(count) => text = &text[count..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    checked_pause(cancel, deadline)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("Cannot send Grok usage request: {error}")),
            }
        }
        Ok(())
    }

    /// The reply to our request `id`. Notifications, the agent's own requests
    /// (both carry a `method`) and unreadable lines never match and are never
    /// answered; the output budget and the deadline bound them.
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
                return Err("Grok usage query cancelled".into());
            }
            if Instant::now() >= deadline {
                return Err(TIMEOUT.into());
            }
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = pending.drain(..=end).collect();
                let Ok(message) = parse_depth(&line, 32) else {
                    continue;
                };
                if message.get("method").is_none()
                    && message.get("id").and_then(Value::as_i64) == Some(id)
                {
                    return String::from_utf8(line)
                        .map_err(|_| "Grok usage response is not UTF-8".into());
                }
            }
            match output.read(&mut buffer) {
                Ok(0) => return Err("Grok exited before returning its usage response; check CLI installation and sign-in".into()),
                Ok(count) => {
                    *total += count;
                    if *total > MAX_RESPONSE { return Err("Grok usage output exceeded 256 KiB".into()); }
                    pending.extend_from_slice(&buffer[..count]);
                },
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => checked_pause(cancel, deadline)?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
                Err(error) => return Err(format!("Cannot read Grok usage response: {error}")),
            }
        }
    }

    /// The raw `x.ai/billing` reply of a private agent.
    pub(super) fn query(cancel: &AtomicBool) -> Result<String, String> {
        let mut command = Command::new("grok");
        command
            // --no-leader: never join or start the shared leader process.
            .args(["agent", "--no-leader", "stdio"])
            // A neutral directory: no project rules, hooks, plugins or MCP
            // servers of this checkout are discovered or started.
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command
            .spawn()
            .map_err(|error| format!("Cannot start installed Grok CLI: {error}"))?;
        let mut agent = OwnedAgent { child, input: None };
        agent.input = agent.child.stdin.take();
        let mut output = agent
            .child
            .stdout
            .take()
            .ok_or("Grok usage stdout is unavailable")?;
        let input = agent
            .input
            .as_mut()
            .ok_or("Grok usage stdin is unavailable")?;
        nonblocking(input.as_raw_fd())?;
        nonblocking(output.as_raw_fd())?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut pending = Vec::new();
        let mut total = 0;
        // No client capability is offered, so the agent has nothing to ask of us.
        send(input, b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1,\"clientCapabilities\":{}}}\n", cancel, deadline)?;
        let initialized = response(&mut output, &mut pending, &mut total, 1, cancel, deadline)?;
        let initialized = parse_depth(initialized.as_bytes(), 32).map_err(str::to_owned)?;
        if initialized
            .get("error")
            .is_some_and(|error| !error.is_null())
        {
            return Err("Grok usage initialization failed; check the installed CLI".into());
        }
        if initialized.get("result").is_none() {
            return Err("Grok usage initialization returned no result".into());
        }
        send(
            input,
            b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"_x.ai/billing\",\"params\":{}}\n",
            cancel,
            deadline,
        )?;
        response(&mut output, &mut pending, &mut total, 2, cancel, deadline)
    }
}
