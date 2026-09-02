//! photos on the AI bus: two read tools over the wall on screen.
//!
//! `search{query}` finds pictures by the words they carry (for the comic
//! archive: the date and the hover text), `show{item}` glides the wall
//! onto one. Both only look and move the camera; nothing is written.
//! Under the window manager the port is the bus, standalone it is the
//! F10 overlay's in-process link, as a module it is the executor — the
//! same `answer` in every case.

use crate::view::PhotosView;
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_strict_json as json;
use makepad_widgets::*;

/// The manifest: who the app is and the tools it exposes.
pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "photos",
        "Photos",
        "The picture wall on screen — a pannable, zoomable grid of a baked \
         collection (the SMBC comic archive by default: each picture's title \
         is its date and the hover text the author wrote). Its tools only \
         look: search the words the pictures carry, and show one picture by \
         its id from a search.",
    )
    .with_tool(ToolDef::new(
        "search",
        "Find pictures whose title or link contains every word of the query. Answers up to 12 hits as `id — title`; use an id with show.",
        r#"{"type":"object","properties":{"query":{"type":"string","description":"words to look for"}},"required":["query"]}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "show",
        "Glide the wall onto one picture so the person sees it large.",
        r#"{"type":"object","properties":{"item":{"type":"integer","description":"a picture id from search"}},"required":["item"]}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "summary",
        "How many pictures the wall shows and from which library.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Read,
    ))
}

/// One string argument of a call, trimmed; `None` when absent or not a string.
fn str_arg(call: &ServiceCall, key: &str) -> Option<String> {
    match json::parse(call.args.as_bytes()) {
        Ok(json::Value::Obj(fields)) => fields
            .into_iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_str().map(|s| s.trim().to_string())),
        _ => None,
    }
}

/// One integer argument of a call (a JSON number, or a numeric string).
fn int_arg(call: &ServiceCall, key: &str) -> Option<i64> {
    match json::parse(call.args.as_bytes()) {
        Ok(json::Value::Obj(fields)) => fields.into_iter().find(|(k, _)| k == key).and_then(|(_, v)| match v {
            json::Value::Int(i) => Some(i),
            json::Value::F64(f) => Some(f as i64),
            json::Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }),
        _ => None,
    }
}

/// Answer one call against the wall. Every branch answers; unknown names
/// are refused with the names that exist.
pub fn answer(cx: &mut Cx, view: &mut PhotosView, call: &ServiceCall) -> ToolResult {
    let id = call.call_id.as_str();
    match call.tool.as_str() {
        "search" => {
            let Some(query) = str_arg(call, "query").filter(|q| !q.is_empty()) else {
                return ToolResult::refused(id, "search needs a `query`");
            };
            let hits = view.search(cx, &query);
            if hits.is_empty() {
                return ToolResult::ok(id, format!("nothing on the wall matches \"{query}\""), "no matches");
            }
            let text = hits
                .iter()
                .map(|(item, title, _)| format!("{item} — {}", title.chars().take(160).collect::<String>()))
                .collect::<Vec<_>>()
                .join("\n");
            let data = json::Value::Arr(
                hits.iter()
                    .map(|(item, title, link)| {
                        json::obj(vec![("id", json::Value::Int(*item)), ("title", json::s(title.clone())), ("link", json::s(link.clone()))])
                    })
                    .collect(),
            )
            .to_json();
            ToolResult::ok(id, text, format!("{} match(es)", hits.len())).with_data(data)
        }
        "show" => {
            let Some(item) = int_arg(call, "item") else {
                return ToolResult::refused(id, "show needs an `item` id from search");
            };
            if view.show(cx, item) {
                ToolResult::ok(id, format!("showing picture {item}"), "shown")
            } else {
                ToolResult::failed(id, format!("no picture {item} on the wall"))
            }
        }
        "summary" => ToolResult::ok(id, view.summary(), "the wall"),
        other => ToolResult::refused(id, format!("photos has no tool `{other}`; it has search, show, summary")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_validates_and_only_reads() {
        let m = manifest();
        assert_eq!(m.id, "photos");
        m.validate().expect("a manifest the wire accepts");
        assert_eq!(m.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["search", "show", "summary"]);
        assert!(m.tools.iter().all(|t| t.risk == Risk::Read));
    }

    #[test]
    fn arguments_are_read_by_name_and_kind() {
        let call = ServiceCall { call_id: "c".into(), tool: "show".into(), args: r#"{"item": 42, "query": " robot "}"#.into() };
        assert_eq!(int_arg(&call, "item"), Some(42));
        assert_eq!(str_arg(&call, "query").as_deref(), Some("robot"));
        let text_id = ServiceCall { call_id: "c".into(), tool: "show".into(), args: r#"{"item": "7"}"#.into() };
        assert_eq!(int_arg(&text_id, "item"), Some(7));
        assert_eq!(int_arg(&call, "nope"), None);
        let bad = ServiceCall { call_id: "c".into(), tool: "show".into(), args: "[]".into() };
        assert_eq!(int_arg(&bad, "item"), None);
    }
}
