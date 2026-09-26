//! Durable normalized messages and shared term postings, owned by a worker.
use crate::source::MailMessage;
use makepad_search::{document_terms, fold, TextIndex};
use makepad_sqlite::{Connection, Value as Sql};
use makepad_strict_json::{obj, parse, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct MessageSummary {
    pub id: u64,
    pub message_id: String,
    pub subject: String,
    pub from: String,
    pub to: String,
    pub date: i64,
    pub mailboxes: Vec<String>,
    /// Further " / "-joined mailbox paths this message belongs to.
    pub labels: Vec<String>,
    pub attachments: Vec<String>,
    pub preview: String,
    pub incomplete: bool,
}

/// The role of a mailbox by its name, for the smart folders and icons. Gmail
/// names its label mailboxes in the account's language.
pub fn mailbox_kind(name: &str) -> Option<&'static str> {
    let name = name.trim().to_lowercase();
    Some(match name.as_str() {
        "inbox" | "postvak in" | "posteingang" | "boîte de réception" | "bandeja de entrada" => "inbox",
        "sent" | "sent mail" | "sent messages" | "sent items" | "verzonden" | "verzonden berichten"
        | "verzonden items" | "gesendet" | "gesendete objekte" | "messages envoyés" | "envoyés"
        | "enviados" | "elementos enviados" => "sent",
        "drafts" | "draft" | "concepten" | "entwürfe" | "brouillons" | "borradores" => "draft",
        "trash" | "deleted messages" | "deleted items" | "bin" | "prullenbak"
        | "verwijderde berichten" | "papierkorb" | "corbeille" | "papelera" => "trash",
        "spam" | "junk" | "junk mail" | "junk e-mail" | "ongewenste e-mail" => "spam",
        "archive" | "all mail" | "alle e-mail" | "archief" | "alle nachrichten"
        | "tous les messages" => "archive",
        "starred" | "flagged" | "met ster" | "markiert" | "suivis" | "destacados" => "starred",
        "important" | "belangrijk" | "wichtig" | "importants" | "importantes" => "important",
        _ => return None,
    })
}
pub const MAILBOX_KINDS: [&str; 8] =
    ["inbox", "sent", "draft", "trash", "spam", "archive", "starred", "important"];
#[derive(Clone, Debug)]
pub struct MailboxSummary {
    pub path: Vec<String>,
    pub count: usize,
}
#[derive(Clone, Debug, Default)]
pub struct SearchPage {
    /// The matches in result order. Shared with the index's records, so a
    /// page of every match in a large mailbox copies pointers, not strings.
    pub hits: Vec<Arc<MessageSummary>>,
    pub total: usize,
    pub indexed: usize,
    pub elapsed_ms: f64,
    pub mailboxes: std::sync::Arc<Vec<MailboxSummary>>,
}
pub struct PreparedMessage {
    summary: MessageSummary,
    body: String,
    presentation: String,
    terms: Vec<String>,
    json: String,
    terms_text: String,
}
impl PreparedMessage {
    pub fn new(message: MailMessage) -> Self {
        let attachments = message.attachments.join(" ");
        let terms = document_terms(&[
            &message.subject,
            &message.from,
            &message.to,
            &message.body,
            &attachments,
        ]);
        let preview = message
            .body
            .split_whitespace()
            .take(50)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(240)
            .collect::<String>();
        let summary = MessageSummary {
            id: 0,
            message_id: message.message_id,
            subject: message.subject,
            from: message.from,
            to: message.to,
            date: message.date,
            mailboxes: message.mailboxes,
            labels: message.labels,
            attachments: message.attachments,
            preview,
            incomplete: message.incomplete,
        };
        let json = obj(vec![
            ("message_id", Value::Str(summary.message_id.clone())),
            ("subject", Value::Str(summary.subject.clone())),
            ("from", Value::Str(summary.from.clone())),
            ("to", Value::Str(summary.to.clone())),
            ("date", Value::Int(summary.date)),
            (
                "mailboxes",
                Value::Arr(summary.mailboxes.iter().cloned().map(Value::Str).collect()),
            ),
            (
                "labels",
                Value::Arr(summary.labels.iter().cloned().map(Value::Str).collect()),
            ),
            (
                "attachments",
                Value::Arr(
                    summary
                        .attachments
                        .iter()
                        .cloned()
                        .map(Value::Str)
                        .collect(),
                ),
            ),
            ("preview", Value::Str(summary.preview.clone())),
            ("incomplete", Value::Bool(summary.incomplete)),
        ])
        .to_json();
        let terms_text = terms.join("\n");
        Self { summary, body: message.body, presentation: message.presentation, terms, json, terms_text }
    }
    pub fn staging_bytes(&self) -> usize {
        self.body.len() + self.presentation.len() + self.json.len() * 2
            + self.terms_text.len() * 2 + self.terms.len() * std::mem::size_of::<String>()
    }
}
pub struct MailIndex {
    db: Connection,
    source: String,
    next_id: u64,
    pub records: BTreeMap<u64, Arc<MessageSummary>>,
    pub keys: HashMap<String, (u64, String)>,
    terms: Vec<TextIndex>,
    term_limits: Vec<u64>,
    mailbox_counts: BTreeMap<Vec<String>, usize>,
    mailbox_snapshot: std::sync::Arc<Vec<MailboxSummary>>,
    mailboxes_dirty: bool,
}
#[derive(Default)]
pub struct CachePartition {
    records: BTreeMap<u64, Arc<MessageSummary>>,
    keys: HashMap<String, (u64, String)>,
    terms: TextIndex,
    mailbox_counts: BTreeMap<Vec<String>, usize>,
    pub read_time: Duration,
    pub build_time: Duration,
}
impl CachePartition {
    pub fn load(
        path: &Path,
        source: &str,
        (mut after, end): (u64, u64),
        progress: &AtomicU64,
        stopped: impl Fn() -> bool,
    ) -> Result<Self, String> {
        let mut loaded = Self::default();
        if after >= end || stopped() { return Ok(loaded); }
        let began = Instant::now();
        let mut db = Connection::open_read_only(path, Duration::from_millis(500)).map_err(err)?;
        loaded.read_time += began.elapsed();
        while after < end {
            if stopped() { return Err("Cache loading stopped".into()); }
            let began = Instant::now();
            // Both bounds are required for the rowid seek plan. Each reader
            // owns its connection/page cache and holds only a bounded row batch.
            let rows = db.query("SELECT id,item_key,revision,metadata,terms FROM mail_documents_v1 WHERE source=? AND id>? AND id<=? ORDER BY id LIMIT 256", &[
                Sql::text(source), Sql::Integer(after as i64), Sql::Integer(end as i64),
            ]).map_err(err)?.rows;
            loaded.read_time += began.elapsed();
            if rows.is_empty() { break; }
            let count = rows.len();
            let began = Instant::now();
            for row in rows {
                if stopped() { return Err("Cache loading stopped".into()); }
                let fields: [Sql; 5] = row.try_into().map_err(|_| "Invalid cache row")?;
                let [Sql::Integer(id), Sql::Text(key), Sql::Text(revision), Sql::Text(metadata), Sql::Text(terms)] = fields else {
                    return Err("Invalid cache record".into());
                };
                if id <= 0 || id as u64 <= after || id as u64 > end {
                    return Err("Invalid cache id".into());
                }
                let id = id as u64;
                let meta = parse(metadata.as_bytes()).map_err(err)?;
                let summary = MessageSummary {
                    id,
                    subject: text(&meta, "subject"),
                    message_id: text(&meta, "message_id"),
                    from: text(&meta, "from"),
                    to: text(&meta, "to"),
                    date: meta.get("date").and_then(Value::as_i64).unwrap_or(0),
                    mailboxes: strings(&meta, "mailboxes"),
                    labels: strings(&meta, "labels"),
                    attachments: strings(&meta, "attachments"),
                    preview: text(&meta, "preview"),
                    incomplete: meta.get("incomplete").and_then(Value::as_bool).unwrap_or(false),
                };
                loaded.terms.insert_terms(id, terms.lines());
                loaded.keys.insert(key, (id, revision));
                for n in 1..=summary.mailboxes.len() {
                    *loaded.mailbox_counts.entry(summary.mailboxes[..n].to_vec()).or_default() += 1;
                }
                for label in &summary.labels {
                    let path: Vec<String> = label.split(" / ").map(str::to_owned).collect();
                    for n in 1..=path.len() {
                        *loaded.mailbox_counts.entry(path[..n].to_vec()).or_default() += 1;
                    }
                }
                loaded.records.insert(id, Arc::new(summary));
                after = id;
            }
            loaded.build_time += began.elapsed();
            progress.fetch_add(count as u64, Ordering::Relaxed);
        }
        Ok(loaded)
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").into()
}
fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_arr)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
impl MailIndex {
    /// Delete the cache database and its journal sidecars so the next `open`
    /// starts empty. Only derived search data lives here; the mail source is
    /// never touched. Call it with no connection open on the file.
    pub fn remove_cache(path: &Path) -> Result<(), String> {
        let sidecar = |suffix: &str| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            match std::fs::remove_file(&name) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("{}: {e}", Path::new(&name).display())),
            }
        };
        sidecar("")?;
        sidecar("-journal")?;
        sidecar("-wal")?;
        sidecar("-shm")
    }
    pub fn open(path: &Path, source: &str) -> Result<Self, String> {
        let parent = path.parent().ok_or("Invalid cache path")?;
        let new_parent = !parent.exists();
        std::fs::create_dir_all(parent).map_err(err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            if new_parent {
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                    .map_err(err)?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path)
                .map_err(err)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(err)?;
        }
        #[cfg(not(unix))]
        let _ = new_parent;
        let mut db = Connection::open(path, Duration::from_millis(500)).map_err(err)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS mail_documents_v1(id INTEGER PRIMARY KEY, source TEXT NOT NULL, item_key TEXT NOT NULL, revision TEXT NOT NULL, metadata TEXT NOT NULL, body TEXT NOT NULL, terms TEXT NOT NULL, UNIQUE(source,item_key));").map_err(err)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS mail_presentations_v1(id INTEGER PRIMARY KEY, html TEXT NOT NULL);").map_err(err)?;
        let max = db
            .query("SELECT MAX(id) FROM mail_documents_v1", &[])
            .map_err(err)?
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(Sql::as_integer)
            .unwrap_or(0);
        Ok(Self {
            db,
            source: source.into(),
            next_id: max as u64 + 1,
            records: BTreeMap::new(),
            keys: HashMap::new(),
            terms: vec![TextIndex::default()],
            term_limits: vec![u64::MAX],
            mailbox_counts: BTreeMap::new(),
            mailbox_snapshot: Default::default(),
            mailboxes_dirty: true,
        })
    }
    /// Disjoint rowid ranges retain their independently built term indexes.
    /// Small caches avoid paying for a vocabulary and connection on every core.
    pub fn cache_ranges(&mut self, workers: usize) -> Vec<(u64, u64)> {
        let max = self.next_id.saturating_sub(1);
        let count = workers.max(1).min(max.div_ceil(2048).max(1) as usize);
        self.terms = (0..count).map(|_| TextIndex::default()).collect();
        self.term_limits = (1..=count).map(|i| (max as u128 * i as u128 / count as u128) as u64).collect();
        let mut start = 0;
        self.term_limits.iter().map(|&end| {
            let range = (start, end);
            start = end;
            range
        }).collect()
    }
    fn term_partition(&self, id: u64) -> usize {
        self.term_limits.partition_point(|&end| end < id).min(self.terms.len() - 1)
    }
    pub fn install_partition(&mut self, partition: usize, loaded: CachePartition) {
        self.terms[partition] = loaded.terms;
        self.records.extend(loaded.records);
        self.keys.extend(loaded.keys);
        for (path, count) in loaded.mailbox_counts {
            *self.mailbox_counts.entry(path).or_default() += count;
        }
        self.mailboxes_dirty = true;
    }
    pub fn begin(&mut self) -> Result<(), String> {
        self.db
            .execute("BEGIN IMMEDIATE", &[])
            .map(|_| ())
            .map_err(err)
    }
    pub fn commit(&mut self) -> Result<(), String> {
        self.db.execute("COMMIT", &[]).map(|_| ()).map_err(err)
    }
    pub fn upsert(
        &mut self,
        key: &str,
        revision: &str,
        prepared: PreparedMessage,
    ) -> Result<(), String> {
        let id = self.keys.get(key).map(|v| v.0).unwrap_or_else(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        });
        let PreparedMessage { mut summary, body, presentation, terms, json, terms_text } = prepared;
        summary.id = id;
        self.db.execute("INSERT OR REPLACE INTO mail_documents_v1(id,source,item_key,revision,metadata,body,terms) VALUES(?,?,?,?,?,?,?)", &[
            Sql::Integer(id as i64), Sql::text(&self.source), Sql::text(key), Sql::text(revision), Sql::text(json), Sql::text(body), Sql::text(terms_text),
        ]).map_err(err)?;
        self.db
            .execute(
                "INSERT OR REPLACE INTO mail_presentations_v1(id,html) VALUES(?,?)",
                &[Sql::Integer(id as i64), Sql::text(presentation)],
            )
            .map_err(err)?;
        let partition = self.term_partition(id);
        self.terms[partition].insert(id, &terms);
        self.keys.insert(key.into(), (id, revision.into()));
        if let Some(old) = self.records.remove(&id) {
            self.count_summary(&old, false);
        }
        self.count_summary(&summary, true);
        self.records.insert(id, Arc::new(summary));
        Ok(())
    }
    pub fn remove(&mut self, key: &str) -> Result<(), String> {
        if let Some((id, _)) = self.keys.remove(key) {
            self.db
                .execute(
                    "DELETE FROM mail_documents_v1 WHERE id=?",
                    &[Sql::Integer(id as i64)],
                )
                .map_err(err)?;
            if let Some(old) = self.records.remove(&id) {
                self.count_summary(&old, false);
            }
            let partition = self.term_partition(id);
            self.terms[partition].remove(id);
            self.db
                .execute(
                    "DELETE FROM mail_presentations_v1 WHERE id=?",
                    &[Sql::Integer(id as i64)],
                )
                .map_err(err)?;
        }
        Ok(())
    }
    /// Count a message under its own mailbox and under every label.
    fn count_summary(&mut self, summary: &MessageSummary, add: bool) {
        let mailboxes = summary.mailboxes.clone();
        self.count_mailbox(&mailboxes, add);
        for label in &summary.labels {
            let path: Vec<String> = label.split(" / ").map(str::to_owned).collect();
            self.count_mailbox(&path, add);
        }
    }
    fn count_mailbox(&mut self, path: &[String], add: bool) {
        for n in 1..=path.len() {
            let prefix = path[..n].to_vec();
            let count = self.mailbox_counts.entry(prefix.clone()).or_default();
            if add {
                *count += 1;
            } else {
                *count = count.saturating_sub(1);
            }
            if *count == 0 {
                self.mailbox_counts.remove(&prefix);
            }
        }
        self.mailboxes_dirty = true;
    }
    /// The source key of a cached message, to read its file again.
    pub fn item_key(&mut self, id: u64) -> Result<String, String> {
        self.db
            .query(
                "SELECT item_key FROM mail_documents_v1 WHERE id=?",
                &[Sql::Integer(id as i64)],
            )
            .map_err(err)?
            .rows
            .first()
            .and_then(|r| r[0].as_text().map(str::to_owned))
            .ok_or_else(|| "Message is no longer cached".into())
    }
    pub fn body(&mut self, id: u64) -> Result<String, String> {
        let markup = self
            .db
            .query(
                "SELECT html FROM mail_presentations_v1 WHERE id=?",
                &[Sql::Integer(id as i64)],
            )
            .map_err(err)?;
        if let Some(text) = markup
            .rows
            .first()
            .and_then(|r| r[0].as_text())
            .filter(|s| !s.is_empty())
        {
            return Ok(text.to_owned());
        }
        self.db
            .query(
                "SELECT body FROM mail_documents_v1 WHERE id=? AND source=?",
                &[Sql::Integer(id as i64), Sql::text(&self.source)],
            )
            .map_err(err)?
            .rows
            .first()
            .and_then(|r| r[0].as_text())
            .map(crate::presentation::plain)
            .ok_or_else(|| "Message is no longer cached".into())
    }
    pub fn search(
        &mut self,
        raw: &str,
        limit: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<SearchPage, String> {
        let start = std::time::Instant::now();
        let query = parse_query(raw)?;
        let mut candidates: Option<BTreeSet<u64>> = None;
        for part in &query {
            if part.negative
                || ["before", "after", "has", "mailbox", "folder", "kind"].contains(&part.field.as_str())
            {
                continue;
            }
            for atom in document_terms(&[&part.value]) {
                let mut set = BTreeSet::new();
                for terms in &mut self.terms {
                    set.extend(terms.candidates(&atom, &cancelled).ok_or("Search superseded")?);
                }
                candidates = Some(match candidates {
                    None => set,
                    Some(old) => old.intersection(&set).copied().collect(),
                });
            }
        }
        let ids: Vec<u64> = candidates.map_or_else(
            || self.records.keys().copied().collect(),
            |s| s.into_iter().collect(),
        );
        let mut matches = Vec::new();
        let mut seen = BTreeSet::new();
        for id in ids {
            if cancelled() {
                return Err("Search superseded".into());
            }
            let Some(row) = self.records.get(&id) else {
                continue;
            };
            let mut body = None;
            let mut yes = true;
            for part in &query {
                let matched = match part.field.as_str() {
                    "mailbox" => row
                        .mailboxes
                        .iter()
                        .map(String::as_str)
                        .chain(row.labels.iter().flat_map(|l| l.split(" / ")))
                        .any(|s| search_text(s).contains(&part.value)),
                    "folder" => {
                        let own = row.mailboxes.join(" / ");
                        std::iter::once(own.as_str())
                            .chain(row.labels.iter().map(String::as_str))
                            .any(|path| {
                                let path = search_text(path);
                                path == part.value
                                    || path.starts_with(&format!("{} / ", part.value))
                            })
                    }
                    "kind" => row
                        .mailboxes
                        .last()
                        .map(String::as_str)
                        .into_iter()
                        .chain(row.labels.iter().filter_map(|l| l.rsplit(" / ").next()))
                        .any(|name| mailbox_kind(name) == Some(part.value.as_str())),
                    "from" => search_text(&row.from).contains(&part.value),
                    "to" => search_text(&row.to).contains(&part.value),
                    "subject" => search_text(&row.subject).contains(&part.value),
                    "filename" => row
                        .attachments
                        .iter()
                        .any(|a| search_text(a).contains(&part.value)),
                    "has" => part.value == "attachment" && !row.attachments.is_empty(),
                    "before" | "after" => {
                        let date = makepad_civil_time::parse_iso(&part.value)
                            .ok_or("Use dates as YYYY-MM-DD")?
                            as i64
                            * 86400;
                        if part.field == "before" {
                            row.date < date
                        } else {
                            row.date >= date
                        }
                    }
                    _ => {
                        let metadata = part.field.is_empty()
                            && [&row.subject, &row.from, &row.to]
                                .iter()
                                .any(|s| search_text(s).contains(&part.value))
                            || part.field.is_empty()
                                && row
                                    .attachments
                                    .iter()
                                    .any(|a| search_text(a).contains(&part.value));
                        if metadata {
                            true
                        } else {
                            if body.is_none() {
                                body = Some(
                                    self.db
                                        .query(
                                            "SELECT body FROM mail_documents_v1 WHERE id=?",
                                            &[Sql::Integer(id as i64)],
                                        )
                                        .map_err(err)?
                                        .rows
                                        .first()
                                        .and_then(|r| r[0].as_text())
                                        .map(search_text)
                                        .unwrap_or_default(),
                                );
                            }
                            body.as_ref().unwrap().contains(&part.value)
                        }
                    }
                };
                if matched == part.negative {
                    yes = false;
                    break;
                }
            }
            if yes && (row.message_id.is_empty() || seen.insert(row.message_id.clone())) {
                let score = query
                    .iter()
                    .filter(|q| !q.negative && search_text(&row.subject).contains(&q.value))
                    .count();
                matches.push((score, row.date, id));
            }
        }
        matches.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        let total = matches.len();
        let hits = matches
            .into_iter()
            .take(limit)
            .filter_map(|(_, _, id)| self.records.get(&id).cloned())
            .collect();
        if self.mailboxes_dirty {
            self.mailbox_snapshot = std::sync::Arc::new(
                self.mailbox_counts
                    .iter()
                    .map(|(path, count)| MailboxSummary {
                        path: path.clone(),
                        count: *count,
                    })
                    .collect(),
            );
            self.mailboxes_dirty = false;
        }
        Ok(SearchPage {
            hits,
            total,
            indexed: self.records.len(),
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            mailboxes: self.mailbox_snapshot.clone(),
        })
    }
}
// MIME line wrapping is presentation, not a break in an exact phrase.
fn search_text(text: &str) -> String {
    fold(text).split_whitespace().collect::<Vec<_>>().join(" ")
}
struct QueryPart {
    field: String,
    value: String,
    negative: bool,
}
fn parse_query(raw: &str) -> Result<Vec<QueryPart>, String> {
    if raw.len() > 4096 {
        return Err("Search is too long".into());
    }
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in raw.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' && quoted {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        } else if c.is_whitespace() && !quoted {
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if quoted {
        return Err("Close the quoted phrase to search".into());
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    if pieces.len() > 32 {
        return Err("Use at most 32 search terms".into());
    }
    let mut parts = Vec::new();
    for piece in pieces {
        let negative = piece.starts_with('-');
        let piece = if negative { &piece[1..] } else { &piece };
        let (field, value) = if let Some((field, value)) = piece.split_once(':') {
            if ![
                "from", "to", "subject", "body", "filename", "before", "after", "has", "mailbox",
                "folder", "kind",
            ]
            .contains(&field)
            {
                return Err(format!("Unknown search field: {field}"));
            }
            if field == "kind" && !MAILBOX_KINDS.contains(&value.to_ascii_lowercase().as_str()) {
                return Err(format!("Use kind: with one of {}", MAILBOX_KINDS.join(", ")));
            }
            (field.to_string(), value)
        } else {
            (String::new(), piece)
        };
        if value.is_empty() {
            return Err("Enter a value after the search field".into());
        }
        if matches!(field.as_str(), "before" | "after")
            && makepad_civil_time::parse_iso(value).is_none()
        {
            return Err("Use dates as YYYY-MM-DD".into());
        }
        if field == "has" && !value.eq_ignore_ascii_case("attachment") {
            return Err("Use has:attachment to find messages with attachments".into());
        }
        parts.push(QueryPart {
            field,
            value: search_text(value),
            negative,
        });
    }
    Ok(parts)
}
