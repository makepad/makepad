//! Account usage of the coding CLIs signed in on this machine (Claude Code, Codex): the account
//! each is logged in as, how much of its session and weekly limits (all models, Fable) it used,
//! and when they reset. Ported from Scope's usage readers with their safety rules: no prompt is
//! ever sent, no credential file is read, and only rate-limit and identity fields are kept.
//!
//! Each tool has one lifetime worker thread holding one warm CLI (Claude's interactive terminal
//! and its `/usage` screen, Codex's app-server and its rate-limit protocol). The worker reads it
//! about every 10 min ([`POLL_SECONDS`]) and publishes a snapshot to the UI through a bounded
//! channel. A failed read never starts a new CLI: the person's Reconnect does, so a CLI stuck
//! at a login cannot loop. An account change ends the old CLI once (its limits are the old
//! account's) and the next read starts one for the new account. The person's Reset limits
//! (Codex) spends one of the signed-in account's free resets on the same worker, between reads,
//! and reads again at once: the refreshed limits are its answer.

use makepad_widgets::makepad_platform::thread::{SignalToUI, TaskHandle, ThreadOptions};
use makepad_widgets::Cx;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often each tool is read (s): its usage moves slowly, and its API is the vendor's.
pub const POLL_SECONDS: u64 = 600;
/// Bytes of CLI output one read keeps at most.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
const MAX_WINDOWS: usize = 12;
/// The latest Unix second the readers and the history accept (9999-12-31).
pub const MAX_UNIX: u64 = 253_402_300_799;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Provider {
    #[default]
    Claude,
    Codex,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Claude, Provider::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Codex => "Codex",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }
}

/// Which limit a window is: the rolling session (5 h), the week over all models, the week of
/// Fable. Other buckets (Sonnet, Spark, OAuth apps) have none and are not shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Session,
    Week,
    Fable,
}

impl Limit {
    pub const ALL: [Limit; 3] = [Limit::Session, Limit::Week, Limit::Fable];

    pub fn label(self) -> &'static str {
        match self {
            Limit::Session => "Session",
            Limit::Week => "Week",
            Limit::Fable => "Fable",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageWindow {
    pub name: String,
    pub limit: Option<Limit>,
    pub used_percent: Option<f64>,
    /// Absolute Unix seconds, when the CLI gave an unambiguous time (Codex), or its local clock
    /// text named this machine's zone (Claude).
    pub reset_at: Option<u64>,
    /// The CLI's own reset text (`Resets Nov 23 at 5pm (Europe/Amsterdam)`).
    pub reset_text: Option<String>,
}

/// One read of a tool: the account it is signed in as and that account's limits, or why not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// The account now (None: signed out, or the CLI did not say).
    pub account: Option<String>,
    /// The plan as the CLI names it (`max_20x`, `pro`).
    pub plan: Option<String>,
    /// The limits read, all of `account`'s (empty when the read failed).
    pub windows: Vec<UsageWindow>,
    /// Free full resets of the limits the account still has (Codex).
    pub reset_credits: Option<u32>,
    /// Why there is no reading.
    pub error: Option<String>,
    /// The CLI cannot go on without the person: Reconnect (or log in) starts it again.
    pub stuck: Option<Stuck>,
}

impl Reading {
    pub fn failed(account: Option<String>, error: impl Into<String>) -> Reading {
        Reading { account, error: Some(truncate(&error.into(), 800)), ..Default::default() }
    }

    pub fn window(&self, limit: Limit) -> Option<&UsageWindow> {
        self.windows.iter().find(|w| w.limit == Some(limit))
    }
}

/// Only the readers of macOS and Linux run a CLI, so only there is one ever stuck (elsewhere
/// the type has no value and no reading is stuck).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stuck {
    /// The CLI waits at a login screen.
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    Login,
    /// The CLI's process ended.
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    Ended,
}

impl Stuck {
    /// What a tool's header says about it.
    pub fn label(self) -> &'static str {
        match self {
            #[cfg(any(target_os = "macos", target_os = "linux", test))]
            Stuck::Login => "stuck: waiting for login",
            #[cfg(any(target_os = "macos", target_os = "linux", test))]
            Stuck::Ended => "the CLI ended",
        }
    }
}

/// What a tool's worker is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Busy {
    #[default]
    Idle,
    Reading,
    /// The person's Reconnect: the old CLI ends, a fresh one reads.
    Reconnecting,
    /// The person's Reset limits: one free reset is being spent.
    Resetting,
}

/// What spending one free reset of an account's limits did, as the CLI answered (only the
/// Codex reader of macOS and Linux spends one: elsewhere the type has no value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetOutcome {
    /// One reset was spent and the limits it covers are fresh.
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    Reset,
    /// No limit was used enough to reset: nothing was spent.
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    NothingToReset,
    /// The account had no reset left: nothing was spent.
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    NoCredit,
    /// The same attempt had already reset them (a retry after an answer that did not arrive).
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    AlreadyRedeemed,
}

impl ResetOutcome {
    /// What the panel says after it when it changed nothing, and whether that is a warning
    /// (None: it went through, and the refreshed limits show it).
    pub fn note(self) -> Option<(&'static str, bool)> {
        match self {
            #[cfg(any(target_os = "macos", target_os = "linux", test))]
            ResetOutcome::Reset | ResetOutcome::AlreadyRedeemed => None,
            #[cfg(any(target_os = "macos", target_os = "linux", test))]
            ResetOutcome::NothingToReset => Some(("no limit was used enough to reset; nothing was spent", false)),
            #[cfg(any(target_os = "macos", target_os = "linux", test))]
            ResetOutcome::NoCredit => Some(("this account has no reset left", true)),
        }
    }
}

/// The person's last Reset limits and what came of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResetNote {
    /// When it was answered (Unix s).
    pub at: u64,
    pub account: String,
    pub outcome: Result<ResetOutcome, String>,
}

/// One tool as its worker publishes it.
#[derive(Clone, Debug, Default)]
pub struct ProviderState {
    pub provider: Provider,
    pub busy: Busy,
    /// The latest read (Unix s; 0: none yet) and what it gave, success or not.
    pub checked_at: u64,
    pub latest: Reading,
    /// The last successful read and when. A later failure keeps it only while the account is
    /// the same: an old reading is never labelled with another account.
    pub last_ok: Option<(u64, Reading)>,
    /// When the next read is due (Unix s).
    pub next_at: u64,
    /// The warm CLI's pid while one runs.
    pub pid: Option<u32>,
    /// Every account this tool was seen signed in as, with its readings over time.
    pub accounts: Arc<Vec<crate::usage_history::AccountHistory>>,
    /// Why the saved history did not load or save.
    pub history_error: Option<String>,
    /// The person's last Reset limits.
    pub reset: Option<ResetNote>,
}

impl ProviderState {
    /// The limits to show for the account now: the latest reading's, or the last successful
    /// one of the same account (with its time: older than the latest check means not current).
    pub fn shown(&self) -> Option<(u64, &Reading)> {
        if self.latest.error.is_none() && self.checked_at > 0 {
            return Some((self.checked_at, &self.latest));
        }
        self.last_ok.as_ref().filter(|(_, r)| self.latest.account.is_none() || r.account == self.latest.account).map(|(at, r)| (*at, r))
    }

    fn record(&mut self, reading: Reading, at: u64) {
        if reading.error.is_none() {
            self.last_ok = Some((at, reading.clone()));
        } else if reading.account.is_some() && self.last_ok.as_ref().is_some_and(|(_, r)| r.account != reading.account) {
            self.last_ok = None;
        }
        self.checked_at = at;
        self.latest = reading;
    }
}

/// One tool's warm CLI, owned by its worker thread.
pub trait Reader: Send + 'static {
    /// One read. Never starts a new CLI after a failure of the last one.
    fn read(&mut self, stop: &AtomicBool) -> Reading;
    /// The person's Reconnect: end the warm CLI (gracefully first) and forget that it is
    /// stuck; the next read starts a fresh one.
    fn reconnect(&mut self);
    /// The warm CLI's pid while it runs.
    fn pid(&self) -> Option<u32>;
    /// Spend one free full reset of the limits of `account`, only while it is the account
    /// signed in (Codex); a tool without such resets refuses.
    fn reset_limits(&mut self, _account: &str, _stop: &AtomicBool) -> Result<ResetOutcome, String> {
        Err("This tool has no free resets of its limits".into())
    }
}

enum Command {
    Reconnect,
    /// The person confirmed spending one of this account's resets.
    ResetLimits(String),
}

struct Link {
    provider: Provider,
    commands: SyncSender<Command>,
    states: Receiver<Arc<ProviderState>>,
    /// A command the full channel did not take yet, sent again at the next poll.
    pending: Option<Command>,
    stop: Arc<AtomicBool>,
    _task: TaskHandle<()>,
}

/// The UI's handle on the usage workers: one per tool, for the app's lifetime.
pub struct UsageWorker {
    links: Vec<Link>,
}

impl UsageWorker {
    /// Starts each tool's worker; `dir` holds the tools' account histories.
    pub fn start(cx: &Cx, dir: &Path) -> UsageWorker {
        let mut links = Vec::new();
        for provider in Provider::ALL {
            let Some(reader) = reader(provider, &dir.join("usage-terminal")) else { continue };
            let (commands, rx) = mpsc::sync_channel(2);
            let (tx, states) = mpsc::sync_channel(1);
            let stop = Arc::new(AtomicBool::new(false));
            let cancel = stop.clone();
            let options = ThreadOptions { name: Some(format!("aihub-usage-{}", provider.key()).into()), ..Default::default() };
            let history = crate::usage_history::History::path(dir, provider);
            match cx.spawn_worker_with(options, move || run(provider, reader, history, rx, tx, cancel)) {
                Ok(task) => links.push(Link { provider, commands, states, pending: None, stop, _task: task }),
                Err(e) => makepad_widgets::log!("aihub: {} usage worker did not start: {e}", provider.label()),
            }
        }
        UsageWorker { links }
    }

    pub fn reconnect(&mut self, provider: Provider) {
        self.send(provider, Command::Reconnect);
    }

    /// Spend one of `account`'s free resets of its limits (the account the person confirmed
    /// for: nothing is spent if another is signed in by then).
    pub fn reset_limits(&mut self, provider: Provider, account: String) {
        self.send(provider, Command::ResetLimits(account));
    }

    fn send(&mut self, provider: Provider, command: Command) {
        if let Some(link) = self.links.iter_mut().find(|l| l.provider == provider) {
            link.pending = Some(command);
            link.retry();
        }
    }

    /// The newest state of each tool that changed since the last poll.
    pub fn poll(&mut self) -> Vec<Arc<ProviderState>> {
        let mut out = Vec::new();
        for link in &mut self.links {
            link.retry();
            let mut latest = None;
            while let Ok(state) = link.states.try_recv() {
                latest = Some(state);
            }
            out.extend(latest);
        }
        out
    }
}

/// A tool's reader, running its CLI in `cwd` (the hub's own folder).
fn reader(provider: Provider, cwd: &Path) -> Option<Box<dyn Reader>> {
    match provider {
        Provider::Claude => Some(Box::new(crate::usage_claude::ClaudeReader::new(cwd.to_path_buf()))),
        Provider::Codex => Some(Box::new(crate::usage_codex::CodexReader::new(cwd.to_path_buf()))),
    }
}

impl Link {
    fn retry(&mut self) {
        if let Some(command) = self.pending.take() {
            match self.commands.try_send(command) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                Err(TrySendError::Full(command)) => self.pending = Some(command),
            }
        }
    }
}

impl Drop for UsageWorker {
    /// Each worker ends its CLI (gracefully first) and returns.
    fn drop(&mut self) {
        for link in &self.links {
            link.stop.store(true, Ordering::Relaxed);
        }
    }
}

fn publish(tx: &SyncSender<Arc<ProviderState>>, pending: &mut Option<Arc<ProviderState>>) -> bool {
    if let Some(state) = pending.take() {
        match tx.try_send(state) {
            Ok(()) => SignalToUI::set_ui_signal(),
            Err(TrySendError::Full(state)) => *pending = Some(state),
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
    true
}

fn run(provider: Provider, mut reader: Box<dyn Reader>, history: PathBuf, commands: Receiver<Command>, tx: SyncSender<Arc<ProviderState>>, stop: Arc<AtomicBool>) {
    let mut history = crate::usage_history::History::open(Some(history), provider);
    let mut state = ProviderState { provider, accounts: Arc::new(history.accounts.clone()), history_error: history.error.clone(), ..Default::default() };
    let mut due = Instant::now();
    let mut pending = None;
    while !stop.load(Ordering::Relaxed) {
        if !publish(&tx, &mut pending) {
            break;
        }
        if Instant::now() >= due {
            if state.busy == Busy::Idle {
                state.busy = Busy::Reading;
            }
            pending = Some(Arc::new(state.clone()));
            if !publish(&tx, &mut pending) {
                break;
            }
            let reading = reader.read(&stop);
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let at = now();
            history.observe(&reading, at);
            state.accounts = Arc::new(history.accounts.clone());
            state.history_error = history.error.clone();
            state.record(reading, at);
            state.busy = Busy::Idle;
            state.pid = reader.pid();
            due = Instant::now() + Duration::from_secs(POLL_SECONDS);
            state.next_at = at + POLL_SECONDS;
            pending = Some(Arc::new(state.clone()));
            continue;
        }
        match commands.recv_timeout(Duration::from_millis(100)) {
            Ok(Command::Reconnect) => {
                state.busy = Busy::Reconnecting;
                pending = Some(Arc::new(state.clone()));
                if !publish(&tx, &mut pending) {
                    break;
                }
                reader.reconnect();
                state.pid = None;
                due = Instant::now();
            }
            Ok(Command::ResetLimits(account)) => {
                state.busy = Busy::Resetting;
                pending = Some(Arc::new(state.clone()));
                if !publish(&tx, &mut pending) {
                    break;
                }
                let outcome = reader.reset_limits(&account, &stop);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                state.reset = Some(ResetNote { at: now(), account, outcome });
                state.busy = Busy::Idle;
                state.pid = reader.pid();
                // read again at once: the refreshed limits are the answer
                due = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // the reader's drop ends its CLI
    drop(reader);
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// The hub's own empty folder the CLIs run in (never a project of the person's).
pub fn private_dir(dir: &Path) -> Result<PathBuf, String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir.to_path_buf())
}

/// A valid account email (bounded, no whitespace or control characters, one `@`).
pub fn account_email(value: Option<&str>) -> Option<String> {
    let email = value?.trim();
    if email.len() > 320 || email.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    (!local.is_empty() && !domain.is_empty() && !domain.contains('@')).then(|| email.to_owned())
}

/// A short plan name from a CLI (`max_20x`, `pro`): bounded, printable.
pub fn plan_name(value: Option<&str>) -> Option<String> {
    let plan = value?.trim();
    (!plan.is_empty() && plan.len() <= 40 && !plan.chars().any(char::is_control)).then(|| plan.to_owned())
}

/// `max_20x` as the view shows it: `Max 20x`.
pub fn plan_label(plan: &str) -> String {
    let text = plan.replace('_', " ");
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn truncate(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Keep the windows a reading may hold, each name once.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub fn push_window(windows: &mut Vec<UsageWindow>, window: UsageWindow) {
    windows.retain(|w| w.name != window.name);
    if windows.len() < MAX_WINDOWS {
        windows.push(window);
    }
}

/// An ISO-8601 time with an explicit UTC offset, as Unix seconds (the tests write their
/// times so).
#[cfg(test)]
pub fn parse_reset_at(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let number = |start: usize, end: usize| {
        let digits = &b[start..end];
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse::<i64>().ok()
    };
    let (year, month, day, hour, minute, second) = (number(0, 4)?, number(5, 7)?, number(8, 10)?, number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=makepad_civil_time::days_in_month(year as i32, month as u32) as i64).contains(&day)
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
            let (h, m) = (number(i + 1, i + 3)?, number(i + 4, i + 6)?);
            if h > 23 || m > 59 {
                return None;
            }
            (h * 3600 + m * 60) * if *sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    let days = makepad_civil_time::from_ymd(year as i32, month as u32, day as u32) as i64;
    u64::try_from(days * 86400 + hour * 3600 + minute * 60 + second - offset).ok()
}

/// This machine's zone name (`Europe/Amsterdam`): `TZ`, else where `/etc/localtime` points.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn local_zone_name() -> Option<String> {
    if let Some(tz) = std::env::var_os("TZ").and_then(|t| t.into_string().ok()).filter(|t| !t.is_empty()) {
        return Some(tz.trim_start_matches(':').to_string());
    }
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let text = target.to_string_lossy();
    text.split_once("zoneinfo/").map(|(_, zone)| zone.to_string())
}

/// Claude's local reset text (`Resets Nov 23 at 5pm (Europe/Amsterdam)`, `Resets 2:50am
/// (Europe/Amsterdam)`) as Unix seconds. The CLI runs as this process's child, so its clock is
/// this machine's zone: a text naming another zone (other than UTC) stays unknown. A text
/// without a year is the next such moment after the read (the CLI shows a reset still to come).
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub fn local_reset_at(text: &str, observed_at: u64, local_zone: Option<&str>) -> Option<u64> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let text = if lower.starts_with("resets ") { &text[7..] } else if lower.starts_with("reset ") { &text[6..] } else { text };
    let (text, zone) = match text.rsplit_once(" (") {
        Some((text, zone)) if zone.ends_with(')') => (text.trim(), Some(&zone[..zone.len() - 1])),
        _ => (text.trim(), None),
    };
    let utc = matches!(zone, Some("UTC" | "Etc/UTC" | "GMT"));
    if let (Some(zone), Some(local)) = (zone, local_zone) {
        if !utc && zone != local {
            return None;
        }
    }
    let (date, clock) = match text.split_once(" at ") {
        Some((date, clock)) => (Some(date.trim()), clock.trim()),
        None => (None, text),
    };
    let clock_lower = clock.to_ascii_lowercase();
    let (digits, pm) = if let Some(d) = clock_lower.strip_suffix("am") {
        (d.trim(), Some(false))
    } else if let Some(d) = clock_lower.strip_suffix("pm") {
        (d.trim(), Some(true))
    } else {
        (clock_lower.trim(), None)
    };
    let (h, m) = match digits.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        Some(_) => return None,
        None if pm.is_some() => (digits.parse::<u32>().ok()?, 0),
        None => return None,
    };
    let hour = match pm {
        Some(pm) if (1..=12).contains(&h) => h % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if h < 24 => h,
        None => return None,
    };
    if m > 59 {
        return None;
    }
    let seconds = (hour * 3600 + m * 60) as i64;
    let wall = |secs: i64| if utc { makepad_civil_time::utc(secs) } else { makepad_civil_time::local_or_utc(secs) };
    let observed = observed_at as i64;
    let here = wall(observed);
    // the instant a wall-clock day and time is, by the zone's offset at that moment
    let instant = |day: i64| {
        let naive = day * 86400 + seconds;
        let guess = naive - wall(naive).offset as i64;
        naive - wall(guess).offset as i64
    };
    let candidates: Vec<i64> = match date {
        Some(date) => {
            let mut parts = date.split_whitespace();
            let month = parts.next()?;
            let day = parts.next()?.trim_end_matches(',').parse::<u32>().ok()?;
            if parts.next().is_some() {
                return None;
            }
            let month = (1..=12u32).find(|m| makepad_civil_time::month_name(*m).get(..3).is_some_and(|n| n.eq_ignore_ascii_case(month.get(..3).unwrap_or(""))))?;
            (here.year - 1..=here.year + 1)
                .filter(|y| day >= 1 && day <= makepad_civil_time::days_in_month(*y, month))
                .map(|y| instant(makepad_civil_time::from_ymd(y, month, day) as i64))
                .collect()
        }
        None => {
            let today = here.day_number() as i64;
            vec![instant(today), instant(today + 1)]
        }
    };
    // a reset still to come when read (an hour's slack: the CLI rounds 5:59pm to 6pm)
    candidates.into_iter().filter(|t| *t >= observed - 3600).min().and_then(|t| u64::try_from(t).ok())
}

/// Strip terminal escapes; `\r` becomes a line break.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub fn strip_ansi(text: &str) -> String {
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

/// A countdown: `2 d 14 h`, `2 h 13 min`, `5 min`.
pub fn countdown(secs: u64) -> String {
    let m = secs.div_ceil(60);
    if m < 60 {
        format!("{m} min")
    } else if m < 24 * 60 {
        format!("{} h {} min", m / 60, m % 60)
    } else {
        format!("{} d {} h", m / 1440, (m % 1440) / 60)
    }
}

/// A moment as a date on the Mac's clock reads: day, month and time (`23 Nov 17:00`), with the
/// year when it is not this year's. Never a weekday or a bare time, so a reset reads as its real
/// date; ` UTC` when this machine's zone is unknown.
pub fn clock_label(at: u64, now: u64) -> String {
    let t = makepad_civil_time::local_or_utc(at as i64);
    let zone = if t.zoned { "" } else { " UTC" };
    let month = makepad_civil_time::month_name(t.month).get(..3).unwrap_or("");
    if t.year == makepad_civil_time::local_or_utc(now as i64).year {
        format!("{} {month} {}{zone}", t.day, t.hm())
    } else {
        format!("{} {month} {} {}{zone}", t.day, t.year, t.hm())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_plan_fields_are_bounded() {
        assert_eq!(account_email(Some(" user@example.com ")).as_deref(), Some("user@example.com"));
        for bad in ["@example.com", "user@", "a b@example.com", "user@example.com@x", "user@exa\nmple.com"] {
            assert_eq!(account_email(Some(bad)), None, "{bad}");
        }
        assert_eq!(plan_label("max_20x"), "Max 20x");
        assert_eq!(plan_name(Some(&"x".repeat(41))), None);
    }

    /// Claude's `/usage` gives local clock texts without a year: they resolve to the next such
    /// moment on this machine's clock, and another zone's text is never guessed.
    #[test]
    fn local_reset_texts_resolve_on_the_machines_clock_and_never_in_another_zone() {
        // in UTC, so the test holds on any box
        let observed = parse_reset_at("2026-11-17T10:30:00Z").unwrap();
        let at = |t: &str| local_reset_at(t, observed, Some("Etc/UTC"));
        assert_eq!(at("Resets Nov 23 at 5pm (UTC)"), parse_reset_at("2026-11-23T17:00:00Z"));
        assert_eq!(at("Resets 2:50am (UTC)"), parse_reset_at("2026-11-18T02:50:00Z"), "the next 2:50am");
        assert_eq!(at("Resets 11:59am (UTC)"), parse_reset_at("2026-11-17T11:59:00Z"), "later today");
        // a year turn: Jan 2 read on Dec 30 is next year's
        let december = parse_reset_at("2026-12-30T12:00:00Z").unwrap();
        assert_eq!(local_reset_at("Resets Jan 2 at 1am (UTC)", december, None), parse_reset_at("2027-01-02T01:00:00Z"));
        // another zone than this machine's, or a malformed clock: unknown
        assert_eq!(local_reset_at("Resets 7pm (America/New_York)", observed, Some("Europe/Amsterdam")), None);
        for bad in ["Resets soon", "Resets 13pm (UTC)", "Resets 7:5pm (UTC)", "Resets Foo 3 at 1am (UTC)"] {
            assert_eq!(at(bad), None, "{bad}");
        }
        assert_eq!(parse_reset_at("2026-09-06T19:40:00+02:00"), parse_reset_at("2026-09-06T17:40:00Z"));
        assert_eq!(parse_reset_at("2025-02-29T12:00:00Z"), None);
        assert_eq!(countdown(13 * 60 + 2 * 3600), "2 h 13 min");
        assert_eq!(countdown(2 * 86400 + 14 * 3600 + 5), "2 d 14 h");
    }

    /// A reset reads as its real date and time on this machine's clock (`23 Nov 17:00`): later
    /// today and later this week too, never a weekday or a bare time; another year names it.
    #[test]
    fn a_time_reads_as_its_date_never_a_weekday() {
        let now = parse_reset_at("2026-11-17T10:30:00Z").unwrap();
        let date = |at: u64, year: bool| {
            let t = makepad_civil_time::local_or_utc(at as i64);
            let month = makepad_civil_time::month_name(t.month).get(..3).unwrap().to_string();
            let zone = if t.zoned { "" } else { " UTC" };
            if year { format!("{} {month} {} {}{zone}", t.day, t.year, t.hm()) } else { format!("{} {month} {}{zone}", t.day, t.hm()) }
        };
        for (at, why) in [(now + 3600, "later today"), (now + 3 * 86400, "in three days"), (now + 6 * 86400 + 600, "in six days")] {
            let label = clock_label(at, now);
            assert_eq!(label, date(at, false), "{why}");
            assert!(!["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().any(|d| label.contains(d)), "{why}: {label}");
        }
        let next_year = parse_reset_at("2027-01-02T12:00:00Z").unwrap();
        assert_eq!(clock_label(next_year, parse_reset_at("2026-12-30T12:00:00Z").unwrap()), date(next_year, true), "another year names it");
        if !makepad_civil_time::local_or_utc(0).zoned {
            assert_eq!(clock_label(parse_reset_at("2026-11-23T17:00:00Z").unwrap(), now), "23 Nov 17:00 UTC");
        }
    }

    /// A failed read keeps the last good numbers only for the same account.
    #[test]
    fn an_old_reading_is_never_shown_for_another_account() {
        let ok = |account: &str| Reading {
            account: Some(account.into()),
            windows: vec![UsageWindow { name: "7-day".into(), limit: Some(Limit::Week), used_percent: Some(40.0), ..Default::default() }],
            ..Default::default()
        };
        let mut s = ProviderState::default();
        s.record(ok("user@example.com"), 100);
        s.record(Reading::failed(Some("user@example.com".into()), "timed out"), 700);
        let (at, shown) = s.shown().expect("the same account keeps its last numbers");
        assert_eq!((at, shown.window(Limit::Week).unwrap().used_percent), (100, Some(40.0)));
        s.record(Reading::failed(Some("other@example.com".into()), "timed out"), 1300);
        assert!(s.shown().is_none(), "another account never shows the old numbers");
        s.record(ok("other@example.com"), 1900);
        assert_eq!(s.shown().unwrap().0, 1900);
    }
}
