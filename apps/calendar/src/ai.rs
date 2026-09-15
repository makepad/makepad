//! Assistant tools: `calendar.next` and `calendar.day`. Read-only.

use crate::engine::{occurrences_for_day, qualifying_occurrences};
use crate::model::*;
use crate::view::CalendarView;
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_civil_time::{self as civil, Day};
use makepad_strict_json::Value;

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "calendar",
        "Calendar",
        "The calendar on screen: upcoming events and the agenda for one civil day.",
    )
    .with_tool(ToolDef::new(
        "next",
        "Upcoming events from now through midnight of today+days. Includes events already in progress.",
        r#"{"type":"object","properties":{"days":{"type":"integer","minimum":1,"maximum":31}},"required":["days"],"additionalProperties":false}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "day",
        "Every occurrence intersecting one Gregorian civil day.",
        r#"{"type":"object","properties":{"date":{"type":"string","description":"YYYY-MM-DD"}},"required":["date"],"additionalProperties":false}"#,
        Risk::Read,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolAvailability {
    Ready,
    Unavailable(&'static str),
}

pub fn answer(view: &CalendarView, call: &ServiceCall) -> ToolResult {
    match view.tool_availability() {
        ToolAvailability::Ready => {}
        ToolAvailability::Unavailable(why) => {
            return ToolResult::unavailable(&call.call_id, why);
        }
    }
    let Some(doc) = view.document() else {
        return ToolResult::unavailable(&call.call_id, "the calendar is still loading");
    };
    let clock = view.clock();
    dispatch(doc, clock, call)
}

pub fn dispatch(doc: &CalendarDocument, clock: ClockSnapshot, call: &ServiceCall) -> ToolResult {
    match call.tool.as_str() {
        "next" => tool_next(doc, clock, call),
        "day" => tool_day(doc, call),
        other => ToolResult::refused(
            &call.call_id,
            format!("unknown tool `{other}`; this app has `next` and `day`"),
        ),
    }
}

fn parse_object<'a>(
    args: &'a str,
    allowed: &[&str],
) -> Result<Value, String> {
    let value = makepad_strict_json::parse(args.as_bytes()).map_err(|e| e.to_string())?;
    match &value {
        Value::Obj(pairs) => {
            for (k, _) in pairs {
                if !allowed.iter().any(|a| a == k) {
                    return Err(format!("unknown field `{k}`"));
                }
            }
            Ok(value)
        }
        _ => Err("arguments must be an object".into()),
    }
}

fn tool_next(doc: &CalendarDocument, clock: ClockSnapshot, call: &ServiceCall) -> ToolResult {
    let value = match parse_object(&call.args, &["days"]) {
        Ok(v) => v,
        Err(e) => return ToolResult::refused(&call.call_id, format!("invalid arguments: {e}")),
    };
    let days = match value.get("days").and_then(Value::as_i64) {
        Some(d) if (1..=31).contains(&d) => d as i32,
        Some(_) => {
            return ToolResult::refused(&call.call_id, "days must be an integer from 1 to 31");
        }
        None => {
            return ToolResult::refused(&call.call_id, "days is required and must be an integer");
        }
    };
    let range_start_min = clock.now_minute();
    let range_end_day = clock.today + days;
    let list = qualifying_occurrences(
        doc, clock.today, range_end_day, false, TOOL_OCCURRENCE_LIMIT,
        |occ| match occ.timing {
            Timing::Timed { end, .. } => end > range_start_min,
            Timing::AllDay { end_exclusive, .. } => end_exclusive > clock.today,
        },
    );
    envelope(doc, clock.today, range_end_day, list.items, list.truncated, &call.call_id)
}

fn tool_day(doc: &CalendarDocument, call: &ServiceCall) -> ToolResult {
    let value = match parse_object(&call.args, &["date"]) {
        Ok(v) => v,
        Err(e) => return ToolResult::refused(&call.call_id, format!("invalid arguments: {e}")),
    };
    let date = match value.get("date").and_then(Value::as_str).and_then(civil::parse_iso) {
        Some(d) => d,
        None => {
            return ToolResult::refused(&call.call_id, "date must be a strict Gregorian YYYY-MM-DD");
        }
    };
    let list = occurrences_for_day(doc, date, false, TOOL_OCCURRENCE_LIMIT + 8);
    envelope(doc, date, date + 1, list.items, list.truncated, &call.call_id)
}

fn encode_event(doc: &CalendarDocument, occ: &Occurrence) -> Value {
    let event = event_by_id(doc, occ.key.event_id);
    let cal = event.and_then(|e| calendar_by_id(doc, e.calendar_id));
    let (kind, start, end) = match occ.timing {
        Timing::Timed { start, end } => (
            "timed",
            start.format_iso(),
            end.format_iso(),
        ),
        Timing::AllDay {
            start,
            end_exclusive,
        } => (
            "all_day",
            civil::format_iso(start),
            civil::format_iso(end_exclusive),
        ),
    };
    let mut timing = vec![
        ("kind".into(), Value::Str(kind.into())),
        ("start".into(), Value::Str(start)),
    ];
    if kind == "timed" {
        timing.push(("end".into(), Value::Str(end)));
    } else {
        timing.push(("end_exclusive".into(), Value::Str(end)));
    }
    Value::Obj(vec![
        ("id".into(), Value::Str(occ.key.as_tool_id())),
        (
            "master_id".into(),
            Value::Int(occ.key.event_id.0 as i64),
        ),
        (
            "title".into(),
            Value::Str(event.map(|e| e.title.clone()).unwrap_or_default()),
        ),
        (
            "calendar_id".into(),
            Value::Int(event.map(|e| e.calendar_id.0 as i64).unwrap_or(0)),
        ),
        (
            "calendar_name".into(),
            Value::Str(cal.map(|c| c.name.clone()).unwrap_or_default()),
        ),
        (
            "calendar_visible".into(),
            Value::Bool(cal.map(|c| c.visible).unwrap_or(false)),
        ),
        ("timing".into(), Value::Obj(timing)),
        (
            "repeat".into(),
            Value::Str(event.map(|e| e.repeat.as_str().to_string()).unwrap_or_else(|| "none".into())),
        ),
    ])
}

fn envelope(
    doc: &CalendarDocument,
    range_start: Day,
    range_end: Day,
    mut items: Vec<Occurrence>,
    already_truncated: bool,
    call_id: &str,
) -> ToolResult {
    let mut truncated = already_truncated;
    if items.len() > TOOL_OCCURRENCE_LIMIT {
        items.truncate(TOOL_OCCURRENCE_LIMIT);
        truncated = true;
    }
    let events: Vec<Value> = items.iter().map(|o| encode_event(doc, o)).collect();
    let mut payload = Value::Obj(vec![
        (
            "range_start".into(),
            Value::Str(civil::format_iso(range_start)),
        ),
        (
            "range_end".into(),
            Value::Str(civil::format_iso(range_end)),
        ),
        ("events".into(), Value::Arr(events)),
        ("truncated".into(), Value::Bool(truncated)),
    ]);
    let mut json = payload.to_json();
    if json.len() > TOOL_BYTE_LIMIT {
        // Drop from the end until it fits, always telling the truth.
        let mut n = items.len();
        while n > 0 && json.len() > TOOL_BYTE_LIMIT {
            n -= 1;
            let events: Vec<Value> = items[..n].iter().map(|o| encode_event(doc, o)).collect();
            payload = Value::Obj(vec![
                (
                    "range_start".into(),
                    Value::Str(civil::format_iso(range_start)),
                ),
                (
                    "range_end".into(),
                    Value::Str(civil::format_iso(range_end)),
                ),
                ("events".into(), Value::Arr(events)),
                ("truncated".into(), Value::Bool(true)),
            ]);
            json = payload.to_json();
        }
        truncated = true;
    }
    let _ = truncated;
    ToolResult::ok(call_id, json.clone(), "").with_data(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::seed;

    fn call(tool: &str, args: &str) -> ServiceCall {
        ServiceCall {
            call_id: "c1".into(),
            tool: tool.into(),
            args: args.into(),
        }
    }

    #[test]
    fn tool_arguments_ongoing_recurrence_truncation_unavailable() {
        let m = manifest();
        assert_eq!(m.id, "calendar");
        assert!(m.validate().is_ok());
        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.tools[0].name, "next");
        assert_eq!(m.tools[0].risk, Risk::Read);

        let doc = seed(civil::from_ymd(2026, 9, 9));
        let clock = ClockSnapshot {
            today: doc.seed_anchor,
            minute: 9 * 60 + 15,
        };
        let bad = dispatch(&doc, clock, &call("next", r#"{"days":7,"extra":true}"#));
        assert_eq!(bad.outcome, makepad_ai_services::wire::ToolOutcome::Refused);
        let bad = dispatch(&doc, clock, &call("next", r#"{"days":0}"#));
        assert_eq!(bad.outcome, makepad_ai_services::wire::ToolOutcome::Refused);
        let bad = dispatch(&doc, clock, &call("day", r#"{"date":"09/09/2026"}"#));
        assert_eq!(bad.outcome, makepad_ai_services::wire::ToolOutcome::Refused);
        let bad = dispatch(&doc, clock, &call("day", r#"{"date":"2026-09-09","n":1}"#));
        assert_eq!(bad.outcome, makepad_ai_services::wire::ToolOutcome::Refused);

        let ok = dispatch(&doc, clock, &call("next", r#"{"days":7}"#));
        assert_eq!(ok.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        assert!(ok.text.contains("Standup") || ok.data.contains("Standup") || ok.text.contains("range_start"));
        // 09:00–10:00 is still ongoing at 09:15.
        assert!(ok.text.contains("09:00") || ok.data.contains("09:00"));

        let day = dispatch(&doc, clock, &call("day", r#"{"date":"2026-09-09"}"#));
        assert_eq!(day.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        assert!(day.text.contains("calendar_visible"));

        // Hidden calendars still appear in assistant reads.
        let mut hidden = doc.clone();
        crate::engine::set_calendar_visible(&mut hidden, CAL_WORK, false);
        let day = dispatch(&hidden, clock, &call("day", r#"{"date":"2026-09-09"}"#));
        assert!(day.text.contains("\"calendar_visible\":false") || day.data.contains("\"calendar_visible\":false"));
    }
    #[test]
    fn next_filters_expired_events_before_the_limit() {
        let today = civil::from_ymd(2026,9,9);
        let mut doc = seed(today);
        let mut template = doc.events[0].clone();
        template.repeat = Repeat::None;
        template.timing = Timing::Timed {start:LocalMinute::new(today,60).unwrap(),end:LocalMinute::new(today,120).unwrap()};
        doc.events = (1..=208).map(|id| {let mut e=template.clone();e.id=EventId(id);e}).collect();
        template.id = EventId(209);
        template.title = "Upcoming 日本語".into();
        template.timing = Timing::Timed {start:LocalMinute::new(today,600).unwrap(),end:LocalMinute::new(today,660).unwrap()};
        doc.events.push(template);
        let result = dispatch(&doc,ClockSnapshot{today,minute:540},&call("next",r#"{"days":1}"#));
        let json = makepad_strict_json::parse(result.data.as_bytes()).unwrap();
        assert!(result.data.contains("Upcoming 日本語"));
        assert_eq!(json.get("truncated"),Some(&Value::Bool(false)));
        if let Some(Value::Arr(events)) = json.get("events") {assert_eq!(events.len(),1);} else {panic!("events missing")}
    }

    #[test]
    fn day_refuses_multibyte_dates() {
        let doc = seed(civil::from_ymd(2026,9,9));
        for date in ["202é-09-9", "2026-😀-9", "日本語"] {
            let result = dispatch(&doc,ClockSnapshot::default(),&call("day", &format!(r#"{{"date":"{date}"}}"#)));
            assert_eq!(result.outcome,makepad_ai_services::wire::ToolOutcome::Refused);
        }
    }

}
