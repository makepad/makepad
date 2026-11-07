//! `apps.json` read into the `AppEntry` table at first use. Every rule the
//! table promises is checked here, so a malformed row stops with the row
//! and the reason (the crate's tests read the table, so a bad row fails
//! them) instead of surfacing in an app.
//!
//! This machine's own apps (`machine::local_apps_path`: checkouts of other repositories the
//! machine runs, which the shared table does not name) are rows of the same shape, after the
//! shared ones: its menu rows after the shared menu rows, its hidden rows last. A local row
//! builds with its `features` wherever it is launched, and two local rows may name one binary
//! (one app run two ways, under the JIT and as its release binary: a process found by its
//! binary is the first row's). A malformed file is said and left out whole; it never stops a
//! host.

use crate::{AppEntry, LaunchPolicy};
use makepad_strict_json::Value;
use std::sync::OnceLock;

const FIELDS: [&str; 12] = [
    "id", "label", "package", "dir", "bin", "manifest", "args", "policy", "menu_visible", "wm_launchable", "features", "mobile",
];

const APPS_JSON: &str = include_str!("../apps.json");

/// The table, parsed once. Strings are leaked: the table lives for the
/// process, and entries hand out `&'static str`.
pub(crate) fn apps() -> &'static [AppEntry] {
    static TABLE: OnceLock<Vec<AppEntry>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let shared = parse(APPS_JSON, "apps.json", None).unwrap_or_else(|err| panic!("{err}"));
        let path = crate::machine::local_apps_path();
        let local = match std::fs::read_to_string(&path) {
            Ok(json) => parse(&json, &path.display().to_string(), Some(&shared)).unwrap_or_else(|err| {
                eprintln!("makepad: {err}: this machine's own apps are left out");
                Vec::new()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => {
                eprintln!("makepad: {}: {err}: this machine's own apps are left out", path.display());
                Vec::new()
            }
        };
        merged(shared, local)
    })
}

/// The shared rows and this machine's: the menu rows first (the shared ones, then the local
/// ones), then the hidden ones.
fn merged(shared: Vec<AppEntry>, local: Vec<AppEntry>) -> Vec<AppEntry> {
    let (shared_menu, shared_hidden): (Vec<_>, Vec<_>) = shared.into_iter().partition(|e| e.menu_visible);
    let (local_menu, local_hidden): (Vec<_>, Vec<_>) = local.into_iter().partition(|e| e.menu_visible);
    shared_menu.into_iter().chain(local_menu).chain(shared_hidden).chain(local_hidden).collect()
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The rows of `json` (`source` names it in errors): the shared table (`shared` None), or this
/// machine's own rows after the `shared` ones.
fn parse(json: &str, source: &str, shared: Option<&[AppEntry]>) -> Result<Vec<AppEntry>, String> {
    let local = shared.is_some();
    let shared = shared.unwrap_or_default();
    // Three levels deep (array, object, string array): within strict_json's MAX_DEPTH (8).
    let table = makepad_strict_json::parse(json.as_bytes()).map_err(|err| format!("{source}: {err}"))?;
    let Value::Arr(rows) = table else { return Err(format!("{source}: the top level must be an array")) };
    let mut entries: Vec<AppEntry> = Vec::new();
    // The first hidden row: every menu row comes before it (menu order).
    let mut seen_hidden: Option<String> = None;
    for (index, row) in rows.iter().enumerate() {
        let Value::Obj(pairs) = row else { return Err(format!("{source} row {index}: not an object")) };
        if let Some((key, _)) = pairs.iter().find(|(key, _)| !FIELDS.contains(&key.as_str())) {
            return Err(format!("{source} row {index}: unknown field {key:?}"));
        }
        let text = |key: &str| -> Result<String, String> {
            match row.get(key) {
                Some(Value::Str(s)) if !s.is_empty() => Ok(s.clone()),
                _ => Err(format!("{source} row {index}: {key:?} must be a non-empty string")),
            }
        };
        let flag = |key: &str| -> Result<bool, String> {
            match row.get(key) {
                Some(Value::Bool(b)) => Ok(*b),
                _ => Err(format!("{source} row {index}: {key:?} must be true or false")),
            }
        };
        let list = |key: &str| -> Result<&'static [&'static str], String> {
            let Some(Value::Arr(items)) = row.get(key) else { return Err(format!("{source} row {index}: {key:?} must be an array")) };
            let items = items
                .iter()
                .map(|item| match item {
                    Value::Str(s) => Ok(leak(s.clone())),
                    _ => Err(format!("{source} row {index}: {key:?} must hold strings")),
                })
                .collect::<Result<Vec<&'static str>, String>>()?;
            Ok(Box::leak(items.into_boxed_slice()))
        };
        let id = text("id")?;
        if shared.iter().chain(&entries).any(|e| e.id == id) {
            return Err(format!("{source}: duplicate id {id:?}"));
        }
        let manifest = match row.get("manifest") {
            Some(Value::Null) => None,
            Some(Value::Str(s)) if !s.is_empty() => Some(leak(s.clone())),
            _ => return Err(format!("{source} {id}: \"manifest\" must be null or a path")),
        };
        let policy = match text("policy")?.as_str() {
            "or_focus" => LaunchPolicy::OrFocus,
            "always_new" => LaunchPolicy::AlwaysNew,
            other => return Err(format!("{source} {id}: unknown policy {other:?}")),
        };
        let (menu_visible, wm_launchable) = (flag("menu_visible")?, flag("wm_launchable")?);
        if menu_visible && !wm_launchable {
            return Err(format!("{source} {id}: a menu row must be launchable by the WM"));
        }
        match (&seen_hidden, menu_visible) {
            (Some(hidden), true) => return Err(format!("{source} {id}: a menu row after the hidden {hidden:?}; menu rows come first")),
            (None, false) => seen_hidden = Some(id.clone()),
            _ => {}
        }
        let mobile = flag("mobile")?;
        if mobile && !wm_launchable {
            return Err(format!("{source} {id}: a mobile app must be launchable by the WM"));
        }
        let bin = text("bin")?;
        // A local row may name the binary of another local row (the same app, run two ways).
        let others = if local { shared } else { entries.as_slice() };
        if others.iter().any(|e| e.bin == bin) {
            return Err(format!("{source} {id}: binary {bin:?} is already another app's"));
        }
        entries.push(AppEntry {
            id: leak(id),
            label: leak(text("label")?),
            package: leak(text("package")?),
            dir: leak(text("dir")?),
            bin: leak(bin),
            manifest,
            args: list("args")?,
            policy,
            menu_visible,
            wm_launchable,
            features: list("features")?,
            mobile,
            local,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, bin: &str, menu: bool) -> String {
        format!(
            "{{\"id\": \"{id}\", \"label\": \"{id}\", \"package\": \"pkg-{bin}\", \"dir\": \"apps/{bin}\", \"bin\": \"{bin}\", \"manifest\": null, \"args\": [], \"policy\": \"or_focus\", \"menu_visible\": {menu}, \"wm_launchable\": true, \"features\": [\"f\"], \"mobile\": false}}"
        )
    }

    /// This machine's rows come after the shared ones (menu rows with the menu rows, hidden rows
    /// last), may run one binary two ways, and never take a shared id or binary; a bad file is
    /// an error, not a stop.
    #[test]
    fn a_machines_own_apps_follow_the_shared_table() {
        let shared = parse(&format!("[{}, {}]", row("one", "one-bin", true), row("rig", "rig-bin", false)), "shared", None).unwrap();
        assert!(shared.iter().all(|e| !e.local));
        let local = parse(&format!("[{}, {}]", row("two", "two-bin", true), row("two-jit", "two-bin", true)), "local", Some(&shared)).unwrap();
        assert!(local.iter().all(|e| e.local && e.features == ["f"]));
        let ids: Vec<&str> = merged(shared.clone(), local).iter().map(|e| e.id).collect();
        assert_eq!(ids, ["one", "two", "two-jit", "rig"]);
        assert!(parse(&format!("[{}]", row("one", "x-bin", true)), "local", Some(&shared)).unwrap_err().contains("duplicate id"));
        assert!(parse(&format!("[{}]", row("three", "one-bin", true)), "local", Some(&shared)).unwrap_err().contains("already another app's"));
        assert!(parse(&format!("[{}, {}]", row("a", "same", true), row("b", "same", true)), "shared", None).unwrap_err().contains("already another app's"));
        assert!(parse("[{\"id\": \"x\"}]", "local", Some(&shared)).is_err());
        assert!(parse("not json", "local", Some(&shared)).is_err());
    }
}
