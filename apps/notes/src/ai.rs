//! Assistant tools: `notes.search` and `notes.read`. Read-only over in-memory state.

use crate::engine::{self, cap_preview_chars, display_title};
use crate::model::*;
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_strict_json::{self as json, Value};

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "notes",
        "Notes",
        "Notes stored on this device: folders, search and the full text of a note.",
    )
    .with_tool(ToolDef::new(
        "search",
        "Find active notes whose title or body contains the query. Answers id, title, preview, folder, edited_ms and pinned.",
        r#"{"type":"object","properties":{"query":{"type":"string","description":"text to find"}},"required":["query"]}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "read",
        "Read one active note by id: folder, title, complete source, timestamps and pinned state.",
        r#"{"type":"object","properties":{"id":{"type":"string","description":"note id such as n000001"}},"required":["id"]}"#,
        Risk::Read,
    ))
}

fn parse_object(args: &str) -> Result<Vec<(String, Value)>, String> {
    match json::parse(args.as_bytes()) {
        Ok(Value::Obj(fields)) => Ok(fields),
        Ok(_) => Err("arguments must be an object".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn require_only(fields: &[(String, Value)], keys: &[&str]) -> Result<(), String> {
    for (k, _) in fields {
        if !keys.contains(&k.as_str()) {
            return Err(format!("unknown argument `{k}`"));
        }
    }
    Ok(())
}

fn field<'a>(fields: &'a [(String, Value)], key: &str) -> Result<&'a Value, String> {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .ok_or_else(|| format!("missing `{key}`"))
}

pub fn answer(load: LoadState, doc: &NotesDocument, call: &ServiceCall) -> ToolResult {
    if load != LoadState::Ready {
        return ToolResult::unavailable(&call.call_id, "not_ready");
    }
    match call.tool.as_str() {
        "search" => search(doc, call),
        "read" => read(doc, call),
        other => ToolResult::refused(
            &call.call_id,
            format!("unknown tool `{other}`; this app has `search` and `read`"),
        ),
    }
}

fn search(doc: &NotesDocument, call: &ServiceCall) -> ToolResult {
    let fields = match parse_object(&call.args) {
        Ok(f) => f,
        Err(e) => return ToolResult::refused(&call.call_id, e),
    };
    if let Err(e) = require_only(&fields, &["query"]) {
        return ToolResult::refused(&call.call_id, e);
    }
    let query = match field(&fields, "query") {
        Ok(Value::Str(s)) => s,
        Ok(_) => return ToolResult::refused(&call.call_id, "query must be a string"),
        Err(e) => return ToolResult::refused(&call.call_id, e),
    };
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return ToolResult::refused(&call.call_id, "query must be a nonempty string");
    }
    if trimmed.len() > MAX_SEARCH_QUERY_BYTES {
        return ToolResult::refused(&call.call_id, "query is too long");
    }
    let hits = engine::search_active(doc, trimmed);
    let total = hits.len();
    let take = hits.iter().take(MAX_SEARCH_HITS);
    let mut items = Vec::new();
    // Reserve the complete envelope, including the worst-case boolean length.
    let envelope_bytes = json::obj(vec![
        ("total", Value::Int(total as i64)),
        ("truncated", Value::Bool(false)),
        ("hits", Value::Arr(Vec::new())),
    ])
    .to_json()
    .len();
    let mut output_bytes = envelope_bytes;
    for note in take {
        let item = json::obj(vec![
            ("id", json::s(note.id.wire())),
            ("title", json::s(display_title(&note.source))),
            (
                "preview",
                json::s(cap_preview_chars(&engine::display_preview(&note.source), MAX_PREVIEW_CHARS)),
            ),
            ("folder", json::s(note.folder.as_str())),
            ("edited_ms", Value::Int(note.edited_ms)),
            ("pinned", Value::Bool(note.pinned)),
        ]);
        let item_bytes = item.to_json().len() + usize::from(!items.is_empty());
        if output_bytes + item_bytes > MAX_SEARCH_OUTPUT_BYTES {
            break;
        }
        output_bytes += item_bytes;
        items.push(item);
    }
    let truncated = items.len() < total;
    let data = json::obj(vec![
        ("total", Value::Int(total as i64)),
        ("truncated", Value::Bool(truncated)),
        ("hits", Value::Arr(items)),
    ])
    .to_json();
    let text = if truncated {
        format!("{total} notes matched; results truncated")
    } else {
        format!("{total} notes matched")
    };
    ToolResult::ok(&call.call_id, text, "").with_data(data)
}

fn read(doc: &NotesDocument, call: &ServiceCall) -> ToolResult {
    let fields = match parse_object(&call.args) {
        Ok(f) => f,
        Err(e) => return ToolResult::refused(&call.call_id, e),
    };
    if let Err(e) = require_only(&fields, &["id"]) {
        return ToolResult::refused(&call.call_id, e);
    }
    let id_s = match field(&fields, "id") {
        Ok(Value::Str(s)) => s,
        Ok(_) => return ToolResult::refused(&call.call_id, "id must be a string"),
        Err(e) => return ToolResult::refused(&call.call_id, e),
    };
    let Some(id) = NoteId::parse(id_s) else {
        return ToolResult::refused(&call.call_id, "invalid id");
    };
    let Some(note) = doc.note(id) else {
        return ToolResult::failed(&call.call_id, "not_found");
    };
    if note.is_deleted() {
        return ToolResult::failed(&call.call_id, "not_found");
    }
    let data = json::obj(vec![
        ("id", json::s(note.id.wire())),
        ("folder", json::s(note.folder.as_str())),
        ("title", json::s(display_title(&note.source))),
        ("source", json::s(&note.source)),
        ("created_ms", Value::Int(note.created_ms)),
        ("edited_ms", Value::Int(note.edited_ms)),
        ("pinned", Value::Bool(note.pinned)),
    ])
    .to_json();
    if data.len() > MAX_READ_OUTPUT_BYTES {
        return ToolResult::failed(&call.call_id, "note is too large to read");
    }
    ToolResult::ok(&call.call_id, display_title(&note.source), "").with_data(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::set_source;
    use crate::model::{ByteSelection, EditKind, EditSession};
    use crate::seed;

    fn call(tool: &str, args: &str) -> ServiceCall {
        ServiceCall {
            call_id: "c1".into(),
            tool: tool.into(),
            args: args.into(),
        }
    }

    #[test]
    fn search_limits_whole_results_and_always_returns_complete_json() {
        for title in ["€".repeat(10_000), "a".repeat(30_000), "\"".repeat(30_000)] {
            let mut doc = NotesDocument::empty(0);
            for id in 1..=3 {
                doc.notes.push(Note {
                    id: NoteId(id), folder: FolderId::Notes,
                    source: format!("{title}\nneedle"), pinned: false,
                    created_ms: 0, edited_ms: 0, deleted_ms: None,
                });
            }
            doc.next_id = 4;
            assert!(crate::storage::decode(&crate::storage::encode(&doc).unwrap()).is_ok());
            let before = doc.clone();
            let result = answer(LoadState::Ready, &doc, &call("search", r#"{"query":"needle"}"#));
            assert_eq!(result.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
            assert!(result.data.len() <= MAX_SEARCH_OUTPUT_BYTES);
            let data = json::parse(result.data.as_bytes()).unwrap();
            assert_eq!(data.get("total").and_then(Value::as_i64), Some(3));
            assert_eq!(data.get("truncated").and_then(Value::as_bool), Some(true));
            let hits = data.get("hits").and_then(Value::as_arr).unwrap();
            assert!(!hits.is_empty() && hits.len() < 3);
            for hit in hits {
                assert_eq!(hit.get("title").and_then(Value::as_str), Some(title.as_str()));
            }
            assert_eq!(doc, before);
        }
    }

    #[test]
    fn manifest_has_exactly_two_read_tools() {
        let m = manifest();
        assert_eq!(m.id, "notes");
        assert!(m.validate().is_ok(), "{:?}", m.validate());
        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.tools[0].name, "search");
        assert_eq!(m.tools[1].name, "read");
        assert_eq!(m.tools[0].risk, Risk::Read);
        assert_eq!(m.tools[1].risk, Risk::Read);
    }

    #[test]
    fn search_and_read_use_accepted_edits_and_skip_deleted() {
        let mut doc = seed::generate(1_788_955_200_000);
        let mut session = EditSession::new(NoteId(1));
        let now = doc.seed_anchor_ms;
        set_source(
            &mut doc,
            &mut session,
            "Weekend groceries\n\nUNSAVED_TOKEN basil".into(),
            ByteSelection::caret(0),
            now,
            EditKind::Paste,
        )
        .unwrap();
        let found = answer(
            LoadState::Ready,
            &doc,
            &call("search", r#"{"query":"UNSAVED_TOKEN"}"#),
        );
        assert_eq!(found.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        assert!(found.data.contains("n000001"));
        let missing = answer(
            LoadState::Ready,
            &doc,
            &call("read", r#"{"id":"n000019"}"#),
        );
        assert!(missing.text.contains("not_found"));
        let ready = answer(LoadState::Loading, &doc, &call("search", r#"{"query":"a"}"#));
        assert!(ready.text.contains("not_ready"));
        let bad = answer(LoadState::Ready, &doc, &call("search", r#"{"query":"a","extra":1}"#));
        assert_eq!(bad.outcome, makepad_ai_services::wire::ToolOutcome::Refused);
        let empty = answer(LoadState::Ready, &doc, &call("search", r#"{"query":"   "}"#));
        assert_eq!(empty.outcome, makepad_ai_services::wire::ToolOutcome::Refused);
        let huge = "x".repeat(MAX_SEARCH_QUERY_BYTES + 1);
        let over = answer(
            LoadState::Ready,
            &doc,
            &call("search", &format!(r#"{{"query":"{huge}"}}"#)),
        );
        assert_eq!(over.outcome, makepad_ai_services::wire::ToolOutcome::Refused);
        let read_ok = answer(LoadState::Ready, &doc, &call("read", r#"{"id":"n000001"}"#));
        assert!(read_ok.data.contains("UNSAVED_TOKEN"));
    }
}
