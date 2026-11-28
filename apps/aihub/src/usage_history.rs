//! Every account a tool's worker has seen signed in, with its readings over time, kept in
//! `usage-<tool>.json` next to the app's settings (ported from Scope's account history). Only
//! the email (Antigravity's CLI names none: its one account is [`LOCAL_ACCOUNT`]), the plan, the
//! limits' used shares and reset times, and when they were read go in it: no CLI output, no
//! credential. The worker owns the file (one per tool, so no two writers); it is the person's
//! own (mode 0600). Every account stays, however many; a long history, or
//! one that would outgrow its file, thins older readings rather than stopping or dropping one.

use crate::usage::{account_email, plan_name, truncate, Limit, Provider, Reading, MAX_UNIX};
use makepad_strict_json::Value;
use std::path::{Path, PathBuf};

/// History key for a provider whose CLI exposes no read-only account email (`agy`).
pub const LOCAL_ACCOUNT: &str = "local";

/// Readings an account keeps; past it the older half keeps every other one.
const SAMPLES: usize = 256;
/// The file's size; a history that would outgrow it thins every account's older readings
/// (every account stays).
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// One limit as read: its used share and when it resets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampleWindow {
    pub used: Option<f64>,
    pub reset_at: Option<u64>,
    /// The CLI's reset text, kept only when it gave no time.
    pub reset_text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub at: u64,
    pub session: Option<SampleWindow>,
    pub week: Option<SampleWindow>,
    pub fable: Option<SampleWindow>,
    pub neon: Option<SampleWindow>,
}

impl Sample {
    pub fn window(&self, limit: Limit) -> Option<&SampleWindow> {
        match limit {
            Limit::Session => self.session.as_ref(),
            Limit::Week => self.week.as_ref(),
            Limit::Fable => self.fable.as_ref(),
            Limit::Neon => self.neon.as_ref(),
        }
    }

    fn same_values(&self, other: &Sample) -> bool {
        self.session == other.session && self.week == other.week && self.fable == other.fable && self.neon == other.neon
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountHistory {
    pub email: String,
    pub plan: Option<String>,
    pub first_seen_at: u64,
    pub last_checked_at: u64,
    pub last_check_succeeded: bool,
    /// Free full resets of the limits it had at its last reading (Codex).
    pub reset_credits: Option<u32>,
    /// Oldest first. A stretch of equal readings keeps its first and its last.
    pub samples: Vec<Sample>,
}

impl AccountHistory {
    pub fn latest(&self) -> Option<&Sample> {
        self.samples.last()
    }
}

pub struct History {
    path: Option<PathBuf>,
    provider: Provider,
    pub accounts: Vec<AccountHistory>,
    pub error: Option<String>,
    /// A file that did not load is never written over.
    load_failed: bool,
}

fn history_account(provider: Provider, account: Option<&str>) -> Option<String> {
    account_email(account).or_else(|| (provider == Provider::Agy).then(|| LOCAL_ACCOUNT.to_owned()))
}

impl History {
    pub fn path(dir: &Path, provider: Provider) -> PathBuf {
        dir.join(format!("usage-{}.json", provider.key()))
    }

    pub fn open(path: Option<PathBuf>, provider: Provider) -> History {
        let mut history = History { path, provider, accounts: Vec::new(), error: None, load_failed: false };
        if let Some(path) = &history.path {
            match read_bounded(path).and_then(|bytes| bytes.map(|b| decode(&b, provider)).transpose()) {
                Ok(accounts) => history.accounts = accounts.unwrap_or_default(),
                Err(e) => {
                    history.error = Some(format!("The saved account history did not load: {e}"));
                    history.load_failed = true;
                }
            }
        }
        history
    }

    /// One read: the account's history gains it (a failure only notes the check).
    pub fn observe(&mut self, reading: &Reading, at: u64) {
        let Some(email) = history_account(self.provider, reading.account.as_deref()) else { return };
        let window = |limit: Limit| {
            reading.window(limit).map(|w| SampleWindow { used: w.used_percent, reset_at: w.reset_at, reset_text: w.reset_at.is_none().then(|| w.reset_text.as_deref().map(|t| truncate(t, 160))).flatten() })
        };
        let sample = Sample { at, session: window(Limit::Session), week: window(Limit::Week), fable: window(Limit::Fable), neon: window(Limit::Neon) };
        let success = reading.error.is_none() && (sample.session.is_some() || sample.week.is_some() || sample.fable.is_some() || sample.neon.is_some());
        if !self.accounts.iter().any(|a| a.email == email) {
            if email == LOCAL_ACCOUNT && !success {
                return;
            }
            self.accounts.push(AccountHistory { email: email.clone(), first_seen_at: at, ..Default::default() });
        }
        let account = self.accounts.iter_mut().find(|a| a.email == email).expect("added above");
        account.last_checked_at = at;
        account.last_check_succeeded = success;
        if success {
            if let Some(plan) = plan_name(reading.plan.as_deref()) {
                account.plan = Some(plan);
            }
            account.reset_credits = reading.reset_credits;
            push_sample(&mut account.samples, sample);
        }
        self.save();
    }

    fn save(&mut self) {
        if self.load_failed {
            return;
        }
        let Some(path) = &self.path else { return };
        let (text, thinned) = encode_within(&mut self.accounts, self.provider, MAX_BYTES);
        if thinned {
            makepad_widgets::log!("aihub: {} usage history outgrew {MAX_BYTES} bytes: every account keeps its newest readings, its older ones thinned", self.provider.label());
        }
        self.error = write_private(path, text.as_bytes()).err().map(|e| format!("The account history was not saved: {e}"));
    }
}

/// The history as written, within `max` bytes: while it is over, every account's older half
/// keeps every other reading (no account goes, each keeps its newest); true when it thinned.
fn encode_within(accounts: &mut [AccountHistory], provider: Provider, max: usize) -> (String, bool) {
    let mut thinned = false;
    loop {
        let text = encode(accounts, provider);
        let before: usize = accounts.iter().map(|a| a.samples.len()).sum();
        if text.len() <= max {
            return (text, thinned);
        }
        accounts.iter_mut().for_each(|a| thin_older_half(&mut a.samples));
        if accounts.iter().map(|a| a.samples.len()).sum::<usize>() == before {
            // nothing left to thin: the write says it did not fit
            return (text, thinned);
        }
        thinned = true;
    }
}

/// The older half of the readings keeps every other one (the oldest stays); the newer half
/// stays whole.
fn thin_older_half(samples: &mut Vec<Sample>) {
    let half = samples.len() / 2;
    let mut i = 0;
    samples.retain(|_| {
        i += 1;
        i > half || i % 2 == 1
    });
}

/// Keeps a stretch of equal readings as its first and last; a full history thins its older
/// half to every other reading, so it covers more time at less detail instead of stopping.
fn push_sample(samples: &mut Vec<Sample>, sample: Sample) {
    let n = samples.len();
    if n >= 2 && samples[n - 1].same_values(&sample) && samples[n - 2].same_values(&sample) {
        samples[n - 1].at = sample.at;
        return;
    }
    samples.push(sample);
    if samples.len() > SAMPLES {
        thin_older_half(samples);
    }
}

fn window_json(w: &Option<SampleWindow>) -> Value {
    let Some(w) = w else { return Value::Null };
    let mut fields = vec![("used", w.used.map(Value::F64).unwrap_or(Value::Null)), ("reset_at", w.reset_at.map(|t| Value::Int(t as i64)).unwrap_or(Value::Null))];
    if let Some(text) = &w.reset_text {
        fields.push(("reset_text", makepad_strict_json::s(text)));
    }
    makepad_strict_json::obj(fields)
}

fn encode(accounts: &[AccountHistory], provider: Provider) -> String {
    let opt = |v: Option<u64>| v.map(|v| Value::Int(v as i64)).unwrap_or(Value::Null);
    let accounts = accounts
        .iter()
        .map(|a| {
            makepad_strict_json::obj(vec![
                ("email", makepad_strict_json::s(&a.email)),
                ("plan", a.plan.as_ref().map(makepad_strict_json::s).unwrap_or(Value::Null)),
                ("first_seen_at", Value::Int(a.first_seen_at as i64)),
                ("last_checked_at", Value::Int(a.last_checked_at as i64)),
                ("last_check_succeeded", Value::Bool(a.last_check_succeeded)),
                ("reset_credits", opt(a.reset_credits.map(u64::from))),
                (
                    "samples",
                    Value::Arr(a.samples.iter().map(|s| {
                        let mut fields = vec![("at", Value::Int(s.at as i64)), ("session", window_json(&s.session)), ("week", window_json(&s.week)), ("fable", window_json(&s.fable))];
                        if s.neon.is_some() {
                            fields.push(("neon", window_json(&s.neon)));
                        }
                        makepad_strict_json::obj(fields)
                    }).collect()),
                ),
            ])
        })
        .collect();
    makepad_strict_json::obj(vec![("version", Value::Int(1)), ("provider", makepad_strict_json::s(provider.key())), ("accounts", Value::Arr(accounts))]).to_json()
}

fn time(value: &Value, key: &str) -> Result<u64, String> {
    value.get(key).and_then(Value::as_u64).filter(|t| *t <= MAX_UNIX).ok_or_else(|| format!("invalid {key}"))
}

fn decode_window(value: Option<&Value>) -> Result<Option<SampleWindow>, String> {
    let Some(value) = value.filter(|v| !v.is_null()) else { return Ok(None) };
    let used = match value.get("used") {
        None | Some(Value::Null) => None,
        Some(Value::Int(n)) if *n >= 0 => Some(*n as f64),
        Some(Value::F64(n)) if n.is_finite() && *n >= 0.0 => Some(*n),
        _ => return Err("invalid used share".into()),
    };
    let reset_at = match value.get("reset_at") {
        None | Some(Value::Null) => None,
        Some(_) => Some(time(value, "reset_at")?),
    };
    let reset_text = match value.get("reset_text").and_then(Value::as_str) {
        Some(t) if t.len() <= 160 && !t.chars().any(char::is_control) => Some(t.to_string()),
        Some(_) => return Err("invalid reset text".into()),
        None => None,
    };
    Ok(Some(SampleWindow { used, reset_at, reset_text }))
}

fn decode(bytes: &[u8], provider: Provider) -> Result<Vec<AccountHistory>, String> {
    let value = makepad_strict_json::parse_depth(bytes, 12).map_err(str::to_owned)?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported version".into());
    }
    if value.get("provider").and_then(Value::as_str) != Some(provider.key()) {
        return Err("another tool's history".into());
    }
    let rows = value.get("accounts").and_then(Value::as_arr).ok_or("invalid account list")?;
    let mut accounts: Vec<AccountHistory> = Vec::new();
    for row in rows {
        let raw = row.get("email").and_then(Value::as_str);
        let email = if provider == Provider::Agy && raw == Some(LOCAL_ACCOUNT) {
            LOCAL_ACCOUNT.to_owned()
        } else {
            account_email(raw).ok_or("invalid email")?
        };
        if accounts.iter().any(|a| a.email == email) {
            return Err("an account twice".into());
        }
        let samples = row.get("samples").and_then(Value::as_arr).filter(|s| s.len() <= SAMPLES).ok_or("invalid samples")?;
        let samples = samples
            .iter()
            .map(|s| Ok(Sample {
                at: time(s, "at")?,
                session: decode_window(s.get("session"))?,
                week: decode_window(s.get("week"))?,
                fable: decode_window(s.get("fable"))?,
                neon: decode_window(s.get("neon"))?,
            }))
            .collect::<Result<Vec<_>, String>>()?;
        accounts.push(AccountHistory {
            email,
            plan: match row.get("plan") {
                None | Some(Value::Null) => None,
                Some(v) => Some(plan_name(v.as_str()).ok_or("invalid plan")?),
            },
            first_seen_at: time(row, "first_seen_at")?,
            last_checked_at: time(row, "last_checked_at")?,
            last_check_succeeded: row.get("last_check_succeeded").and_then(Value::as_bool).ok_or("invalid check state")?,
            reset_credits: row.get("reset_credits").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()),
            samples,
        });
    }
    Ok(accounts)
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    private_file(&metadata)?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    private_file(&file.metadata().map_err(|e| e.to_string())?)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("over 4 MiB".into());
    }
    Ok(Some(bytes))
}

/// A regular file of the person's own, readable by nobody else.
fn private_file(metadata: &std::fs::Metadata) -> Result<(), String> {
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err("not a bounded regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        extern "C" {
            fn geteuid() -> u32;
        }
        if metadata.uid() != unsafe { geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err("not the person's own file with mode 0600".into());
        }
    }
    Ok(())
}

/// Written whole through a new file renamed over the old one, mode 0600.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    if bytes.len() > MAX_BYTES {
        return Err("over 4 MiB".into());
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    crate::usage::private_dir(parent)?;
    match std::fs::symlink_metadata(path) {
        Ok(m) => private_file(&m)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    let temporary = path.with_extension(format!("json.{}.new", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::UsageWindow;

    fn reading(account: &str, week: f64, fable: f64) -> Reading {
        Reading {
            account: Some(account.into()),
            plan: Some("max_20x".into()),
            windows: vec![
                UsageWindow { name: "7-day".into(), limit: Some(Limit::Week), used_percent: Some(week), reset_at: Some(1_800_000_000), reset_text: None },
                UsageWindow { name: "7-day Fable".into(), limit: Some(Limit::Fable), used_percent: Some(fable), reset_at: Some(1_800_000_000), reset_text: None },
            ],
            ..Default::default()
        }
    }

    /// The accounts seen build up and survive a restart; a failed read only notes the check.
    #[test]
    fn accounts_build_up_persist_and_failures_add_no_reading() {
        let dir = std::env::temp_dir().join(format!("aihub-usage-history-{}-{}", std::process::id(), crate::usage::now()));
        let path = History::path(&dir, Provider::Claude);
        let mut h = History::open(Some(path.clone()), Provider::Claude);
        h.observe(&reading("user@example.com", 10.0, 2.0), 100);
        h.observe(&reading("user@example.com", 30.0, 5.0), 700);
        h.observe(&reading("other@example.com", 90.0, 40.0), 1300);
        h.observe(&Reading::failed(Some("user@example.com".into()), "timed out"), 1900);
        h.observe(&Reading::failed(None, "signed out"), 2500);
        assert_eq!(h.error, None);
        let again = History::open(Some(path.clone()), Provider::Claude);
        assert_eq!(again.accounts, h.accounts, "a restart keeps every account and reading");
        let user = again.accounts.iter().find(|a| a.email == "user@example.com").unwrap();
        assert_eq!(user.samples.iter().map(|s| s.week.as_ref().unwrap().used).collect::<Vec<_>>(), vec![Some(10.0), Some(30.0)]);
        assert!(!user.last_check_succeeded && user.last_checked_at == 1900, "the failed check is noted, its numbers are not");
        assert_eq!(user.plan.as_deref(), Some("max_20x"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "the person's own");
        }
        assert!(History::open(Some(path.clone()), Provider::Codex).error.is_some(), "another tool's file is not taken");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn agy_local_account_persists_neon_and_skips_ghost_failures() {
        let dir = std::env::temp_dir().join(format!("aihub-usage-agy-{}-{}", std::process::id(), crate::usage::now()));
        let path = History::path(&dir, Provider::Agy);
        assert_eq!(path, dir.join("usage-agy.json"), "Antigravity has its own history file");
        let mut h = History::open(Some(path.clone()), Provider::Agy);
        let failure = Reading::failed(None, "timed out");
        h.observe(&failure, 100);
        assert!(h.accounts.is_empty() && !path.exists(), "a first failed read creates neither an account nor a file");

        let good = Reading {
            plan: Some("Google AI Ultra".into()),
            windows: vec![UsageWindow { limit: Some(Limit::Neon), used_percent: Some(35.0), reset_at: Some(3600), ..Default::default() }],
            ..Default::default()
        };
        h.observe(&good, 700);
        assert_eq!(h.accounts.len(), 1, "a successful anonymous read creates one account");
        let account = &h.accounts[0];
        assert_eq!((account.email.as_str(), account.plan.as_deref()), (LOCAL_ACCOUNT, Some("Google AI Ultra")), "the local account keeps its plan");
        assert!(account.last_check_succeeded && account.first_seen_at == 700, "a Neon-only reading counts as the first success");
        let samples = account.samples.clone();
        assert_eq!(samples, [Sample { at: 700, neon: Some(SampleWindow { used: Some(35.0), reset_at: Some(3600), reset_text: None }), ..Default::default() }], "the successful read keeps its Neon window");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "usage-agy.json is private");
        }
        let mut again = History::open(Some(path), Provider::Agy);
        assert_eq!(again.accounts, h.accounts, "the local account and its Neon window survive a restart");
        again.observe(&failure, 1300);
        let account = &again.accounts[0];
        assert!(!account.last_check_succeeded && account.last_checked_at == 1300, "a later failure updates the check time and outcome");
        assert_eq!(account.samples, samples, "a failed read preserves the saved readings");
        assert_eq!(History::open(again.path.clone(), Provider::Agy).accounts, again.accounts, "the failed check survives a restart");
        std::fs::remove_dir_all(dir).unwrap();

        for provider in [Provider::Claude, Provider::Codex] {
            assert_eq!(decode(encode(&h.accounts, provider).as_bytes(), provider), Err("invalid email".into()), "{provider:?} requires a saved email");
            let mut legacy = History::open(None, provider);
            legacy.observe(&reading("user@example.com", 10.0, 2.0), 100);
            let encoded = encode(&legacy.accounts, provider);
            assert!(!encoded.contains("\"neon\""), "{provider:?} keeps the original sample fields when Neon is absent");
        }
        h.accounts[0].email = "not-an-email".into();
        assert_eq!(decode(encode(&h.accounts, Provider::Agy).as_bytes(), Provider::Agy), Err("invalid email".into()), "only the exact local key is accepted without an email");
    }

    /// Every account stays, however many (a 33rd pushes none out); a history that would outgrow
    /// its file thins every account's older readings instead, each keeping its newest.
    #[test]
    fn every_account_stays_and_a_full_file_thins_readings_instead() {
        let mut h = History::open(None, Provider::Claude);
        for i in 0..40u64 {
            let email = format!("user{i}@example.com");
            for t in 0..12u64 {
                h.observe(&reading(&email, (i + t) as f64, t as f64), 10_000 * i + 600 * t + 1);
            }
        }
        assert_eq!(h.accounts.len(), 40, "every account seen stays");
        let dir = std::env::temp_dir().join(format!("aihub-usage-accounts-{}-{}", std::process::id(), crate::usage::now()));
        h.path = Some(History::path(&dir, Provider::Claude));
        h.save();
        assert_eq!(h.error, None);
        assert_eq!(History::open(h.path.clone(), Provider::Claude).accounts, h.accounts, "all 40 load again");
        std::fs::remove_dir_all(&dir).unwrap();
        let newest: Vec<Sample> = h.accounts.iter().map(|a| a.latest().unwrap().clone()).collect();
        let full = encode(&h.accounts, Provider::Claude).len();
        let (text, thinned) = encode_within(&mut h.accounts, Provider::Claude, full / 2);
        assert!(thinned && text.len() <= full / 2, "a history over its size thins to fit");
        assert_eq!(h.accounts.len(), 40, "thinning drops no account");
        assert_eq!(h.accounts.iter().map(|a| a.latest().unwrap().clone()).collect::<Vec<_>>(), newest, "each keeps its newest reading");
        assert!(h.accounts.iter().all(|a| a.samples.len() < 12 && a.samples.len() >= 2));
    }

    /// Equal readings keep their stretch's first and last; a full history thins its older half
    /// and keeps going.
    #[test]
    fn a_long_history_thins_instead_of_stopping() {
        let mut samples = Vec::new();
        let at = |t: u64, used: f64| Sample { at: t, week: Some(SampleWindow { used: Some(used), reset_at: None, reset_text: None }), ..Default::default() };
        for t in 0..5 {
            push_sample(&mut samples, at(t, 10.0));
        }
        assert_eq!(samples.iter().map(|s| s.at).collect::<Vec<_>>(), vec![0, 4], "a flat stretch is its two ends");
        for t in 5..(5 + 3 * SAMPLES as u64) {
            push_sample(&mut samples, at(t, t as f64));
        }
        assert!(samples.len() <= SAMPLES && samples.len() > SAMPLES / 2);
        assert_eq!(samples.first().unwrap().at, 0, "the oldest reading stays");
        assert_eq!(samples.last().unwrap().at, 4 + 3 * SAMPLES as u64, "the newest is there");
    }
}
