//! Read-only adapter for downloaded Apple Mail .emlx files and exported .eml.
use crate::mime;
use crate::source::*;
use makepad_widgets::makepad_platform::thread::{Lane, TaskHandle, TaskPool};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, UNIX_EPOCH},
};

/// Message rowid (also its emlx file name) → the other mailbox paths it is
/// labelled with, each " / "-joined.
type Labels = Arc<HashMap<u64, Vec<String>>>;

pub struct AppleMailSource {
    root: PathBuf,
    id: String,
    dirs: VecDeque<DirectoryWork>,
    scans: Vec<TaskHandle<DirectoryBatch>>,
    pool: Option<TaskPool>,
    read_root: PathBuf,
    unreadable: usize,
    labels: Labels,
    /// Envelope Index stamp and time of the last label load.
    labels_loaded: Option<(u64, Instant)>,
}
impl AppleMailSource {
    pub fn new(root: PathBuf) -> Self {
        Self {
            id: format!("apple-mail:{}", root.display()),
            read_root: root.clone(),
            root,
            dirs: VecDeque::new(),
            scans: Vec::new(),
            pool: None,
            unreadable: 0,
            labels: Arc::new(HashMap::new()),
            labels_loaded: None,
        }
    }
}
impl MailSource for AppleMailSource {
    fn id(&self) -> &str {
        &self.id
    }
    fn begin_sync(&mut self, _cursor: Option<&str>) -> Result<(), String> {
        let entries = fs::read_dir(&self.root).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(1) {
                "Mail access is blocked by macOS. In System Settings → Privacy & Security → Full Disk Access, add Makepad Mail, then quit and reopen it. Apple Mail's messages will only be read.".to_string()
            } else { format!("Cannot open the mail source: {e}") }
        })?;
        self.read_root = self.root.canonicalize().map_err(|e| e.to_string())?;
        if self.labels_loaded.is_none() {
            log_layout(&self.root);
        }
        // Mail rewrites its index constantly; a sync runs every 30 s. Reload
        // the labels when the index changed, and at most every five minutes.
        let stamp = envelope_stamp(&self.root);
        let stale = self
            .labels_loaded
            .map_or(true, |(s, at)| s != stamp && at.elapsed() >= Duration::from_secs(300));
        if stale {
            self.labels = Arc::new(load_labels(&self.root));
            self.labels_loaded = Some((stamp, Instant::now()));
        }
        self.dirs = VecDeque::from([DirectoryWork { path: self.root.clone(), entries: Some(entries) }]);
        self.scans.clear();
        self.unreadable = 0;
        Ok(())
    }
    fn configure_workers(&mut self, pool: TaskPool) {
        self.pool = Some(pool);
    }
    fn next_batch(&mut self, budget: usize) -> Result<SourceBatch, String> {
        let pool = self.pool.as_ref().ok_or("Mail discovery workers are not configured")?;
        let mut batch = SourceBatch::default();
        // Directory reads and file metadata probes run concurrently with MIME
        // decoding. The coordinator only drains bounded, completed batches.
        for i in (0..self.scans.len()).rev() {
            if batch.entries.len() >= budget { break; }
            if let Some(result) = self.scans[i].try_take() {
                drop(self.scans.swap_remove(i));
                match result {
                    Ok(result) => {
                        batch.entries.extend(result.entries);
                        self.unreadable += result.unreadable;
                        self.dirs.extend(result.children.into_iter().map(|path| DirectoryWork { path, entries: None }));
                        // Finish open directories first, bounding open handles
                        // by the number of concurrent scan jobs.
                        if let Some(work) = result.remaining { self.dirs.push_front(work); }
                    }
                    Err(_) => self.unreadable += 1,
                }
            }
        }
        while self.scans.len() < pool.heavy_workers() && !self.dirs.is_empty() {
            let Ok(slot) = pool.reserve(Lane::Heavy) else { break; };
            let work = self.dirs.pop_front().unwrap();
            let root = self.root.clone();
            let labels = self.labels.clone();
            // A large folder yields after a fixed number of entries so scans
            // cannot monopolize the decoder pool or produce unbounded results.
            self.scans.push(slot.submit_named("mail-discover", move || work.scan(&root, 128, &labels)));
        }
        batch.complete = self.dirs.is_empty() && self.scans.is_empty();
        batch.full_inventory = true;
        batch.unreadable = self.unreadable;
        Ok(batch)
    }
    fn reader(&self) -> std::sync::Arc<dyn MailReader> {
        std::sync::Arc::new(AppleMailReader {
            root: self.read_root.clone(),
            labels: self.labels.clone(),
        })
    }
}
/// Log the mailbox directories below each store version and account, so a
/// mailbox missing from the index can be told apart from one the scanner
/// skipped. Bounded; names only.
fn log_layout(root: &Path) {
    fn dirs(path: &Path) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(path) else {
            makepad_widgets::log!("mail: cannot list {}", path.display());
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs
    }
    fn names(paths: &[PathBuf]) -> String {
        paths
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
    let mut lines = 0;
    for version in dirs(root) {
        for account in dirs(&version) {
            let boxes = dirs(&account);
            if !boxes.iter().any(|b| b.extension().is_some_and(|e| e == "mbox")) {
                continue;
            }
            let shown = account.strip_prefix(root).unwrap_or(&account);
            makepad_widgets::log!("mail: {} → {}", shown.display(), names(&boxes));
            for parent in boxes.iter().filter(|b| b.extension().is_some_and(|e| e == "mbox")) {
                let inner = dirs(parent);
                if inner.iter().any(|b| b.extension().is_some_and(|e| e == "mbox")) {
                    makepad_widgets::log!("mail:   {} → {}", parent.file_name().and_then(|n| n.to_str()).unwrap_or(""), names(&inner));
                }
            }
            lines += 1;
            if lines >= 64 {
                makepad_widgets::log!("mail: layout listing truncated");
                return;
            }
        }
    }
}
/// Mail stores a Gmail message's file once, under All Mail, and records the
/// other mailboxes it belongs to (Inbox, Sent, Starred, Important) as labels
/// in its Envelope Index. Map each message rowid, which is also the emlx file
/// name, to those mailbox paths. Nothing else in the index is read.
fn load_labels(root: &Path) -> HashMap<u64, Vec<String>> {
    use makepad_sqlite::Connection;
    let began = Instant::now();
    let mut labels: HashMap<u64, Vec<String>> = HashMap::new();
    let Ok(versions) = fs::read_dir(root) else { return labels };
    for version in versions.filter_map(|e| e.ok()).map(|e| e.path()) {
        let path = version.join("MailData/Envelope Index");
        if !path.is_file() {
            continue;
        }
        let mut db = match Connection::open_read_only(&path, Duration::from_millis(2000)) {
            Ok(db) => db,
            Err(e) => {
                makepad_widgets::log!("mail: cannot open {}: {e}", path.display());
                continue;
            }
        };
        // One label row per message per mailbox: far past the default budget.
        db.limits_mut().max_rows = 8_000_000;
        db.limits_mut().max_steps = 400_000_000;
        let mut mailboxes: HashMap<i64, String> = HashMap::new();
        match db.query("SELECT ROWID, url FROM mailboxes", &[]) {
            Ok(result) => {
                for row in &result.rows {
                    let (Some(id), Some(url)) = (row[0].as_integer(), row[1].as_text()) else {
                        continue;
                    };
                    // scheme://account@host/path → "[Gmail] / Sent Mail"
                    let Some(path) = url.splitn(4, '/').nth(3) else { continue };
                    let name = path.split('/').map(percent_decode).collect::<Vec<_>>().join(" / ");
                    if !name.is_empty() {
                        mailboxes.insert(id, name);
                    }
                }
            }
            Err(e) => {
                makepad_widgets::log!("mail: envelope mailboxes: {e}");
                continue;
            }
        }
        let rows = match db.query("SELECT message_id, mailbox_id FROM labels", &[]) {
            Ok(result) => result.rows,
            Err(e) => {
                makepad_widgets::log!("mail: envelope labels: {e}");
                continue;
            }
        };
        let mut count = 0usize;
        for row in &rows {
            let (Some(message), Some(mailbox)) = (row[0].as_integer(), row[1].as_integer()) else {
                continue;
            };
            let Some(name) = mailboxes.get(&mailbox) else { continue };
            if message < 0 {
                continue;
            }
            labels.entry(message as u64).or_default().push(name.clone());
            count += 1;
        }
        makepad_widgets::log!(
            "mail: envelope index: {} mailboxes, {count} label rows, {:.2}s",
            mailboxes.len(),
            began.elapsed().as_secs_f64()
        );
    }
    for names in labels.values_mut() {
        names.sort();
        names.dedup();
    }
    labels
}
/// Size and modification time of every Envelope Index and its write-ahead
/// log, folded together: changes when Mail has written to the index.
fn envelope_stamp(root: &Path) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut fold = |value: u128| {
        for b in value.to_le_bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    let Ok(versions) = fs::read_dir(root) else { return hash };
    for version in versions.filter_map(|e| e.ok()).map(|e| e.path()) {
        for name in ["MailData/Envelope Index", "MailData/Envelope Index-wal"] {
            let Ok(meta) = fs::metadata(version.join(name)) else { continue };
            fold(meta.len() as u128);
            fold(meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos()));
        }
    }
    hash
}
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
            if let (Some(a), Some(b)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
/// FNV-1a over a message's labels: part of its revision, so a label change
/// (archived, sent, starred) re-reads the message even though its file is
/// unchanged.
fn label_stamp(labels: &[String]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for label in labels {
        for b in label.bytes().chain(std::iter::once(b'\n')) {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}
/// `12345.emlx` / `12345.partial.emlx` → 12345, the Envelope Index rowid.
fn message_stem(path: &Path) -> Option<u64> {
    path.file_name()?.to_str()?.split('.').next()?.parse().ok()
}
struct DirectoryWork {
    path: PathBuf,
    entries: Option<fs::ReadDir>,
}
#[derive(Default)]
struct DirectoryBatch {
    entries: Vec<SourceEntry>,
    children: Vec<PathBuf>,
    remaining: Option<DirectoryWork>,
    unreadable: usize,
}
impl DirectoryWork {
    fn scan(mut self, root: &Path, budget: usize, labels: &HashMap<u64, Vec<String>>) -> DirectoryBatch {
        let mut batch = DirectoryBatch::default();
        if self.entries.is_none() {
            match fs::read_dir(&self.path) {
                Ok(entries) => self.entries = Some(entries),
                Err(e) => {
                    makepad_widgets::log!("mail: cannot read {}: {e}", self.path.display());
                    batch.unreadable += 1;
                    return batch;
                }
            }
        }
        let entries = self.entries.as_mut().unwrap();
        for _ in 0..budget {
            let entry = match entries.next() {
                None => return batch,
                Some(Err(_)) => { batch.unreadable += 1; continue; }
                Some(Ok(entry)) => entry,
            };
            let Ok(kind) = entry.file_type() else {
                batch.unreadable += 1;
                continue;
            };
            // Never follow links; our derived cache is not source mail.
            if kind.is_dir() {
                if entry.file_name() != ".makepad-search" { batch.children.push(entry.path()); }
            } else if kind.is_file() {
                let path = entry.path();
                if !matches!(path.extension().and_then(|s| s.to_str()), Some("emlx" | "eml")) { continue; }
                let Ok(meta) = entry.metadata() else {
                    batch.unreadable += 1;
                    continue;
                };
                let time = meta.modified().unwrap_or(UNIX_EPOCH)
                    .duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
                let Some(key) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) else {
                    batch.unreadable += 1;
                    continue;
                };
                let stamp = message_stem(&path)
                    .and_then(|stem| labels.get(&stem))
                    .map_or(0, |l| label_stamp(l));
                batch.entries.push(SourceEntry {
                    key: key.into(),
                    revision: format!("presentation-3:{}:{time}:{stamp:x}", meta.len()),
                });
            }
        }
        batch.remaining = Some(self);
        batch
    }
}
struct AppleMailReader {
    root: PathBuf,
    labels: Labels,
}
/// One message file read into memory, with the RFC 822 bytes located inside
/// the emlx envelope.
struct Loaded {
    path: PathBuf,
    bytes: Vec<u8>,
    raw: std::ops::Range<usize>,
    modified: i64,
}
impl Loaded {
    fn raw(&self) -> &[u8] {
        &self.bytes[self.raw.clone()]
    }
}
impl AppleMailReader {
    fn load(&self, relative: &Path) -> Result<Loaded, String> {
        if relative
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
        {
            return Err("Invalid source message key".into());
        }
        let path = self.root.join(relative);
        // Recheck containment: Mail may move files while a scan is in flight.
        let canonical = path.canonicalize().map_err(|e| e.to_string())?;
        if !canonical.starts_with(&self.root) {
            return Err("Message moved outside the mail source".into());
        }
        let mut file = fs::File::open(&canonical).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err("Message exceeds the 64 MB decoding limit".into());
        }
        let raw = if path.extension().is_some_and(|s| s == "emlx") {
            let end = bytes
                .iter()
                .position(|b| *b == b'\n')
                .ok_or("Invalid emlx header")?;
            let len: usize = std::str::from_utf8(&bytes[..end])
                .map_err(|_| "Invalid emlx length")?
                .trim()
                .parse()
                .map_err(|_| "Invalid emlx length")?;
            let stop = (end + 1).checked_add(len).ok_or("Invalid emlx length")?;
            if stop > bytes.len() {
                return Err("Message has not finished downloading".into());
            }
            end + 1..stop
        } else {
            0..bytes.len()
        };
        let modified = file
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        Ok(Loaded { path: canonical, bytes, raw, modified })
    }
}
impl MailReader for AppleMailReader {
    fn read(&self, entry: &SourceEntry) -> Result<MailMessage, String> {
        let relative = Path::new(&entry.key);
        let loaded = self.load(relative)?;
        let mut message = mime::parse(loaded.raw())?;
        message.mailboxes = relative
            .components()
            .filter_map(|p| {
                let name = p.as_os_str().to_str()?;
                name.strip_suffix(".mbox").map(str::to_string)
            })
            .collect();
        if message.mailboxes.is_empty() {
            message.mailboxes.push("Downloaded mail".into());
        }
        let own = message.mailboxes.join(" / ");
        if let Some(labels) = message_stem(&loaded.path).and_then(|stem| self.labels.get(&stem)) {
            message.labels = labels.iter().filter(|l| **l != own).cloned().collect();
        }
        if message.date == 0 {
            message.date = loaded.modified;
        }
        Ok(message)
    }
    fn attachment(&self, entry: &SourceEntry, index: usize) -> Result<MailAttachment, String> {
        let loaded = self.load(Path::new(&entry.key))?;
        let part = mime::attachment(loaded.raw(), index)?
            .ok_or("This message has no such attachment")?;
        if !part.bytes.is_empty() {
            return Ok(MailAttachment { name: part.name, bytes: part.bytes });
        }
        // Mail strips large attachments out of `<id>.partial.emlx` and files
        // them beside the mailbox's Messages folder as
        // `Attachments/<id>/<part>/<name>`.
        let stem = loaded
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('.').next())
            .unwrap_or("");
        let external = loaded
            .path
            .parent()
            .and_then(Path::parent)
            .map(|dir| dir.join("Attachments").join(stem).join(&part.part).join(&part.name));
        let Some(external) = external else {
            return Err("This attachment hasn’t been downloaded. Open the message in Mail to fetch it.".into());
        };
        if !external.is_file() {
            eprintln!("mail: attachment not on disk at {}", external.display());
            return Err("This attachment hasn’t been downloaded. Open the message in Mail to fetch it.".into());
        }
        let canonical = external.canonicalize().map_err(|e| e.to_string())?;
        if !canonical.starts_with(&self.root) {
            return Err("Attachment is outside the mail source".into());
        }
        let size = fs::metadata(&canonical).map_err(|e| e.to_string())?.len();
        if size > 512 * 1024 * 1024 {
            return Err("Attachment exceeds the 512 MB limit".into());
        }
        let bytes = fs::read(&canonical).map_err(|e| e.to_string())?;
        Ok(MailAttachment { name: part.name, bytes })
    }
}
