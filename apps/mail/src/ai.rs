//! Read-only assistant tools over the mail document. Neither tool marks read.

use crate::engine::search;
use crate::model::*;
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_strict_json::{parse, Value};

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "mail",
        "Mail",
        "The local demo mailbox: search messages and read one message's plain text. Fake data only; sending stays in Sent.",
    )
    .with_tool(ToolDef::new(
        "search",
        "Search subject, sender and body. Returns at most 50 hits, newest first.",
        r#"{"type":"object","properties":{"query":{"type":"string","description":"Literal substring, 1 to 256 characters after trim"}},"required":["query"]}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "read",
        "Read one message by id such as m-000008. Returns plain text and attachment names.",
        r#"{"type":"object","properties":{"id":{"type":"string","description":"Message id such as m-000008"}},"required":["id"]}"#,
        Risk::Read,
    ))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadState {
    #[default]
    Loading,
    Ready,
    Error,
}

pub fn answer(doc: Option<&MailDocument>, load: LoadState, call: &ServiceCall) -> ToolResult {
    match call.tool.as_str() {
        "search" | "read" => {}
        _ => return ToolResult::refused(&call.call_id, "unknown tool; this app has search and read"),
    }
    match load {
        LoadState::Loading => {
            return ToolResult::unavailable(&call.call_id, "mail is still loading");
        }
        LoadState::Error => {
            return ToolResult::unavailable(&call.call_id, "mail failed to load");
        }
        LoadState::Ready => {}
    }
    let Some(doc) = doc else {
        return ToolResult::unavailable(&call.call_id, "mail is still loading");
    };
    match call.tool.as_str() {
        "search" => search_tool(doc, call),
        "read" => read_tool(doc, call),
        _ => unreachable!(),
    }
}

fn search_tool(doc: &MailDocument, call: &ServiceCall) -> ToolResult {
    let args = match parse_object(&call.args) {
        Ok(v) => v,
        Err(err) => return ToolResult::refused(&call.call_id, err),
    };
    if let Err(err) = only_keys(&args, &["query"]) {
        return ToolResult::refused(&call.call_id, err);
    }
    let Some(Value::Str(query)) = args.iter().find(|(k, _)| k == "query").map(|(_, v)| v) else {
        return ToolResult::refused(&call.call_id, "search requires string field `query`");
    };
    let query = query.trim();
    if query.is_empty() || query.chars().count() > MAX_SEARCH_QUERY {
        return ToolResult::refused(&call.call_id, "query must be 1 to 256 characters");
    }
    let ids = search(doc, query, SearchScope::AllMail);
    let total = ids.len();
    let mut hits: Vec<Value> = ids
        .iter()
        .take(MAX_SEARCH_HITS)
        .filter_map(|id| doc.message(*id))
        .map(|m| {
            makepad_strict_json::obj(vec![
                ("id", Value::Str(m.id.format())),
                ("subject", Value::Str(m.subject.clone())),
                ("from", encode_address(&m.from)),
                ("date", Value::Str(format_iso_timestamp(m.date_secs))),
                ("mailbox", Value::Str(m.folder.as_str().into())),
            ])
        })
        .collect();
    loop {
        let payload = makepad_strict_json::obj(vec![
            ("hits", Value::Arr(hits.clone())),
            ("total", Value::Int(total as i64)),
            ("truncated", Value::Bool(hits.len() < total)),
        ]);
        if payload.to_json().len() <= MAX_TOOL_RESPONSE_BYTES {
            return bound_ok(&call.call_id, payload);
        }
        hits.pop();
    }
}

fn read_tool(doc: &MailDocument, call: &ServiceCall) -> ToolResult {
    let args = match parse_object(&call.args) {
        Ok(v) => v,
        Err(err) => return ToolResult::refused(&call.call_id, err),
    };
    if let Err(err) = only_keys(&args, &["id"]) {
        return ToolResult::refused(&call.call_id, err);
    }
    let Some(Value::Str(id_raw)) = args.iter().find(|(k, _)| k == "id").map(|(_, v)| v) else {
        return ToolResult::refused(&call.call_id, "read requires string field `id`");
    };
    let Some(id) = MessageId::parse(id_raw) else {
        return ToolResult::refused(&call.call_id, "malformed id; expected m-000001");
    };
    let Some(message) = doc.message(id) else {
        return ToolResult::failed(&call.call_id, format!("unknown id {id_raw}"));
    };
    let mut fields = vec![
        ("id", Value::Str(message.id.format())),
        ("subject", Value::Str(message.subject.clone())),
        ("from", encode_address(&message.from)),
        (
            "to",
            Value::Arr(message.to.iter().map(encode_address).collect()),
        ),
        (
            "cc",
            Value::Arr(message.cc.iter().map(encode_address).collect()),
        ),
        ("date", Value::Str(format_iso_timestamp(message.date_secs))),
        ("mailbox", Value::Str(message.folder.as_str().into())),
        ("text", Value::Str(message.body_text.clone())),
        (
            "attachments",
            Value::Arr(
                message
                    .attachments
                    .iter()
                    .map(|a| {
                        makepad_strict_json::obj(vec![
                            ("name", Value::Str(a.name.clone())),
                            ("mime", Value::Str(a.mime.clone())),
                            ("size_bytes", Value::Int(a.size_bytes as i64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("demo", Value::Bool(true)),
    ];
    if let Some(draft) = &message.draft_input {
        fields.push(("to_raw", Value::Str(draft.to_raw.clone())));
        fields.push(("cc_raw", Value::Str(draft.cc_raw.clone())));
    }
    bound_ok(&call.call_id, makepad_strict_json::obj(fields))
}

fn encode_address(address: &Address) -> Value {
    makepad_strict_json::obj(vec![
        ("name", Value::Str(address.name.clone())),
        ("email", Value::Str(address.email.clone())),
    ])
}

fn parse_object(args: &str) -> Result<Vec<(String, Value)>, String> {
    if args.len() > 8192 { return Err("arguments exceed 8 KiB".into()); }
    let value = parse(args.as_bytes()).map_err(|_| "malformed arguments".to_string())?;
    match value {
        Value::Obj(pairs) => Ok(pairs),
        _ => Err("arguments must be a JSON object".into()),
    }
}

fn only_keys(pairs: &[(String, Value)], allowed: &[&str]) -> Result<(), String> {
    for (k, _) in pairs {
        if !allowed.contains(&k.as_str()) {
            return Err("unknown field".into());
        }
    }
    Ok(())
}

fn bound_ok(call_id: &str, payload: Value) -> ToolResult {
    let json = payload.to_json();
    if json.len() > MAX_TOOL_RESPONSE_BYTES {
        return ToolResult::failed(call_id, "result would exceed 64 KiB");
    }
    ToolResult::ok(call_id, json.clone(), "").with_data(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::seed;
    use crate::seed::test_anchor;
    use crate::storage::encode_document;

    fn call(tool: &str, args: &str) -> ServiceCall {
        ServiceCall { call_id: "c1".into(), tool: tool.into(), args: args.into() }
    }

    #[test]
    fn the_manifest_declares_two_read_tools() {
        let m = manifest();
        assert_eq!(m.id, "mail");
        assert!(m.validate().is_ok());
        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.tools[0].name, "search");
        assert_eq!(m.tools[1].name, "read");
        assert!(m.tools.iter().all(|t| t.risk == Risk::Read));
    }

    #[test]
    fn tools_reject_bad_args_and_do_not_mutate() {
        let doc = seed(test_anchor());
        let before = encode_document(&doc).unwrap();
        let unread = doc.message(MessageId(1)).unwrap().unread;
        assert_eq!(
            answer(Some(&doc), LoadState::Loading, &call("search", r#"{"query":"x"}"#)).outcome,
            makepad_ai_services::wire::ToolOutcome::Unavailable
        );
        assert!(answer(Some(&doc), LoadState::Ready, &call("nope", "{}")).text.contains("unknown tool"));
        assert!(answer(Some(&doc), LoadState::Ready, &call("search", r#"{"query":"x","extra":1}"#)).text.contains("unknown field"));
        assert!(answer(Some(&doc), LoadState::Ready, &call("search", r#"{"query":""}"#)).text.contains("query"));
        assert!(answer(Some(&doc), LoadState::Ready, &call("read", r#"{"id":"nope"}"#)).text.contains("malformed"));
        let missing = answer(Some(&doc), LoadState::Ready, &call("read", r#"{"id":"m-000099"}"#));
        assert_eq!(missing.outcome, makepad_ai_services::wire::ToolOutcome::Failed);
        let hits = answer(Some(&doc), LoadState::Ready, &call("search", r#"{"query":"rotterdam"}"#));
        assert_eq!(hits.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        assert!(hits.text.contains("\"hits\""));
        let read = answer(Some(&doc), LoadState::Ready, &call("read", r#"{"id":"m-000008"}"#));
        assert_eq!(read.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        assert!(read.text.contains("rail-tickets.pdf"));
        assert!(read.text.contains("\"demo\":true"));
        let after = encode_document(&doc).unwrap();
        assert_eq!(before, after);
        assert_eq!(doc.message(MessageId(1)).unwrap().unread, unread);
    }
    #[test]
    fn unicode_queries_and_hostile_arguments_are_bounded() {
        let doc = seed(test_anchor());
        let query = "é".repeat(129);
        let args = makepad_strict_json::obj(vec![("query", Value::Str(query))]).to_json();
        assert_eq!(answer(Some(&doc), LoadState::Ready, &call("search", &args)).outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        for (tool, args) in [("read", format!(r#"{{"id":"{}"}}"#, "x".repeat(100_000))), ("search", format!(r#"{{"{}":1}}"#, "x".repeat(100_000))), ("read", "{broken".into())] {
            let result = answer(Some(&doc), LoadState::Ready, &call(tool, &args));
            assert!(result.text.len() < 256);
            assert_ne!(result.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        }
        let result = answer(Some(&doc), LoadState::Ready, &call(&"x".repeat(100_000), "{}"));
        assert!(result.text.len() < 256);
    }

    #[test]
    fn search_truncates_by_encoded_bytes_and_count_without_mutating() {
        let mut doc = seed(test_anchor());
        for m in &mut doc.messages {
            m.subject = "needle".into();
            m.from.name = "é\\".repeat(700);
        }
        // Valid documents can contain large sender metadata; results still fit the tool cap.
        doc.me.name = "é\\".repeat(700);
        encode_document(&doc).unwrap();
        let before = doc.clone();
        let result = answer(Some(&doc), LoadState::Ready, &call("search", r#"{"query":"needle"}"#));
        assert_eq!(result.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        assert!(result.text.len() <= MAX_TOOL_RESPONSE_BYTES);
        let value = parse(result.text.as_bytes()).unwrap();
        let hits = value.get("hits").unwrap().as_arr().unwrap();
        assert!(!hits.is_empty() && hits.len() < 50);
        assert_eq!(value.get("total").unwrap().as_i64(), Some(60));
        assert_eq!(value.get("truncated").unwrap().as_bool(), Some(true));
        let expected = search(&doc, "needle", SearchScope::AllMail);
        for (hit, id) in hits.iter().zip(expected) {
            assert_eq!(hit.get("id").unwrap().as_str(), Some(id.format().as_str()));
        }
        assert_eq!(doc, before);
        for m in &mut doc.messages { m.from.name.clear(); }
        let result = answer(Some(&doc), LoadState::Ready, &call("search", r#"{"query":"needle"}"#));
        assert_eq!(parse(result.text.as_bytes()).unwrap().get("hits").unwrap().as_arr().unwrap().len(), 50);
    }

}
