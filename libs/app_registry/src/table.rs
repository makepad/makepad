//! `apps.json` read into the `AppEntry` table at first use. Every rule the
//! table promises is checked here, so a malformed row stops with the row
//! and the reason (the crate's tests read the table, so a bad row fails
//! them) instead of surfacing in an app.

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
    TABLE.get_or_init(|| parse(APPS_JSON))
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn parse(json: &str) -> Vec<AppEntry> {
    // Three levels deep (array, object, string array): within strict_json's MAX_DEPTH (8).
    let table = makepad_strict_json::parse(json.as_bytes()).unwrap_or_else(|err| panic!("apps.json: {err}"));
    let Value::Arr(rows) = table else { panic!("apps.json: the top level must be an array") };
    let mut entries: Vec<AppEntry> = Vec::new();
    // The first hidden row: every menu row comes before it (menu order).
    let mut seen_hidden: Option<String> = None;
    for (index, row) in rows.iter().enumerate() {
        let Value::Obj(pairs) = row else { panic!("apps.json row {index}: not an object") };
        for (key, _) in pairs {
            assert!(FIELDS.contains(&key.as_str()), "apps.json row {index}: unknown field {key:?}");
        }
        let text = |key: &str| -> String {
            match row.get(key) {
                Some(Value::Str(s)) if !s.is_empty() => s.clone(),
                _ => panic!("apps.json row {index}: {key:?} must be a non-empty string"),
            }
        };
        let flag = |key: &str| -> bool {
            match row.get(key) {
                Some(Value::Bool(b)) => *b,
                _ => panic!("apps.json row {index}: {key:?} must be true or false"),
            }
        };
        let list = |key: &str| -> &'static [&'static str] {
            match row.get(key) {
                Some(Value::Arr(items)) => {
                    let items: Vec<&'static str> = items
                        .iter()
                        .map(|item| match item {
                            Value::Str(s) => leak(s.clone()),
                            _ => panic!("apps.json row {index}: {key:?} must hold strings"),
                        })
                        .collect();
                    Box::leak(items.into_boxed_slice())
                }
                _ => panic!("apps.json row {index}: {key:?} must be an array"),
            }
        };
        let id = text("id");
        assert!(!entries.iter().any(|e| e.id == id), "apps.json: duplicate id {id:?}");
        let manifest = match row.get("manifest") {
            Some(Value::Null) => None,
            Some(Value::Str(s)) if !s.is_empty() => Some(leak(s.clone())),
            _ => panic!("apps.json {id}: \"manifest\" must be null or a path"),
        };
        let policy = match text("policy").as_str() {
            "or_focus" => LaunchPolicy::OrFocus,
            "always_new" => LaunchPolicy::AlwaysNew,
            other => panic!("apps.json {id}: unknown policy {other:?}"),
        };
        let (menu_visible, wm_launchable) = (flag("menu_visible"), flag("wm_launchable"));
        assert!(!menu_visible || wm_launchable, "apps.json {id}: a menu row must be launchable by the WM");
        match (&seen_hidden, menu_visible) {
            (Some(hidden), true) => panic!("apps.json {id}: a menu row after the hidden {hidden:?}; menu rows come first"),
            (None, false) => seen_hidden = Some(id.clone()),
            _ => {}
        }
        let mobile = flag("mobile");
        assert!(!mobile || wm_launchable, "apps.json {id}: a mobile app must be launchable by the WM");
        let bin = text("bin");
        assert!(!entries.iter().any(|e| e.bin == bin), "apps.json {id}: binary {bin:?} is already another app's");
        entries.push(AppEntry {
            id: leak(id),
            label: leak(text("label")),
            package: leak(text("package")),
            dir: leak(text("dir")),
            bin: leak(bin),
            manifest,
            args: list("args"),
            policy,
            menu_visible,
            wm_launchable,
            features: list("features"),
            mobile,
        });
    }
    entries
}
