//! Mail domain types. Widgets and storage handles stay in `view.rs`.

use std::collections::BTreeSet;
use std::fmt;

pub const SCHEMA_VERSION: u32 = 1;
pub const SEED_VERSION: u32 = 1;
pub const SEED_ID: u32 = 0x4D41494C;

pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
pub const MAX_ID: u32 = 999_999;
pub const MAX_MESSAGES: usize = 1000;
pub const MAX_SUBJECT_CHARS: usize = 256;
pub const MAX_BODY_BYTES: usize = 16 * 1024;
pub const MAX_RECIPIENT_FIELD_BYTES: usize = 4 * 1024;
pub const MAX_RECIPIENTS: usize = 20;
pub const MAX_ATTACHMENTS: usize = 8;
pub const MAX_SEARCH_HITS: usize = 50;
pub const MAX_TOOL_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_SEARCH_QUERY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub u32);

impl MessageId {
    pub fn format(self) -> String {
        format!("m-{:06}", self.0)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("m-")?;
        if rest.len() != 6 || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        rest.parse::<u32>().ok().filter(|id| *id > 0).map(MessageId)
    }
}

impl ThreadId {
    pub fn format(self) -> String {
        format!("t-{:06}", self.0)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("t-")?;
        if rest.len() != 6 || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        rest.parse::<u32>().ok().filter(|id| *id > 0).map(ThreadId)
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub name: String,
    pub email: String,
}

impl Address {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self { name: name.into(), email: email.into() }
    }

    pub fn display(&self) -> String {
        if self.name.is_empty() {
            self.email.clone()
        } else {
            format!("{} <{}>", self.name, self.email)
        }
    }

    pub fn initials(&self) -> String {
        let mut out = String::new();
        for part in self.name.split_whitespace() {
            if let Some(c) = part.chars().find(|c| c.is_alphabetic()) {
                out.extend(c.to_uppercase());
                if out.chars().count() == 2 {
                    break;
                }
            }
        }
        if out.is_empty() {
            if let Some(c) = self.email.chars().find(|c| c.is_alphabetic()) {
                out.extend(c.to_uppercase());
            }
        }
        if out.is_empty() {
            out.push('?');
        }
        out
    }

    pub fn email_key(&self) -> String {
        self.email.trim().to_ascii_lowercase()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Folder {
    Inbox,
    Drafts,
    Sent,
    Archive,
    Junk,
    Trash,
    Projects,
    Travel,
}

impl Folder {
    pub const ALL: [Folder; 8] = [
        Folder::Inbox,
        Folder::Drafts,
        Folder::Sent,
        Folder::Archive,
        Folder::Junk,
        Folder::Trash,
        Folder::Projects,
        Folder::Travel,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Folder::Inbox => "inbox",
            Folder::Drafts => "drafts",
            Folder::Sent => "sent",
            Folder::Archive => "archive",
            Folder::Junk => "junk",
            Folder::Trash => "trash",
            Folder::Projects => "projects",
            Folder::Travel => "travel",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Folder::Inbox => "Inbox",
            Folder::Drafts => "Drafts",
            Folder::Sent => "Sent",
            Folder::Archive => "Archive",
            Folder::Junk => "Junk",
            Folder::Trash => "Trash",
            Folder::Projects => "Projects",
            Folder::Travel => "Travel",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "inbox" => Some(Folder::Inbox),
            "drafts" => Some(Folder::Drafts),
            "sent" => Some(Folder::Sent),
            "archive" => Some(Folder::Archive),
            "junk" => Some(Folder::Junk),
            "trash" => Some(Folder::Trash),
            "projects" => Some(Folder::Projects),
            "travel" => Some(Folder::Travel),
            _ => None,
        }
    }

    pub fn is_move_destination(self) -> bool {
        matches!(
            self,
            Folder::Inbox | Folder::Archive | Folder::Junk | Folder::Projects | Folder::Travel
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mailbox {
    Folder(Folder),
    Vips,
    Flagged,
}

impl Mailbox {
    pub const ALL: [Mailbox; 10] = [
        Mailbox::Folder(Folder::Inbox),
        Mailbox::Vips,
        Mailbox::Flagged,
        Mailbox::Folder(Folder::Drafts),
        Mailbox::Folder(Folder::Sent),
        Mailbox::Folder(Folder::Archive),
        Mailbox::Folder(Folder::Junk),
        Mailbox::Folder(Folder::Trash),
        Mailbox::Folder(Folder::Projects),
        Mailbox::Folder(Folder::Travel),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Mailbox::Folder(folder) => folder.label(),
            Mailbox::Vips => "VIPs",
            Mailbox::Flagged => "Flagged",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mailbox::Folder(folder) => folder.as_str(),
            Mailbox::Vips => "vips",
            Mailbox::Flagged => "flagged",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "vips" => Some(Mailbox::Vips),
            "flagged" => Some(Mailbox::Flagged),
            other => Folder::parse(other).map(Mailbox::Folder),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Received,
    Draft,
    Sent,
}

impl MessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::Received => "received",
            MessageKind::Draft => "draft",
            MessageKind::Sent => "sent",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "received" => Some(MessageKind::Received),
            "draft" => Some(MessageKind::Draft),
            "sent" => Some(MessageKind::Sent),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub size_bytes: u64,
}

impl Attachment {
    pub fn size_label(&self) -> String {
        if self.size_bytes >= 1024 * 1024 {
            format!("{:.1} MB", self.size_bytes as f64 / (1024.0 * 1024.0))
        } else if self.size_bytes >= 1024 {
            format!("{} KB", (self.size_bytes + 512) / 1024)
        } else {
            format!("{} B", self.size_bytes)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftInput {
    pub to_raw: String,
    pub cc_raw: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub thread_id: ThreadId,
    pub in_reply_to: Option<MessageId>,
    pub kind: MessageKind,
    pub folder: Folder,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    pub body_text: String,
    pub attachments: Vec<Attachment>,
    pub date_secs: i64,
    pub updated_secs: i64,
    pub unread: bool,
    pub flagged: bool,
    pub draft_input: Option<DraftInput>,
}

impl Message {
    pub fn is_draft(&self) -> bool {
        self.kind == MessageKind::Draft || self.draft_input.is_some()
    }

    pub fn preview(&self) -> String {
        preview_text(&self.body_text)
    }

    pub fn subject_or_placeholder(&self) -> String {
        let trimmed = self.subject.trim();
        if trimmed.is_empty() {
            "(No Subject)".into()
        } else {
            trimmed.to_string()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedAnchor {
    pub day: String,
    pub midnight_utc: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailDocument {
    pub schema_version: u32,
    pub seed_version: u32,
    pub seed_anchor: SeedAnchor,
    pub next_message_id: u32,
    pub next_thread_id: u32,
    pub me: Address,
    pub vip_emails: Vec<String>,
    pub messages: Vec<Message>,
}

impl MailDocument {
    pub fn message(&self, id: MessageId) -> Option<&Message> {
        self.messages.iter().find(|m| m.id == id)
    }

    pub fn message_mut(&mut self, id: MessageId) -> Option<&mut Message> {
        self.messages.iter_mut().find(|m| m.id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScope {
    CurrentMailbox,
    AllMail,
}

impl SearchScope {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchScope::CurrentMailbox => "mailbox",
            SearchScope::AllMail => "all",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Mailboxes,
    Messages,
    Read(MessageId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScrollAnchor {
    pub first_message: Option<MessageId>,
    pub offset_points: f64,
}

impl Default for ScrollAnchor {
    fn default() -> Self {
        Self { first_message: None, offset_points: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListContext {
    pub mailbox: Mailbox,
    pub query: String,
    pub scope: SearchScope,
    pub unread_only: bool,
    pub scroll: ScrollAnchor,
}

impl Default for ListContext {
    fn default() -> Self {
        Self {
            mailbox: Mailbox::Folder(Folder::Inbox),
            query: String::new(),
            scope: SearchScope::CurrentMailbox,
            unread_only: false,
            scroll: ScrollAnchor::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiState {
    pub history: Vec<Route>,
    pub list: ListContext,
    pub selected: Option<MessageId>,
    pub composer: Option<MessageId>,
    pub expanded: BTreeSet<MessageId>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            history: vec![Route::Mailboxes],
            list: ListContext::default(),
            selected: None,
            composer: None,
            expanded: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageRow {
    pub id: MessageId,
    pub thread_id: ThreadId,
    pub thread_count: usize,
    pub sender: String,
    pub subject: String,
    pub preview: String,
    pub date_secs: i64,
    pub unread: bool,
    pub flagged: bool,
    pub has_attachment: bool,
    pub mailbox: Mailbox,
    pub is_draft: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailError {
    MissingMessage,
    InvalidDestination,
    IdExhausted,
    NotADraft,
    InvalidRecipients(RecipientError),
    TooManyMessages,
    SubjectTooLong,
    BodyTooLarge,
    RecipientFieldTooLarge,
    TooManyAttachments,
    DocumentTooLarge,
    InvalidDocument(String),
}

impl fmt::Display for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MailError::MissingMessage => write!(f, "that message is gone"),
            MailError::InvalidDestination => write!(f, "that folder cannot receive this message"),
            MailError::IdExhausted => write!(f, "mail identifiers are exhausted"),
            MailError::NotADraft => write!(f, "not a draft"),
            MailError::InvalidRecipients(err) => write!(f, "{err}"),
            MailError::TooManyMessages => write!(f, "this mailbox is full (1,000 messages)"),
            MailError::SubjectTooLong => write!(f, "subject is longer than 256 characters"),
            MailError::BodyTooLarge => write!(f, "message body is larger than 16 KiB"),
            MailError::RecipientFieldTooLarge => write!(f, "recipient field is larger than 4 KiB"),
            MailError::TooManyAttachments => write!(f, "a message may have at most eight attachments"),
            MailError::DocumentTooLarge => write!(f, "mail document is larger than 1 MiB"),
            MailError::InvalidDocument(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecipientError {
    EmptyToken,
    InvalidAddress(String),
    TooMany,
    FieldTooLarge,
}

impl fmt::Display for RecipientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecipientError::EmptyToken => write!(f, "an address is empty"),
            RecipientError::InvalidAddress(raw) => write!(f, "invalid address `{raw}`"),
            RecipientError::TooMany => write!(f, "at most 20 recipients across To and Cc"),
            RecipientError::FieldTooLarge => write!(f, "recipient field is larger than 4 KiB"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutation {
    pub selected: Option<MessageId>,
    pub composer: Option<MessageId>,
    pub notice: Option<String>,
    pub left_results: bool,
}

pub fn preview_text(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 140 {
        collapsed
    } else {
        let mut out = String::new();
        for (i, ch) in collapsed.chars().enumerate() {
            if i >= 140 {
                break;
            }
            out.push(ch);
        }
        out.push('…');
        out
    }
}

pub fn normalize_search(text: &str) -> String {
    text.trim().to_lowercase()
}

pub fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '(' | ')' | '#' | '+' | '-' | '.'
            | '!' | '|' | '<' | '>' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

pub fn format_iso_timestamp(secs: i64) -> String {
    let day = (secs.div_euclid(86_400)) as i32;
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = makepad_civil_time::to_ymd(day);
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

pub fn format_list_date(date_secs: i64, today_midnight: i64) -> String {
    let day = date_secs.div_euclid(86_400) as i32;
    let today = today_midnight.div_euclid(86_400) as i32;
    if day == today {
        let tod = date_secs.rem_euclid(86_400);
        if tod == 0 {
            "Today".into()
        } else {
            format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60)
        }
    } else {
        let (y, m, d) = makepad_civil_time::to_ymd(day);
        let (ty, _, _) = makepad_civil_time::to_ymd(today);
        let month = makepad_civil_time::MONTH_ABBR[(m.saturating_sub(1) % 12) as usize];
        if y == ty {
            format!("{d} {month}")
        } else {
            format!("{d} {month} {y}")
        }
    }
}

pub fn format_reader_date(date_secs: i64) -> String {
    let day = date_secs.div_euclid(86_400) as i32;
    let tod = date_secs.rem_euclid(86_400);
    let (y, m, d) = makepad_civil_time::to_ymd(day);
    let month = makepad_civil_time::MONTH_ABBR[(m.saturating_sub(1) % 12) as usize];
    format!(
        "{d} {month} {y}  {:02}:{:02}",
        tod / 3600,
        (tod % 3600) / 60
    )
}

pub fn anchor_from_unix(secs: i64) -> SeedAnchor {
    let secs = secs.max(0);
    let day = (secs / 86_400) as i32;
    SeedAnchor {
        day: makepad_civil_time::format_iso(day),
        midnight_utc: day as i64 * 86_400,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_canonical_strings() {
        assert_eq!(MessageId(16).format(), "m-000016");
        assert_eq!(MessageId::parse("m-000016"), Some(MessageId(16)));
        assert_eq!(ThreadId(1016).format(), "t-001016");
        assert!(MessageId::parse("m-16").is_none());
        assert!(MessageId::parse("x-000016").is_none());
        assert!(ThreadId::parse("t-1").is_none());
        assert_eq!(MessageId(MAX_ID).format(), "m-999999");
        assert_eq!(MessageId::parse("m-999999"), Some(MessageId(MAX_ID)));
        assert_ne!(MessageId(1_000_000).format().len(), "m-000000".len());
        assert!(MessageId::parse(&MessageId(1_000_000).format()).is_none());
        assert!(ThreadId::parse(&ThreadId(1_000_000).format()).is_none());
    }

    #[test]
    fn initials_use_name_letters() {
        assert_eq!(Address::new("Alex Morgan", "alex@example.test").initials(), "AM");
        assert_eq!(Address::new("Iris de Vries", "iris.de.vries@example.test").initials(), "ID");
        assert_eq!(Address::new("", "studio.updates@example.test").initials(), "S");
    }

    #[test]
    fn markdown_escape_keeps_paragraphs_and_neutralizes_markup() {
        let escaped = escape_markdown("Hello **world**\n\nSee <script>");
        assert!(escaped.contains("\\*\\*world\\*\\*"));
        assert!(escaped.contains("\\<script\\>"));
        assert!(escaped.contains("\n\n"));
    }
}
