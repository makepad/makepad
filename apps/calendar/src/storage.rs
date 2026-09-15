//! JSON document encode/decode and save coalescing.

use crate::model::*;
use crate::seed::seed;
use makepad_civil_time::{self as civil, Day};
use makepad_strict_json::{parse, Value};

pub const JAIL_KEY: &str = "calendar.json";
pub const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    Missing,
    Loaded(CalendarDocument),
    Error { message: String },
}

pub fn load_bytes(bytes: Option<&[u8]>) -> LoadOutcome {
    match bytes {
        None => LoadOutcome::Missing,
        Some(raw) => match decode(raw) {
            Ok(doc) => LoadOutcome::Loaded(doc),
            Err(message) => LoadOutcome::Error { message },
        },
    }
}

pub fn seed_if_missing(bytes: Option<&[u8]>, today: Day) -> LoadOutcome {
    match load_bytes(bytes) {
        LoadOutcome::Missing => LoadOutcome::Loaded(seed(today)),
        other => other,
    }
}

fn obj_get<'a>(v: &'a Value, key: &str) -> Result<&'a Value, String> {
    v.get(key)
        .ok_or_else(|| format!("missing field `{key}`"))
}

fn expect_obj<'a>(v: &'a Value, ctx: &str) -> Result<&'a [(String, Value)], String> {
    match v {
        Value::Obj(pairs) => Ok(pairs),
        _ => Err(format!("{ctx} must be an object")),
    }
}

fn as_u32(v: &Value, ctx: &str) -> Result<u32, String> {
    v.as_i64()
        .filter(|i| *i >= 0 && *i <= u32::MAX as i64)
        .map(|i| i as u32)
        .ok_or_else(|| format!("{ctx} must be a non-negative integer"))
}

fn as_u8(v: &Value, ctx: &str) -> Result<u8, String> {
    v.as_i64()
        .filter(|i| *i >= 0 && *i <= u8::MAX as i64)
        .map(|i| i as u8)
        .ok_or_else(|| format!("{ctx} must be a small integer"))
}

fn as_bool(v: &Value, ctx: &str) -> Result<bool, String> {
    v.as_bool()
        .ok_or_else(|| format!("{ctx} must be a boolean"))
}

fn as_str<'a>(v: &'a Value, ctx: &str) -> Result<&'a str, String> {
    v.as_str()
        .ok_or_else(|| format!("{ctx} must be a string"))
}

fn as_arr<'a>(v: &'a Value, ctx: &str) -> Result<&'a [Value], String> {
    v.as_arr()
        .ok_or_else(|| format!("{ctx} must be an array"))
}

fn known_keys(obj: &[(String, Value)], allowed: &[&str], ctx: &str) -> Result<(), String> {
    for (k, _) in obj {
        if !allowed.iter().any(|a| a == k) {
            return Err(format!("unknown field `{k}` in {ctx}"));
        }
    }
    Ok(())
}

fn decode_timing(v: &Value) -> Result<Timing, String> {
    let pairs = expect_obj(v, "timing")?;
    known_keys(pairs, &["kind", "start", "end", "end_exclusive"], "timing")?;
    let kind = as_str(obj_get(v, "kind")?, "timing.kind")?;
    match kind {
        "timed" => {
            let start = LocalMinute::parse_iso(as_str(obj_get(v, "start")?, "timing.start")?)
                .ok_or_else(|| "malformed timed start".to_string())?;
            let end = LocalMinute::parse_iso(as_str(obj_get(v, "end")?, "timing.end")?)
                .ok_or_else(|| "malformed timed end".to_string())?;
            Ok(Timing::Timed { start, end })
        }
        "all_day" => {
            let start = civil::parse_iso(as_str(obj_get(v, "start")?, "timing.start")?)
                .ok_or_else(|| "malformed all-day start".to_string())?;
            let end_exclusive =
                civil::parse_iso(as_str(obj_get(v, "end_exclusive")?, "timing.end_exclusive")?)
                    .ok_or_else(|| "malformed all-day end".to_string())?;
            Ok(Timing::AllDay {
                start,
                end_exclusive,
            })
        }
        other => Err(format!("unknown timing kind `{other}`")),
    }
}

fn decode_event(v: &Value) -> Result<CalendarEvent, String> {
    let pairs = expect_obj(v, "event")?;
    known_keys(
        pairs,
        &["id", "calendar_id", "title", "timing", "repeat", "notes"],
        "event",
    )?;
    Ok(CalendarEvent {
        id: EventId(as_u32(obj_get(v, "id")?, "event.id")?),
        calendar_id: CalendarId(as_u8(obj_get(v, "calendar_id")?, "event.calendar_id")?),
        title: as_str(obj_get(v, "title")?, "event.title")?.to_string(),
        timing: decode_timing(obj_get(v, "timing")?)?,
        repeat: Repeat::parse(as_str(obj_get(v, "repeat")?, "event.repeat")?)
            .ok_or_else(|| "unknown repeat".to_string())?,
        notes: as_str(obj_get(v, "notes")?, "event.notes")?.to_string(),
    })
}

fn decode_calendar(v: &Value) -> Result<Calendar, String> {
    let pairs = expect_obj(v, "calendar")?;
    known_keys(pairs, &["id", "name", "colour", "visible"], "calendar")?;
    Ok(Calendar {
        id: CalendarId(as_u8(obj_get(v, "id")?, "calendar.id")?),
        name: as_str(obj_get(v, "name")?, "calendar.name")?.to_string(),
        colour: CalendarColour::parse(as_str(obj_get(v, "colour")?, "calendar.colour")?)
            .ok_or_else(|| "unknown colour".to_string())?,
        visible: as_bool(obj_get(v, "visible")?, "calendar.visible")?,
    })
}

pub fn decode(bytes: &[u8]) -> Result<CalendarDocument, String> {
    let value = parse(bytes).map_err(|e| e.to_string())?;
    let pairs = expect_obj(&value, "document")?;
    known_keys(
        pairs,
        &[
            "schema_version",
            "revision",
            "seed_version",
            "seed_anchor",
            "next_event_id",
            "preferences",
            "calendars",
            "events",
        ],
        "document",
    )?;
    let prefs_v = obj_get(&value, "preferences")?;
    let prefs_pairs = expect_obj(prefs_v, "preferences")?;
    known_keys(prefs_pairs, &["wide_mode", "default_calendar"], "preferences")?;
    let doc = CalendarDocument {
        schema_version: as_u32(obj_get(&value, "schema_version")?, "schema_version")?,
        revision: as_u32(obj_get(&value, "revision")?, "revision")?,
        seed_version: as_u32(obj_get(&value, "seed_version")?, "seed_version")?,
        seed_anchor: civil::parse_iso(as_str(obj_get(&value, "seed_anchor")?, "seed_anchor")?)
            .ok_or_else(|| "malformed seed_anchor".to_string())?,
        next_event_id: as_u32(obj_get(&value, "next_event_id")?, "next_event_id")?,
        preferences: Preferences {
            wide_mode: CalendarMode::parse(as_str(
                obj_get(prefs_v, "wide_mode")?,
                "preferences.wide_mode",
            )?)
            .ok_or_else(|| "unknown wide_mode".to_string())?,
            default_calendar: CalendarId(as_u8(
                obj_get(prefs_v, "default_calendar")?,
                "preferences.default_calendar",
            )?),
        },
        calendars: as_arr(obj_get(&value, "calendars")?, "calendars")?
            .iter()
            .map(decode_calendar)
            .collect::<Result<Vec<_>, _>>()?,
        events: as_arr(obj_get(&value, "events")?, "events")?
            .iter()
            .map(decode_event)
            .collect::<Result<Vec<_>, _>>()?,
    };
    validate_document(&doc)?;
    Ok(doc)
}

fn s(v: impl Into<String>) -> Value {
    Value::Str(v.into())
}
fn i(v: i64) -> Value {
    Value::Int(v)
}
fn b(v: bool) -> Value {
    Value::Bool(v)
}

fn encode_timing(t: Timing) -> Value {
    match t {
        Timing::Timed { start, end } => Value::Obj(vec![
            ("kind".into(), s("timed")),
            ("start".into(), s(start.format_iso())),
            ("end".into(), s(end.format_iso())),
        ]),
        Timing::AllDay {
            start,
            end_exclusive,
        } => Value::Obj(vec![
            ("kind".into(), s("all_day")),
            ("start".into(), s(civil::format_iso(start))),
            (
                "end_exclusive".into(),
                s(civil::format_iso(end_exclusive)),
            ),
        ]),
    }
}

pub fn encode_value(doc: &CalendarDocument) -> Value {
    let calendars = doc
        .calendars
        .iter()
        .map(|c| {
            Value::Obj(vec![
                ("id".into(), i(c.id.0 as i64)),
                ("name".into(), s(c.name.clone())),
                ("colour".into(), s(c.colour.as_str())),
                ("visible".into(), b(c.visible)),
            ])
        })
        .collect();
    let events = doc
        .events
        .iter()
        .map(|e| {
            Value::Obj(vec![
                ("id".into(), i(e.id.0 as i64)),
                ("calendar_id".into(), i(e.calendar_id.0 as i64)),
                ("title".into(), s(e.title.clone())),
                ("timing".into(), encode_timing(e.timing)),
                ("repeat".into(), s(e.repeat.as_str())),
                ("notes".into(), s(e.notes.clone())),
            ])
        })
        .collect();
    Value::Obj(vec![
        ("schema_version".into(), i(doc.schema_version as i64)),
        ("revision".into(), i(doc.revision as i64)),
        ("seed_version".into(), i(doc.seed_version as i64)),
        ("seed_anchor".into(), s(civil::format_iso(doc.seed_anchor))),
        ("next_event_id".into(), i(doc.next_event_id as i64)),
        (
            "preferences".into(),
            Value::Obj(vec![
                ("wide_mode".into(), s(doc.preferences.wide_mode.as_str())),
                (
                    "default_calendar".into(),
                    i(doc.preferences.default_calendar.0 as i64),
                ),
            ]),
        ),
        ("calendars".into(), Value::Arr(calendars)),
        ("events".into(), Value::Arr(events)),
    ])
}

pub fn encode(doc: &CalendarDocument) -> Result<Vec<u8>, String> {
    let json = encode_value(doc).to_json();
    let bytes = json.into_bytes();
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "document is {} bytes; the cap is {MAX_DOCUMENT_BYTES}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistStatus {
    Loading,
    Ready,
    Saving,
    Saved,
    LoadError,
    SaveError,
}

impl PersistStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Loading => "Loading",
            Self::Ready => "",
            Self::Saving => "Saving",
            Self::Saved => "Saved",
            Self::LoadError => "Could not load — Retry",
            Self::SaveError => "Changes not saved — Retry",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Persist {
    pub load_id: Option<u64>,
    pub save_id: Option<u64>,
    pub in_flight_revision: Option<u32>,
    pub pending_revision: Option<u32>,
    pub status: PersistStatus,
    pub preserved_bytes: Option<Vec<u8>>,
}

impl Default for PersistStatus {
    fn default() -> Self {
        Self::Loading
    }
}

impl Persist {
    pub fn begin_load(&mut self, id: u64) {
        self.load_id = Some(id);
        self.status = PersistStatus::Loading;
    }

    /// After a mutation: coalesce onto the in-flight write.
    pub fn on_mutate(&mut self, revision: u32) -> bool {
        if self.save_id.is_some() {
            self.pending_revision = Some(revision);
            self.status = PersistStatus::Saving;
            false
        } else {
            self.pending_revision = Some(revision);
            self.status = PersistStatus::Saving;
            true
        }
    }

    pub fn take_pending(&mut self, id: u64) -> Option<u32> {
        let rev = self.pending_revision.take()?;
        self.save_id = Some(id);
        self.in_flight_revision = Some(rev);
        self.status = PersistStatus::Saving;
        Some(rev)
    }

    pub fn on_save_response(&mut self, id: u64, ok: bool) -> SaveAck {
        if self.save_id != Some(id) {
            return SaveAck::Ignored;
        }
        self.save_id = None;
        let saved_rev = self.in_flight_revision.take();
        if !ok {
            self.status = PersistStatus::SaveError;
            if let Some(rev) = saved_rev {
                self.pending_revision = Some(self.pending_revision.unwrap_or(rev).max(rev));
            }
            return SaveAck::Failed;
        }
        if self.pending_revision.is_some() {
            self.status = PersistStatus::Saving;
            SaveAck::WriteNext
        } else {
            self.status = PersistStatus::Saved;
            SaveAck::Idle
        }
    }

    pub fn on_load_response(&mut self, id: u64) -> bool {
        if self.load_id != Some(id) {
            return false;
        }
        self.load_id = None;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveAck {
    Ignored,
    Failed,
    WriteNext,
    Idle,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::seed;

    #[test]
    fn json_round_trip_unicode_and_refusals() {
        let mut doc = seed(civil::from_ymd(2026, 9, 9));
        doc.events[0].title = "Design review — 日本語 📅".into();
        doc.events[0].notes = "é".repeat(40);
        let bytes = encode(&doc).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.events[0].title, doc.events[0].title);
        assert_eq!(back.events[0].notes, doc.events[0].notes);
        assert_eq!(back.events.len(), 40);

        assert!(matches!(load_bytes(None), LoadOutcome::Missing));
        assert!(matches!(
            load_bytes(Some(b"")),
            LoadOutcome::Error { .. }
        ));
        assert!(matches!(
            load_bytes(Some(b"{")),
            LoadOutcome::Error { .. }
        ));
        let dup = br#"{"schema_version":1,"schema_version":1,"revision":1,"seed_version":1,"seed_anchor":"2026-09-09","next_event_id":41,"preferences":{"wide_mode":"month","default_calendar":1},"calendars":[],"events":[]}"#;
        assert!(decode(dup).is_err(), "duplicate keys");
        let future = br#"{"schema_version":99,"revision":1,"seed_version":1,"seed_anchor":"2026-09-09","next_event_id":1,"preferences":{"wide_mode":"month","default_calendar":1},"calendars":[{"id":1,"name":"Home","colour":"home","visible":true}],"events":[]}"#;
        assert!(decode(future).is_err(), "future schema");
        let dup_id = br#"{"schema_version":1,"revision":1,"seed_version":1,"seed_anchor":"2026-09-09","next_event_id":3,"preferences":{"wide_mode":"month","default_calendar":1},"calendars":[{"id":1,"name":"Home","colour":"home","visible":true}],"events":[{"id":1,"calendar_id":1,"title":"A","timing":{"kind":"all_day","start":"2026-09-09","end_exclusive":"2026-09-10"},"repeat":"none","notes":""},{"id":1,"calendar_id":1,"title":"B","timing":{"kind":"all_day","start":"2026-09-09","end_exclusive":"2026-09-10"},"repeat":"none","notes":""}]}"#;
        assert!(decode(dup_id).is_err(), "duplicate event ids");
        let bad_end = br#"{"schema_version":1,"revision":1,"seed_version":1,"seed_anchor":"2026-09-09","next_event_id":2,"preferences":{"wide_mode":"month","default_calendar":1},"calendars":[{"id":1,"name":"Home","colour":"home","visible":true}],"events":[{"id":1,"calendar_id":1,"title":"A","timing":{"kind":"timed","start":"2026-09-09T10:00","end":"2026-09-09T09:00"},"repeat":"none","notes":""}]}"#;
        assert!(decode(bad_end).is_err(), "invalid endpoints");
        let empty = br#"{"schema_version":1,"revision":1,"seed_version":1,"seed_anchor":"2026-09-09","next_event_id":1,"preferences":{"wide_mode":"month","default_calendar":1},"calendars":[{"id":1,"name":"Home","colour":"home","visible":true}],"events":[]}"#;
        let empty_doc = decode(empty).unwrap();
        assert!(empty_doc.events.is_empty());
        assert!(!matches!(
            seed_if_missing(Some(empty), civil::from_ymd(2026, 9, 9)),
            LoadOutcome::Loaded(d) if !d.events.is_empty()
        ));
        match seed_if_missing(None, civil::from_ymd(2026, 9, 9)) {
            LoadOutcome::Loaded(d) => assert_eq!(d.events.len(), 40),
            _ => panic!("missing seeds"),
        }
        assert!(matches!(
            seed_if_missing(Some(b""), civil::from_ymd(2026, 9, 9)),
            LoadOutcome::Error { .. }
        ));
    }

    #[test]
    fn save_coalescing_stale_ids_and_retry() {
        let mut p = Persist::default();
        p.begin_load(1);
        assert!(p.on_load_response(1));
        assert!(!p.on_load_response(1), "stale load id");
        assert!(p.on_mutate(2));
        assert_eq!(p.take_pending(10), Some(2));
        assert!(!p.on_mutate(3), "coalesce while in flight");
        assert_eq!(p.on_save_response(99, true), SaveAck::Ignored);
        assert_eq!(p.on_save_response(10, true), SaveAck::WriteNext);
        assert_eq!(p.take_pending(11), Some(3));
        assert_eq!(p.on_save_response(11, true), SaveAck::Idle);
        assert_eq!(p.status, PersistStatus::Saved);
        p.on_mutate(4);
        p.take_pending(12);
        assert_eq!(p.on_save_response(12, false), SaveAck::Failed);
        assert_eq!(p.status, PersistStatus::SaveError);
        assert_eq!(p.pending_revision, Some(4));
        assert!(p.on_mutate(4) || p.pending_revision == Some(4));
    }
    #[test]
    fn persisted_multibyte_dates_are_errors_not_panics() {
        let doc = seed(civil::from_ymd(2026,9,9));
        let json = String::from_utf8(encode(&doc).unwrap()).unwrap();
        for date in ["202é-09-9", "2026-😀-9", "日本語"] {
            let invalid = json.replace("2026-09-09",date);
            assert!(decode(invalid.as_bytes()).is_err());
            assert!(matches!(load_bytes(Some(invalid.as_bytes())),LoadOutcome::Error{..}));
        }
    }

}
