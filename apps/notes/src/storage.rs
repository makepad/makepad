//! JSON codec and the pure save-state reducer. No widget types.

use crate::model::*;
use makepad_strict_json::{self as json, Value};

pub fn encode(doc: &NotesDocument) -> Result<Vec<u8>, &'static str> {
    let mut notes = Vec::with_capacity(doc.notes.len());
    for note in &doc.notes {
        notes.push(json::obj(vec![
            ("id", json::s(note.id.wire())),
            ("folder", json::s(note.folder.as_str())),
            ("source", json::s(&note.source)),
            ("pinned", Value::Bool(note.pinned)),
            ("created_ms", Value::Int(note.created_ms)),
            ("edited_ms", Value::Int(note.edited_ms)),
            (
                "deleted_ms",
                match note.deleted_ms {
                    Some(ms) => Value::Int(ms),
                    None => Value::Null,
                },
            ),
        ]));
    }
    let root = json::obj(vec![
        ("version", Value::Int(doc.version as i64)),
        ("seed_version", Value::Int(doc.seed_version as i64)),
        ("seed_anchor_ms", Value::Int(doc.seed_anchor_ms)),
        ("next_id", Value::Int(i64::try_from(doc.next_id).map_err(|_| "next_id out of range")?)),
        ("revision", Value::Int(i64::try_from(doc.revision).map_err(|_| "revision out of range")?)),
        ("notes", Value::Arr(notes)),
    ]);
    let text = root.to_json();
    if text.len() > MAX_STORAGE_BYTES {
        return Err("encoded document is too large");
    }
    Ok(text.into_bytes())
}

fn req_i64(obj: &Value, key: &str) -> Result<i64, &'static str> {
    obj.get(key).and_then(Value::as_i64).ok_or("missing or invalid integer field")
}

fn req_u64(obj: &Value, key: &str) -> Result<u64, &'static str> {
    let n = req_i64(obj, key)?;
    if n < 0 {
        return Err("integer field is negative");
    }
    Ok(n as u64)
}

fn req_str<'a>(obj: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    obj.get(key).and_then(Value::as_str).ok_or("missing or invalid string field")
}

fn req_bool(obj: &Value, key: &str) -> Result<bool, &'static str> {
    obj.get(key).and_then(Value::as_bool).ok_or("missing or invalid boolean field")
}

fn decode_note(value: &Value) -> Result<Note, &'static str> {
    let id = NoteId::parse(req_str(value, "id")?).ok_or("invalid note id")?;
    let folder = FolderId::parse(req_str(value, "folder")?).ok_or("invalid folder")?;
    let source = req_str(value, "source")?.to_string();
    if source.len() > MAX_SOURCE_BYTES {
        return Err("note source too large");
    }
    let pinned = req_bool(value, "pinned")?;
    let created_ms = req_i64(value, "created_ms")?;
    let edited_ms = req_i64(value, "edited_ms")?;
    if edited_ms < created_ms {
        return Err("edited before created");
    }
    let deleted_ms = match value.get("deleted_ms") {
        None | Some(Value::Null) => None,
        Some(Value::Int(ms)) => {
            if *ms < created_ms {
                return Err("deleted before created");
            }
            Some(*ms)
        }
        _ => return Err("invalid deleted_ms"),
    };
    Ok(Note {
        id,
        folder,
        source,
        pinned,
        created_ms,
        edited_ms,
        deleted_ms,
    })
}

pub fn decode(bytes: &[u8]) -> Result<NotesDocument, &'static str> {
    if bytes.len() > MAX_STORAGE_BYTES {
        return Err("stored document is too large");
    }
    let value = json::parse(bytes).map_err(|_| "malformed json")?;
    let version = req_i64(&value, "version")?;
    if version != DOCUMENT_VERSION as i64 {
        return Err("unsupported version");
    }
    let seed_version = req_u64(&value, "seed_version")? as u32;
    let seed_anchor_ms = req_i64(&value, "seed_anchor_ms")?;
    let next_id = req_u64(&value, "next_id")?;
    let revision = req_u64(&value, "revision")?;
    let notes_v = value.get("notes").and_then(Value::as_arr).ok_or("missing notes array")?;
    if notes_v.len() > MAX_NOTES {
        return Err("too many notes");
    }
    let mut notes = Vec::with_capacity(notes_v.len());
    let mut seen = std::collections::HashSet::new();
    let mut aggregate = 0usize;
    for item in notes_v {
        let note = decode_note(item)?;
        if !seen.insert(note.id) {
            return Err("duplicate note id");
        }
        if note.id.0 >= next_id {
            return Err("note id meets or exceeds next_id");
        }
        aggregate = aggregate.saturating_add(note.source.len());
        if aggregate > MAX_AGGREGATE_SOURCE_BYTES {
            return Err("aggregate source too large");
        }
        notes.push(note);
    }
    Ok(NotesDocument {
        version: DOCUMENT_VERSION,
        seed_version,
        seed_anchor_ms,
        next_id,
        revision,
        notes,
    })
}

pub enum LoadOutcome {
    Missing,
    Loaded(NotesDocument),
    Error { message: &'static str },
}

pub fn load_from_storage(bytes: Option<&[u8]>) -> LoadOutcome {
    match bytes {
        None => LoadOutcome::Missing,
        Some(bytes) => match decode(bytes) {
            Ok(doc) => LoadOutcome::Loaded(doc),
            Err(message) => LoadOutcome::Error { message },
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveMachine {
    pub current: u64,
    pub saved: u64,
    pub in_flight: Option<(u64, u64)>,
    pub error: Option<String>,
    pub debounce_deadline_ms: Option<i64>,
}

impl Default for SaveMachine {
    fn default() -> Self {
        Self {
            current: 0,
            saved: 0,
            in_flight: None,
            error: None,
            debounce_deadline_ms: None,
        }
    }
}

impl SaveMachine {
    pub fn is_saved(&self) -> bool {
        self.current == self.saved && self.in_flight.is_none() && self.error.is_none()
    }

    pub fn status_label(&self, load: LoadState) -> &'static str {
        match load {
            LoadState::Loading => "",
            LoadState::Error => "Couldn't load",
            LoadState::Ready => {
                if self.error.is_some() {
                    "Couldn't save"
                } else if self.is_saved() {
                    "Saved"
                } else {
                    "Saving"
                }
            }
        }
    }
}

pub enum SaveEvent {
    Changed { revision: u64, now_ms: i64, debounce: bool },
    Timer { now_ms: i64 },
    Ack { request_id: u64, ok: bool, message: String },
    Retry { now_ms: i64 },
}

pub enum SaveEffect {
    None,
    Write { revision: u64 },
}

pub fn reduce_save(machine: &mut SaveMachine, event: SaveEvent) -> SaveEffect {
    match event {
        SaveEvent::Changed { revision, now_ms, debounce } => {
            machine.current = revision;
            machine.error = None;
            if debounce {
                machine.debounce_deadline_ms = Some(now_ms + SAVE_DEBOUNCE_MS);
                SaveEffect::None
            } else {
                machine.debounce_deadline_ms = None;
                start_write_if_idle(machine)
            }
        }
        SaveEvent::Timer { now_ms } => {
            if let Some(deadline) = machine.debounce_deadline_ms {
                if now_ms >= deadline {
                    machine.debounce_deadline_ms = None;
                    return start_write_if_idle(machine);
                }
            }
            SaveEffect::None
        }
        SaveEvent::Ack { request_id, ok, message } => {
            let Some((rid, rev)) = machine.in_flight else {
                return SaveEffect::None;
            };
            if rid != request_id {
                return SaveEffect::None;
            }
            machine.in_flight = None;
            if ok {
                machine.saved = rev;
                machine.error = None;
                if machine.current > machine.saved {
                    start_write_if_idle(machine)
                } else {
                    SaveEffect::None
                }
            } else {
                machine.error = Some(message);
                SaveEffect::None
            }
        }
        SaveEvent::Retry { now_ms: _ } => {
            machine.error = None;
            machine.debounce_deadline_ms = None;
            start_write_if_idle(machine)
        }
    }
}

fn start_write_if_idle(machine: &mut SaveMachine) -> SaveEffect {
    if machine.in_flight.is_some() {
        return SaveEffect::None;
    }
    if machine.current == machine.saved && machine.error.is_none() {
        return SaveEffect::None;
    }
    SaveEffect::Write { revision: machine.current }
}

pub fn record_write(machine: &mut SaveMachine, request_id: u64, revision: u64) {
    machine.in_flight = Some((request_id, revision));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed;

    #[test]
    fn json_round_trip_and_rejects() {
        let doc = seed::generate(1_788_955_200_000);
        let bytes = encode(&doc).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("\\n"));
        let back = decode(&bytes).unwrap();
        assert_eq!(back.notes.len(), doc.notes.len());
        assert_eq!(back.notes[0].source, doc.notes[0].source);
        assert_eq!(back.next_id, 21);
        assert!(decode(b"not json").is_err());
        assert!(decode(br#"{"version":1,"seed_version":1,"seed_anchor_ms":0,"next_id":1,"revision":0,"notes":[{"id":"n000001","folder":"notes","source":"","pinned":false,"created_ms":0,"edited_ms":0,"deleted_ms":null},{"id":"n000001","folder":"notes","source":"","pinned":false,"created_ms":0,"edited_ms":0,"deleted_ms":null}]}"#).is_err());
        assert!(decode(br#"{"version":1,"seed_version":1,"seed_anchor_ms":0,"next_id":1,"revision":0,"notes":[],"notes":[]}"#).is_err());
        assert!(decode(br#"{"version":1,"seed_version":1,"seed_anchor_ms":0,"next_id":1,"revision":0,"notes":[{"id":"n000001","folder":"inbox","source":"","pinned":false,"created_ms":0,"edited_ms":0,"deleted_ms":null}]}"#).is_err());
        assert!(decode(br#"{"version":2,"seed_version":1,"seed_anchor_ms":0,"next_id":1,"revision":0,"notes":[]}"#).is_err());
        assert!(decode(br#"{"version":1,"seed_version":1,"seed_anchor_ms":0,"next_id":1,"revision":0,"notes":[{"id":"n000001","folder":"notes","source":"","pinned":false,"created_ms":10,"edited_ms":0,"deleted_ms":null}]}"#).is_err());
        match load_from_storage(None) {
            LoadOutcome::Missing => {}
            _ => panic!("missing storage must seed"),
        }
        let empty = NotesDocument::empty(0);
        let empty_bytes = encode(&empty).unwrap();
        match load_from_storage(Some(&empty_bytes)) {
            LoadOutcome::Loaded(doc) => assert!(doc.notes.is_empty()),
            _ => panic!("valid empty must load"),
        }
        match load_from_storage(Some(b"{")) {
            LoadOutcome::Error { .. } => {}
            _ => panic!("corruption must not seed"),
        }
    }

    #[test]
    fn save_reducer_debounce_coalesce_and_false_saved() {
        let mut m = SaveMachine::default();
        assert!(matches!(
            reduce_save(&mut m, SaveEvent::Changed { revision: 1, now_ms: 0, debounce: true }),
            SaveEffect::None
        ));
        assert!(!m.is_saved());
        assert!(matches!(
            reduce_save(&mut m, SaveEvent::Timer { now_ms: 100 }),
            SaveEffect::None
        ));
        assert!(matches!(
            reduce_save(&mut m, SaveEvent::Timer { now_ms: 300 }),
            SaveEffect::Write { revision: 1 }
        ));
        record_write(&mut m, 10, 1);
        assert!(matches!(
            reduce_save(&mut m, SaveEvent::Changed { revision: 2, now_ms: 400, debounce: true }),
            SaveEffect::None
        ));
        assert!(m.in_flight.is_some());
        let effect = reduce_save(
            &mut m,
            SaveEvent::Ack { request_id: 10, ok: true, message: String::new() },
        );
        assert!(matches!(effect, SaveEffect::Write { revision: 2 }));
        assert_eq!(m.saved, 1);
        assert!(!m.is_saved());
        record_write(&mut m, 11, 2);
        reduce_save(
            &mut m,
            SaveEvent::Ack { request_id: 11, ok: false, message: "io".into() },
        );
        assert_eq!(m.error.as_deref(), Some("io"));
        assert!(!m.is_saved());
        assert_eq!(m.status_label(LoadState::Ready), "Couldn't save");
        let effect = reduce_save(&mut m, SaveEvent::Retry { now_ms: 900 });
        assert!(matches!(effect, SaveEffect::Write { revision: 2 }));
        record_write(&mut m, 12, 2);
        reduce_save(
            &mut m,
            SaveEvent::Ack { request_id: 12, ok: true, message: String::new() },
        );
        assert!(m.is_saved());
        assert_eq!(m.status_label(LoadState::Ready), "Saved");
        // Immediate change while idle writes now.
        let effect = reduce_save(
            &mut m,
            SaveEvent::Changed { revision: 3, now_ms: 1000, debounce: false },
        );
        assert!(matches!(effect, SaveEffect::Write { revision: 3 }));
    }
}
