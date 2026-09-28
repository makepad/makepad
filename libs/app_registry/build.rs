//! Turns `apps.json` into `OUT_DIR/apps.rs`: a static `[AppEntry]` table.
//! Every rule the table promises is checked here, so a malformed row stops
//! the build with the row and the reason instead of surfacing in an app.

use makepad_strict_json::Value;
use std::fmt::Write;

const FIELDS: [&str; 11] = [
    "id", "label", "package", "dir", "bin", "manifest", "args", "policy", "menu_visible", "wm_launchable", "features",
];

fn main() {
    println!("cargo:rerun-if-changed=apps.json");
    println!("cargo:rerun-if-changed=build.rs");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("apps.json");
    let text = std::fs::read(&path).expect("apps.json");
    // Three levels deep (array, object, string array): within strict_json's MAX_DEPTH (8).
    let table = makepad_strict_json::parse(&text).unwrap_or_else(|err| panic!("apps.json: {err}"));
    let Value::Arr(rows) = table else { panic!("apps.json: the top level must be an array") };
    let mut out = String::from("pub(crate) static APPS: &[AppEntry] = &[\n");
    let mut ids: Vec<String> = Vec::new();
    let mut bins: Vec<String> = Vec::new();
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
        let list = |key: &str| -> Vec<String> {
            match row.get(key) {
                Some(Value::Arr(items)) => items
                    .iter()
                    .map(|item| match item {
                        Value::Str(s) => s.clone(),
                        _ => panic!("apps.json row {index}: {key:?} must hold strings"),
                    })
                    .collect(),
                _ => panic!("apps.json row {index}: {key:?} must be an array"),
            }
        };
        let id = text("id");
        assert!(!ids.contains(&id), "apps.json: duplicate id {id:?}");
        ids.push(id.clone());
        let manifest = match row.get("manifest") {
            Some(Value::Null) => None,
            Some(Value::Str(s)) if !s.is_empty() => Some(s.clone()),
            _ => panic!("apps.json {id}: \"manifest\" must be null or a path"),
        };
        let policy = match text("policy").as_str() {
            "or_focus" => "OrFocus",
            "always_new" => "AlwaysNew",
            other => panic!("apps.json {id}: unknown policy {other:?}"),
        };
        let (menu_visible, wm_launchable) = (flag("menu_visible"), flag("wm_launchable"));
        assert!(!menu_visible || wm_launchable, "apps.json {id}: a menu row must be launchable by the WM");
        match (&seen_hidden, menu_visible) {
            (Some(hidden), true) => panic!("apps.json {id}: a menu row after the hidden {hidden:?}; menu rows come first"),
            (None, false) => seen_hidden = Some(id.clone()),
            _ => {}
        }
        let bin = text("bin");
        assert!(!bins.contains(&bin), "apps.json {id}: binary {bin:?} is already another app's");
        bins.push(bin.clone());
        writeln!(
            out,
            "    AppEntry {{ id: {id:?}, label: {:?}, package: {:?}, dir: {:?}, bin: {:?}, manifest: {manifest:?}, \
             args: &{:?}, policy: LaunchPolicy::{policy}, menu_visible: {menu_visible}, wm_launchable: {wm_launchable}, \
             features: &{:?} }},",
            text("label"),
            text("package"),
            text("dir"),
            bin,
            list("args"),
            list("features"),
        )
        .unwrap();
    }
    out.push_str("];\n");
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("apps.rs");
    std::fs::write(path, out).unwrap();
}
