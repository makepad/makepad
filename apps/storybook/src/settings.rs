//! Settings that survive a run: the theme, the baseline date, the last story.
//!
//! One tab-separated `key<TAB>value` line per setting in a file under the
//! user's config directory, or wherever `MAKEPAD_STORYBOOK_SETTINGS` points
//! (the tests point it at a scratch file). Missing or unreadable means
//! defaults; every write rewrites the whole file.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

static STORE: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

pub const THEME: &str = "theme";
pub const BASELINE: &str = "baseline";
pub const LAST_STORY: &str = "last_story";
pub const NEW_ONLY: &str = "new_only";

pub fn path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MAKEPAD_STORYBOOK_SETTINGS") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(home).join(".config").join("makepad-storybook").join("settings.txt"))
}

fn with<R>(f: impl FnOnce(&mut BTreeMap<String, String>) -> R) -> R {
    let mut guard = STORE.lock().unwrap();
    if guard.is_none() {
        let mut map = BTreeMap::new();
        if let Some(p) = path() {
            if let Ok(text) = std::fs::read_to_string(&p) {
                for line in text.lines() {
                    if let Some((k, v)) = line.split_once('\t') {
                        map.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }
        *guard = Some(map);
    }
    f(guard.as_mut().unwrap())
}

pub fn get(key: &str) -> Option<String> {
    with(|map| map.get(key).cloned())
}

pub fn set(key: &str, value: &str) {
    with(|map| {
        map.insert(key.to_string(), value.to_string());
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let text: String = map.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect();
            let _ = std::fs::write(&p, text);
        }
    })
}

pub fn baseline() -> String {
    get(BASELINE)
        .filter(|b| crate::registry::is_iso_date(b))
        .unwrap_or_else(|| crate::registry::DEFAULT_BASELINE.to_string())
}
