//! The person's own presets: plain functions over files, for any host (a
//! preset browser listing them, an AI tool writing them).
//!
//! - The downloaded pack is read-only: `<data>/presets/cream-of-the-crop`.
//! - User presets live in `<data>/presets/mine/<name>.milk` ([`user_dir`]),
//!   next to the pack, so a browser scanning `<data>/presets` sees both.
//! - A write keeps the previous text in `<data>/presets/mine/.history/<name>/`
//!   ([`revert`] goes back one version); a delete moves the file to
//!   `<data>/presets/mine/.trash`.
//! - Pack presets are copy-on-write: an edit of one goes to a user copy
//!   ([`writable`]) whose first line says `// from: <pack path>`.
//!
//! Names are `[A-Za-z0-9 _.,&()'-]{1,80}` (no separators), so every path
//! stays inside the user folder. `<data>` is [`crate::browser::data_dir`].

use crate::library::scan_cached;
use crate::library::{PresetLibrary, CREAM_OF_THE_CROP_DIR};
use std::path::{Path, PathBuf};

/// The largest preset text accepted.
pub const MAX_PRESET_BYTES: usize = 256 * 1024;
const HISTORY_KEPT: usize = 60;

pub fn presets_dir() -> Option<PathBuf> {
    Some(crate::browser::data_dir()?.join("presets"))
}

/// The downloaded pack's root (it may not exist yet).
pub fn pack_dir() -> Option<PathBuf> {
    Some(presets_dir()?.join(CREAM_OF_THE_CROP_DIR))
}

pub fn user_dir() -> Option<PathBuf> {
    Some(presets_dir()?.join("mine"))
}

pub fn valid_name(name: &str) -> Result<(), String> {
    let ok = !name.trim().is_empty()
        && name.len() <= 80
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || " _.,&()'-".contains(c));
    if ok {
        Ok(())
    } else {
        Err(format!("`{name}` is not a preset name: 1 to 80 letters, digits, spaces and _ . , & ( ) ' -"))
    }
}

fn user_path(name: &str) -> Option<PathBuf> {
    valid_name(name).ok()?;
    Some(user_dir()?.join(format!("{name}.milk")))
}

/// One user preset.
#[derive(Clone, Debug, PartialEq)]
pub struct UserPreset {
    pub name: String,
    pub path: PathBuf,
    /// The pack preset it was copied from (`// from:`), if any.
    pub from: Option<String>,
}

/// The user presets, by name.
pub fn list_user() -> Vec<UserPreset> {
    let Some(dir) = user_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<UserPreset> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("milk")) {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            valid_name(&name).ok()?;
            let from = std::fs::read_to_string(&path).ok().and_then(|t| from_line(&t));
            Some(UserPreset { name, path, from })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// The pack, indexed (with its cache). Slow the first time: run it off
/// the UI thread.
pub fn scan_pack() -> Option<PresetLibrary> {
    let data = crate::browser::data_dir()?;
    let pack = pack_dir()?;
    scan_cached(&pack, Some(&data.join("index.txt"))).ok().map(|(library, _)| library)
}

/// A preset by name: a user preset's name, else a pack preset's relative
/// path (`Fractal/Nested/x.milk`) or file stem, looked up in `pack`.
pub fn find(name: &str, pack: Option<&PresetLibrary>) -> Option<PathBuf> {
    if let Some(path) = user_path(name).filter(|p| p.is_file()) {
        return Some(path);
    }
    let pack = pack?;
    let rel = name.trim_end_matches(".milk");
    pack.entries
        .iter()
        .find(|e| e.rel.trim_end_matches(".milk") == rel)
        .or_else(|| pack.entries.iter().find(|e| e.name == rel))
        .map(|e| e.path.clone())
}

pub fn is_user(name: &str) -> bool {
    user_path(name).is_some_and(|p| p.is_file())
}

pub fn read_user(name: &str) -> Option<String> {
    std::fs::read_to_string(user_path(name)?).ok()
}

/// `// from: …` on the first lines (a `.milk` line that is not `key=value`
/// is ignored by every reader).
pub fn from_line(text: &str) -> Option<String> {
    text.lines().take(4).find_map(|l| l.trim().strip_prefix("// from:").map(|r| r.trim().to_string()))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn history_dir(name: &str) -> Option<PathBuf> {
    Some(user_dir()?.join(".history").join(name))
}

/// Write a user preset (the previous text goes to its history).
pub fn write_user(name: &str, text: &str) -> Result<PathBuf, String> {
    valid_name(name)?;
    if text.len() > MAX_PRESET_BYTES {
        return Err(format!("a preset is at most {} KiB", MAX_PRESET_BYTES / 1024));
    }
    let path = user_path(name).ok_or("no data folder for presets")?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if let Ok(previous) = std::fs::read_to_string(&path) {
        if previous == text {
            return Ok(path);
        }
        if let Some(dir) = history_dir(name) {
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = std::fs::write(dir.join(format!("{:016}.milk", now_ms())), previous);
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
                    files.sort();
                    while files.len() > HISTORY_KEPT {
                        let _ = std::fs::remove_file(files.remove(0));
                    }
                }
            }
        }
    }
    let temp = path.with_extension("milk.tmp");
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// `base`, or `base 2`, `base 3`, …: a name no user preset has.
pub fn unique_name(base: &str) -> String {
    let base: String = base.chars().take(74).collect();
    if !is_user(&base) {
        return base;
    }
    (2..).map(|n| format!("{base} {n}")).find(|n| !is_user(n)).unwrap()
}

/// A user copy of the preset at `source` (a pack file or another user
/// preset), named `name` (made unique). Returns the name it got.
pub fn copy_from(source: &Path, name: &str) -> Result<String, String> {
    valid_name(name)?;
    let text = std::fs::read_to_string(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let origin = from_line(&text).unwrap_or_else(|| {
        let pack = pack_dir();
        pack.as_deref()
            .and_then(|p| source.strip_prefix(p).ok())
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
    });
    let body: String = text.lines().filter(|l| !l.trim().starts_with("// from:")).collect::<Vec<_>>().join("\n");
    let name = unique_name(name);
    write_user(&name, &format!("// from: {origin}\n{body}\n"))?;
    Ok(name)
}

/// Copy-on-write: a user preset is itself; a pack preset becomes a user
/// copy "<stem> (mine)" (or the copy made before).
pub fn writable(name: &str, pack: Option<&PresetLibrary>) -> Result<String, String> {
    if is_user(name) {
        return Ok(name.to_string());
    }
    let source = find(name, pack).ok_or_else(|| format!("no preset named `{name}`"))?;
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "preset".into());
    let clean: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || " _.,&()'-".contains(c) { c } else { ' ' }).collect();
    let mine = format!("{} (mine)", clean.trim().chars().take(70).collect::<String>());
    if is_user(&mine) {
        return Ok(mine);
    }
    copy_from(&source, &mine)
}

/// Go back one version. Returns the text now in the file.
pub fn revert(name: &str) -> Result<String, String> {
    let dir = history_dir(name).ok_or("no data folder for presets")?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map_err(|_| format!("`{name}` has no earlier version"))?.flatten().map(|e| e.path()).collect();
    files.sort();
    let last = files.pop().ok_or_else(|| format!("`{name}` has no earlier version"))?;
    let text = std::fs::read_to_string(&last).map_err(|e| e.to_string())?;
    std::fs::write(user_path(name).ok_or("bad name")?, &text).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(last);
    Ok(text)
}

/// Move a user preset to the trash.
pub fn delete_user(name: &str) -> Result<(), String> {
    let path = user_path(name).filter(|p| p.is_file()).ok_or_else(|| format!("no user preset named `{name}`"))?;
    let trash = user_dir().ok_or("no data folder for presets")?.join(".trash");
    std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
    std::fs::rename(&path, trash.join(format!("{:016}__{name}.milk", now_ms()))).map_err(|e| e.to_string())
}
