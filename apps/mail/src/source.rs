//! Mail transport boundary. Adapters own discovery/sync; the index owns search.
//! A Gmail adapter can keep its historyId in `cursor`, emit tombstones, and
//! provide parsed MIME through the same normalized message contract.
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct MailMessage {
    pub message_id: String,
    pub subject: String,
    pub from: String,
    pub to: String,
    pub date: i64,
    pub mailboxes: Vec<String>,
    /// Other mailboxes that also hold this message, each a " / "-joined
    /// path. Mail stores a Gmail message once and records Inbox, Sent,
    /// Starred and Important membership as labels.
    pub labels: Vec<String>,
    pub body: String,
    /// Sanitized, theme-neutral HTML. Empty means the adapter provides plain text.
    pub presentation: String,
    pub attachments: Vec<String>,
    pub incomplete: bool,
}

#[derive(Clone, Debug)]
pub struct SourceEntry {
    pub key: String,
    pub revision: String,
}

#[derive(Default)]
pub struct SourceBatch {
    pub entries: Vec<SourceEntry>,
    pub removed: Vec<String>,
    pub complete: bool,
    pub cursor: Option<String>,
    /// Only a successful complete inventory permits pruning missing entries.
    pub full_inventory: bool,
    pub unreadable: usize,
}

/// One attachment's decoded bytes, ready to be written out and opened.
pub struct MailAttachment {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub trait MailReader: Send + Sync {
    fn read(&self, entry: &SourceEntry) -> Result<MailMessage, String>;
    /// The `index`th attachment of `entry`, in the order
    /// `MailMessage::attachments` lists them.
    fn attachment(&self, _entry: &SourceEntry, _index: usize) -> Result<MailAttachment, String> {
        Err("This mail source cannot open attachments".into())
    }
}

pub trait MailSource: Send {
    fn id(&self) -> &str;
    fn begin_sync(&mut self, cursor: Option<&str>) -> Result<(), String>;
    /// Optional shared bounded executor for adapters with parallel discovery.
    fn configure_workers(&mut self, _pool: makepad_widgets::makepad_platform::thread::TaskPool) {}
    /// Bounded discovery, including directories with no mail in them.
    fn next_batch(&mut self, budget: usize) -> Result<SourceBatch, String>;
    fn reader(&self) -> std::sync::Arc<dyn MailReader>;
}

#[derive(Clone)]
pub struct LocalConfig {
    pub root: PathBuf,
    pub cache: PathBuf,
}

impl LocalConfig {
    pub fn for_mail_root(root: PathBuf, cache_override: Option<PathBuf>) -> Self {
        // Keep derived message text alongside the protected source by default.
        // An explicit override is useful for exported mail and synthetic data.
        let cache = cache_override.unwrap_or_else(|| root.join(".makepad-search/search.sqlite3"));
        Self { root, cache }
    }
}
