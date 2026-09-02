//! photos on the AI bus: three read tools over the wall on screen, and one
//! that adds to it.
//!
//! `search{query}` finds pictures by the words they carry (for the comic
//! archive: the date and the hover text), `show{item}` glides the wall
//! onto one, `summary` says what is open. `add{path}` bakes one picture
//! from disk into the open library — the way a generated image lands on
//! the wall — and answers LATER, when the bake thread reports; the view
//! carries that answer through its reply hook, so `answer` says
//! [`Answered::Later`] and the caller (the port, the module executor)
//! waits. Under the window manager the port is the bus, standalone it is
//! the F10 overlay's in-process link, as a module it is the executor —
//! the same `answer` in every case.

use crate::view::PhotosView;
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_strict_json as json;
use makepad_widgets::*;
use std::path::{Path, PathBuf};

/// A call's answer: now, or later through the view's reply hook.
pub enum Answered {
    Now(ToolResult),
    Later,
}

/// The manifest: who the app is and the tools it exposes.
pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "photos",
        "Photos",
        "The picture wall on screen — a pannable, zoomable grid of a baked \
         collection (the SMBC comic archive by default: each picture's title \
         is its date and the hover text the author wrote). Its tools only \
         look: search the words the pictures carry, and show one picture by \
         its id from a search. `add` puts a picture file from this machine \
         on the wall — a generated image saved under the makepad home, or \
         a file in the person's home — and shows it.",
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
        "filter",
        "Filter the wall on screen to the pictures whose title or link holds every word, best first — the pictures fly to their new places; an empty query shows everything again. Use search to LIST matches, filter to SHOW them.",
        r#"{"type":"object","properties":{"query":{"type":"string","description":"words to keep on the wall; empty clears"}},"required":["query"]}"#,
        Risk::Act,
    ))
    .with_tool(ToolDef::new(
        "summary",
        "How many pictures the wall shows and from which library.",
        r#"{"type":"object","properties":{}}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "add",
        "Add a picture file from this machine to the wall and show it: an absolute path to a PNG/JPEG/WebP/GIF under the makepad home's gen folder or under the person's home. Bakes it into the open library; answers when done.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"absolute path of the image file"},"title":{"type":"string","description":"a caption for the wall, optional"}},"required":["path"]}"#,
        Risk::Act,
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
pub fn answer(cx: &mut Cx, view: &mut PhotosView, call: &ServiceCall) -> Answered {
    Answered::Now(match call.tool.as_str() {
        "add" => {
            let id = call.call_id.as_str();
            let Some(path) = str_arg(call, "path").filter(|p| !p.is_empty()) else {
                return Answered::Now(ToolResult::refused(id, "add needs an absolute `path` to an image file"));
            };
            let path = PathBuf::from(&path);
            if let Err(why) = addable(&path) {
                return Answered::Now(ToolResult::refused(id, why));
            }
            let title = str_arg(call, "title").filter(|t| !t.is_empty()).unwrap_or_else(|| {
                path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            });
            return match view.start_add(id, &path, &title) {
                Ok(()) => Answered::Later,
                Err(why) => Answered::Now(ToolResult::failed(id, why)),
            };
        }
        _ => answer_now(cx, view, call),
    })
}

/// The jail for `add`: an existing image file under the makepad home's
/// `gen` folder (where the assistant's pictures land) or under the
/// person's own home. Never a relative path, never anything else.
pub fn addable(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("refused: the path must be absolute".to_string());
    }
    let ext_ok = path
        .extension()
        .map(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif"))
        .unwrap_or(false);
    if !ext_ok {
        return Err("refused: only png, jpg, jpeg, webp or gif files go on the wall".to_string());
    }
    let inside = allowed_roots().iter().any(|root| path.starts_with(root));
    if !inside {
        return Err("refused: only files under the makepad home's gen folder or the person's home".to_string());
    }
    if !path.is_file() {
        return Err(format!("refused: {} is not a file on this machine", path.display()));
    }
    Ok(())
}

/// Where an added picture may come from. Mirrors the hub's home rule
/// (`MAKEPAD_HOME`, else `~/.makepad`) without linking the hub: the
/// photos lib is a web pilot and must stay light.
pub fn allowed_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("MAKEPAD_HOME") {
        roots.push(PathBuf::from(home).join("gen"));
    }
    if let Some(user_home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let user_home = PathBuf::from(user_home);
        roots.push(user_home.join(".makepad").join("gen"));
        roots.push(user_home);
    }
    roots
}

fn answer_now(cx: &mut Cx, view: &mut PhotosView, call: &ServiceCall) -> ToolResult {
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
        "filter" => {
            let query = str_arg(call, "query").unwrap_or_default();
            view.set_query(cx, &query);
            let shown = view.visible(cx);
            let total = view.pictures();
            if query.is_empty() {
                ToolResult::ok(id, format!("the wall shows all {total} pictures again"), "cleared")
            } else if shown == 0 {
                ToolResult::ok(id, format!("nothing on the wall matches \"{query}\""), "no matches")
            } else {
                ToolResult::ok(id, format!("the wall shows the {shown} of {total} pictures matching \"{query}\""), format!("{shown} shown"))
            }
        }
        "summary" => ToolResult::ok(id, view.summary(), "the wall"),
        other => ToolResult::refused(id, format!("photos has no tool `{other}`; it has search, show, filter, summary, add")),
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
        assert_eq!(m.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["search", "show", "filter", "summary", "add"]);
        assert!(m.tools.iter().filter(|t| t.name != "add" && t.name != "filter").all(|t| t.risk == Risk::Read));
        assert_eq!(m.tool("filter").unwrap().risk, Risk::Act);
        assert_eq!(m.tool("add").unwrap().risk, Risk::Act);
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

    #[test]
    fn add_takes_only_image_files_inside_the_allowed_roots() {
        assert!(addable(Path::new("relative.png")).unwrap_err().contains("absolute"));
        assert!(addable(Path::new("/etc/passwd")).unwrap_err().contains("only png"));
        assert!(addable(Path::new("/etc/hosts.png")).unwrap_err().contains("only files under"));
        let home = std::env::var_os("HOME").map(PathBuf::from).expect("HOME");
        let missing = home.join("surely-not-here-zz.png");
        assert!(addable(&missing).unwrap_err().contains("not a file"));
        assert!(allowed_roots().iter().any(|r| r.ends_with("gen")));
    }
}
