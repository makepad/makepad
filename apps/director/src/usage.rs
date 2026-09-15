//! Account quota observations, polled off the UI in a single lifetime worker.
//! Missing tools, unavailable quotas and old observations remain explicit.
use makepad_strict_json::Value;
use makepad_widgets::makepad_platform::thread::{
    SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const POLL_SECONDS: u64 = 120;
pub const ERROR_RETRY_SECONDS: u64 = 600;
pub const MAX_RAW_BYTES: usize = 16 * 1024;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_WINDOWS: usize = 12;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsageProvider {
    #[default]
    Claude,
    Codex,
}
impl UsageProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Fable",
            Self::Codex => "Astra",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageWindow {
    pub name: String,
    /// Only the requested account/session or weekly quota; unrelated buckets
    /// (Spark, reserve, credits) intentionally have no display scope.
    pub scope: Option<&'static str>,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    /// Absolute Unix seconds only when the source supplies an unambiguous time.
    pub reset_at: Option<u64>,
    /// Original reset text remains available for local/relative/unknown zones.
    pub reset_text: Option<String>,
}
impl UsageWindow {
    /// Date/time components preserve the CLI's timezone and relative date text.
    /// A missing year or timezone is never guessed from the machine's clock.
    pub fn reset_parts(&self) -> (Option<String>, Option<String>, Option<String>) {
        if let Some(epoch) = self.reset_at.filter(|n| *n <= 253_402_300_799) {
            let label = format_reset_at(epoch);
            return (
                Some(label[..10].into()),
                Some(label[11..16].into()),
                Some("UTC".into()),
            );
        }
        let Some(text) = self.reset_text.as_deref() else {
            return (None, None, None);
        };
        let text = text.trim();
        let lower = text.to_ascii_lowercase();
        let text = if lower.starts_with("resets ") {
            &text[7..]
        } else if lower.starts_with("reset ") {
            &text[6..]
        } else {
            text
        };
        let (text, zone) = match text.rsplit_once(" (") {
            Some((text, zone)) if zone.ends_with(')') => {
                (text, Some(zone[..zone.len() - 1].to_owned()))
            }
            _ => (text, None),
        };
        let (date, time) = match text.split_once(" at ") {
            Some((date, time)) => (Some(date.to_owned()), time),
            None => (None, text),
        };
        let clock = time.to_ascii_lowercase();
        let clock = clock
            .strip_suffix("am")
            .or_else(|| clock.strip_suffix("pm"))
            .unwrap_or(&clock)
            .trim();
        let parts = clock.split(':').collect::<Vec<_>>();
        let valid_time = (1..=2).contains(&parts.len())
            && parts[0].parse::<u8>().is_ok_and(|h| h < 24)
            && parts
                .get(1)
                .is_none_or(|m| m.len() == 2 && m.parse::<u8>().is_ok_and(|m| m < 60))
            && (parts.len() == 2 || time.ends_with("am") || time.ends_with("pm"));
        (date, valid_time.then(|| time.to_owned()), zone)
    }

    pub fn reset_brief(&self) -> String {
        let (date, time, zone) = self.reset_parts();
        let part = if self.scope == Some("session") {
            time
        } else {
            date
        };
        let Some(mut part) = part else {
            return "—".into();
        };
        if part.len() == 10 && part.as_bytes()[4] == b'-' && part.as_bytes()[7] == b'-' {
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            if let (Ok(month), Ok(day)) = (part[5..7].parse::<usize>(), part[8..].parse::<u8>()) {
                if (1..=12).contains(&month) {
                    part = format!("{} {day}", MONTHS[month - 1]);
                }
            }
        }
        if self.scope == Some("session") && zone.as_deref() == Some("UTC") {
            part.push_str(" UTC");
        }
        part
    }

    pub fn reset_label(&self) -> String {
        self.reset_at
            .map(format_reset_at)
            .or_else(|| self.reset_text.clone())
            .unwrap_or_else(|| "Reset unknown".into())
    }
    pub fn summary(&self) -> String {
        let quota = self
            .used_percent
            .or_else(|| self.remaining_percent.map(|p| 100.0 - p))
            .map(|p| format!("{p:.0}% used"))
            .unwrap_or_else(|| "usage unknown".into());
        format!("{}: {quota} · resets {}", self.name, self.reset_label())
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProviderUsage {
    pub provider: UsageProvider,
    pub source: String,
    /// Last successful observation. Zero means no successful observation yet.
    pub observed_at: u64,
    pub windows: Vec<UsageWindow>,
    pub plan: Option<String>,
    /// Latest CLI identity, independent of the last successful quota sample.
    pub account_email: Option<String>,
    /// Identity verified for the retained quota observation, which can differ
    /// from the latest account lookup when a refresh fails.
    pub quota_account_email: Option<String>,
    pub raw: String,
    pub error: Option<String>,
}
impl ProviderUsage {
    pub fn limits(&self) -> [Option<&UsageWindow>; 2] {
        ["session", "week"].map(|scope| {
            self.windows
                .iter()
                .filter(|w| w.scope == Some(scope))
                .min_by(|a, b| {
                    a.remaining_percent
                        .unwrap_or(f64::INFINITY)
                        .total_cmp(&b.remaining_percent.unwrap_or(f64::INFINITY))
                })
        })
    }
    pub fn is_stale(&self, at: u64) -> bool {
        self.error.is_some()
            || self.observed_at == 0
            || at.saturating_sub(self.observed_at) > POLL_SECONDS * 3
    }
    pub fn summary(&self) -> String {
        let window = self.windows.first().map(UsageWindow::summary);
        match window {
            Some(window) => format!(
                "{} · {}{}",
                self.provider.label(),
                window,
                if self.is_stale(now()) {
                    " · stale"
                } else {
                    ""
                }
            ),
            None => format!(
                "{} · {}",
                self.provider.label(),
                self.error.as_deref().unwrap_or("Checking account limits")
            ),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UsageSnapshot {
    pub account_history: Vec<AccountUsageHistory>,
    pub history_error: Option<String>,
    pub providers: Vec<ProviderUsage>,
    pub polling: Option<UsageProvider>,
    pub next_poll_at: u64,
    /// Exact persistent helper PIDs, so observation never labels quota polls
    /// as agents doing project work.
    pub background_pids: Vec<u32>,
}
impl UsageSnapshot {
    pub fn provider(&self, provider: UsageProvider) -> Option<&ProviderUsage> {
        self.providers.iter().find(|p| p.provider == provider)
    }
    pub fn report(&self) -> String {
        let mut report = String::new();
        for provider in &self.providers {
            report.push_str(&format!(
                "{}{}\nSource: {}\n",
                provider.provider.label(),
                provider
                    .plan
                    .as_ref()
                    .map(|p| format!(" · {p}"))
                    .unwrap_or_default(),
                provider.source
            ));
            if let Some(email) = &provider.account_email {
                report.push_str(&format!("Account: {email}\n"));
            }
            if provider.observed_at > 0 {
                report.push_str(&format!(
                    "Observed {}s ago{}\n",
                    now().saturating_sub(provider.observed_at),
                    if provider.is_stale(now()) {
                        " · stale"
                    } else {
                        ""
                    }
                ));
            }
            for window in &provider.windows {
                report.push_str(&window.summary());
                report.push('\n');
            }
            if let Some(error) = &provider.error {
                report.push_str(&format!("Unavailable: {error}\n"));
            }
            report.push('\n');
        }
        if let Some(provider) = self.polling {
            report.push_str(&format!("Checking {}…\n", provider.label()));
        }
        report
    }
}

include!("usage_history.rs");

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub struct UsageWorker {
    commands: SyncSender<()>,
    snapshots: Receiver<Arc<UsageSnapshot>>,
    stop: Arc<AtomicBool>,
    task: TaskHandle<()>,
    retry_refresh: bool,
}
impl UsageWorker {
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        Self::start_inner(spawner, None)
    }
    pub fn start_with_history(spawner: &ThreadSpawner, path: &std::path::Path) -> Result<Self, String> {
        Self::start_inner(spawner, Some(path.to_owned()))
    }
    fn start_inner(spawner: &ThreadSpawner, history_path: Option<std::path::PathBuf>) -> Result<Self, String> {
        let (commands, rx) = mpsc::sync_channel(1);
        let (tx, snapshots) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let task = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("studio-usage".into()),
                    ..Default::default()
                },
                move || run(rx, tx, cancel, history_path),
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            snapshots,
            stop,
            task,
            retry_refresh: false,
        })
    }
    pub fn refresh(&mut self) {
        self.retry_refresh = true;
        self.retry();
    }
    fn retry(&mut self) {
        if self.retry_refresh {
            match self.commands.try_send(()) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => self.retry_refresh = false,
                Err(TrySendError::Full(_)) => {}
            }
        }
    }
    pub fn poll(&mut self) -> Option<Arc<UsageSnapshot>> {
        self.retry();
        let mut latest = None;
        while let Ok(snapshot) = self.snapshots.try_recv() {
            latest = Some(snapshot);
        }
        latest
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}
impl Drop for UsageWorker {
    fn drop(&mut self) {
        self.request_stop();
    }
}

fn publish(tx: &SyncSender<Arc<UsageSnapshot>>, pending: &mut Option<Arc<UsageSnapshot>>) -> bool {
    if let Some(snapshot) = pending.take() {
        match tx.try_send(snapshot) {
            Ok(()) => SignalToUI::set_ui_signal(),
            Err(TrySendError::Full(snapshot)) => *pending = Some(snapshot),
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
    true
}

fn retain_last_success(previous: &mut ProviderUsage, mut incoming: ProviderUsage) {
    let previous_owner = previous.quota_account_email.clone().or_else(|| {
        previous.error.is_none().then(|| previous.account_email.clone()).flatten()
    });
    // An unknown current identity may retain explicitly stale anonymous
    // display data, but later learning a different identity cannot relabel it.
    let same_owner = incoming.account_email.is_none()
        || incoming.account_email.as_ref() == previous_owner.as_ref();
    if incoming.error.is_some() && previous.observed_at > 0 && same_owner {
        incoming.observed_at = previous.observed_at;
        incoming.windows = previous.windows.clone();
        incoming.plan = previous.plan.clone();
        incoming.quota_account_email = previous_owner;
    }
    incoming.raw = truncate(&incoming.raw, MAX_RAW_BYTES);
    incoming.windows.truncate(MAX_WINDOWS);
    *previous = incoming;
}

fn run(commands: Receiver<()>, tx: SyncSender<Arc<UsageSnapshot>>, stop: Arc<AtomicBool>, history_path: Option<std::path::PathBuf>) {
    let mut snapshot = UsageSnapshot {
        providers: vec![
            ProviderUsage {
                provider: UsageProvider::Claude,
                source: "Fable terminal · /usage".into(),
                ..Default::default()
            },
            ProviderUsage {
                provider: UsageProvider::Codex,
                source: "Codex CLI account/rateLimits/read".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut history = UsageHistoryStore::open(history_path);
    snapshot.account_history = history.accounts.clone();
    snapshot.history_error = history.error.clone();
    let mut claude = ClaudeTerminal::default();
    let mut due = [Instant::now(); 2];
    let mut pending = None;
    while !stop.load(Ordering::Relaxed) {
        if !publish(&tx, &mut pending) {
            return;
        }
        if let Some(index) = (0..2).find(|i| Instant::now() >= due[*i]) {
            let provider = snapshot.providers[index].provider;
            snapshot.polling = Some(provider);
            pending = Some(Arc::new(snapshot.clone()));
            if !publish(&tx, &mut pending) {
                return;
            }
            let incoming = match provider {
                UsageProvider::Claude => claude.fetch(&stop),
                UsageProvider::Codex => crate::usage_codex::fetch(&stop),
            };
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let interval = if incoming.error.is_some() {
                ERROR_RETRY_SECONDS
            } else {
                POLL_SECONDS
            };
            history.observe(&incoming, now());
            snapshot.account_history = history.accounts.clone();
            snapshot.history_error = history.error.clone();
            retain_last_success(&mut snapshot.providers[index], incoming);
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                snapshot.background_pids = claude
                    .session
                    .as_ref()
                    .map(|s| s.pty.child_pid() as u32)
                    .into_iter()
                    .collect();
            }
            due[index] = Instant::now() + Duration::from_secs(interval);
            snapshot.polling = None;
            snapshot.next_poll_at = now()
                + due
                    .iter()
                    .map(|at| at.saturating_duration_since(Instant::now()).as_secs())
                    .min()
                    .unwrap_or(0);
            pending = Some(Arc::new(snapshot.clone()));
            continue;
        }
        match commands.recv_timeout(Duration::from_millis(100)) {
            Ok(()) => due = [Instant::now(); 2],
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn failed(provider: UsageProvider, source: &str, error: impl Into<String>) -> ProviderUsage {
    ProviderUsage {
        provider,
        source: source.into(),
        error: Some(error.into()),
        ..Default::default()
    }
}

/// One owned interactive terminal for this worker's lifetime. Slash commands
/// are sent only after the CLI prompt is ready; no model prompt is submitted.
#[derive(Default)]
struct ClaudeTerminal {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    session: Option<ClaudeSession>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    account_email: Option<String>,
}
impl ClaudeTerminal {
    fn fetch(&mut self, stop: &AtomicBool) -> ProviderUsage {
        const SOURCE: &str = "Fable terminal · /usage";
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let email = claude_account_email(stop);
            if email != self.account_email {
                self.session = None;
                self.account_email = email.clone();
            }
            let result = (|| {
                if stop.load(Ordering::Relaxed) {
                    return Err("Usage polling stopped".into());
                }
                if self.session.is_none() {
                    self.session = Some(ClaudeSession::start()?);
                }
                self.session.as_mut().unwrap().query(stop)
            })();
            let mut usage = match result {
                Ok(text) => {
                    let mut usage = parse_claude_usage(&text, now());
                    usage.source = SOURCE.into();
                    usage
                }
                Err(error) => {
                    self.session = None;
                    failed(UsageProvider::Claude, SOURCE, error)
                }
            };
            let current_email = claude_account_email(stop);
            if current_email != email {
                self.session = None;
                self.account_email = current_email.clone();
                usage.windows.clear();
                usage.observed_at = 0;
                usage.error = Some("Account identity changed or became unavailable during the usage refresh".into());
            } else if usage.error.is_none() && current_email.is_some() {
                usage.quota_account_email = current_email.clone();
            }
            usage.account_email = current_email;
            usage
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = stop;
            failed(
                UsageProvider::Claude,
                SOURCE,
                "Background terminal usage polling requires macOS or Linux",
            )
        }
    }
}

pub(crate) fn account_email(value: Option<&str>) -> Option<String> {
    let email = value?.trim();
    if email.len() > 320 || email.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    (!local.is_empty() && !domain.is_empty() && !domain.contains('@')).then(|| email.to_owned())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn parse_claude_account(raw: &[u8]) -> Option<String> {
    if raw.len() > MAX_RAW_BYTES {
        return None;
    }
    let value = makepad_strict_json::parse_depth(raw, 8).ok()?;
    if value.get("loggedIn").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    account_email(value.get("email").and_then(Value::as_str))
}

/// Read only the installed CLI's identity projection; never open credential
/// files or retain the rest of its output. Runs on the existing usage worker.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn claude_account_email(stop: &AtomicBool) -> Option<String> {
    use std::{
        io::{self, Read},
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Child, Command, Stdio},
    };
    extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
    }
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                // This fresh child is its own process group, never a user session.
                unsafe {
                    kill(-(self.0.id() as i32), 9);
                }
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    let mut child = OwnedChild(
        Command::new("claude")
            .args(["auth", "status", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .ok()?,
    );
    let mut output = child.0.stdout.take()?;
    #[cfg(target_os = "macos")]
    const O_NONBLOCK: i32 = 0x0004;
    #[cfg(target_os = "linux")]
    const O_NONBLOCK: i32 = 0x0800;
    let fd = output.as_raw_fd();
    // SAFETY: the pipe remains owned here; F_GETFL/F_SETFL take integers.
    unsafe {
        let flags = fcntl(fd, 3);
        if flags < 0 || fcntl(fd, 4, flags | O_NONBLOCK) < 0 {
            return None;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut raw = Vec::new();
    let mut eof = false;
    loop {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return None;
        }
        let mut buf = [0u8; 4096];
        match output.read(&mut buf) {
            Ok(0) => eof = true,
            Ok(n) => {
                if raw.len() + n > MAX_RAW_BYTES {
                    return None;
                }
                raw.extend_from_slice(&buf[..n]);
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
        if eof {
            if let Some(status) = child.0.try_wait().ok()? {
                return status
                    .success()
                    .then(|| parse_claude_account(&raw))
                    .flatten();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct ClaudeSession {
    pty: makepad_terminal::pty::Pty,
    terminal: makepad_terminal::term::terminal::Terminal,
    stream: makepad_terminal::term::stream::Stream,
    ready: bool,
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
impl ClaudeSession {
    fn start() -> Result<Self, String> {
        use makepad_terminal::{
            pty::Pty,
            term::{stream::Stream, terminal::Terminal},
        };
        // Fixed command; user configuration remains unchanged. Disable model
        // tools, startup hooks and MCPs for this status-only terminal.
        let command = r#"exec claude --tools '' --strict-mcp-config --mcp-config '{"mcpServers":{}}' --settings '{"disableAllHooks":true}' --setting-sources user --ax-screen-reader"#;
        let pty = Pty::spawn(
            160,
            64,
            None,
            Some(command),
            &[("NO_COLOR", "1"), ("CLICOLOR", "0")],
            None,
        )
        .map_err(|e| format!("Cannot start Fable usage terminal: {e}"))?;
        Ok(Self {
            pty,
            terminal: Terminal::new(160, 64),
            stream: Stream::new(),
            ready: false,
        })
    }
    fn screen(&self) -> String {
        let screen = self.terminal.screen();
        (screen.total_rows().saturating_sub(64)..screen.total_rows())
            .filter_map(|i| screen.row_virtual(i))
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn send(
        &mut self,
        mut bytes: &[u8],
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        while !bytes.is_empty() {
            if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
                return Err("Fable usage input cancelled or timed out".into());
            }
            match self.pty.try_write(bytes) {
                Ok(n) if n > 0 => bytes = &bytes[n..],
                Ok(_) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
    fn query(&mut self, stop: &AtomicBool) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut total = 0;
        let mut last_output = Instant::now();
        let mut submitted = false;
        let mut submitted_at = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err("Usage polling stopped".into());
            }
            if Instant::now() >= deadline {
                return Err("Fable /usage timed out; check CLI sign-in or startup prompts".into());
            }
            for _ in 0..32 {
                let Some(chunk) = self.pty.try_read() else {
                    break;
                };
                total += chunk.len();
                if total > MAX_OUTPUT_BYTES {
                    return Err("Fable /usage exceeded its output budget".into());
                }
                self.stream.process(&chunk, &mut self.terminal);
                last_output = Instant::now();
            }
            let replies = self.terminal.take_outbound();
            if replies.len() > 8192 {
                return Err("Fable terminal reply budget exceeded".into());
            }
            self.send(&replies, stop, deadline)?;
            if self.pty.child_exited() {
                return Err(
                    "Fable usage terminal exited; check the installed Claude CLI and sign-in"
                        .into(),
                );
            }
            let screen = self.screen();
            if !submitted {
                let prompt = screen
                    .lines()
                    .any(|line| matches!(line.trim(), "$" | "❯" | ">"));
                if (self.ready || prompt) && last_output.elapsed() > Duration::from_millis(250) {
                    self.send(b"/usage\r", stop, deadline)?;
                    submitted = true;
                    submitted_at = Instant::now();
                    self.ready = false;
                    last_output = Instant::now();
                }
            } else if submitted_at.elapsed() > Duration::from_millis(1500)
                && !screen.contains("Refreshing…")
            {
                let parsed = parse_claude_usage(&screen, now());
                if parsed.error.is_none() && parsed.limits().iter().all(Option::is_some) {
                    self.send(b"\x1b", stop, deadline)?;
                    self.ready = true;
                    return Ok(screen);
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for ClaudeSession {
    fn drop(&mut self) {
        // Escape dismisses /usage; /exit ends only this status session. Pty's
        // owned-process cleanup remains the bounded fallback.
        let _ = self.pty.try_write(b"\x1b");
        std::thread::sleep(Duration::from_millis(60));
        let _ = self.pty.try_write(b"/exit\r");
        let deadline = Instant::now() + Duration::from_millis(750);
        while !self.pty.child_exited() && Instant::now() < deadline {
            for _ in 0..8 {
                if self.pty.try_read().is_none() {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// Accept known quota fields or explicitly labelled terminal percentages.
/// Token/cost analytics are never converted into account rate-limit numbers.
pub fn parse_claude_usage(raw: &str, observed_at: u64) -> ProviderUsage {
    let text = strip_ansi(raw);
    let mut usage = ProviderUsage {
        provider: UsageProvider::Claude,
        source: "claude-usage".into(),
        raw: truncate(&text, MAX_RAW_BYTES),
        ..Default::default()
    };
    if text.len() > MAX_OUTPUT_BYTES {
        usage.error = Some("Usage output exceeds 256 KiB".into());
        return usage;
    }
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        match makepad_strict_json::parse(trimmed.as_bytes()) {
            Ok(value) => parse_claude_json(&value, &mut usage, 0),
            Err(error) => {
                usage.error = Some(format!("Invalid usage JSON: {error}"));
                return usage;
            }
        }
    } else {
        parse_claude_text(&text, &mut usage);
    }
    if usage.windows.is_empty() {
        usage.error = Some(
            if text.contains("command not found")
                || text.contains("not found")
                || text.contains("No such file")
            {
                "claude-usage is unavailable on PATH".into()
            } else {
                "The command did not report recognizable account quota windows; no limits inferred"
                    .into()
            },
        );
    } else if usage.error.is_none() {
        usage.observed_at = observed_at;
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("stale") && lower.contains("cache") {
        usage.error = Some("The usage command reported stale cached data".into());
        usage.observed_at = 0;
    }
    usage
}

fn percent(value: &Value) -> Option<f64> {
    let n = match value {
        Value::Int(n) => *n as f64,
        Value::F64(n) => *n,
        _ => return None,
    };
    (n.is_finite() && (0.0..=100.0).contains(&n)).then_some(n)
}

fn parse_claude_json(value: &Value, usage: &mut ProviderUsage, depth: usize) {
    if depth > 3 || usage.windows.len() >= MAX_WINDOWS {
        return;
    }
    if let Some(error) = value
        .get("error")
        .filter(|e| !matches!(e, Value::Null | Value::Bool(false)))
    {
        usage.error = Some(truncate(
            error.as_str().unwrap_or("Usage command reported an error"),
            512,
        ));
    }
    if value.get("stale").and_then(Value::as_bool) == Some(true) {
        usage.error = Some("The usage command reported stale cached data".into());
    }
    if let Some(plan) = value.get("plan").and_then(Value::as_str) {
        usage.plan = Some(truncate(plan, 80));
    }
    for (key, name) in [
        ("five_hour", "5-hour"),
        ("seven_day", "7-day"),
        ("seven_day_opus", "7-day Opus"),
        ("seven_day_sonnet", "7-day Sonnet"),
        ("seven_day_fable", "7-day Fable"),
        ("seven_day_oauth_apps", "7-day OAuth apps"),
        ("seven_day_cowork", "7-day Cowork"),
        ("extra_usage", "Extra usage"),
    ] {
        let Some(window) = value.get(key) else {
            continue;
        };
        if matches!(window, Value::Null) {
            continue;
        }
        let used = window.get("utilization").and_then(percent);
        let reset = window.get("resets_at");
        let reset_text = reset.and_then(Value::as_str).map(|s| truncate(s, 256));
        let reset_at = reset
            .and_then(Value::as_u64)
            .filter(|n| *n <= 253_402_300_799)
            .or_else(|| reset_text.as_deref().and_then(parse_reset_at));
        if used.is_some() || reset_at.is_some() || reset_text.is_some() {
            let scope = match key {
                "five_hour" => Some("session"),
                "seven_day" | "seven_day_fable" => Some("week"),
                _ => None,
            };
            let row = UsageWindow {
                name: name.into(),
                scope,
                used_percent: used,
                remaining_percent: used.map(|p| 100.0 - p),
                reset_at,
                reset_text,
            };
            usage.windows.retain(|w| w.name != row.name);
            usage.windows.push(row);
        }
    }
    for key in ["usage", "data", "claude"] {
        if let Some(child) = value.get(key) {
            parse_claude_json(child, usage, depth + 1);
        }
    }
}

fn parse_claude_text(text: &str, usage: &mut ProviderUsage) {
    let mut current: Option<UsageWindow> = None;
    for line in text.lines() {
        let line = line
            .trim()
            .trim_matches(|c: char| matches!(c, '│' | '┃' | '║'))
            .trim();
        let lower = line.to_ascii_lowercase();
        let heading = if lower.contains("current session")
            || lower.contains("5-hour")
            || lower.contains("5 hour")
            || lower.starts_with("5h ")
        {
            Some("5-hour")
        } else if lower.contains("current week")
            || lower.contains("7-day")
            || lower.contains("7 day")
            || lower.contains("weekly")
        {
            Some(if lower.contains("sonnet") {
                "7-day Sonnet"
            } else if lower.contains("opus") {
                "7-day Opus"
            } else if lower.contains("fable") {
                "7-day Fable"
            } else {
                "7-day"
            })
        } else {
            None
        };
        if let Some(name) = heading {
            if let Some(row) = current.take() {
                push_text_window(usage, row);
            }
            let scope = match name {
                "5-hour" => Some("session"),
                "7-day" | "7-day Fable" => Some("week"),
                _ => None,
            };
            current = Some(UsageWindow {
                name: name.into(),
                scope,
                ..Default::default()
            });
        }
        let Some(row) = &mut current else { continue };
        if let Some(mark) = lower.rfind('%') {
            let before = &lower[..mark];
            let start = before
                .char_indices()
                .rev()
                .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            let prefix = before[..start].chars().next_back();
            let malformed = prefix
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.' | '_'));
            if let Ok(p) = before[start..].parse::<f64>() {
                if !malformed && p.is_finite() && (0.0..=100.0).contains(&p) {
                    if lower[mark + 1..].trim_start().starts_with("left")
                        || lower[mark + 1..].trim_start().starts_with("remaining")
                    {
                        row.remaining_percent = Some(p);
                        row.used_percent = Some(100.0 - p);
                    } else if lower[mark + 1..].trim_start().starts_with("used") {
                        row.used_percent = Some(p);
                        row.remaining_percent = Some(100.0 - p);
                    }
                }
            }
        }
        if let Some(start) = lower.find("reset") {
            let reset = line[start..].trim();
            row.reset_text = Some(truncate(reset, 256));
            row.reset_at = reset.split_whitespace().find_map(parse_reset_at);
        }
    }
    if let Some(row) = current {
        push_text_window(usage, row);
    }
}
fn push_text_window(usage: &mut ProviderUsage, row: UsageWindow) {
    if row.used_percent.is_none() && row.remaining_percent.is_none() {
        return;
    }
    usage.windows.retain(|w| w.name != row.name);
    if usage.windows.len() < MAX_WINDOWS {
        usage.windows.push(row);
    }
}

fn truncate(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn strip_ansi(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut result = String::new();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.peek() == Some(&'\\')) {
                            if c == '\x1b' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\r' {
            result.push('\n');
        } else if c == '\n' || c == '\t' || !c.is_control() {
            result.push(c);
        }
    }
    result
}

/// Parse ISO-8601 timestamps with an explicit UTC offset. Human local/reset
/// countdown text is intentionally left uninterpreted in UsageWindow.
pub fn parse_reset_at(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let number = |start, end| {
        let digits = &b[start..end];
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse::<i64>().ok()
    };
    let (year, month, day, hour, minute, second) = (
        number(0, 4)?,
        number(5, 7)?,
        number(8, 10)?,
        number(11, 13)?,
        number(14, 16)?,
        number(17, 19)?,
    );
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=days[(month - 1) as usize]).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let offset = match b.get(i) {
        Some(b'Z' | b'z') if i + 1 == b.len() => 0,
        Some(sign @ (b'+' | b'-')) if i + 6 == b.len() && b[i + 3] == b':' => {
            let h = number(i + 1, i + 3)?;
            let m = number(i + 4, i + 6)?;
            if h > 23 || m > 59 {
                return None;
            }
            (h * 3600 + m * 60) * if *sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    u64::try_from(days * 86400 + hour * 3600 + minute * 60 + second - offset).ok()
}

pub fn format_reset_at(epoch: u64) -> String {
    if epoch > 253_402_300_799 {
        return format!("Unix {epoch}");
    }
    let days = (epoch / 86400) as i64 + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        epoch % 86400 / 3600,
        epoch % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_reads_only_valid_signed_in_email() {
        assert_eq!(parse_claude_account(br#"{"loggedIn":true,"email":"person@example.com","accessToken":"not retained","orgId":"not retained"}"#).as_deref(), Some("person@example.com"));
        for raw in [
            r#"{"loggedIn":false,"email":"person@example.com"}"#,
            r#"{"email":"person@example.com"}"#,
            r#"{"loggedIn":true,"email":null}"#,
            r#"{"loggedIn":true,"email":"person@example.com\nsecret"}"#,
            r#"{"loggedIn":true,"email":"@example.com"}"#,
            r#"{"loggedIn":true,"email":"person@example.com@elsewhere"}"#,
            "invalid json",
        ] {
            assert!(parse_claude_account(raw.as_bytes()).is_none());
        }
    }

    #[test]
    fn account_change_does_not_relabel_old_quotas() {
        let mut previous = parse_claude_usage(r#"{"five_hour":{"utilization":75}}"#, 100);
        previous.account_email = Some("old@example.com".into());
        let mut incoming = failed(UsageProvider::Claude, "test", "unavailable");
        incoming.account_email = Some("new@example.com".into());
        retain_last_success(&mut previous, incoming);
        assert_eq!(previous.account_email.as_deref(), Some("new@example.com"));
        assert!(previous.windows.is_empty());
        assert_eq!(previous.observed_at, 0);
    }

    #[test]
    fn reset_components_keep_local_and_utc_dates_distinct() {
        let usage = parse_claude_usage("Current session\n15% used\nResets 12:19am (Europe/Amsterdam)\nCurrent week (all models)\n20% used\nResets Sep 10 at 1:59am (Europe/Amsterdam)", 1);
        let [Some(session), Some(week)] = usage.limits() else {
            panic!("missing limits")
        };
        assert_eq!(
            session.reset_parts(),
            (
                None,
                Some("12:19am".into()),
                Some("Europe/Amsterdam".into())
            )
        );
        assert_eq!(
            week.reset_parts(),
            (
                Some("Sep 10".into()),
                Some("1:59am".into()),
                Some("Europe/Amsterdam".into())
            )
        );
        assert_eq!(session.reset_brief(), "12:19am");
        assert_eq!(week.reset_brief(), "Sep 10");
        let utc = UsageWindow {
            scope: Some("week"),
            reset_at: parse_reset_at("2026-09-13T01:00:00+02:00"),
            ..Default::default()
        };
        assert_eq!(
            utc.reset_parts(),
            (
                Some("2026-09-12".into()),
                Some("23:00".into()),
                Some("UTC".into())
            )
        );
        assert_eq!(utc.reset_brief(), "Sep 12");
        let unknown = UsageWindow {
            reset_text: Some("Resets soon".into()),
            ..Default::default()
        };
        assert_eq!(unknown.reset_parts(), (None, None, None));
        assert_eq!(unknown.reset_brief(), "—");
    }
    #[test]
    fn actual_usage_view_ignores_local_analytics_and_selects_requested_limits() {
        let usage = parse_claude_usage("[Screen Reader Mode: on via flag]\nClaude Code v2.1.263\nCurrent session\n0% 0% used\nResets 12:20am (Europe/Amsterdam)\nCurrent week (all models)\n20% 20% used\nResets Sep 10 at 2am (Europe/Amsterdam)\n+50% weekly limits promo through Sep 13\nCurrent week (Fable)\n0% 0% used\nWhat's contributing to your limits usage?\n76% of your usage was at >150k context\n", 123);
        assert!(usage.error.is_none());
        let limits = usage.limits();
        assert_eq!(limits[0].unwrap().remaining_percent, Some(100.0));
        assert_eq!(limits[1].unwrap().remaining_percent, Some(80.0));
        assert!(limits[1].unwrap().reset_label().contains("Sep 10"));
    }

    #[test]
    fn quota_json_preserves_unknowns_and_absolute_resets() {
        let usage = parse_claude_usage(
            r#"{"five_hour":{"utilization":37.5,"resets_at":"2026-09-06T19:40:00+02:00"},"seven_day":{"utilization":null,"resets_at":"Sunday at 10pm (CEST)"},"seven_day_opus":null}"#,
            123,
        );
        assert_eq!(usage.error, None);
        assert_eq!(usage.observed_at, 123);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].remaining_percent, Some(62.5));
        assert_eq!(usage.windows[0].reset_label(), "2026-09-06 17:40 UTC");
        assert_eq!(usage.windows[1].used_percent, None);
        assert_eq!(usage.windows[1].reset_at, None);
        assert_eq!(
            usage.windows[1].reset_text.as_deref(),
            Some("Sunday at 10pm (CEST)")
        );
    }
    #[test]
    fn terminal_quota_uses_labels_and_never_guesses_a_timezone() {
        let usage=parse_claude_usage("\x1b[32mCurrent session\x1b[0m\n████45% used\nResets 7pm (Europe/Amsterdam)\nCurrent week (all models)\n20% left\nResets Sep 12 at 6am\n",99);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].used_percent, Some(45.0));
        assert_eq!(usage.windows[1].used_percent, Some(80.0));
        assert_eq!(usage.windows[0].reset_at, None);
        assert_eq!(
            usage.windows[0].reset_text.as_deref(),
            Some("Resets 7pm (Europe/Amsterdam)")
        );
    }
    #[test]
    fn cost_reports_missing_tools_and_malformed_numbers_are_unavailable() {
        for raw in [
            r#"{"total_cost":50,"tokens":10000}"#,
            "zsh: command not found: claude-usage",
            "Current session\n-5% used",
            "Current session\n1e2% used",
            r#"{"five_hour":{"utilization":200}}"#,
            r#"{"five_hour":{"utilization":10},"five_hour":{"utilization":20}}"#,
        ] {
            let usage = parse_claude_usage(raw, 99);
            assert!(usage.error.is_some());
            assert_eq!(usage.observed_at, 0);
            assert!(usage.windows.is_empty());
        }
    }
    #[test]
    fn failure_retains_last_observation_but_marks_it_stale() {
        let mut previous = parse_claude_usage(r#"{"five_hour":{"utilization":75}}"#, 100);
        previous.account_email = Some("old@example.com".into());
        retain_last_success(
            &mut previous,
            failed(
                UsageProvider::Claude,
                "claude-usage",
                "temporarily unavailable",
            ),
        );
        assert_eq!(previous.observed_at, 100);
        assert_eq!(previous.windows[0].used_percent, Some(75.0));
        assert!(previous.is_stale(101));
        assert!(
            previous.account_email.is_none(),
            "unavailable identity must not report an old login"
        );
    }
    #[test]
    fn explicit_timezones_roundtrip_and_invalid_dates_stay_unknown() {
        assert_eq!(parse_reset_at("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_reset_at("2024-02-29T12:34:56.123Z")
                .map(format_reset_at)
                .as_deref(),
            Some("2024-02-29 12:34 UTC")
        );
        assert_eq!(
            parse_reset_at("2026-09-06T19:40:00+02:00"),
            parse_reset_at("2026-09-06T17:40:00Z")
        );
        for value in [
            "2025-02-29T12:00:00Z",
            "2026-09-06T25:00:00Z",
            "2026-09-06T12:00:00",
            "2026-09-06T12:00:00+-1:00",
            "tomorrow 9am",
        ] {
            assert_eq!(parse_reset_at(value), None);
        }
    }
}
