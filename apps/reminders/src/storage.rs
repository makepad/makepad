//! JSON document encode/decode and the storage request state machine.

use crate::model::*;
use makepad_civil_time as civil;
use makepad_strict_json::{self as json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Utf8,
    Json(&'static str),
    UnsupportedVersion,
    TooLarge,
    Invalid(&'static str),
}

impl DecodeError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Utf8 => "Document is not valid UTF-8",
            Self::Json(msg) => msg,
            Self::UnsupportedVersion => "Unsupported document version",
            Self::TooLarge => "Document is too large",
            Self::Invalid(msg) => msg,
        }
    }
}

fn obj_get<'a>(obj: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    obj.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn require_obj(value: &Value) -> Result<&[(String, Value)], DecodeError> {
    match value {
        Value::Obj(pairs) => Ok(pairs),
        _ => Err(DecodeError::Invalid("expected object")),
    }
}

fn require_arr(value: &Value) -> Result<&[Value], DecodeError> {
    match value {
        Value::Arr(items) => Ok(items),
        _ => Err(DecodeError::Invalid("expected array")),
    }
}

fn require_str(value: &Value) -> Result<&str, DecodeError> {
    value
        .as_str()
        .ok_or(DecodeError::Invalid("expected string"))
}

fn require_u32(value: &Value) -> Result<u32, DecodeError> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(DecodeError::Invalid("expected integer"))
}

fn require_u64(value: &Value) -> Result<u64, DecodeError> {
    value
        .as_u64()
        .ok_or(DecodeError::Invalid("expected integer"))
}

fn require_bool(value: &Value) -> Result<bool, DecodeError> {
    value
        .as_bool()
        .ok_or(DecodeError::Invalid("expected boolean"))
}

fn known_keys(obj: &[(String, Value)], allowed: &[&str]) -> Result<(), DecodeError> {
    for (key, _) in obj {
        if !allowed.contains(&key.as_str()) {
            return Err(DecodeError::Invalid("unknown field"));
        }
    }
    Ok(())
}

fn parse_date_value(value: &Value) -> Result<civil::Day, DecodeError> {
    let text = require_str(value)?;
    parse_date(text).map_err(DecodeError::Invalid)
}

fn parse_time_value(value: &Value) -> Result<u16, DecodeError> {
    let text = require_str(value)?;
    parse_time(text).map_err(DecodeError::Invalid)
}

fn decode_due(value: &Value) -> Result<Option<Due>, DecodeError> {
    if value.is_null() {
        return Ok(None);
    }
    let obj = require_obj(value)?;
    known_keys(obj, &["date", "time"])?;
    let day = parse_date_value(obj_get(obj, "date").ok_or(DecodeError::Invalid("missing date"))?)?;
    let minute = match obj_get(obj, "time") {
        None | Some(Value::Null) => None,
        Some(v) => Some(parse_time_value(v)?),
    };
    Ok(Some(Due { day, minute }))
}

fn decode_completed(value: &Value) -> Result<Option<CompletedAt>, DecodeError> {
    if value.is_null() {
        return Ok(None);
    }
    let obj = require_obj(value)?;
    known_keys(obj, &["date", "time"])?;
    let day = parse_date_value(obj_get(obj, "date").ok_or(DecodeError::Invalid("missing date"))?)?;
    let minute =
        parse_time_value(obj_get(obj, "time").ok_or(DecodeError::Invalid("missing time"))?)?;
    Ok(Some(CompletedAt { day, minute }))
}

fn decode_list(value: &Value) -> Result<ReminderList, DecodeError> {
    let obj = require_obj(value)?;
    known_keys(obj, &["id", "name", "colour", "order"])?;
    let id = require_u32(obj_get(obj, "id").ok_or(DecodeError::Invalid("missing id"))?)?;
    if id == 0 {
        return Err(DecodeError::Invalid("ids must be positive"));
    }
    let name =
        require_str(obj_get(obj, "name").ok_or(DecodeError::Invalid("missing name"))?)?.to_string();
    if list_name_error(&name).is_some() {
        return Err(DecodeError::Invalid("invalid list name"));
    }
    let colour = ListColour::parse(require_str(
        obj_get(obj, "colour").ok_or(DecodeError::Invalid("missing colour"))?,
    )?)
    .ok_or(DecodeError::Invalid("invalid colour"))?;
    let order = require_u32(obj_get(obj, "order").ok_or(DecodeError::Invalid("missing order"))?)?;
    Ok(ReminderList {
        id,
        name,
        colour,
        order,
    })
}

fn decode_reminder(value: &Value) -> Result<Reminder, DecodeError> {
    let obj = require_obj(value)?;
    known_keys(
        obj,
        &[
            "id",
            "list_id",
            "title",
            "notes",
            "due",
            "flagged",
            "priority",
            "order",
            "completed_at",
        ],
    )?;
    let id = require_u32(obj_get(obj, "id").ok_or(DecodeError::Invalid("missing id"))?)?;
    if id == 0 {
        return Err(DecodeError::Invalid("ids must be positive"));
    }
    let list_id =
        require_u32(obj_get(obj, "list_id").ok_or(DecodeError::Invalid("missing list_id"))?)?;
    let title = require_str(obj_get(obj, "title").ok_or(DecodeError::Invalid("missing title"))?)?
        .to_string();
    if title_error(&title).is_some() {
        return Err(DecodeError::Invalid("invalid title"));
    }
    let notes = match obj_get(obj, "notes") {
        None => String::new(),
        Some(v) => require_str(v)?.to_string(),
    };
    if notes_error(&notes).is_some() {
        return Err(DecodeError::Invalid("invalid notes"));
    }
    let due = match obj_get(obj, "due") {
        None => None,
        Some(v) => decode_due(v)?,
    };
    let flagged = match obj_get(obj, "flagged") {
        None => false,
        Some(v) => require_bool(v)?,
    };
    let priority = match obj_get(obj, "priority") {
        None => Priority::None,
        Some(v) => {
            Priority::parse(require_str(v)?).ok_or(DecodeError::Invalid("invalid priority"))?
        }
    };
    let order = require_u32(obj_get(obj, "order").ok_or(DecodeError::Invalid("missing order"))?)?;
    let completed_at = match obj_get(obj, "completed_at") {
        None => None,
        Some(v) => decode_completed(v)?,
    };
    Ok(Reminder {
        id,
        list_id,
        title,
        notes,
        due,
        flagged,
        priority,
        order,
        completed_at,
    })
}

pub fn decode(bytes: &[u8]) -> Result<Document, DecodeError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(DecodeError::TooLarge);
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(DecodeError::Utf8);
    }
    let value = json::parse(bytes).map_err(DecodeError::Json)?;
    let obj = require_obj(&value)?;
    known_keys(
        obj,
        &[
            "schema_version",
            "seed_version",
            "seed_day",
            "revision",
            "next_reminder_id",
            "show_completed",
            "lists",
            "reminders",
        ],
    )?;
    let schema_version = require_u32(
        obj_get(obj, "schema_version").ok_or(DecodeError::Invalid("missing schema_version"))?,
    )?;
    if schema_version != SCHEMA_VERSION {
        return Err(DecodeError::UnsupportedVersion);
    }
    let seed_version = require_u32(
        obj_get(obj, "seed_version").ok_or(DecodeError::Invalid("missing seed_version"))?,
    )?;
    let seed_day = parse_date_value(
        obj_get(obj, "seed_day").ok_or(DecodeError::Invalid("missing seed_day"))?,
    )?;
    let revision =
        require_u64(obj_get(obj, "revision").ok_or(DecodeError::Invalid("missing revision"))?)?;
    let next_reminder_id = require_u32(
        obj_get(obj, "next_reminder_id").ok_or(DecodeError::Invalid("missing next_reminder_id"))?,
    )?;
    if next_reminder_id == 0 {
        return Err(DecodeError::Invalid("next_reminder_id must be positive"));
    }
    let show_completed = match obj_get(obj, "show_completed") {
        None => false,
        Some(v) => require_bool(v)?,
    };
    let lists = require_arr(obj_get(obj, "lists").ok_or(DecodeError::Invalid("missing lists"))?)?
        .iter()
        .map(decode_list)
        .collect::<Result<Vec<_>, _>>()?;
    if lists.len() > MAX_LISTS {
        return Err(DecodeError::Invalid("too many lists"));
    }
    let reminders =
        require_arr(obj_get(obj, "reminders").ok_or(DecodeError::Invalid("missing reminders"))?)?
            .iter()
            .map(decode_reminder)
            .collect::<Result<Vec<_>, _>>()?;
    if reminders.len() > MAX_REMINDERS {
        return Err(DecodeError::Invalid("too many reminders"));
    }
    let mut list_ids = std::collections::BTreeSet::new();
    for list in &lists {
        if !list_ids.insert(list.id) {
            return Err(DecodeError::Invalid("duplicate list id"));
        }
    }
    let mut reminder_ids = std::collections::BTreeSet::new();
    for item in &reminders {
        if !reminder_ids.insert(item.id) {
            return Err(DecodeError::Invalid("duplicate reminder id"));
        }
        if !list_ids.contains(&item.list_id) {
            return Err(DecodeError::Invalid("orphan list reference"));
        }
        if item.id >= next_reminder_id {
            return Err(DecodeError::Invalid("next_reminder_id reused"));
        }
    }
    Ok(Document {
        schema_version,
        seed_version,
        seed_day,
        revision,
        next_reminder_id,
        lists,
        reminders,
        show_completed,
    })
}

fn due_value(due: Option<Due>) -> Value {
    match due {
        None => Value::Null,
        Some(due) => json::obj(vec![
            ("date", json::s(civil::format_iso(due.day))),
            (
                "time",
                match due.minute {
                    Some(minute) => json::s(format_time(minute)),
                    None => Value::Null,
                },
            ),
        ]),
    }
}

fn completed_value(completed: Option<CompletedAt>) -> Value {
    match completed {
        None => Value::Null,
        Some(at) => json::obj(vec![
            ("date", json::s(civil::format_iso(at.day))),
            ("time", json::s(format_time(at.minute))),
        ]),
    }
}

pub fn encode(doc: &Document) -> Result<Vec<u8>, DecodeError> {
    let lists = Value::Arr(
        doc.lists
            .iter()
            .map(|list| {
                json::obj(vec![
                    ("id", Value::Int(list.id as i64)),
                    ("name", json::s(list.name.clone())),
                    ("colour", json::s(list.colour.as_str())),
                    ("order", Value::Int(list.order as i64)),
                ])
            })
            .collect(),
    );
    let reminders = Value::Arr(
        doc.reminders
            .iter()
            .map(|item| {
                json::obj(vec![
                    ("id", Value::Int(item.id as i64)),
                    ("list_id", Value::Int(item.list_id as i64)),
                    ("title", json::s(item.title.clone())),
                    ("notes", json::s(item.notes.clone())),
                    ("due", due_value(item.due)),
                    ("flagged", Value::Bool(item.flagged)),
                    ("priority", json::s(item.priority.as_str())),
                    ("order", Value::Int(item.order as i64)),
                    ("completed_at", completed_value(item.completed_at)),
                ])
            })
            .collect(),
    );
    let value = json::obj(vec![
        ("schema_version", Value::Int(doc.schema_version as i64)),
        ("seed_version", Value::Int(doc.seed_version as i64)),
        ("seed_day", json::s(civil::format_iso(doc.seed_day))),
        ("revision", Value::Int(doc.revision as i64)),
        ("next_reminder_id", Value::Int(doc.next_reminder_id as i64)),
        ("show_completed", Value::Bool(doc.show_completed)),
        ("lists", lists),
        ("reminders", reminders),
    ]);
    let text = value.to_json();
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(DecodeError::TooLarge);
    }
    Ok(text.into_bytes())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoragePhase {
    NeedGet,
    GetInFlight,
    ListInFlight,
    Ready,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    Seed,
    Loaded(Document),
    Error(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageEvent {
    GetMissing,
    GetEmpty,
    GetBytes(Vec<u8>),
    GetFailed,
    ListEmpty,
    ListNotEmpty,
    ListFailed,
    SetOk { request_id: u64 },
    SetFailed { request_id: u64 },
    Unrelated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageAction {
    Get,
    List,
    None,
}

#[derive(Clone, Debug)]
pub struct StorageMachine {
    pub phase: StoragePhase,
    pub saved_revision: u64,
    pub inflight: Option<(u64, u64)>,
    pub dirty_revision: u64,
    pub error: Option<&'static str>,
}

impl Default for StorageMachine {
    fn default() -> Self {
        Self {
            phase: StoragePhase::NeedGet,
            saved_revision: 0,
            inflight: None,
            dirty_revision: 0,
            error: None,
        }
    }
}

impl StorageMachine {
    pub fn start(&mut self) -> StorageAction {
        self.phase = StoragePhase::GetInFlight;
        StorageAction::Get
    }

    pub fn on_get(&mut self, event: StorageEvent) -> Option<LoadOutcome> {
        if self.phase != StoragePhase::GetInFlight {
            return None;
        }
        match event {
            StorageEvent::GetMissing => {
                self.phase = StoragePhase::ListInFlight;
                None
            }
            StorageEvent::GetEmpty => {
                self.phase = StoragePhase::Error;
                self.error = Some("Saved reminders are empty");
                Some(LoadOutcome::Error("Saved reminders are empty"))
            }
            StorageEvent::GetBytes(bytes) => match decode(&bytes) {
                Ok(doc) => {
                    self.phase = StoragePhase::Ready;
                    self.saved_revision = doc.revision;
                    self.dirty_revision = doc.revision;
                    Some(LoadOutcome::Loaded(doc))
                }
                Err(err) => {
                    self.phase = StoragePhase::Error;
                    let message = err.message();
                    self.error = Some(message);
                    Some(LoadOutcome::Error(message))
                }
            },
            StorageEvent::GetFailed => {
                self.phase = StoragePhase::Error;
                self.error = Some("Could not load reminders");
                Some(LoadOutcome::Error("Could not load reminders"))
            }
            StorageEvent::Unrelated => None,
            _ => None,
        }
    }

    pub fn on_list(&mut self, event: StorageEvent) -> Option<LoadOutcome> {
        if self.phase != StoragePhase::ListInFlight {
            return None;
        }
        match event {
            StorageEvent::ListEmpty => {
                self.phase = StoragePhase::Ready;
                Some(LoadOutcome::Seed)
            }
            StorageEvent::ListNotEmpty => {
                self.phase = StoragePhase::Error;
                self.error = Some("Storage jail is not empty");
                Some(LoadOutcome::Error("Storage jail is not empty"))
            }
            StorageEvent::ListFailed => {
                self.phase = StoragePhase::Error;
                self.error = Some("Could not load reminders");
                Some(LoadOutcome::Error("Could not load reminders"))
            }
            StorageEvent::Unrelated => None,
            _ => None,
        }
    }

    pub fn mark_dirty(&mut self, revision: u64) {
        self.dirty_revision = revision;
    }

    pub fn wants_save(&self) -> bool {
        self.phase == StoragePhase::Ready
            && self.error.is_none()
            && self.inflight.is_none()
            && self.dirty_revision > self.saved_revision
    }

    pub fn begin_save(&mut self, request_id: u64, revision: u64) {
        self.inflight = Some((request_id, revision));
        self.error = None;
    }

    pub fn on_set(&mut self, event: StorageEvent) -> bool {
        let Some((id, revision)) = self.inflight else {
            return false;
        };
        match event {
            StorageEvent::SetOk { request_id } if request_id == id => {
                self.inflight = None;
                if revision > self.saved_revision {
                    self.saved_revision = revision;
                }
                true
            }
            StorageEvent::SetFailed { request_id } if request_id == id => {
                self.inflight = None;
                self.error = Some("Could not save reminders");
                true
            }
            StorageEvent::Unrelated
            | StorageEvent::SetOk { .. }
            | StorageEvent::SetFailed { .. } => false,
            _ => false,
        }
    }

    pub fn retry_save(&mut self) -> bool {
        if self.phase != StoragePhase::Ready || self.inflight.is_some() {
            return false;
        }
        self.error = None;
        self.wants_save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::seed;
    use makepad_civil_time::from_ymd;

    #[test]
    fn json_round_trip_and_refusals() {
        let doc = seed(from_ymd(2026, 9, 9));
        let bytes = encode(&doc).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(doc, back);
        assert!(decode(&[0xff, 0xfe]).is_err());
        assert!(matches!(
            decode(br#"{"schema_version":1,"schema_version":1}"#),
            Err(DecodeError::Json(_))
        ));
        let mut bad = doc.clone();
        bad.schema_version = 2;
        let encoded = encode(&bad).unwrap();
        assert!(matches!(
            decode(&encoded),
            Err(DecodeError::UnsupportedVersion)
        ));
        assert!(decode(br#"{"schema_version":1,"seed_version":1,"seed_day":"2026-09-09","revision":1,"next_reminder_id":2,"show_completed":false,"lists":[],"reminders":[{"id":1,"list_id":9,"title":"x","notes":"","due":null,"flagged":false,"priority":"none","order":0,"completed_at":null}]}"#).is_err());
        assert!(decode(br#"{"schema_version":1,"seed_version":1,"seed_day":"2026-09-09","revision":1,"next_reminder_id":2,"show_completed":false,"lists":[{"id":1,"name":"A","colour":"teal","order":0}],"reminders":[]}"#).is_err());
        assert!(decode(br#"{"schema_version":1,"seed_version":1,"seed_day":"2026-13-01","revision":1,"next_reminder_id":1,"show_completed":false,"lists":[],"reminders":[]}"#).is_err());
        let dup = br#"{"schema_version":1,"seed_version":1,"seed_day":"2026-09-09","revision":1,"next_reminder_id":3,"show_completed":false,"lists":[{"id":1,"name":"A","colour":"blue","order":0}],"reminders":[{"id":1,"list_id":1,"title":"x","notes":"","due":null,"flagged":false,"priority":"none","order":0,"completed_at":null},{"id":1,"list_id":1,"title":"y","notes":"","due":null,"flagged":false,"priority":"none","order":1,"completed_at":null}]}"#;
        assert!(decode(dup).is_err());
        assert!(decode(br#"{"schema_version":1,"seed_version":1,"seed_day":"2026-09-09","revision":1,"next_reminder_id":2,"show_completed":false,"lists":[{"id":1,"name":"A","colour":"blue","order":0}],"reminders":[{"id":1,"list_id":1,"title":"x","notes":"","due":{"date":"2026-09-09","time":"24:00"},"flagged":false,"priority":"none","order":0,"completed_at":null}]}"#).is_err());
        let quoted = seed(from_ymd(2026, 9, 9));
        let mut quoted = quoted;
        quoted.reminders[0].title = r#"Say "hello" \ slash"#.into();
        let back = decode(&encode(&quoted).unwrap()).unwrap();
        assert_eq!(back.reminders[0].title, r#"Say "hello" \ slash"#);
    }

    #[test]
    fn storage_state_machine_missing_empty_stale_and_retry() {
        let mut machine = StorageMachine::default();
        assert!(matches!(machine.start(), StorageAction::Get));
        assert!(machine.on_get(StorageEvent::Unrelated).is_none());
        assert!(machine.on_get(StorageEvent::GetMissing).is_none());
        match machine.on_list(StorageEvent::ListEmpty) {
            Some(LoadOutcome::Seed) => {}
            other => panic!("{other:?}"),
        }
        let mut fail = StorageMachine::default();
        fail.start();
        match fail.on_get(StorageEvent::GetEmpty) {
            Some(LoadOutcome::Error(_)) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(fail.phase, StoragePhase::Error);
        let mut load_fail = StorageMachine::default();
        load_fail.start();
        match load_fail.on_get(StorageEvent::GetFailed) {
            Some(LoadOutcome::Error(_)) => {}
            other => panic!("{other:?}"),
        }
        let mut ready = StorageMachine::default();
        ready.phase = StoragePhase::Ready;
        ready.saved_revision = 1;
        ready.mark_dirty(2);
        assert!(ready.wants_save());
        ready.begin_save(10, 2);
        ready.mark_dirty(3);
        assert!(!ready.wants_save());
        assert!(ready.on_set(StorageEvent::SetOk { request_id: 10 }));
        assert_eq!(ready.saved_revision, 2);
        assert!(ready.wants_save());
        ready.begin_save(11, 3);
        assert!(ready.on_set(StorageEvent::SetFailed { request_id: 11 }));
        assert!(ready.retry_save());
        assert!(!ready.on_set(StorageEvent::SetOk { request_id: 99 }));
    }
    #[test]
    fn failed_save_suspends_automatic_writes_until_explicit_retry() {
        let mut machine = StorageMachine {
            phase: StoragePhase::Ready,
            saved_revision: 1,
            dirty_revision: 2,
            ..Default::default()
        };
        machine.begin_save(10, 2);
        assert!(!machine.on_set(StorageEvent::SetOk { request_id: 11 }));
        assert_eq!(machine.inflight, Some((10, 2)));
        machine.on_set(StorageEvent::SetFailed { request_id: 10 });
        for revision in 2..=5 {
            machine.mark_dirty(revision);
            assert!(!machine.wants_save());
            assert_eq!(machine.error, Some("Could not save reminders"));
            assert_eq!(machine.saved_revision, 1);
        }
        assert!(machine.retry_save());
        machine.begin_save(12, 5);
        machine.on_set(StorageEvent::SetOk { request_id: 12 });
        assert_eq!(machine.saved_revision, 5);
        assert!(!machine.wants_save());
        assert_eq!(machine.error, None);
    }

    #[test]
    fn document_limits_and_existing_empty_document_are_validated() {
        let mut doc = seed(from_ymd(2026, 9, 9));
        assert_eq!(
            decode(&vec![b' '; MAX_DOCUMENT_BYTES + 1]),
            Err(DecodeError::TooLarge)
        );
        let mut bad = doc.clone();
        bad.lists = (1..=33)
            .map(|id| ReminderList {
                id,
                ..doc.lists[0].clone()
            })
            .collect();
        assert!(decode(&encode(&bad).unwrap()).is_err());
        bad = doc.clone();
        bad.lists[1].id = bad.lists[0].id;
        assert!(decode(&encode(&bad).unwrap()).is_err());
        bad = doc.clone();
        bad.reminders = (1..=2001)
            .map(|id| Reminder {
                id,
                ..doc.reminders[0].clone()
            })
            .collect();
        bad.next_reminder_id = 2002;
        assert!(decode(&encode(&bad).unwrap()).is_err());
        for notes in ["a".repeat(4097), "界".repeat(1366)] {
            bad = doc.clone();
            bad.reminders[0].notes = notes;
            assert!(decode(&encode(&bad).unwrap()).is_err());
        }
        for value in ["", &"a".repeat(241)] {
            bad = doc.clone();
            bad.reminders[0].title = value.into();
            assert!(decode(&encode(&bad).unwrap()).is_err());
        }
        for list_name in ["", &"界".repeat(65)] {
            bad = doc.clone();
            bad.lists[0].name = list_name.into();
            assert!(decode(&encode(&bad).unwrap()).is_err());
        }
        doc.reminders.clear();
        let bytes = encode(&doc).unwrap();
        let mut machine = StorageMachine::default();
        machine.start();
        assert_eq!(
            machine.on_get(StorageEvent::GetBytes(bytes)),
            Some(LoadOutcome::Loaded(doc.clone()))
        );
        assert_eq!(machine.saved_revision, doc.revision);
        assert!(!machine.wants_save());
        assert!(machine.on_list(StorageEvent::ListEmpty).is_none());
        assert!(machine.on_get(StorageEvent::GetMissing).is_none());
        for event in [StorageEvent::ListFailed, StorageEvent::ListNotEmpty] {
            let mut machine = StorageMachine::default();
            machine.start();
            machine.on_get(StorageEvent::GetMissing);
            assert!(matches!(
                machine.on_list(event),
                Some(LoadOutcome::Error(_))
            ));
            assert!(!machine.wants_save());
        }
    }

    #[test]
    fn seed_stays_unacknowledged_until_the_first_set_succeeds() {
        let mut machine = StorageMachine::default();
        machine.start();
        assert!(machine.on_get(StorageEvent::GetMissing).is_none());
        match machine.on_list(StorageEvent::ListEmpty) {
            Some(LoadOutcome::Seed) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(machine.saved_revision, 0);
        machine.mark_dirty(1);
        assert!(
            machine.wants_save(),
            "the seed must be written before it is treated as saved"
        );
        machine.begin_save(7, 1);
        assert!(!machine.wants_save());
        assert!(machine.on_set(StorageEvent::SetOk { request_id: 7 }));
        assert_eq!(machine.saved_revision, 1);
        assert!(!machine.wants_save());
    }
}
