//! One JSON document, `mail.json`. Strict JSON, UTF-8, schema 1.

use crate::engine::validate_document;
use crate::model::*;
use makepad_strict_json::{parse, Value};

pub const STORAGE_KEY: &str = "mail.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Ignore,
    Ack { revision: u64, submit_queued: Option<u64> },
    Fail { revision: u64, retry: u64 },
}

#[derive(Clone, Debug, Default)]
pub struct SaveQueue {
    pub revision: u64,
    pub acked: u64,
    pub in_flight: Option<(u64, u64)>,
    pub queued: Option<u64>,
}

impl SaveQueue {
    pub fn note_dirty(&mut self) -> u64 {
        self.revision = self.revision.saturating_add(1);
        self.queued = Some(self.revision);
        self.revision
    }

    pub fn take_submit(&mut self) -> Option<u64> {
        if self.in_flight.is_some() {
            return None;
        }
        self.queued = None;
        if self.revision > self.acked {
            Some(self.revision)
        } else {
            None
        }
    }

    pub fn begin(&mut self, request: u64, revision: u64) {
        self.in_flight = Some((request, revision));
        if self.queued == Some(revision) {
            self.queued = None;
        }
    }

    pub fn on_response(&mut self, request: u64, ok: bool) -> SaveOutcome {
        match self.in_flight {
            Some((rid, rev)) if rid == request => {
                self.in_flight = None;
                if ok {
                    self.acked = self.acked.max(rev);
                    self.queued = (self.revision > self.acked).then_some(self.revision);
                    SaveOutcome::Ack { revision: rev, submit_queued: (self.revision > self.acked).then_some(self.revision) }
                } else {
                    let retry = self.revision;
                    self.queued = Some(retry);
                    SaveOutcome::Fail { revision: rev, retry }
                }
            }
            _ => SaveOutcome::Ignore,
        }
    }

    pub fn pending(&self) -> bool {
        self.in_flight.is_some() || self.queued.is_some() || self.revision > self.acked
    }
}

pub fn encode_document(doc: &MailDocument) -> Result<Vec<u8>, MailError> {
    validate_document(doc)?;
    let json = encode_value(doc).to_json();
    let bytes = json.into_bytes();
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(MailError::DocumentTooLarge);
    }
    Ok(bytes)
}

pub fn decode_document(bytes: &[u8]) -> Result<MailDocument, MailError> {
    if bytes.is_empty() {
        return Err(MailError::InvalidDocument("existing document is empty".into()));
    }
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(MailError::DocumentTooLarge);
    }
    let value = parse(bytes).map_err(|e| MailError::InvalidDocument(e.into()))?;
    let doc = decode_value(&value)?;
    validate_document(&doc)?;
    Ok(doc)
}

fn encode_value(doc: &MailDocument) -> Value {
    Value::Obj(vec![
        ("schema_version".into(), Value::Int(doc.schema_version as i64)),
        ("seed_version".into(), Value::Int(doc.seed_version as i64)),
        (
            "seed_anchor".into(),
            Value::Obj(vec![
                ("day".into(), Value::Str(doc.seed_anchor.day.clone())),
                ("midnight_utc".into(), Value::Int(doc.seed_anchor.midnight_utc)),
            ]),
        ),
        ("next_message_id".into(), Value::Int(doc.next_message_id as i64)),
        ("next_thread_id".into(), Value::Int(doc.next_thread_id as i64)),
        ("me".into(), encode_address(&doc.me)),
        (
            "vip_emails".into(),
            Value::Arr(doc.vip_emails.iter().cloned().map(Value::Str).collect()),
        ),
        (
            "messages".into(),
            Value::Arr(doc.messages.iter().map(encode_message).collect()),
        ),
    ])
}

fn encode_address(address: &Address) -> Value {
    Value::Obj(vec![
        ("name".into(), Value::Str(address.name.clone())),
        ("email".into(), Value::Str(address.email.clone())),
    ])
}

fn encode_message(message: &Message) -> Value {
    Value::Obj(vec![
        ("id".into(), Value::Str(message.id.format())),
        ("thread_id".into(), Value::Str(message.thread_id.format())),
        (
            "in_reply_to".into(),
            match message.in_reply_to {
                Some(id) => Value::Str(id.format()),
                None => Value::Null,
            },
        ),
        ("kind".into(), Value::Str(message.kind.as_str().into())),
        ("folder".into(), Value::Str(message.folder.as_str().into())),
        ("from".into(), encode_address(&message.from)),
        ("to".into(), Value::Arr(message.to.iter().map(encode_address).collect())),
        ("cc".into(), Value::Arr(message.cc.iter().map(encode_address).collect())),
        ("subject".into(), Value::Str(message.subject.clone())),
        ("body_text".into(), Value::Str(message.body_text.clone())),
        (
            "attachments".into(),
            Value::Arr(message.attachments.iter().map(encode_attachment).collect()),
        ),
        ("date_secs".into(), Value::Int(message.date_secs)),
        ("updated_secs".into(), Value::Int(message.updated_secs)),
        ("unread".into(), Value::Bool(message.unread)),
        ("flagged".into(), Value::Bool(message.flagged)),
        (
            "draft_input".into(),
            match &message.draft_input {
                Some(input) => Value::Obj(vec![
                    ("to_raw".into(), Value::Str(input.to_raw.clone())),
                    ("cc_raw".into(), Value::Str(input.cc_raw.clone())),
                ]),
                None => Value::Null,
            },
        ),
    ])
}

fn encode_attachment(attachment: &Attachment) -> Value {
    Value::Obj(vec![
        ("name".into(), Value::Str(attachment.name.clone())),
        ("mime".into(), Value::Str(attachment.mime.clone())),
        ("size_bytes".into(), Value::Int(attachment.size_bytes as i64)),
    ])
}

fn decode_value(value: &Value) -> Result<MailDocument, MailError> {
    let obj = expect_obj(value)?;
    require_keys(obj, &[
        "schema_version",
        "seed_version",
        "seed_anchor",
        "next_message_id",
        "next_thread_id",
        "me",
        "vip_emails",
        "messages",
    ])?;
    let schema_version = expect_u32(get(obj, "schema_version")?)?;
    if schema_version != SCHEMA_VERSION {
        return Err(MailError::InvalidDocument(format!("unsupported schema {schema_version}")));
    }
    let seed_anchor = decode_anchor(get(obj, "seed_anchor")?)?;
    let vip = expect_arr(get(obj, "vip_emails")?)?;
    let messages = expect_arr(get(obj, "messages")?)?;
    Ok(MailDocument {
        schema_version,
        seed_version: expect_u32(get(obj, "seed_version")?)?,
        seed_anchor,
        next_message_id: expect_u32(get(obj, "next_message_id")?)?,
        next_thread_id: expect_u32(get(obj, "next_thread_id")?)?,
        me: decode_address(get(obj, "me")?)?,
        vip_emails: vip
            .iter()
            .map(|v| expect_str(v).map(|s| s.to_string()))
            .collect::<Result<Vec<_>, _>>()?,
        messages: messages.iter().map(decode_message).collect::<Result<Vec<_>, _>>()?,
    })
}

fn decode_anchor(value: &Value) -> Result<SeedAnchor, MailError> {
    let obj = expect_obj(value)?;
    require_keys(obj, &["day", "midnight_utc"])?;
    Ok(SeedAnchor {
        day: expect_str(get(obj, "day")?)?.to_string(),
        midnight_utc: expect_i64(get(obj, "midnight_utc")?)?,
    })
}

fn decode_address(value: &Value) -> Result<Address, MailError> {
    let obj = expect_obj(value)?;
    require_keys(obj, &["name", "email"])?;
    Ok(Address {
        name: expect_str(get(obj, "name")?)?.to_string(),
        email: expect_str(get(obj, "email")?)?.to_string(),
    })
}

fn decode_message(value: &Value) -> Result<Message, MailError> {
    let obj = expect_obj(value)?;
    require_keys(obj, &[
        "id",
        "thread_id",
        "in_reply_to",
        "kind",
        "folder",
        "from",
        "to",
        "cc",
        "subject",
        "body_text",
        "attachments",
        "date_secs",
        "updated_secs",
        "unread",
        "flagged",
        "draft_input",
    ])?;
    let id = MessageId::parse(expect_str(get(obj, "id")?)?)
        .ok_or_else(|| MailError::InvalidDocument("malformed message id".into()))?;
    let thread_id = ThreadId::parse(expect_str(get(obj, "thread_id")?)?)
        .ok_or_else(|| MailError::InvalidDocument("malformed thread id".into()))?;
    let in_reply_to = match get(obj, "in_reply_to")? {
        Value::Null => None,
        Value::Str(s) => Some(
            MessageId::parse(s).ok_or_else(|| MailError::InvalidDocument("malformed in_reply_to".into()))?,
        ),
        _ => return Err(MailError::InvalidDocument("in_reply_to must be a string or null".into())),
    };
    let kind = MessageKind::parse(expect_str(get(obj, "kind")?)?)
        .ok_or_else(|| MailError::InvalidDocument("unknown message kind".into()))?;
    let folder = Folder::parse(expect_str(get(obj, "folder")?)?)
        .ok_or_else(|| MailError::InvalidDocument("unknown folder".into()))?;
    let draft_input = match get(obj, "draft_input")? {
        Value::Null => None,
        other => Some(decode_draft(other)?),
    };
    Ok(Message {
        id,
        thread_id,
        in_reply_to,
        kind,
        folder,
        from: decode_address(get(obj, "from")?)?,
        to: expect_arr(get(obj, "to")?)?
            .iter()
            .map(decode_address)
            .collect::<Result<Vec<_>, _>>()?,
        cc: expect_arr(get(obj, "cc")?)?
            .iter()
            .map(decode_address)
            .collect::<Result<Vec<_>, _>>()?,
        subject: expect_str(get(obj, "subject")?)?.to_string(),
        body_text: expect_str(get(obj, "body_text")?)?.to_string(),
        attachments: expect_arr(get(obj, "attachments")?)?
            .iter()
            .map(decode_attachment)
            .collect::<Result<Vec<_>, _>>()?,
        date_secs: expect_i64(get(obj, "date_secs")?)?,
        updated_secs: expect_i64(get(obj, "updated_secs")?)?,
        unread: expect_bool(get(obj, "unread")?)?,
        flagged: expect_bool(get(obj, "flagged")?)?,
        draft_input,
    })
}

fn decode_draft(value: &Value) -> Result<DraftInput, MailError> {
    let obj = expect_obj(value)?;
    require_keys(obj, &["to_raw", "cc_raw"])?;
    Ok(DraftInput {
        to_raw: expect_str(get(obj, "to_raw")?)?.to_string(),
        cc_raw: expect_str(get(obj, "cc_raw")?)?.to_string(),
    })
}

fn decode_attachment(value: &Value) -> Result<Attachment, MailError> {
    let obj = expect_obj(value)?;
    require_keys(obj, &["name", "mime", "size_bytes"])?;
    Ok(Attachment {
        name: expect_str(get(obj, "name")?)?.to_string(),
        mime: expect_str(get(obj, "mime")?)?.to_string(),
        size_bytes: expect_u64(get(obj, "size_bytes")?)?,
    })
}

fn expect_obj(value: &Value) -> Result<&Vec<(String, Value)>, MailError> {
    match value {
        Value::Obj(pairs) => Ok(pairs),
        _ => Err(MailError::InvalidDocument("expected object".into())),
    }
}

fn expect_arr(value: &Value) -> Result<&[Value], MailError> {
    value.as_arr().ok_or_else(|| MailError::InvalidDocument("expected array".into()))
}

fn expect_str(value: &Value) -> Result<&str, MailError> {
    value.as_str().ok_or_else(|| MailError::InvalidDocument("expected string".into()))
}

fn expect_bool(value: &Value) -> Result<bool, MailError> {
    value.as_bool().ok_or_else(|| MailError::InvalidDocument("expected bool".into()))
}

fn expect_i64(value: &Value) -> Result<i64, MailError> {
    value.as_i64().ok_or_else(|| MailError::InvalidDocument("expected integer".into()))
}

fn expect_u32(value: &Value) -> Result<u32, MailError> {
    let n = expect_i64(value)?;
    u32::try_from(n).map_err(|_| MailError::InvalidDocument("integer out of range".into()))
}

fn expect_u64(value: &Value) -> Result<u64, MailError> {
    let n = expect_i64(value)?;
    u64::try_from(n).map_err(|_| MailError::InvalidDocument("integer out of range".into()))
}

fn get<'a>(obj: &'a [(String, Value)], key: &str) -> Result<&'a Value, MailError> {
    obj.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .ok_or_else(|| MailError::InvalidDocument(format!("missing {key}")))
}

fn require_keys(obj: &[(String, Value)], keys: &[&str]) -> Result<(), MailError> {
    for (k, _) in obj {
        if !keys.contains(&k.as_str()) {
            return Err(MailError::InvalidDocument(format!("unknown field {k}")));
        }
    }
    for key in keys {
        if !obj.iter().any(|(k, _)| k == key) {
            return Err(MailError::InvalidDocument(format!("missing {key}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::{generate, test_anchor};

    #[test]
    fn round_trip_unicode_newlines_and_nulls() {
        let mut doc = generate(test_anchor());
        let message = doc.message_mut(MessageId(16)).unwrap();
        message.subject = "Harbor — café\npreview".into();
        message.body_text = "Line one\n\nLine two with 日本語".into();
        message.in_reply_to = None;
        message.draft_input = None;
        let bytes = encode_document(&doc).unwrap();
        let again = decode_document(&bytes).unwrap();
        let message = again.message(MessageId(16)).unwrap();
        assert_eq!(message.subject, "Harbor — café\npreview");
        assert_eq!(message.body_text, "Line one\n\nLine two with 日本語");
        assert!(message.draft_input.is_none());
        assert!(message.in_reply_to.is_none());
        let draft = again.message(MessageId(41)).unwrap();
        assert_eq!(draft.draft_input.as_ref().unwrap().to_raw, "theo@");
    }

    #[test]
    fn decode_rejects_malformed_and_wrong_schema() {
        assert!(decode_document(b"").is_err());
        assert!(decode_document(b"[]").is_err());
        assert!(decode_document(b"{\"schema_version\":1} trailing").is_err());
        assert!(decode_document(br#"{"schema_version":1,"schema_version":1}"#).is_err());
        let mut json = String::from_utf8(encode_document(&generate(test_anchor())).unwrap()).unwrap();
        json = json.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
        assert!(decode_document(json.as_bytes()).is_err());
        assert!(decode_document(br#"{"schema_version":"1"}"#).is_err());
    }

    #[test]
    fn save_queue_keeps_the_latest_revision() {
        let mut q = SaveQueue::default();
        let r1 = q.note_dirty();
        assert_eq!(q.take_submit(), Some(r1));
        q.begin(10, r1);
        let r2 = q.note_dirty();
        let r3 = q.note_dirty();
        assert_eq!(q.on_response(99, true), SaveOutcome::Ignore);
        match q.on_response(10, true) {
            SaveOutcome::Ack { revision, submit_queued } => {
                assert_eq!(revision, r1);
                assert_eq!(submit_queued, Some(r3));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(r2 + 1, r3);
        q.begin(11, r3);
        match q.on_response(11, false) {
            SaveOutcome::Fail { revision, retry } => {
                assert_eq!(revision, r3);
                assert_eq!(retry, r3);
            }
            other => panic!("{other:?}"),
        }
    }
    #[test]
    fn failed_r1_queued_r2_then_edit_r3_submits_r3_and_drains() {
        let mut queue = SaveQueue::default();
        let r1 = queue.note_dirty(); queue.begin(10, r1);
        queue.note_dirty();
        assert_eq!(queue.on_response(10, false), SaveOutcome::Fail {revision: 1, retry: 2});
        let r3 = queue.note_dirty();
        assert_eq!(queue.take_submit(), Some(r3));
        queue.begin(11, r3);
        assert_eq!(queue.on_response(10, true), SaveOutcome::Ignore);
        queue.note_dirty();
        assert_eq!(queue.on_response(11, true), SaveOutcome::Ack {revision: 3, submit_queued: Some(4)});
        assert!(queue.pending());
        assert_eq!(queue.take_submit(), Some(4));
        queue.begin(12, 4);
        assert_eq!(queue.on_response(12, true), SaveOutcome::Ack {revision: 4, submit_queued: None});
        assert!(!queue.pending());
        assert_eq!(queue.take_submit(), None);
    }

    #[test]
    fn otherwise_valid_storage_rejects_every_draft_and_reference_violation() {
        let base = generate(test_anchor());
        let reject = |doc: &MailDocument, expected: &str| {
            // Bypass the encoder's validation to exercise the loader with a complete fixture.
            let bytes = encode_value(doc).to_json().into_bytes();
            let err = decode_document(&bytes).unwrap_err().to_string();
            assert!(err.contains(expected), "expected {expected}, got {err}");
        };
        let mut doc = base.clone(); doc.message_mut(MessageId(39)).unwrap().from = base.messages[0].from.clone();
        reject(&doc, "draft sender");
        let mut doc = base.clone(); doc.message_mut(MessageId(39)).unwrap().to.push(base.me.clone());
        reject(&doc, "parsed recipients");
        let mut doc = base.clone(); doc.message_mut(MessageId(16)).unwrap().folder = Folder::Drafts;
        reject(&doc, "draft");
        let mut doc = base.clone(); doc.message_mut(MessageId(16)).unwrap().to = vec![base.me.clone(); 21];
        reject(&doc, "20 recipients");
        let mut doc = base.clone(); doc.next_message_id = 101; doc.message_mut(MessageId(16)).unwrap().in_reply_to = Some(MessageId(100));
        reject(&doc, "replies to unknown");
        let mut doc = base.clone(); doc.message_mut(MessageId(16)).unwrap().in_reply_to = Some(MessageId(16));
        reject(&doc, "replies to unknown");
        let mut doc = base.clone(); doc.message_mut(MessageId(16)).unwrap().in_reply_to = Some(MessageId(1));
        reject(&doc, "replies to unknown");
        let mut doc = base.clone(); doc.message_mut(MessageId(31)).unwrap().from = base.messages[0].from.clone();
        reject(&doc, "another sender");
        let mut doc = base.clone(); doc.message_mut(MessageId(16)).unwrap().body_text = "x".repeat(MAX_BODY_BYTES + 1);
        reject(&doc, "16 KiB");
        let mut doc = base; doc.message_mut(MessageId(16)).unwrap().subject = "é".repeat(MAX_SUBJECT_CHARS + 1);
        reject(&doc, "256 characters");
    }

    #[test]
    fn malformed_complete_fixtures_fail_for_the_intended_reason() {
        let json = String::from_utf8(encode_document(&generate(test_anchor())).unwrap()).unwrap();
        let fixtures = [
            (format!("{json} trailing"), "trailing"),
            (json.replacen("\"schema_version\":1", "\"schema_version\":1,\"schema_version\":1", 1), "duplicate"),
            (json.replacen("\"schema_version\":1", "\"schema_version\":\"1\"", 1), "expected integer"),
            (json.replacen("\"schema_version\":1", "\"schema_version\":2", 1), "unsupported schema"),
        ];
        for (fixture, reason) in fixtures {
            let err = decode_document(fixture.as_bytes()).unwrap_err().to_string();
            assert!(err.to_lowercase().contains(reason), "{err}");
        }
        assert_eq!(decode_document(&vec![b' '; MAX_DOCUMENT_BYTES + 1]), Err(MailError::DocumentTooLarge));
    }

}
