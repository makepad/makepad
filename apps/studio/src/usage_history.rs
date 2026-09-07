// Worker-owned, bounded account metadata. No raw CLI text or credentials enter this file.
const USAGE_HISTORY_ACCOUNTS: usize = 32;
const USAGE_HISTORY_SAMPLES: usize = 32;
const USAGE_HISTORY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct AccountUsageSample {
    pub observed_at: u64,
    pub source: String,
    pub plan: Option<String>,
    pub session: Option<UsageWindow>,
    pub week: Option<UsageWindow>,
}
impl AccountUsageSample {
    pub fn json(&self) -> Value {
        makepad_strict_json::obj(vec![
            ("observed_at", Value::Int(self.observed_at as i64)),
            ("source", makepad_strict_json::s(&self.source)),
            ("plan", self.plan.as_ref().map(makepad_strict_json::s).unwrap_or(Value::Null)),
            ("session", self.session.as_ref().map(usage_history_window_json).unwrap_or(Value::Null)),
            ("week", self.week.as_ref().map(usage_history_window_json).unwrap_or(Value::Null)),
        ])
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct AccountUsageHistory {
    pub provider: UsageProvider,
    pub email: String,
    pub first_seen_at: u64,
    pub last_seen_at: u64,
    pub last_checked_at: u64,
    pub last_check_succeeded: bool,
    /// Oldest to newest distinct quota/reset values. Repeated values refresh
    /// the last sample's observation timestamp without adding another row.
    pub samples: Vec<AccountUsageSample>,
}
impl AccountUsageHistory {
    pub fn latest(&self) -> Option<&AccountUsageSample> { self.samples.last() }
    pub fn json(&self) -> Value {
        makepad_strict_json::obj(vec![
            ("provider", makepad_strict_json::s(self.provider.as_str())),
            ("email", makepad_strict_json::s(&self.email)),
            ("first_seen_at", Value::Int(self.first_seen_at as i64)),
            ("last_seen_at", Value::Int(self.last_seen_at as i64)),
            ("last_checked_at", Value::Int(self.last_checked_at as i64)),
            ("last_check_succeeded", Value::Bool(self.last_check_succeeded)),
            ("samples", Value::Arr(self.samples.iter().map(AccountUsageSample::json).collect())),
        ])
    }
}
fn usage_history_window_json(window: &UsageWindow) -> Value {
    makepad_strict_json::obj(vec![
        ("name", makepad_strict_json::s(&window.name)),
        ("used_percent", window.used_percent.map(Value::F64).unwrap_or(Value::Null)),
        ("remaining_percent", window.remaining_percent.map(Value::F64).unwrap_or(Value::Null)),
        ("reset_at", window.reset_at.map(|at| Value::Int(at as i64)).unwrap_or(Value::Null)),
        ("reset_text", window.reset_text.as_ref().map(makepad_strict_json::s).unwrap_or(Value::Null)),
    ])
}
struct UsageHistoryStore {
    path: Option<std::path::PathBuf>,
    accounts: Vec<AccountUsageHistory>,
    error: Option<String>,
    load_failed: bool,
}
impl UsageHistoryStore {
    fn open(path: Option<std::path::PathBuf>) -> Self {
        let mut store = Self { path, accounts: Vec::new(), error: None, load_failed: false };
        if let Some(path) = &store.path {
            match usage_history_read(path) {
                Ok(Some(bytes)) => match usage_history_decode(&bytes) {
                    Ok(accounts) => store.accounts = accounts,
                    Err(error) => { store.error = Some(format!("Saved usage history was not loaded: {error}")); store.load_failed = true; }
                },
                Ok(None) => {},
                Err(error) => { store.error = Some(format!("Saved usage history was not loaded: {error}")); store.load_failed = true; }
            }
        }
        store.sort();
        store
    }
    fn sort(&mut self) {
        self.accounts.sort_by(|a, b| b.last_checked_at.cmp(&a.last_checked_at)
            .then_with(|| a.provider.as_str().cmp(b.provider.as_str())).then_with(|| a.email.cmp(&b.email)));
    }
    fn observe(&mut self, incoming: &ProviderUsage, checked_at: u64) {
        let Some(email) = account_email(incoming.account_email.as_deref()) else { return; };
        let verified = incoming.error.is_none() && incoming.observed_at > 0
            && incoming.quota_account_email.as_deref() == Some(email.as_str());
        let [session, week] = incoming.limits();
        let session = (incoming.provider == UsageProvider::Claude).then_some(session).flatten().cloned();
        let week = week.cloned();
        let success = verified && (session.is_some() || week.is_some());
        if !self.accounts.iter().any(|account| account.provider == incoming.provider && account.email == email) {
            let count = self.accounts.iter().filter(|account| account.provider == incoming.provider).count();
            if count >= USAGE_HISTORY_ACCOUNTS {
                if let Some(index) = self.accounts.iter().rposition(|account| account.provider == incoming.provider) { self.accounts.remove(index); }
            }
            self.accounts.push(AccountUsageHistory {
                provider: incoming.provider, email: email.clone(), first_seen_at: checked_at,
                last_seen_at: checked_at, last_checked_at: checked_at, last_check_succeeded: false, samples: Vec::new(),
            });
        }
        let account = self.accounts.iter_mut().find(|account| account.provider == incoming.provider && account.email == email).unwrap();
        account.last_seen_at = checked_at;
        account.last_checked_at = checked_at;
        account.last_check_succeeded = success;
        if success {
            let sample = AccountUsageSample { observed_at: incoming.observed_at, source: truncate(&incoming.source, 160), plan: incoming.plan.as_ref().map(|plan| truncate(plan, 80)), session, week };
            if account.samples.last().is_some_and(|last| last.session == sample.session && last.week == sample.week) {
                *account.samples.last_mut().unwrap() = sample;
            } else {
                if account.samples.len() >= USAGE_HISTORY_SAMPLES { account.samples.remove(0); }
                account.samples.push(sample);
            }
        }
        self.sort();
        if self.load_failed { return; }
        if let Some(path) = &self.path {
            let value = makepad_strict_json::obj(vec![("version", Value::Int(1)), ("accounts", Value::Arr(self.accounts.iter().map(AccountUsageHistory::json).collect()))]);
            self.error = usage_history_write(path, value.to_json().as_bytes()).err().map(|error| format!("Usage history could not be saved: {error}"));
        }
    }
}
fn usage_history_text(value: &Value, key: &str, max: usize) -> Result<String, String> {
    let text = value.get(key).and_then(Value::as_str).ok_or_else(|| format!("Invalid usage history {key}"))?;
    if text.len() > max || text.chars().any(char::is_control) { return Err(format!("Invalid usage history {key}")); }
    Ok(text.into())
}
fn usage_history_time(value: &Value, key: &str) -> Result<u64, String> {
    value.get(key).and_then(Value::as_u64).filter(|at| *at <= 253_402_300_799).ok_or_else(|| format!("Invalid usage history {key}"))
}
fn usage_history_window(value: &Value, scope: &'static str) -> Result<Option<UsageWindow>, String> {
    if value.is_null() { return Ok(None); }
    let percent = |key: &str| -> Result<Option<f64>, String> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Int(value)) if *value >= 0 => Ok(Some(*value as f64)),
            Some(Value::F64(value)) if value.is_finite() && *value >= 0.0 => Ok(Some(*value)),
            _ => Err(format!("Invalid history {key}")),
        }
    };
    let reset_at = match value.get("reset_at") { None | Some(Value::Null) => None, Some(_) => Some(usage_history_time(value, "reset_at")?) };
    let reset_text = match value.get("reset_text") { None | Some(Value::Null) => None, Some(_) => Some(usage_history_text(value, "reset_text", 512)?) };
    Ok(Some(UsageWindow { name: usage_history_text(value, "name", 240)?, scope: Some(scope), used_percent: percent("used_percent")?, remaining_percent: percent("remaining_percent")?, reset_at, reset_text }))
}
fn usage_history_decode(bytes: &[u8]) -> Result<Vec<AccountUsageHistory>, String> {
    let value = makepad_strict_json::parse_depth(bytes, 12).map_err(str::to_owned)?;
    if value.get("version").and_then(Value::as_u64) != Some(1) { return Err("Unsupported usage history version".into()); }
    let accounts = value.get("accounts").and_then(Value::as_arr).filter(|accounts| accounts.len() <= USAGE_HISTORY_ACCOUNTS * 2).ok_or("Invalid usage account history count")?;
    let mut result = Vec::<AccountUsageHistory>::new();
    for value in accounts {
        let provider = match value.get("provider").and_then(Value::as_str) { Some("claude") => UsageProvider::Claude, Some("codex") => UsageProvider::Codex, _ => return Err("Invalid history provider".into()) };
        let email = account_email(value.get("email").and_then(Value::as_str)).ok_or("Invalid history email")?;
        if result.iter().any(|account| account.provider == provider && account.email == email)
            || result.iter().filter(|account| account.provider == provider).count() >= USAGE_HISTORY_ACCOUNTS { return Err("Duplicate account or provider history exceeds its bound".into()); }
        let rows = value.get("samples").and_then(Value::as_arr).filter(|samples| samples.len() <= USAGE_HISTORY_SAMPLES).ok_or("Invalid account sample count")?;
        let mut samples = Vec::new();
        for row in rows {
            let session = usage_history_window(row.get("session").unwrap_or(&Value::Null), "session")?;
            let week = usage_history_window(row.get("week").unwrap_or(&Value::Null), "week")?;
            if session.is_none() && week.is_none() { return Err("History sample has no session or week observation".into()); }
            if provider == UsageProvider::Codex && session.is_some() { return Err("Astra history records only its weekly limit".into()); }
            samples.push(AccountUsageSample {
                observed_at: usage_history_time(row, "observed_at")?, source: usage_history_text(row, "source", 160)?,
                plan: match row.get("plan") { None | Some(Value::Null) => None, Some(_) => Some(usage_history_text(row, "plan", 80)?) }, session, week,
            });
        }
        result.push(AccountUsageHistory {
            provider, email, first_seen_at: usage_history_time(value, "first_seen_at")?, last_seen_at: usage_history_time(value, "last_seen_at")?,
            last_checked_at: usage_history_time(value, "last_checked_at")?,
            last_check_succeeded: value.get("last_check_succeeded").and_then(Value::as_bool).ok_or("Invalid history check status")?, samples,
        });
    }
    Ok(result)
}
fn usage_history_read(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    usage_history_regular(&metadata)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")]
        options.custom_flags(0x100 | 0x4);
        #[cfg(target_os = "linux")]
        options.custom_flags(0x20000 | 0x800);
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    usage_history_regular(&file.metadata().map_err(|error| error.to_string())?)?;
    let mut bytes = Vec::new();
    file.take(USAGE_HISTORY_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() > USAGE_HISTORY_BYTES { return Err("Usage history exceeds 4 MiB".into()); }
    Ok(Some(bytes))
}
fn usage_history_regular(metadata: &std::fs::Metadata) -> Result<(), String> {
    if !metadata.is_file() || metadata.len() > USAGE_HISTORY_BYTES as u64 { return Err("Usage history is not a bounded regular file".into()); }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        extern "C" { fn geteuid() -> u32; }
        if metadata.uid() != unsafe { geteuid() } || metadata.mode() & 0o077 != 0 { return Err("Usage history must be user-owned with mode 0600".into()); }
    }
    Ok(())
}
fn usage_history_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    if bytes.len() > USAGE_HISTORY_BYTES { return Err("Usage history exceeds 4 MiB".into()); }
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    let mut directories = std::fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    { use std::os::unix::fs::DirBuilderExt; directories.mode(0o700); }
    directories.create(parent).map_err(|error| error.to_string())?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => usage_history_regular(&metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error.to_string()),
    }
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".studio-usage-{}-{sequence}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(&temporary).map_err(|error| error.to_string())?;
    let result = (|| {
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        std::fs::File::open(parent).and_then(|directory| directory.sync_all()).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temporary); }
    result
}
