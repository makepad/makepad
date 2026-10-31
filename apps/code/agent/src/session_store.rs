//! Sessions on disk: one small JSON file per session under `session_dir`.

use super::context::Item;
use super::event::SessionInfo;
use makepad_strict_json::{self as json, Value};
use std::path::{Path, PathBuf};

pub struct Saved {
    pub id: String,
    pub title: String,
    pub started: u64,
    pub items: Vec<Item>,
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `2026-10-11 20:15` (UTC) from unix seconds.
pub fn format_when(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // civil-from-days
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

pub fn new_id() -> String {
    format!("s{}", now_secs())
}

fn path_of(dir: &Path, id: &str) -> Option<PathBuf> {
    // Ids are file names: refuse anything that could leave the folder.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    Some(dir.join(format!("{id}.json")))
}

fn item_value(item: &Item) -> Value {
    match item {
        Item::User { text, refs, .. } => {
            let mut fields = vec![("k", json::s("user")), ("t", json::s(text.clone()))];
            if !refs.is_empty() {
                fields.push(("refs", Value::Arr(refs.iter().map(|r| json::s(r.clone())).collect())));
            }
            json::obj(fields)
        }
        Item::Assistant(t) => json::obj(vec![("k", json::s("assistant")), ("t", json::s(t.clone()))]),
        Item::Summary(t) => json::obj(vec![("k", json::s("summary")), ("t", json::s(t.clone()))]),
        Item::Tool { name, args, result, ok } => json::obj(vec![
            ("k", json::s("tool")),
            ("name", json::s(name.clone())),
            ("args", json::s(args.clone())),
            ("result", json::s(result.clone())),
            ("ok", Value::Bool(*ok)),
        ]),
    }
}

fn item_from(v: &Value) -> Option<Item> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(match v.get("k")?.as_str()? {
        "user" => Item::User {
            text: text("t")?,
            refs: v
                .get("refs")
                .and_then(Value::as_arr)
                .map(|a| a.iter().filter_map(|r| r.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            context: String::new(),
        },
        "assistant" => Item::Assistant(text("t")?),
        "summary" => Item::Summary(text("t")?),
        "tool" => Item::Tool {
            name: text("name")?,
            args: text("args")?,
            result: text("result")?,
            ok: v.get("ok").and_then(Value::as_bool).unwrap_or(true),
        },
        _ => return None,
    })
}

pub fn save(dir: &Path, saved: &Saved) -> std::io::Result<()> {
    let path = path_of(dir, &saved.id)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad session id"))?;
    std::fs::create_dir_all(dir)?;
    let doc = json::obj(vec![
        ("version", Value::Int(1)),
        ("id", json::s(saved.id.clone())),
        ("title", json::s(saved.title.clone())),
        ("started", Value::Int(saved.started as i64)),
        ("items", Value::Arr(saved.items.iter().map(item_value).collect())),
    ]);
    // Write beside, then rename, so a crash never leaves half a session.
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, doc.to_json())?;
    std::fs::rename(&tmp, &path)
}

pub fn load(dir: &Path, id: &str) -> Result<Saved, String> {
    let path = path_of(dir, id).ok_or("bad session id")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read session {id}: {e}"))?;
    // Sessions nest deeper than the default cap allows (obj > arr > obj).
    let v = json::parse_depth(&bytes, 16).map_err(|e| format!("session {id} is damaged: {e}"))?;
    let items = v.get("items").and_then(Value::as_arr).ok_or("session has no items")?;
    Ok(Saved {
        id: id.to_string(),
        title: v.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        started: v.get("started").and_then(Value::as_u64).unwrap_or(0),
        items: items.iter().filter_map(item_from).collect(),
    })
}

/// Remove a saved session (its file). Removing one that is not there is
/// not an error.
pub fn delete(dir: &Path, id: &str) -> std::io::Result<()> {
    let path = path_of(dir, id).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad session id"))?;
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Newest first.
pub fn list(dir: &Path) -> Vec<SessionInfo> {
    let mut rows: Vec<(u64, SessionInfo)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".json") else { continue };
        if let Ok(s) = load(dir, id) {
            let turns = s.items.iter().filter(|i| matches!(i, Item::User { .. })).count();
            rows.push((s.started, SessionInfo { id: s.id, title: s.title, when: format_when(s.started), turns }));
        }
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter().map(|r| r.1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(format_when(0), "1970-01-01 00:00");
        assert_eq!(format_when(1_760_000_000), "2025-10-09 08:53");
    }

    #[test]
    fn save_load_list_roundtrip() {
        let dir = std::env::temp_dir().join(format!("code-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let saved = Saved {
            id: "s100".into(),
            title: "fix \"it\"".into(),
            started: 100,
            items: vec![
                Item::User { text: "hi\nthere".into(), refs: vec!["src/a.rs".into(), "docs".into()], context: String::new() },
                Item::user("plain"),
                Item::Tool { name: "grep".into(), args: "pattern=x".into(), result: "a:1:y".into(), ok: false },
                Item::Summary("s".into()),
            ],
        };
        save(&dir, &saved).unwrap();
        let back = load(&dir, "s100").unwrap();
        assert_eq!(back.items, saved.items);
        assert_eq!(back.title, saved.title);
        let rows = list(&dir);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].turns, 2);
        assert!(load(&dir, "../x").is_err());
        delete(&dir, "s100").unwrap();
        assert!(list(&dir).is_empty());
        delete(&dir, "s100").unwrap();
        assert!(delete(&dir, "../x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
