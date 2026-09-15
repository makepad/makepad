//! Pure mail operations. Time is always supplied; this module never reads a clock.

use crate::model::*;
use crate::seed::{self, PEOPLE};
use std::collections::{BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    Compact,
    WideTwo,
    WideThree,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutMetrics {
    pub kind: LayoutKind,
    pub short: bool,
    pub toolbar_h: f64,
    pub sidebar_w: f64,
    pub list_w: f64,
    pub reader_w: f64,
    pub show_sidebar: bool,
    pub row_h: f64,
    pub list_header_h: f64,
    pub bottom_reserve: f64,
    pub compose_w: f64,
    pub compose_h: f64,
    pub date_w: f64,
    pub preview_lines: u32,
    pub collapse_extra_actions: bool,
    pub status_visible: bool,
    pub text_left: f64,
    pub date_reserve: f64,
}

impl Default for LayoutMetrics {
    fn default() -> Self {
        layout_for(1240.0, 800.0)
    }
}

pub fn layout_for(width: f64, height: f64) -> LayoutMetrics {
    let width = width.max(0.0);
    let height = height.max(0.0);
    let short = height < 420.0;
    if width < 700.0 {
        LayoutMetrics {
            kind: LayoutKind::Compact,
            short,
            toolbar_h: 52.0,
            sidebar_w: 0.0,
            list_w: width,
            reader_w: width,
            show_sidebar: false,
            row_h: if short { 76.0 } else { 104.0 },
            list_header_h: if short { 44.0 } else { 52.0 },
            bottom_reserve: if short { 0.0 } else { 68.0 },
            compose_w: width,
            compose_h: (height - 24.0).max(0.0),
            date_w: 76.0,
            preview_lines: if short { 1 } else { 2 },
            collapse_extra_actions: false,
            status_visible: !short,
            text_left: 32.0,
            date_reserve: 76.0,
        }
    } else {
        let three = width >= 1100.0 && !short;
        let sidebar_w = if three { 220.0 } else { 0.0 };
        let list_w = if three { 360.0 } else { 310.0 };
        let divider = if three { 2.0 } else { 1.0 };
        let reader_w = (width - sidebar_w - list_w - divider).max(0.0);
        // Selector/title, seven 44-point actions, search, padding, and the
        // nine gaps around the flexible spacer must all fit on one row.
        let full_toolbar_w = if three { 220.0 } else { 140.0 }
            + 7.0 * 44.0 + 240.0 + 12.0 + 9.0 * 8.0;
        LayoutMetrics {
            kind: if three { LayoutKind::WideThree } else { LayoutKind::WideTwo },
            short,
            toolbar_h: if short { 48.0 } else { 56.0 },
            sidebar_w,
            list_w,
            reader_w,
            show_sidebar: three,
            row_h: if short { 76.0 } else { 84.0 },
            list_header_h: if short { 44.0 } else { 64.0 },
            bottom_reserve: if short { 0.0 } else { 24.0 },
            compose_w: 680.0,
            compose_h: (height - 16.0).max(0.0).min(600.0),
            date_w: 68.0,
            preview_lines: if short { 1 } else { 2 },
            collapse_extra_actions: short || width < full_toolbar_w,
            status_visible: !short,
            text_left: 28.0,
            date_reserve: 68.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellLayer {
    Loading,
    Error,
    Wide,
    Compact,
}

pub fn shell_layer(ready: bool, error: bool, compact: bool) -> ShellLayer {
    if error {
        ShellLayer::Error
    } else if !ready {
        ShellLayer::Loading
    } else if compact {
        ShellLayer::Compact
    } else {
        ShellLayer::Wide
    }
}

pub fn layout_needs_apply(
    last_kind: Option<(LayoutKind, bool)>,
    last_size: Option<(u32, u32)>,
    last_ready: Option<(bool, bool)>,
    metrics: LayoutMetrics,
    width: f64,
    height: f64,
    ready: bool,
    error: bool,
) -> bool {
    let size = (width.round() as u32, height.round() as u32);
    last_kind != Some((metrics.kind, metrics.short))
        || last_size != Some(size)
        || last_ready != Some((ready, error))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackScreen {
    Root,
    Messages,
    Message,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackStep {
    Stay,
    PopRoot,
    PopMessages,
    PushMessages,
    PushMessage,
}

pub fn stack_step(current: StackScreen, desired: Route) -> StackStep {
    let want = match desired {
        Route::Mailboxes => StackScreen::Root,
        Route::Messages => StackScreen::Messages,
        Route::Read(_) => StackScreen::Message,
    };
    if current == want {
        return StackStep::Stay;
    }
    match (current, want) {
        (_, StackScreen::Root) => StackStep::PopRoot,
        (StackScreen::Root, StackScreen::Messages) | (StackScreen::Root, StackScreen::Message) => {
            StackStep::PushMessages
        }
        (StackScreen::Messages, StackScreen::Message) => StackStep::PushMessage,
        (StackScreen::Message, StackScreen::Messages) => StackStep::PopMessages,
        _ => StackStep::Stay,
    }
}

pub fn mailbox_picker_size() -> (f64, f64) {
    (280.0, 244.0)
}

pub fn should_leave_reader_after_removal(left_results: bool, still_in_rows: bool) -> bool {
    left_results && !still_in_rows
}

pub fn quit_on_save_outcome(closing: bool, failed: bool, pending: bool) -> bool {
    closing && !failed && !pending
}

pub fn scroll_anchor_from_rows(rows: &[MessageRow], first_index: usize, offset: f64) -> ScrollAnchor {
    ScrollAnchor {
        first_message: rows.get(first_index).map(|r| r.id),
        offset_points: offset,
    }
}

pub fn restore_scroll_index(rows: &[MessageRow], anchor: &ScrollAnchor) -> Option<(usize, f64)> {
    let id = anchor.first_message?;
    let index = rows.iter().position(|r| r.id == id)?;
    Some((index, anchor.offset_points))
}

pub fn seed(anchor: SeedAnchor) -> MailDocument {
    seed::generate(anchor)
}

pub fn validate_document(doc: &MailDocument) -> Result<(), MailError> {
    if doc.schema_version != SCHEMA_VERSION {
        return Err(MailError::InvalidDocument(format!(
            "unsupported schema {}",
            doc.schema_version
        )));
    }
    if doc.messages.len() > MAX_MESSAGES {
        return Err(MailError::TooManyMessages);
    }
    if parse_bare_address(&doc.me.email).is_err() {
        return Err(MailError::InvalidDocument("identity is missing an email".into()));
    }
    if doc.seed_version != SEED_VERSION
        || makepad_civil_time::parse_iso(&doc.seed_anchor.day)
            .map(|day| day as i64 * 86_400) != Some(doc.seed_anchor.midnight_utc)
        || doc.vip_emails.iter().any(|email| parse_bare_address(email.trim()).is_err())
    {
        return Err(MailError::InvalidDocument("invalid seed anchor, version or VIP address".into()));
    }
    let mut seen = BTreeSet::new();
    let mut max_id = 0u32;
    let mut max_thread = 0u32;
    for message in &doc.messages {
        if !seen.insert(message.id) {
            return Err(MailError::InvalidDocument(format!("duplicate id {}", message.id)));
        }
        if message.id.0 == 0 || message.id.0 > MAX_ID
            || message.thread_id.0 == 0 || message.thread_id.0 > MAX_ID
        {
            return Err(MailError::IdExhausted);
        }
        if message.to.len() + message.cc.len() > MAX_RECIPIENTS {
            return Err(MailError::InvalidRecipients(RecipientError::TooMany));
        }
        if std::iter::once(&message.from).chain(&message.to).chain(&message.cc)
            .any(|address| parse_bare_address(&address.email).is_err())
        {
            return Err(MailError::InvalidDocument("invalid stored address".into()));
        }
        if message.attachments.iter().any(|a| a.size_bytes > i64::MAX as u64) {
            return Err(MailError::InvalidDocument("attachment size out of range".into()));
        }
        if message.kind == MessageKind::Sent && message.from != doc.me {
            return Err(MailError::InvalidDocument("sent message has another sender".into()));
        }
        max_id = max_id.max(message.id.0);
        max_thread = max_thread.max(message.thread_id.0);
        if message.subject.chars().count() > MAX_SUBJECT_CHARS {
            return Err(MailError::SubjectTooLong);
        }
        if message.body_text.len() > MAX_BODY_BYTES {
            return Err(MailError::BodyTooLarge);
        }
        if message.attachments.len() > MAX_ATTACHMENTS {
            return Err(MailError::TooManyAttachments);
        }
        if let Some(draft) = &message.draft_input {
            if draft.to_raw.len() > MAX_RECIPIENT_FIELD_BYTES || draft.cc_raw.len() > MAX_RECIPIENT_FIELD_BYTES {
                return Err(MailError::RecipientFieldTooLarge);
            }
            if message.kind != MessageKind::Draft || message.folder != Folder::Drafts {
                return Err(MailError::InvalidDocument(format!(
                    "{} is a draft in the wrong place",
                    message.id
                )));
            }
            if message.from != doc.me || !message.to.is_empty() || !message.cc.is_empty() {
                return Err(MailError::InvalidDocument("draft sender or parsed recipients are invalid".into()));
            }
            if message.unread {
                return Err(MailError::InvalidDocument(format!("{} draft is unread", message.id)));
            }
        } else if message.kind == MessageKind::Draft || message.folder == Folder::Drafts {
            return Err(MailError::InvalidDocument(format!("{} draft has no input", message.id)));
        }
        if let Some(parent) = message.in_reply_to {
            if parent == message.id || doc.message(parent).is_none_or(|p| p.thread_id != message.thread_id) {
                return Err(MailError::InvalidDocument(format!(
                    "{} replies to unknown {}",
                    message.id, parent
                )));
            }
        }
    }
    if doc.next_message_id > MAX_ID + 1 || doc.next_thread_id > MAX_ID + 1 {
        return Err(MailError::IdExhausted);
    }
    if doc.next_message_id <= max_id {
        return Err(MailError::InvalidDocument("next_message_id does not exceed allocated ids".into()));
    }
    if doc.next_thread_id <= max_thread {
        return Err(MailError::InvalidDocument("next_thread_id does not exceed allocated ids".into()));
    }
    Ok(())
}

pub fn mailbox_predicate(doc: &MailDocument, mailbox: Mailbox) -> impl Fn(&Message) -> bool + '_ {
    let vips: BTreeSet<String> = doc.vip_emails.iter().map(|e| e.trim().to_ascii_lowercase()).collect();
    move |message: &Message| match mailbox {
        Mailbox::Folder(folder) => message.folder == folder,
        Mailbox::Vips => {
            !matches!(message.folder, Folder::Drafts | Folder::Junk | Folder::Trash)
                && vips.contains(&message.from.email_key())
        }
        Mailbox::Flagged => {
            message.flagged && !matches!(message.folder, Folder::Drafts | Folder::Junk | Folder::Trash)
        }
    }
}

pub fn mailbox_messages(doc: &MailDocument, mailbox: Mailbox, unread_only: bool) -> Vec<MessageId> {
    let pred = mailbox_predicate(doc, mailbox);
    let mut ids: Vec<(i64, u32, MessageId)> = doc
        .messages
        .iter()
        .filter(|m| pred(m) && (!unread_only || m.unread))
        .map(|m| (m.date_secs, m.id.0, m.id))
        .collect();
    sort_ids(&mut ids);
    ids.into_iter().map(|(_, _, id)| id).collect()
}

pub fn mailbox_count(doc: &MailDocument, mailbox: Mailbox) -> usize {
    match mailbox {
        Mailbox::Folder(Folder::Drafts) => {
            doc.messages.iter().filter(|m| m.folder == Folder::Drafts).count()
        }
        other => mailbox_messages(doc, other, true).len(),
    }
}

fn sort_ids(ids: &mut [(i64, u32, MessageId)]) {
    ids.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
}

fn sort_messages<'a>(messages: &mut [&'a Message]) {
    messages.sort_by(|a, b| b.date_secs.cmp(&a.date_secs).then(a.id.0.cmp(&b.id.0)));
}

pub fn list_rows(doc: &MailDocument, context: &ListContext) -> Vec<MessageRow> {
    let query = context.query.trim();
    if !query.is_empty() {
        return search_rows(doc, query, context);
    }
    let ids = mailbox_messages(doc, context.mailbox, context.unread_only);
    // The permitted individual-message presentation keeps every member reachable.
    // Thread IDs/counts remain visible without a bounded conversation sub-list.
    ids.iter().filter_map(|id| doc.message(*id)).map(|message| {
        let count = if message.is_draft() { 1 } else { conversation(doc, message.id).len() };
        row_from_message(message, count, context.mailbox)
    }).collect()
}

fn row_from_message(message: &Message, thread_count: usize, mailbox: Mailbox) -> MessageRow {
    let sender = if message.is_draft() {
        let raw = message
            .draft_input
            .as_ref()
            .map(|d| d.to_raw.trim().to_string())
            .unwrap_or_default();
        if raw.is_empty() {
            "Draft".into()
        } else {
            raw
        }
    } else if message.kind == MessageKind::Sent {
        let to = message
            .to
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| message.from.name.clone());
        format!("To: {to}")
    } else {
        message.from.name.clone()
    };
    MessageRow {
        id: message.id,
        thread_id: message.thread_id,
        thread_count,
        sender,
        subject: message.subject_or_placeholder(),
        preview: message.preview(),
        date_secs: message.date_secs,
        unread: message.unread,
        flagged: message.flagged,
        has_attachment: !message.attachments.is_empty(),
        mailbox,
        is_draft: message.is_draft(),
    }
}

pub fn search(doc: &MailDocument, query: &str, scope: SearchScope) -> Vec<MessageId> {
    let needle = normalize_search(query);
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(i64, u32, MessageId)> = Vec::new();
    for message in &doc.messages {
        if scope == SearchScope::CurrentMailbox {
            // Caller intersects with the mailbox; All Mail is the search default.
        }
        if message_matches(message, &needle) {
            hits.push((message.date_secs, message.id.0, message.id));
        }
    }
    sort_ids(&mut hits);
    hits.into_iter().map(|(_, _, id)| id).collect()
}

fn search_rows(doc: &MailDocument, query: &str, context: &ListContext) -> Vec<MessageRow> {
    let needle = normalize_search(query);
    let pred = mailbox_predicate(doc, context.mailbox);
    let mut rows = Vec::new();
    for message in &doc.messages {
        let in_scope = match context.scope {
            SearchScope::CurrentMailbox => pred(message),
            SearchScope::AllMail => true,
        };
        if !in_scope || (context.unread_only && !message.unread) {
            continue;
        }
        if message_matches(message, &needle) {
            rows.push(row_from_message(message, 1, Mailbox::Folder(message.folder)));
        }
    }
    rows.sort_by(|a, b| b.date_secs.cmp(&a.date_secs).then(a.id.0.cmp(&b.id.0)));
    rows
}

fn message_matches(message: &Message, needle: &str) -> bool {
    contains_ci(&message.subject, needle)
        || contains_ci(&message.from.name, needle)
        || contains_ci(&message.from.email, needle)
        || contains_ci(&message.body_text, needle)

}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(needle)
}

pub fn conversation(doc: &MailDocument, selected_id: MessageId) -> Vec<MessageId> {
    let Some(selected) = doc.message(selected_id) else {
        return Vec::new();
    };
    let folder = selected.folder;
    let restrict = matches!(folder, Folder::Junk | Folder::Trash);
    let mut members: Vec<&Message> = doc
        .messages
        .iter()
        .filter(|m| {
            m.thread_id == selected.thread_id
                && (m.id == selected_id
                    || (!m.is_draft()
                        && if restrict {
                            m.folder == folder
                        } else {
                            !matches!(m.folder, Folder::Junk | Folder::Trash)
                        }))
        })
        .collect();
    sort_messages(&mut members);
    members.into_iter().map(|m| m.id).collect()
}

pub fn parse_recipients(raw: &str) -> Result<Vec<Address>, RecipientError> {
    parse_recipients_with(raw, PEOPLE)
}

pub fn parse_recipients_with(
    raw: &str,
    known: &[(&str, &str)],
) -> Result<Vec<Address>, RecipientError> {
    if raw.len() > MAX_RECIPIENT_FIELD_BYTES {
        return Err(RecipientError::FieldTooLarge);
    }
    if raw.trim().is_empty() { return Ok(Vec::new()); }
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for token in raw.split([',', ';']) {
        let trimmed = token.trim();
        if trimmed.is_empty() { return Err(RecipientError::EmptyToken); }
        let address = resolve_address(trimmed, known)?;
        if seen.insert(address.email_key()) { out.push(address); }
        if out.len() > MAX_RECIPIENTS { return Err(RecipientError::TooMany); }
    }
    Ok(out)
}

fn resolve_address(raw: &str, known: &[(&str, &str)]) -> Result<Address, RecipientError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RecipientError::EmptyToken);
    }
    parse_bare_address(trimmed).map_err(|_| RecipientError::InvalidAddress(trimmed.to_string()))?;
    let lower = trimmed.to_ascii_lowercase();
    for (name, email) in known {
        if email.eq_ignore_ascii_case(trimmed) {
            return Ok(Address::new(*name, *email));
        }
    }
    let name = lower
        .split('@')
        .next()
        .unwrap_or(&lower)
        .replace('.', " ");
    Ok(Address::new(title_case(&name), lower))
}

fn parse_bare_address(raw: &str) -> Result<(), ()> {
    if raw.chars().any(|c| c.is_whitespace() || c.is_control() || !c.is_ascii()) {
        return Err(());
    }
    let mut parts = raw.split('@');
    let local = parts.next().ok_or(())?;
    let domain = parts.next().ok_or(())?;
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return Err(());
    }
    if !domain.contains('.') || domain.split('.').any(str::is_empty) || raw.contains(['<', '>', '"', ',', ';']) {
        return Err(());
    }
    Ok(())
}

fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug)]
pub enum Command {
    SetRead { id: MessageId, unread: bool },
    SetFlagged { id: MessageId, flagged: bool },
    Move { id: MessageId, folder: Folder },
    Archive { id: MessageId },
    Delete { id: MessageId },
    NewDraft,
    Reply { id: MessageId },
    Forward { id: MessageId },
    EditDraft {
        id: MessageId,
        to_raw: String,
        cc_raw: String,
        subject: String,
        body: String,
    },
    DiscardDraft { id: MessageId },
    SendDraft { id: MessageId },
}

pub fn apply(doc: &mut MailDocument, command: Command, now_secs: i64) -> Result<Mutation, MailError> {
    let mut candidate = doc.clone();
    let mutation = apply_candidate(&mut candidate, command, now_secs)?;
    // Encoding is part of the transaction: never install an unsavable snapshot.
    crate::storage::encode_document(&candidate)?;
    *doc = candidate;
    Ok(mutation)
}

fn apply_candidate(doc: &mut MailDocument, command: Command, now_secs: i64) -> Result<Mutation, MailError> {
    match command {
        Command::SetRead { id, unread } => {
            let message = doc.message_mut(id).ok_or(MailError::MissingMessage)?;
            message.unread = unread;
            message.updated_secs = now_secs;
            Ok(Mutation { selected: Some(id), composer: None, notice: None, left_results: false })
        }
        Command::SetFlagged { id, flagged } => {
            let message = doc.message_mut(id).ok_or(MailError::MissingMessage)?;
            message.flagged = flagged;
            message.updated_secs = now_secs;
            Ok(Mutation { selected: Some(id), composer: None, notice: None, left_results: false })
        }
        Command::Move { id, folder } => {
            if !folder.is_move_destination() {
                return Err(MailError::InvalidDestination);
            }
            move_message(doc, id, folder, now_secs)
        }
        Command::Archive { id } => move_message(doc, id, Folder::Archive, now_secs),
        Command::Delete { id } => {
            let current = doc.message(id).ok_or(MailError::MissingMessage)?;
            if current.folder == Folder::Trash && !current.is_draft() {
                return Err(MailError::InvalidDestination);
            }
            if current.is_draft() {
                return Err(MailError::InvalidDocument("drafts use discard".into()));
            }
            move_message(doc, id, Folder::Trash, now_secs)
        }
        Command::NewDraft => new_draft(doc, now_secs, None, None, "", "", String::new(), Vec::new()),
        Command::Reply { id } => reply(doc, id, now_secs),
        Command::Forward { id } => forward(doc, id, now_secs),
        Command::EditDraft { id, to_raw, cc_raw, subject, body } => {
            edit_draft(doc, id, to_raw, cc_raw, subject, body, now_secs)
        }
        Command::DiscardDraft { id } => {
            let index = doc.messages.iter().position(|m| m.id == id).ok_or(MailError::MissingMessage)?;
            if !doc.messages[index].is_draft() {
                return Err(MailError::NotADraft);
            }
            doc.messages.remove(index);
            Ok(Mutation { selected: None, composer: None, notice: None, left_results: true })
        }
        Command::SendDraft { id } => send_draft(doc, id, now_secs),
    }
}

fn move_message(
    doc: &mut MailDocument,
    id: MessageId,
    folder: Folder,
    now_secs: i64,
) -> Result<Mutation, MailError> {
    let message = doc.message_mut(id).ok_or(MailError::MissingMessage)?;
    if message.is_draft() {
        return Err(MailError::InvalidDestination);
    }
    message.folder = folder;
    message.updated_secs = now_secs;
    Ok(Mutation { selected: Some(id), composer: None, notice: None, left_results: true })
}

fn alloc_message_id(doc: &mut MailDocument) -> Result<MessageId, MailError> {
    let id = doc.next_message_id;
    if id == 0 || id > MAX_ID { return Err(MailError::IdExhausted); }
    doc.next_message_id = id.checked_add(1).ok_or(MailError::IdExhausted)?;
    Ok(MessageId(id))
}

fn alloc_thread_id(doc: &mut MailDocument) -> Result<ThreadId, MailError> {
    let id = doc.next_thread_id;
    if id == 0 || id > MAX_ID { return Err(MailError::IdExhausted); }
    doc.next_thread_id = id.checked_add(1).ok_or(MailError::IdExhausted)?;
    Ok(ThreadId(id))
}

fn new_draft(
    doc: &mut MailDocument,
    now_secs: i64,
    thread_id: Option<ThreadId>,
    in_reply_to: Option<MessageId>,
    to_raw: &str,
    cc_raw: &str,
    subject: impl Into<String>,
    attachments: Vec<Attachment>,
) -> Result<Mutation, MailError> {
    if doc.messages.len() >= MAX_MESSAGES {
        return Err(MailError::TooManyMessages);
    }
    let id = alloc_message_id(doc)?;
    let thread_id = match thread_id {
        Some(id) => id,
        None => alloc_thread_id(doc)?,
    };
    let subject = subject.into();
    if subject.chars().count() > MAX_SUBJECT_CHARS {
        return Err(MailError::SubjectTooLong);
    }
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(MailError::TooManyAttachments);
    }
    doc.messages.push(Message {
        id,
        thread_id,
        in_reply_to,
        kind: MessageKind::Draft,
        folder: Folder::Drafts,
        from: doc.me.clone(),
        to: Vec::new(),
        cc: Vec::new(),
        subject,
        body_text: String::new(),
        attachments,
        date_secs: now_secs,
        updated_secs: now_secs,
        unread: false,
        flagged: false,
        draft_input: Some(DraftInput { to_raw: to_raw.into(), cc_raw: cc_raw.into() }),
    });
    Ok(Mutation { selected: None, composer: Some(id), notice: None, left_results: false })
}

fn prefix_once(subject: &str, prefix: &str) -> String {
    let trimmed = subject.trim();
    if trimmed.to_ascii_lowercase().starts_with(&prefix.trim_end().to_ascii_lowercase()) {
        trimmed.to_string()
    } else if trimmed.is_empty() {
        prefix.trim_end().to_string()
    } else {
        format!("{prefix}{trimmed}")
    }
}

fn quote_original(message: &Message) -> String {
    let date = format_reader_date(message.date_secs);
    format!(
        "\n\nOn {date}, {} wrote:\n{}",
        message.from.display(),
        message.body_text
    )
}

fn reply(doc: &mut MailDocument, id: MessageId, now_secs: i64) -> Result<Mutation, MailError> {
    let original = doc.message(id).cloned().ok_or(MailError::MissingMessage)?;
    let quoted = quote_original(&original);
    if quoted.len() > MAX_BODY_BYTES {
        return Err(MailError::BodyTooLarge);
    }
    let me_key = doc.me.email_key();
    let to_raw = if original.from.email_key() == me_key {
        original
            .to
            .iter()
            .filter(|a| a.email_key() != me_key)
            .map(|a| a.email.clone())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        original.from.email.clone()
    };
    let subject = prefix_once(&original.subject, "Re: ");
    let mut mutation = new_draft(
        doc,
        now_secs,
        Some(original.thread_id),
        Some(original.id),
        &to_raw,
        "",
        subject,
        Vec::new(),
    )?;
    if let Some(draft_id) = mutation.composer {
        if let Some(draft) = doc.message_mut(draft_id) {
            draft.body_text = quoted;
        }
    }
    mutation.selected = Some(id);
    Ok(mutation)
}

fn forward(doc: &mut MailDocument, id: MessageId, now_secs: i64) -> Result<Mutation, MailError> {
    let original = doc.message(id).cloned().ok_or(MailError::MissingMessage)?;
    let subject = prefix_once(&original.subject, "Fwd: ");
    let header = format!(
        "\n\n---------- Forwarded message ----------\nFrom: {}\nDate: {}\nSubject: {}\nTo: {}\n\n{}",
        original.from.display(),
        format_reader_date(original.date_secs),
        original.subject,
        original.to.iter().map(|a| a.display()).collect::<Vec<_>>().join(", "),
        original.body_text
    );
    if header.len() > MAX_BODY_BYTES {
        return Err(MailError::BodyTooLarge);
    }
    let mut mutation = new_draft(
        doc,
        now_secs,
        None,
        None,
        "",
        "",
        subject,
        original.attachments.clone(),
    )?;
    if let Some(draft_id) = mutation.composer {
        if let Some(draft) = doc.message_mut(draft_id) {
            draft.body_text = header;
        }
    }
    mutation.selected = Some(id);
    Ok(mutation)
}

fn edit_draft(
    doc: &mut MailDocument,
    id: MessageId,
    to_raw: String,
    cc_raw: String,
    subject: String,
    body: String,
    now_secs: i64,
) -> Result<Mutation, MailError> {
    if to_raw.len() > MAX_RECIPIENT_FIELD_BYTES || cc_raw.len() > MAX_RECIPIENT_FIELD_BYTES {
        return Err(MailError::RecipientFieldTooLarge);
    }
    if subject.chars().count() > MAX_SUBJECT_CHARS {
        return Err(MailError::SubjectTooLong);
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(MailError::BodyTooLarge);
    }
    let message = doc.message_mut(id).ok_or(MailError::MissingMessage)?;
    if !message.is_draft() {
        return Err(MailError::NotADraft);
    }
    message.draft_input = Some(DraftInput { to_raw, cc_raw });
    message.subject = subject;
    message.body_text = body;
    message.date_secs = now_secs;
    message.updated_secs = now_secs;
    Ok(Mutation { selected: None, composer: Some(id), notice: None, left_results: false })
}

fn send_draft(doc: &mut MailDocument, id: MessageId, now_secs: i64) -> Result<Mutation, MailError> {
    let me = doc.me.clone();
    let known = PEOPLE;
    let (to_raw, cc_raw) = {
        let message = doc.message(id).ok_or(MailError::MissingMessage)?;
        if !message.is_draft() {
            return Err(MailError::NotADraft);
        }
        let input = message.draft_input.as_ref().ok_or(MailError::NotADraft)?;
        (input.to_raw.clone(), input.cc_raw.clone())
    };
    let mut to = parse_recipients_with(&to_raw, known).map_err(MailError::InvalidRecipients)?;
    let mut cc = parse_recipients_with(&cc_raw, known).map_err(MailError::InvalidRecipients)?;
    if to.len() + cc.len() > MAX_RECIPIENTS {
        return Err(MailError::InvalidRecipients(RecipientError::TooMany));
    }
    if to.is_empty() && cc.is_empty() {
        return Err(MailError::InvalidRecipients(RecipientError::EmptyToken));
    }
    let mut seen = BTreeSet::new();
    to.retain(|a| seen.insert(a.email_key()));
    cc.retain(|a| seen.insert(a.email_key()));
    let message = doc.message_mut(id).ok_or(MailError::MissingMessage)?;
    message.kind = MessageKind::Sent;
    message.folder = Folder::Sent;
    message.from = me;
    message.to = to;
    message.cc = cc;
    message.draft_input = None;
    message.unread = false;
    message.date_secs = now_secs;
    message.updated_secs = now_secs;
    Ok(Mutation {
        selected: Some(id),
        composer: None,
        notice: Some("Saved to Sent · Demo".into()),
        left_results: true,
    })
}

pub fn reconcile_anchor(anchor: &mut ScrollAnchor, before: &[MessageRow], after: &[MessageRow]) {
    if anchor.first_message.is_some_and(|id| after.iter().any(|r| r.id == id)) { return; }
    let index = anchor.first_message.and_then(|id| before.iter().position(|r| r.id == id)).unwrap_or(0);
    // Prefer the closest surviving successor, then the closest predecessor.
    anchor.first_message = before.iter().skip(index).chain(before.iter().take(index).rev())
        .find(|old| after.iter().any(|new| new.id == old.id)).map(|r| r.id)
        .or_else(|| after.first().map(|r| r.id));
    if anchor.first_message.is_none() { anchor.offset_points = 0.0; }
}

pub fn next_surviving(rows: &[MessageRow], gone: MessageId) -> Option<MessageId> {
    if let Some(index) = rows.iter().position(|r| r.id == gone) {
        return rows.get(index + 1).or_else(|| rows.get(index.saturating_sub(1))).map(|r| r.id);
    }
    rows.first().map(|r| r.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::test_anchor;

    fn doc() -> MailDocument {
        seed(test_anchor())
    }

    #[test]
    fn layout_thresholds_match_the_contract() {
        assert_eq!(layout_for(402.0, 780.0).kind, LayoutKind::Compact);
        assert_eq!(layout_for(699.0, 800.0).kind, LayoutKind::Compact);
        assert_eq!(layout_for(700.0, 800.0).kind, LayoutKind::WideTwo);
        assert_eq!(layout_for(1240.0, 800.0).kind, LayoutKind::WideThree);
        let landscape = layout_for(874.0, 300.0);
        assert_eq!(landscape.kind, LayoutKind::WideTwo);
        assert!(landscape.short);
        assert!(landscape.show_sidebar == false);
        let negative = layout_for(-10.0, -4.0);
        assert_eq!(negative.kind, LayoutKind::Compact);
        assert_eq!(negative.list_w, 0.0);
    }

    #[test]
    fn every_folder_projects_and_virtual_mailboxes_exclude_junk() {
        let doc = doc();
        assert_eq!(mailbox_messages(&doc, Mailbox::Folder(Folder::Inbox), false).len(), 30);
        assert_eq!(mailbox_messages(&doc, Mailbox::Folder(Folder::Sent), false).len(), 8);
        assert_eq!(mailbox_messages(&doc, Mailbox::Folder(Folder::Drafts), false).len(), 3);
        assert_eq!(mailbox_messages(&doc, Mailbox::Folder(Folder::Archive), false).len(), 8);
        assert_eq!(mailbox_messages(&doc, Mailbox::Folder(Folder::Junk), false).len(), 2);
        assert_eq!(mailbox_messages(&doc, Mailbox::Folder(Folder::Trash), false).len(), 2);
        assert_eq!(mailbox_messages(&doc, Mailbox::Folder(Folder::Projects), false).len(), 4);
        assert_eq!(mailbox_messages(&doc, Mailbox::Folder(Folder::Travel), false).len(), 3);
        for id in mailbox_messages(&doc, Mailbox::Vips, false) {
            let m = doc.message(id).unwrap();
            assert!(!matches!(m.folder, Folder::Drafts | Folder::Junk | Folder::Trash));
            assert!(doc.vip_emails.iter().any(|e| e.eq_ignore_ascii_case(&m.from.email)));
        }
        for id in mailbox_messages(&doc, Mailbox::Flagged, false) {
            let m = doc.message(id).unwrap();
            assert!(m.flagged);
            assert!(!matches!(m.folder, Folder::Drafts | Folder::Junk | Folder::Trash));
        }
        assert_eq!(mailbox_count(&doc, Mailbox::Folder(Folder::Drafts)), 3);
        assert_eq!(
            mailbox_count(&doc, Mailbox::Folder(Folder::Inbox)),
            mailbox_messages(&doc, Mailbox::Folder(Folder::Inbox), true).len()
        );
    }

    #[test]
    fn individual_rows_keep_every_thread_member_and_draft_reachable() {
        let doc = doc();
        let ctx = ListContext { mailbox: Mailbox::Folder(Folder::Inbox), ..Default::default() };
        let rows = list_rows(&doc, &ctx);
        let harbor = rows.iter().find(|r| r.thread_id == ThreadId(1)).unwrap();
        assert_eq!(rows.len(), 30);
        assert_eq!(harbor.thread_count, 3);
        assert!(harbor.has_attachment, "id 2 carries the checklist");
        assert!(rows.iter().find(|r| r.id == MessageId(1)).unwrap().unread);
        let ctx = ListContext { mailbox: Mailbox::Folder(Folder::Drafts), ..Default::default() };
        let drafts = list_rows(&doc, &ctx);
        assert_eq!(drafts.len(), 3);
        assert!(drafts.iter().all(|r| r.is_draft && r.thread_count == 1));
    }

    #[test]
    fn search_is_literal_casefold_and_date_ordered() {
        let doc = doc();
        let hits = search(&doc, "  ROTTERDAM ", SearchScope::AllMail);
        assert!(!hits.is_empty());
        let mut last = i64::MAX;
        for id in &hits {
            let m = doc.message(*id).unwrap();
            assert!(m.date_secs <= last);
            last = m.date_secs;
            let blob = format!("{} {} {}", m.subject, m.from.email, m.body_text).to_lowercase();
            assert!(blob.contains("rotterdam"));
        }
        assert!(search(&doc, "zzzz-no-match", SearchScope::AllMail).is_empty());
        let punct = search(&doc, "NL-", SearchScope::AllMail);
        assert!(!punct.is_empty());
        let ctx = ListContext {
            mailbox: Mailbox::Folder(Folder::Inbox),
            query: "harbor".into(),
            scope: SearchScope::CurrentMailbox,
            unread_only: true,
            scroll: ScrollAnchor::default(),
        };
        let rows = list_rows(&doc, &ctx);
        assert!(rows.iter().all(|r| r.unread && r.mailbox == Mailbox::Folder(Folder::Inbox)));
        let all = ListContext { scope: SearchScope::AllMail, query: "harbor".into(), ..ctx };
        let all_rows = list_rows(&doc, &all);
        assert!(all_rows.len() >= rows.len());
    }

    #[test]
    fn conversation_keeps_selected_and_respects_junk() {
        let doc = doc();
        let ids = conversation(&doc, MessageId(1));
        assert_eq!(ids.first().copied(), Some(MessageId(31)));
        assert!(ids.contains(&MessageId(1)));
        assert!(ids.contains(&MessageId(2)));
        let junk = doc.message(MessageId(50)).unwrap();
        let only = conversation(&doc, junk.id);
        assert!(only.contains(&junk.id));
        assert!(only.iter().all(|id| doc.message(*id).unwrap().folder == Folder::Junk));
    }

    #[test]
    fn mutations_are_idempotent_and_checked() {
        let mut doc = doc();
        let now = doc.seed_anchor.midnight_utc;
        apply(&mut doc, Command::SetRead { id: MessageId(1), unread: false }, now).unwrap();
        apply(&mut doc, Command::SetRead { id: MessageId(1), unread: false }, now).unwrap();
        assert!(!doc.message(MessageId(1)).unwrap().unread);
        apply(&mut doc, Command::SetFlagged { id: MessageId(1), flagged: true }, now).unwrap();
        apply(&mut doc, Command::Archive { id: MessageId(1) }, now).unwrap();
        assert_eq!(doc.message(MessageId(1)).unwrap().folder, Folder::Archive);
        assert!(doc.message(MessageId(1)).unwrap().flagged);
        assert!(apply(&mut doc, Command::Move { id: MessageId(1), folder: Folder::Drafts }, now).is_err());
        assert!(apply(&mut doc, Command::SetRead { id: MessageId(9999), unread: false }, now).is_err());
        apply(&mut doc, Command::Delete { id: MessageId(3) }, now).unwrap();
        assert_eq!(doc.message(MessageId(3)).unwrap().folder, Folder::Trash);
        assert!(apply(&mut doc, Command::Delete { id: MessageId(3) }, now).is_err());
        doc.next_message_id = u32::MAX;
        assert!(matches!(
            apply(&mut doc, Command::NewDraft, now),
            Err(MailError::IdExhausted)
        ));
    }

    #[test]
    fn drafts_round_trip_incomplete_recipients_and_prefixes() {
        let mut doc = doc();
        let now = doc.seed_anchor.midnight_utc + 10;
        let draft = doc.message(MessageId(41)).unwrap();
        assert_eq!(draft.draft_input.as_ref().unwrap().to_raw, "theo@");
        assert!(draft.to.is_empty());
        let parsed = parse_recipients("MAYA.CHEN@example.test, maya.chen@example.test; iris.de.vries@example.test");
        let people = parsed.unwrap();
        assert_eq!(people.len(), 2);
        assert!(parse_recipients("ok@example.test,,x@example.test").is_err());
        assert!(parse_recipients("nope").is_err());
        assert!(parse_recipients("a@b").is_err());
        assert!(parse_recipients("a@b@c.com").is_err());
        assert!(parse_recipients("has space@example.test").is_err());
        let sent = doc.message(MessageId(31)).cloned().unwrap();
        let mutation = apply(&mut doc, Command::Reply { id: sent.id }, now).unwrap();
        let reply = doc.message(mutation.composer.unwrap()).unwrap().clone();
        assert_eq!(reply.thread_id, sent.thread_id);
        assert!(reply.subject.to_ascii_lowercase().starts_with("re:"));
        assert!(!reply.subject.to_ascii_lowercase()[3..].contains("re:"));
        assert!(reply.body_text.contains("wrote:"));
        let again = apply(&mut doc, Command::Reply { id: reply.id }, now + 1).unwrap();
        let reply2 = doc.message(again.composer.unwrap()).unwrap();
        let lower = reply2.subject.to_ascii_lowercase();
        assert_eq!(lower.matches("re:").count(), 1);
        let fwd = apply(&mut doc, Command::Forward { id: MessageId(2) }, now + 2).unwrap();
        let forwarded = doc.message(fwd.composer.unwrap()).unwrap();
        assert_ne!(forwarded.thread_id, ThreadId(1));
        assert!(forwarded.subject.to_ascii_lowercase().starts_with("fwd:"));
        assert_eq!(forwarded.attachments.len(), 1);
        assert_eq!(forwarded.draft_input.as_ref().unwrap().to_raw, "");
    }

    #[test]
    fn send_validates_then_converts_the_same_id() {
        let mut doc = doc();
        let now = doc.seed_anchor.midnight_utc + 50;
        let drafts_before = mailbox_count(&doc, Mailbox::Folder(Folder::Drafts));
        assert!(apply(&mut doc, Command::SendDraft { id: MessageId(41) }, now).is_err());
        assert_eq!(doc.message(MessageId(41)).unwrap().folder, Folder::Drafts);
        apply(
            &mut doc,
            Command::EditDraft {
                id: MessageId(39),
                to_raw: "maya.chen@example.test".into(),
                cc_raw: String::new(),
                subject: String::new(),
                body: String::new(),
            },
            now,
        )
        .unwrap();
        let sent = apply(&mut doc, Command::SendDraft { id: MessageId(39) }, now).unwrap();
        assert_eq!(sent.notice.as_deref(), Some("Saved to Sent · Demo"));
        let message = doc.message(MessageId(39)).unwrap();
        assert_eq!(message.folder, Folder::Sent);
        assert_eq!(message.kind, MessageKind::Sent);
        assert!(message.draft_input.is_none());
        assert_eq!(message.date_secs, now);
        assert_eq!(message.thread_id, ThreadId(1039));
        assert_eq!(mailbox_count(&doc, Mailbox::Folder(Folder::Drafts)), drafts_before - 1);
        assert!(matches!(
            apply(&mut doc, Command::SendDraft { id: MessageId(39) }, now + 1),
            Err(MailError::NotADraft)
        ));
        assert_eq!(doc.messages.iter().filter(|m| m.id == MessageId(39)).count(), 1);
    }

    #[test]
    fn opening_marks_only_the_selected_message() {
        let mut doc = doc();
        let now = doc.seed_anchor.midnight_utc;
        assert!(doc.message(MessageId(1)).unwrap().unread);
        assert!(doc.message(MessageId(3)).unwrap().unread);
        apply(&mut doc, Command::SetRead { id: MessageId(1), unread: false }, now).unwrap();
        assert!(!doc.message(MessageId(1)).unwrap().unread);
        assert!(doc.message(MessageId(3)).unwrap().unread);
    }
    #[test]
    fn oversized_quotes_and_document_growth_are_atomic() {
        for command in [Command::Reply {id: MessageId(1)}, Command::Forward {id: MessageId(1)}] {
            let mut doc = doc();
            doc.message_mut(MessageId(1)).unwrap().body_text = "x".repeat(MAX_BODY_BYTES);
            let before = doc.clone();
            assert_eq!(apply(&mut doc, command, 1), Err(MailError::BodyTooLarge));
            assert_eq!(doc, before);
        }
        let mut doc = doc();
        // Leave less than one draft body of space, with every message individually valid.
        for m in &mut doc.messages { m.body_text = "x".repeat(MAX_BODY_BYTES); }
        let mut next = doc.messages[15].clone();
        next.id = MessageId(61);
        next.in_reply_to = None;
        next.body_text.clear();
        doc.next_message_id = 62;
        doc.messages.push(next);
        let mut id = 61;
        loop {
            let remaining = MAX_DOCUMENT_BYTES - crate::storage::encode_document(&doc).unwrap().len();
            if remaining <= MAX_BODY_BYTES {
                doc.message_mut(MessageId(id)).unwrap().body_text = "x".repeat(remaining);
                break;
            }
            doc.message_mut(MessageId(id)).unwrap().body_text = "x".repeat(MAX_BODY_BYTES);
            id += 1;
            let mut next = doc.messages[15].clone();
            next.id = MessageId(id); next.in_reply_to = None; next.body_text.clear();
            doc.next_message_id = id + 1; doc.messages.push(next);
        }
        assert_eq!(crate::storage::encode_document(&doc).unwrap().len(), MAX_DOCUMENT_BYTES);
        let before = doc.clone();
        assert_eq!(apply(&mut doc, Command::NewDraft, 1), Err(MailError::DocumentTooLarge));
        assert_eq!(doc, before);
    }

    #[test]
    fn canonical_id_exhaustion_never_consumes_another_counter() {
        for (messages, threads) in [(MAX_ID + 1, 1061), (61, MAX_ID + 1), (u32::MAX, 1061)] {
            let mut doc = doc();
            doc.next_message_id = messages;
            doc.next_thread_id = threads;
            let before = doc.clone();
            assert_eq!(apply(&mut doc, Command::NewDraft, 1), Err(MailError::IdExhausted));
            assert_eq!(doc, before);
        }
        let mut doc = doc();
        doc.next_message_id = MAX_ID;
        doc.next_thread_id = MAX_ID;
        let id = apply(&mut doc, Command::NewDraft, 1).unwrap().composer.unwrap();
        assert_eq!(id, MessageId(MAX_ID));
        let bytes = crate::storage::encode_document(&doc).unwrap();
        assert_eq!(crate::storage::decode_document(&bytes).unwrap(), doc);
        let before = doc.clone();
        assert_eq!(apply(&mut doc, Command::NewDraft, 1), Err(MailError::IdExhausted));
        assert_eq!(doc, before);
    }

    #[test]
    fn bare_addresses_precede_name_resolution_and_prefixes_need_no_space() {
        for raw in ["Maya Chen", "maya.chen", "a@.test", "a@b.", ",a@b.test", "a@b.test,,c@d.test"] {
            assert!(parse_recipients(raw).is_err(), "{raw}");
        }
        let people = parse_recipients("MAYA.CHEN@example.test; maya.chen@example.test").unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].name, "Maya Chen");
        let mut doc = doc();
        for (prefix, command) in [("Re:hello", Command::Reply {id: MessageId(1)}), ("fWd:hello", Command::Forward {id: MessageId(1)})] {
            doc.message_mut(MessageId(1)).unwrap().subject = prefix.into();
            let id = apply(&mut doc, command, 1).unwrap().composer.unwrap();
            assert_eq!(doc.message(id).unwrap().subject, prefix);
        }
        assert_eq!(prefix_once("", "Re: "), "Re:");
    }

    #[test]
    fn search_uses_only_subject_sender_body_with_unicode_and_date_ties() {
        let mut doc = doc();
        for m in &mut doc.messages {
            m.subject = "plain".into(); m.body_text = "plain".into();
            m.from = Address::new("Sender", "sender@example.test");
        }
        assert!(search(&doc, "alex@example.test", SearchScope::AllMail).is_empty());
        for (id, field) in [(1, "subject"), (2, "name"), (3, "email"), (4, "body")] {
            let m = doc.message_mut(MessageId(id)).unwrap();
            m.date_secs = 100;
            match field {
                "subject" => m.subject = "CAFÉ.*".into(),
                "name" => m.from.name = "CAFÉ.*".into(),
                "email" => m.from.email = "café.*@example.test".into(),
                _ => m.body_text = "CAFÉ.*".into(),
            }
        }
        assert_eq!(search(&doc, " café.* ", SearchScope::AllMail), vec![MessageId(1), MessageId(2), MessageId(3), MessageId(4)]);
        assert!(search(&doc, "cafe", SearchScope::AllMail).is_empty());
        let context = ListContext {query: "café.*".into(), scope: SearchScope::AllMail, ..Default::default()};
        assert!(list_rows(&doc, &context).iter().all(|r| r.mailbox == Mailbox::Folder(Folder::Inbox)));
    }

    #[test]
    fn long_conversations_expose_every_concrete_message() {
        let mut doc = doc();
        for i in 16..=25 { doc.message_mut(MessageId(i)).unwrap().thread_id = ThreadId(1); }
        let rows = list_rows(&doc, &ListContext::default());
        assert_eq!(rows.len(), 30);
        let members = conversation(&doc, MessageId(1));
        assert_eq!(members.len(), 13);
        for id in members {
            if doc.message(id).unwrap().folder == Folder::Inbox {
                let row = rows.iter().find(|r| r.id == id).unwrap();
                assert_eq!(row.thread_count, 13);
            }
        }
    }

    #[test]
    fn removed_scroll_anchor_uses_nearest_surviving_message() {
        let rows = list_rows(&doc(), &ListContext::default());
        let mut anchor = ScrollAnchor {first_message: Some(rows[5].id), offset_points: -19.0};
        let mut after = rows.clone(); after.remove(5);
        reconcile_anchor(&mut anchor, &rows, &after);
        assert_eq!(anchor.first_message, Some(rows[6].id));
        assert_eq!(anchor.offset_points, -19.0);
        reconcile_anchor(&mut anchor, &after, &[]);
        assert_eq!(anchor, ScrollAnchor::default());
    }

    #[test]
    fn edit_draft_rejects_oversize_subject_and_body_without_mutating() {
        let mut doc = doc();
        let before = doc.message(MessageId(39)).unwrap().clone();
        let to_raw = before.draft_input.as_ref().unwrap().to_raw.clone();
        assert!(matches!(
            apply(
                &mut doc,
                Command::EditDraft {
                    id: MessageId(39),
                    to_raw: to_raw.clone(),
                    cc_raw: String::new(),
                    subject: "x".repeat(257),
                    body: before.body_text.clone(),
                },
                1,
            ),
            Err(MailError::SubjectTooLong)
        ));
        assert_eq!(doc.message(MessageId(39)).unwrap().subject, before.subject);
        assert!(matches!(
            apply(
                &mut doc,
                Command::EditDraft {
                    id: MessageId(39),
                    to_raw,
                    cc_raw: String::new(),
                    subject: before.subject.clone(),
                    body: "x".repeat(MAX_BODY_BYTES + 1),
                },
                1,
            ),
            Err(MailError::BodyTooLarge)
        ));
        assert_eq!(doc.message(MessageId(39)).unwrap().body_text, before.body_text);
    }

    #[test]
    fn short_and_compact_metrics_are_applied_and_compose_follows_width() {
        let landscape = layout_for(874.0, 300.0);
        assert_eq!(landscape.toolbar_h, 48.0);
        assert_eq!(landscape.row_h, 76.0);
        assert_eq!(landscape.list_header_h, 44.0);
        assert_eq!(landscape.list_w, 310.0);
        assert_eq!(landscape.compose_w, 680.0);
        assert_eq!(landscape.compose_h, 284.0);
        assert!(landscape.collapse_extra_actions);
        assert!(!landscape.status_visible);
        assert_eq!(landscape.preview_lines, 1);
        let compact = layout_for(402.0, 780.0);
        assert_eq!(compact.compose_w, 402.0);
        assert_eq!(compact.row_h, 104.0);
        assert_eq!(compact.date_w, 76.0);
        assert_eq!(layout_for(600.0, 780.0).compose_w, 600.0);
        assert_eq!(layout_for(1240.0, 800.0).row_h, 84.0);
        assert_eq!(layout_for(1240.0, 800.0).date_w, 68.0);
        assert!(!layout_for(800.0, 800.0).collapse_extra_actions);
    }

    #[test]
    fn load_state_transition_applies_shell_without_a_layout_class_change() {
        let metrics = layout_for(1240.0, 800.0);
        assert!(!layout_needs_apply(
            Some((metrics.kind, metrics.short)),
            Some((1240, 800)),
            Some((false, false)),
            metrics,
            1240.0,
            800.0,
            false,
            false,
        ));
        assert!(layout_needs_apply(
            Some((metrics.kind, metrics.short)),
            Some((1240, 800)),
            Some((false, false)),
            metrics,
            1240.0,
            800.0,
            true,
            false,
        ));
        assert_eq!(shell_layer(false, false, false), ShellLayer::Loading);
        assert_eq!(shell_layer(true, false, false), ShellLayer::Wide);
        assert_eq!(shell_layer(true, false, true), ShellLayer::Compact);
        assert_eq!(shell_layer(false, true, false), ShellLayer::Error);
    }

    #[test]
    fn stack_reconciles_one_step_from_root_to_reader() {
        let id = MessageId(1);
        assert_eq!(stack_step(StackScreen::Root, Route::Read(id)), StackStep::PushMessages);
        assert_eq!(stack_step(StackScreen::Messages, Route::Read(id)), StackStep::PushMessage);
        assert_eq!(stack_step(StackScreen::Message, Route::Read(id)), StackStep::Stay);
        assert_eq!(stack_step(StackScreen::Message, Route::Messages), StackStep::PopMessages);
        assert_eq!(stack_step(StackScreen::Messages, Route::Mailboxes), StackStep::PopRoot);
        assert_eq!(stack_step(StackScreen::Root, Route::Mailboxes), StackStep::Stay);
    }

    #[test]
    fn failed_close_retains_the_instance_and_removal_leaves_unread_reader() {
        assert!(!quit_on_save_outcome(true, true, true));
        assert!(quit_on_save_outcome(true, false, false));
        assert!(should_leave_reader_after_removal(true, false));
        assert!(!should_leave_reader_after_removal(true, true));
        assert!(!should_leave_reader_after_removal(false, false));
        assert_eq!(mailbox_picker_size(), (280.0, 244.0));
        assert_eq!(Mailbox::ALL.len(), 10);
        assert_eq!(MAX_ATTACHMENTS, 8);
        let rows = list_rows(&doc(), &ListContext::default());
        let anchor = scroll_anchor_from_rows(&rows, 4, -12.0);
        assert_eq!(restore_scroll_index(&rows, &anchor), Some((4, -12.0)));
    }

}
