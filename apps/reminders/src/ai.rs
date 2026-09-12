//! Read-only assistant tools `due` and `list`.

use crate::engine::{due_within, project};
use crate::model::*;
use makepad_ai_services::wire::{
    Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult, MAX_ARGS_BYTES, MAX_DATA_BYTES,
    MAX_RESULT_BYTES,
};
use makepad_civil_time as civil;
use makepad_strict_json::{self as json, Value};

const MAX_ITEMS: usize = 50;
const MAX_OUTPUT_BYTES: usize = if MAX_DATA_BYTES < MAX_RESULT_BYTES {
    MAX_DATA_BYTES
} else {
    MAX_RESULT_BYTES
};
const MAX_NOTES_EXCERPT: usize = 160;

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "reminders",
        "Reminders",
        "Personal reminders: due items, lists, flags and completion.",
    )
    .with_tool(ToolDef::new(
        "due",
        "Incomplete dated reminders due before today plus N days, including overdue items and today.",
        r#"{"type":"object","properties":{"days":{"type":"integer","minimum":1,"maximum":31}},"required":["days"],"additionalProperties":false}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "list",
        "Reminders in a smart list or personal list, using the same projection as the UI.",
        r#"{"type":"object","properties":{"name":{"type":"string","minLength":1,"maxLength":64}},"required":["name"],"additionalProperties":false}"#,
        Risk::Read,
    ))
}

pub fn answer(
    document: Option<&Document>,
    saved_revision: u64,
    now: Now,
    call: &ServiceCall,
) -> ToolResult {
    let Some(document) = document else {
        return ToolResult::unavailable(&call.call_id, "Reminders are still loading");
    };
    match call.tool.as_str() {
        "due" => due_tool(document, saved_revision, now, call),
        "list" => list_tool(document, saved_revision, now, call),
        other => ToolResult::refused(
            &call.call_id,
            format!("unknown tool `{other}`; this app has `due` and `list`"),
        ),
    }
}

fn parse_object(args: &str) -> Result<Vec<(String, Value)>, &'static str> {
    if args.len() > MAX_ARGS_BYTES {
        return Err("arguments are too large");
    }
    let value = json::parse(args.as_bytes()).map_err(|_| "arguments must be a JSON object")?;
    match value {
        Value::Obj(pairs) => Ok(pairs),
        _ => Err("arguments must be a JSON object"),
    }
}

fn due_tool(document: &Document, saved_revision: u64, now: Now, call: &ServiceCall) -> ToolResult {
    let obj = match parse_object(&call.args) {
        Ok(obj) => obj,
        Err(msg) => return ToolResult::refused(&call.call_id, msg),
    };
    if obj.len() != 1 || obj[0].0 != "days" {
        return ToolResult::refused(&call.call_id, "due requires only integer days 1–31");
    }
    let days = match obj[0].1.as_i64() {
        Some(n) if (1..=31).contains(&n) => n as i32,
        _ => return ToolResult::refused(&call.call_id, "due requires only integer days 1–31"),
    };
    let items = due_within(document, now, days);
    pack(document, saved_revision, now, &items, call)
}

fn list_tool(document: &Document, saved_revision: u64, now: Now, call: &ServiceCall) -> ToolResult {
    let obj = match parse_object(&call.args) {
        Ok(obj) => obj,
        Err(msg) => return ToolResult::refused(&call.call_id, msg),
    };
    if obj.len() != 1 || obj[0].0 != "name" {
        return ToolResult::refused(&call.call_id, "list requires a list name");
    }
    let name = match &obj[0].1 {
        Value::Str(s) => s,
        _ => return ToolResult::refused(&call.call_id, "list requires a list name"),
    };
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_LIST_NAME_CHARS {
        return ToolResult::refused(&call.call_id, "list name must be 1–64 characters");
    }
    let Some(filter) = Filter::parse_name(trimmed, &document.lists) else {
        return ToolResult::refused(&call.call_id, format!("unknown list `{trimmed}`"));
    };
    let projection = project(document, filter, now);
    let items: Vec<&Reminder> = projection
        .groups
        .iter()
        .flat_map(|g| g.items.iter())
        .filter_map(|item| document.reminder(item.id))
        .collect();
    pack(document, saved_revision, now, &items, call)
}

fn excerpt(notes: &str) -> (String, bool) {
    let count = notes.chars().count();
    if count <= MAX_NOTES_EXCERPT {
        (notes.to_string(), false)
    } else {
        (notes.chars().take(MAX_NOTES_EXCERPT).collect(), true)
    }
}

fn item_value(document: &Document, item: &Reminder) -> Value {
    let list_name = document
        .list(item.list_id)
        .map(|l| l.name.as_str())
        .unwrap_or("");
    let (excerpt, truncated) = excerpt(&item.notes);
    let (due_date, due_time) = match item.due {
        Some(due) => (
            Value::Str(civil::format_iso(due.day)),
            match due.minute {
                Some(minute) => json::s(format_time(minute)),
                None => Value::Null,
            },
        ),
        None => (Value::Null, Value::Null),
    };
    json::obj(vec![
        ("id", Value::Int(item.id as i64)),
        ("title", json::s(item.title.clone())),
        ("list_id", Value::Int(item.list_id as i64)),
        ("list_name", json::s(list_name)),
        ("due_date", due_date),
        ("due_time", due_time),
        ("flagged", Value::Bool(item.flagged)),
        ("priority", json::s(item.priority.as_str())),
        ("completed", Value::Bool(item.is_completed())),
        ("notes_excerpt", json::s(excerpt)),
        ("notes_truncated", Value::Bool(truncated)),
    ])
}

fn pack(
    document: &Document,
    saved_revision: u64,
    now: Now,
    items: &[&Reminder],
    call: &ServiceCall,
) -> ToolResult {
    let total = items.len();
    let mut packed = Vec::new();
    let mut truncated = false;
    for item in items {
        if packed.len() >= MAX_ITEMS {
            truncated = true;
            break;
        }
        packed.push(item_value(document, item));
        let probe = envelope(
            document.revision,
            saved_revision,
            now,
            total,
            &packed,
            false,
        );
        if probe.len() > MAX_OUTPUT_BYTES {
            packed.pop();
            truncated = true;
            break;
        }
    }
    if packed.len() < total {
        truncated = true;
    }
    let json = envelope(
        document.revision,
        saved_revision,
        now,
        total,
        &packed,
        truncated,
    );
    ToolResult::ok(
        &call.call_id,
        json.clone(),
        format!("{} reminders", packed.len()),
    )
    .with_data(json)
}

fn envelope(
    revision: u64,
    saved_revision: u64,
    now: Now,
    total: usize,
    items: &[Value],
    truncated: bool,
) -> String {
    json::obj(vec![
        ("revision", Value::Int(revision as i64)),
        ("saved_revision", Value::Int(saved_revision as i64)),
        (
            "as_of",
            json::s(format!(
                "{}T{}",
                civil::format_iso(now.day),
                format_time(now.minute)
            )),
        ),
        ("total", Value::Int(total as i64)),
        ("returned", Value::Int(items.len() as i64)),
        ("truncated", Value::Bool(truncated)),
        ("items", Value::Arr(items.to_vec())),
    ])
    .to_json()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::seed;
    use makepad_civil_time::from_ymd;

    fn call(tool: &str, args: &str) -> ServiceCall {
        ServiceCall {
            call_id: "c1".into(),
            tool: tool.into(),
            args: args.into(),
        }
    }

    fn now() -> Now {
        Now {
            day: from_ymd(2026, 9, 9),
            minute: 12 * 60,
        }
    }

    #[test]
    fn tools_validate_bounds_names_truncation_and_do_not_mutate() {
        let m = manifest();
        assert_eq!(m.id, "reminders");
        assert!(m.validate().is_ok());
        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.tools[0].name, "due");
        assert_eq!(m.tools[0].risk, Risk::Read);
        let mut document = seed(now().day);
        let rev = document.revision;
        let loading = answer(None, 0, now(), &call("due", r#"{"days":7}"#));
        assert_eq!(
            loading.outcome,
            makepad_ai_services::wire::ToolOutcome::Unavailable
        );
        let bad = answer(Some(&document), 1, now(), &call("due", r#"{"days":0}"#));
        assert_eq!(bad.outcome, makepad_ai_services::wire::ToolOutcome::Refused);
        let extra = answer(
            Some(&document),
            1,
            now(),
            &call("due", r#"{"days":7,"x":1}"#),
        );
        assert_eq!(
            extra.outcome,
            makepad_ai_services::wire::ToolOutcome::Refused
        );
        let unknown = answer(Some(&document), 1, now(), &call("search", r#"{}"#));
        assert_eq!(
            unknown.outcome,
            makepad_ai_services::wire::ToolOutcome::Refused
        );
        let ok = answer(Some(&document), 1, now(), &call("due", r#"{"days":7}"#));
        assert!(ok.outcome.is_ok(), "{}", ok.text);
        let parsed = json::parse(ok.text.as_bytes()).unwrap();
        assert_eq!(parsed.get("total").and_then(|v| v.as_i64()), Some(18));
        assert_eq!(parsed.get("returned").and_then(|v| v.as_i64()), Some(18));
        let work = answer(
            Some(&document),
            1,
            now(),
            &call("list", r#"{"name":"work"}"#),
        );
        assert!(work.outcome.is_ok(), "{}", work.text);
        let missing = answer(
            Some(&document),
            1,
            now(),
            &call("list", r#"{"name":"Nope"}"#),
        );
        assert_eq!(
            missing.outcome,
            makepad_ai_services::wire::ToolOutcome::Refused
        );
        document.reminders[0].notes = "é".repeat(200);
        let flagged = answer(
            Some(&document),
            1,
            now(),
            &call("list", r#"{"name":"Groceries"}"#),
        );
        let parsed = json::parse(flagged.text.as_bytes()).unwrap();
        let items = parsed.get("items").and_then(|v| v.as_arr()).unwrap();
        let first = items
            .iter()
            .find(|v| v.get("id").and_then(|v| v.as_i64()) == Some(1))
            .unwrap();
        assert_eq!(
            first
                .get("notes_excerpt")
                .and_then(|v| v.as_str())
                .unwrap()
                .chars()
                .count(),
            160
        );
        assert_eq!(
            first.get("notes_truncated").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(document.revision, rev);
    }
    fn many_document(count: u32, title: &str, notes: &str) -> Document {
        let mut document = seed(now().day);
        let template = document.reminder(1).unwrap().clone();
        document.reminders = (1..=count)
            .map(|id| Reminder {
                id,
                order: id,
                title: title.into(),
                notes: notes.into(),
                ..template.clone()
            })
            .collect();
        document.next_reminder_id = count + 1;
        document
    }

    #[test]
    fn transport_caps_keep_whole_items_and_structured_json() {
        for (title, notes) in [
            ("x".repeat(240), "n".repeat(160)),
            ("界\\\"".repeat(80), "\n界\t\\\"".repeat(40)),
        ] {
            let document = many_document(50, &title, &notes);
            let before = document.clone();
            let request = call("list", r#"{"name":"All"}"#);
            let result = answer(Some(&document), 0, now(), &request);
            assert!(!result.data.is_empty());
            assert!(result.text.len() <= MAX_RESULT_BYTES);
            assert!(result.data.len() <= MAX_DATA_BYTES);
            let mut bounded = result.clone();
            bounded.bound();
            assert_eq!(
                bounded, result,
                "transport must neither drop data nor truncate JSON"
            );
            assert_eq!(result, answer(Some(&document), 0, now(), &request));
            let value = json::parse(result.data.as_bytes()).unwrap();
            let items = value.get("items").unwrap().as_arr().unwrap();
            assert!(!items.is_empty() && items.len() < 50);
            assert_eq!(value.get("total").unwrap().as_i64(), Some(50));
            assert_eq!(
                value.get("returned").unwrap().as_i64(),
                Some(items.len() as i64)
            );
            assert_eq!(value.get("truncated").unwrap().as_bool(), Some(true));
            assert_eq!(value.get("saved_revision").unwrap().as_i64(), Some(0));
            for (index, item) in items.iter().enumerate() {
                assert_eq!(item.get("id").unwrap().as_i64(), Some(index as i64 + 1));
                assert_eq!(item.get("title").unwrap().as_str(), Some(title.as_str()));
                assert_eq!(
                    item.get("notes_excerpt").unwrap().as_str(),
                    Some(excerpt(&notes).0.as_str())
                );
            }
            assert_eq!(document, before);
        }
    }

    #[test]
    fn item_limit_and_all_argument_boundaries() {
        let document = many_document(60, "a", "");
        let result = answer(
            Some(&document),
            1,
            now(),
            &call("list", r#"{"name":"  gRoCeRiEs  "}"#),
        );
        let value = json::parse(result.data.as_bytes()).unwrap();
        assert_eq!(value.get("total").unwrap().as_i64(), Some(60));
        assert_eq!(value.get("returned").unwrap().as_i64(), Some(50));
        assert_eq!(value.get("truncated").unwrap().as_bool(), Some(true));
        for args in [
            r#"{"days":0}"#,
            r#"{"days":32}"#,
            r#"{"days":1.5}"#,
            r#"{"days":"1"}"#,
            r#"{"days":null}"#,
            r#"{"days":true}"#,
            r#"{"days":1,"days":2}"#,
            r#"{}"#,
            r#"[]"#,
        ] {
            assert!(
                !answer(Some(&document), 1, now(), &call("due", args))
                    .outcome
                    .is_ok(),
                "{args}"
            );
        }
        for days in [1, 31] {
            assert!(answer(
                Some(&document),
                1,
                now(),
                &call("due", &format!(r#"{{"days":{days}}}"#))
            )
            .outcome
            .is_ok());
        }
        for args in [
            r#"{"name":""}"#,
            r#"{"name":"   "}"#,
            r#"{"name":5}"#,
            r#"{"name":null}"#,
            r#"{"name":"All","extra":1}"#,
            r#"{"name":"All","name":"All"}"#,
            r#"{}"#,
        ] {
            assert!(
                !answer(Some(&document), 1, now(), &call("list", args))
                    .outcome
                    .is_ok(),
                "{args}"
            );
        }
        let long = json::obj(vec![("name", json::s("界".repeat(65)))]).to_json();
        assert!(!answer(Some(&document), 1, now(), &call("list", &long))
            .outcome
            .is_ok());
        let seed = seed(now().day);
        for name in [
            "Today",
            "Scheduled",
            "All",
            "Flagged",
            "Completed",
            "Groceries",
            "Work",
            "Home",
            "Travel",
        ] {
            let args = json::obj(vec![("name", json::s(name))]).to_json();
            let result = answer(Some(&seed), 1, now(), &call("list", &args));
            let value = json::parse(result.data.as_bytes()).unwrap();
            let actual: Vec<_> = value
                .get("items")
                .unwrap()
                .as_arr()
                .unwrap()
                .iter()
                .map(|item| item.get("id").unwrap().as_i64().unwrap() as u32)
                .collect();
            let expected: Vec<_> =
                project(&seed, Filter::parse_name(name, &seed.lists).unwrap(), now())
                    .groups
                    .into_iter()
                    .flat_map(|group| group.items.into_iter().map(|item| item.id))
                    .collect();
            assert_eq!(actual, expected);
        }
    }
}
