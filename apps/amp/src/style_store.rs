//! The style list as something a person (and the AI) can edit: plain
//! functions over files, no UI.
//!
//! - **Built-in styles** are the ones the app ships (`resources/styles`,
//!   and the copies built into the binary). They are never written: an
//!   edit aimed at one forks it first (copy-on-write) into a user style
//!   titled "<Title> (mine)" whose header says `// from: <name>`.
//! - **User styles** live in `<data>/styles/<name>.splash`. They can be
//!   written, renamed, deleted (to `<data>/styles/.trash`, with undo) and
//!   reverted: every write keeps the previous text in
//!   `<data>/styles/.history/<name>/`.
//! - **Order and hiding** are the person's: `<data>/styles/order.txt` (one
//!   name per line) and `<data>/styles/hidden.txt` (hidden built-ins).
//!
//! Names are `[a-z0-9_-]{1,40}` and never a built-in's name, so every path
//! stays inside `<data>/styles`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// The largest style file accepted from a write.
pub const MAX_STYLE_BYTES: usize = 256 * 1024;
/// Earlier versions kept per user style.
const HISTORY_KEPT: usize = 60;

/// One style, as the style manager lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleInfo {
    pub name: String,
    pub title: String,
    /// Ships with the app (never written).
    pub builtin: bool,
    /// Hidden from the picker (built-ins only).
    pub hidden: bool,
    /// The built-in it was forked from (`// from:`).
    pub from: Option<String>,
    /// The AI is writing it right now.
    pub generating: bool,
}

impl StyleInfo {
    pub fn user(&self) -> bool {
        !self.builtin
    }
}

thread_local! {
    /// Styles the AI is building, for the "generating…" mark. UI thread only.
    static GENERATING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// A style shown from other text than its file: the last version that
    /// applied cleanly, while the file holds one that did not.
    static OVERRIDES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

pub fn set_generating(name: &str, on: bool) {
    GENERATING.with(|g| {
        let mut g = g.borrow_mut();
        if on {
            g.insert(name.to_string());
        } else {
            g.remove(name);
        }
    });
}

pub fn generating(name: &str) -> bool {
    GENERATING.with(|g| g.borrow().contains(name))
}

/// Show `name` from `text` instead of its file (`None`: the file again).
pub fn set_override(name: &str, text: Option<String>) {
    OVERRIDES.with(|o| {
        let mut o = o.borrow_mut();
        match text {
            Some(text) => o.insert(name.to_string(), text),
            None => o.remove(name),
        };
    });
}

pub fn overridden(name: &str) -> bool {
    OVERRIDES.with(|o| o.borrow().contains_key(name))
}

// ---- where things are ----

pub fn user_dir() -> Option<PathBuf> {
    Some(crate::worker::data_dir()?.join("styles"))
}

fn resource_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join("styles")
}

pub fn valid_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty() && name.len() <= 40 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!("`{name}` is not a style name: use 1 to 40 of a-z, 0-9, _ and -"))
    }
}

/// A title turned into a style name: `Braun Radio 1970` → `braun_radio_1970`.
pub fn name_from_title(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_end_matches('_').chars().take(40).collect::<String>();
    if out.is_empty() {
        "style".into()
    } else {
        out
    }
}

/// `base`, or `base_2`, `base_3`, … : a name no style has.
pub fn unique_name(base: &str) -> String {
    let base: String = base.chars().take(36).collect();
    if !exists(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}_{n}")).find(|n| !exists(n)).unwrap()
}

fn user_path(name: &str) -> Option<PathBuf> {
    valid_name(name).ok()?;
    Some(user_dir()?.join(format!("{name}.splash")))
}

pub fn builtin_names() -> Vec<String> {
    let mut names: Vec<String> = crate::hw::EMBEDDED.iter().map(|(n, _)| n.to_string()).collect();
    if let Ok(entries) = std::fs::read_dir(resource_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) == Some("splash") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if !names.iter().any(|n| n == stem) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
    }
    names
}

pub fn is_builtin(name: &str) -> bool {
    crate::hw::EMBEDDED.iter().any(|(n, _)| *n == name) || resource_dir().join(format!("{name}.splash")).is_file()
}

pub fn is_user(name: &str) -> bool {
    !is_builtin(name) && user_path(name).is_some_and(|p| p.is_file())
}

pub fn exists(name: &str) -> bool {
    is_builtin(name) || is_user(name)
}

pub fn user_names() -> Vec<String> {
    let Some(dir) = user_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("splash") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_string();
            (valid_name(&stem).is_ok() && !is_builtin(&stem)).then_some(stem)
        })
        .collect();
    names.sort();
    names
}

// ---- reading ----

pub fn builtin_source(name: &str) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(resource_dir().join(format!("{name}.splash"))) {
        return Some(text);
    }
    crate::hw::EMBEDDED.iter().find(|(n, _)| *n == name).map(|(_, s)| s.to_string())
}

/// What the loader shows for a user style: its override, else its file.
/// `None` for anything that is not a user style.
pub fn user_source(name: &str) -> Option<String> {
    if is_builtin(name) {
        return None;
    }
    if let Some(text) = OVERRIDES.with(|o| o.borrow().get(name).cloned()) {
        return Some(text);
    }
    std::fs::read_to_string(user_path(name)?).ok()
}

/// The user style file's modification time (the hot-reload stamp).
pub fn user_stamp(name: &str) -> Option<std::time::SystemTime> {
    if is_builtin(name) {
        return None;
    }
    std::fs::metadata(user_path(name)?).and_then(|m| m.modified()).ok()
}

/// A style's text as written: the user file, else the built-in.
pub fn read(name: &str) -> Option<String> {
    if !is_builtin(name) {
        if let Some(path) = user_path(name) {
            if let Ok(text) = std::fs::read_to_string(path) {
                return Some(text);
            }
        }
    }
    builtin_source(name)
}

/// A `// key: value` line near the top.
pub fn header(source: &str, key: &str) -> Option<String> {
    source.lines().take(24).find_map(|line| {
        let rest = line.trim().strip_prefix("//")?.trim();
        let value = rest.strip_prefix(key)?.trim_start().strip_prefix(':')?;
        Some(value.trim().to_string())
    })
}

/// `source` with its `// key: value` header line set (added at the top when
/// missing).
pub fn with_header(source: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    let at = lines.iter().take(24).position(|line| {
        line.trim().strip_prefix("//").map(str::trim).and_then(|r| r.strip_prefix(key)).is_some_and(|r| r.trim_start().starts_with(':'))
    });
    let line = format!("// {key}: {value}");
    match at {
        Some(at) => lines[at] = line,
        None => {
            // After the other header lines.
            let is_header = |l: &String| {
                l.trim().strip_prefix("// ").and_then(|r| r.split_once(':')).is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            };
            let insert = lines.iter().take_while(|l| is_header(l)).count();
            lines.insert(insert, line);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

pub fn title_of(name: &str) -> String {
    let source = read(name).unwrap_or_default();
    header(&source, "title")
        .or_else(|| crate::hw::STYLES.iter().find(|(n, _)| *n == name).map(|(_, t)| t.to_string()))
        .unwrap_or_else(|| {
            let mut c = name.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
}

// ---- order and hiding ----

fn list_file(file: &str) -> Vec<String> {
    let Some(path) = user_dir().map(|d| d.join(file)) else { return Vec::new() };
    std::fs::read_to_string(path).map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

fn write_list_file(file: &str, names: &[String]) -> Result<(), String> {
    let dir = user_dir().ok_or("no data folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text: String = names.iter().map(|n| format!("{n}\n")).collect();
    std::fs::write(dir.join(file), text).map_err(|e| e.to_string())
}

pub fn hidden() -> HashSet<String> {
    list_file("hidden.txt").into_iter().collect()
}

pub fn set_hidden(name: &str, hide: bool) -> Result<(), String> {
    if !exists(name) {
        return Err(format!("no style named `{name}`"));
    }
    let mut list: Vec<String> = list_file("hidden.txt").into_iter().filter(|n| n != name).collect();
    if hide {
        list.push(name.to_string());
    }
    write_list_file("hidden.txt", &list)
}

/// Keep the person's order: `names` first, in this order, then the rest.
pub fn reorder(names: &[String]) -> Result<(), String> {
    write_list_file("order.txt", names)
}

/// Every style, in the person's order, hidden ones included.
pub fn list_all() -> Vec<StyleInfo> {
    let hidden = hidden();
    let mut names = builtin_names();
    // The app's own order for built-ins: the first eight, then by header.
    let fixed = |n: &str| crate::hw::STYLES.iter().position(|(s, _)| *s == n);
    let mut keyed: Vec<(f64, String, String)> = names
        .drain(..)
        .map(|name| {
            let source = builtin_source(&name).unwrap_or_default();
            let order = match fixed(&name) {
                Some(at) => at as f64,
                None => 100.0 + header(&source, "order").and_then(|o| o.parse().ok()).unwrap_or(1000.0),
            };
            let title = title_of(&name);
            (order, title, name)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
    let mut all: Vec<StyleInfo> = keyed
        .into_iter()
        .map(|(_, title, name)| StyleInfo { hidden: hidden.contains(&name), generating: generating(&name), name, title, builtin: true, from: None })
        .collect();
    for name in user_names() {
        let source = read(&name).unwrap_or_default();
        all.push(StyleInfo {
            title: title_of(&name),
            from: header(&source, "from"),
            hidden: hidden.contains(&name),
            generating: generating(&name),
            builtin: false,
            name,
        });
    }
    // The person's order wins where it names a style.
    let order = list_file("order.txt");
    if !order.is_empty() {
        let rank = |n: &str| order.iter().position(|o| o == n).unwrap_or(usize::MAX);
        let base: HashMap<String, usize> = all.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
        all.sort_by_key(|s| (rank(&s.name), base[&s.name]));
    }
    all
}

/// The picker's list: `hw::style_list` hands its entries here to drop the
/// hidden ones and follow the person's order.
pub fn arrange_picker(list: &mut Vec<crate::hw::StyleEntry>) {
    let all = list_all();
    list.retain(|e| all.iter().find(|s| s.name == e.name).is_none_or(|s| !s.hidden));
    let rank = |n: &str| all.iter().position(|s| s.name == n).unwrap_or(usize::MAX);
    list.sort_by_key(|e| rank(&e.name));
}

// ---- writing ----

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn history_dir(name: &str) -> Option<PathBuf> {
    Some(user_dir()?.join(".history").join(name))
}

fn keep_history(name: &str, previous: &str) {
    let Some(dir) = history_dir(name) else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(dir.join(format!("{:016}.splash", now_ms())), previous);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        while files.len() > HISTORY_KEPT {
            let _ = std::fs::remove_file(files.remove(0));
        }
    }
}

/// Write a user style's whole text (the previous text goes to its
/// history). Refuses built-ins: write to [`writable`]'s name.
pub fn write(name: &str, text: &str) -> Result<(), String> {
    valid_name(name)?;
    if is_builtin(name) {
        return Err(format!("`{name}` is built in; edits go to a copy (see writable)"));
    }
    if text.len() > MAX_STYLE_BYTES {
        return Err(format!("a style is at most {} KiB", MAX_STYLE_BYTES / 1024));
    }
    let path = user_path(name).ok_or("no data folder")?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if let Ok(previous) = std::fs::read_to_string(&path) {
        if previous == text {
            return Ok(());
        }
        keep_history(name, &previous);
    }
    let temp = path.with_extension("splash.tmp");
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())
}

/// The text a new style starts from: an empty style that renders (the
/// base templates), or a copy of `from`.
pub fn starter(title: &str, from: Option<&str>) -> Result<String, String> {
    let text = match from {
        Some(from) => read(from).ok_or_else(|| format!("no style named `{from}`"))?,
        None => "use mod.prelude.widgets_internal.*\n\ntrue\n".to_string(),
    };
    let mut text = with_header(&text, "title", title);
    if let Some(from) = from {
        // A copy of a copy remembers the built-in it started from.
        let origin = header(&text, "from").filter(|o| is_builtin(o)).unwrap_or_else(|| from.to_string());
        text = with_header(&text, "from", &origin);
    }
    Ok(text)
}

/// A new user style `name` (made unique), titled `title`, from `from` or
/// empty. Returns the name it got.
pub fn create(name: &str, title: Option<&str>, from: Option<&str>) -> Result<String, String> {
    valid_name(name)?;
    let name = unique_name(name);
    let title = title.map(str::to_string).unwrap_or_else(|| {
        let mut c = name.replace(['_', '-'], " ").chars().collect::<Vec<_>>();
        if let Some(f) = c.first_mut() {
            *f = f.to_ascii_uppercase();
        }
        c.into_iter().collect()
    });
    let text = starter(&title, from)?;
    write(&name, &text)?;
    Ok(name)
}

/// A copy of any style, as a user style: "<Title> (copy)".
pub fn duplicate(from: &str, name: Option<&str>) -> Result<String, String> {
    if !exists(from) {
        return Err(format!("no style named `{from}`"));
    }
    let base = name.map(str::to_string).unwrap_or_else(|| format!("{from}_copy"));
    let title = format!("{} (copy)", title_of(from).trim_end_matches(" (mine)"));
    create(&base, Some(&title), Some(from))
}

/// Copy-on-write: the user style an edit aimed at `name` goes to. A user
/// style is itself; a built-in is forked into "<Title> (mine)" (or the
/// fork made before, when there is one).
pub fn writable(name: &str) -> Result<String, String> {
    if !is_builtin(name) {
        return if is_user(name) { Ok(name.to_string()) } else { Err(format!("no style named `{name}`")) };
    }
    if let Some(fork) = user_names().into_iter().find(|u| read(u).and_then(|s| header(&s, "from")).as_deref() == Some(name) && u.ends_with("_mine")) {
        return Ok(fork);
    }
    let title = format!("{} (mine)", title_of(name));
    create(&format!("{name}_mine"), Some(&title), Some(name))
}

pub fn rename(name: &str, new_title: &str) -> Result<String, String> {
    if is_builtin(name) {
        return Err(format!("`{name}` is built in; duplicate it to get one you can rename"));
    }
    let text = read(name).ok_or_else(|| format!("no style named `{name}`"))?;
    let new_name = name_from_title(new_title);
    let text = with_header(&text, "title", new_title.trim());
    if new_name == name {
        write(name, &text)?;
        return Ok(name.to_string());
    }
    valid_name(&new_name)?;
    let new_name = unique_name(&new_name);
    write(&new_name, &text)?;
    let (old, new) = (user_path(name).unwrap(), user_path(&new_name).unwrap());
    let _ = std::fs::remove_file(old);
    // The history follows the style.
    if let (Some(a), Some(b)) = (history_dir(name), history_dir(&new_name)) {
        let _ = std::fs::rename(a, b);
    }
    let order: Vec<String> = list_file("order.txt").into_iter().map(|n| if n == name { new_name.clone() } else { n }).collect();
    let _ = write_list_file("order.txt", &order);
    let _ = new;
    Ok(new_name)
}

/// Delete a user style into the trash; [`undo_delete`] brings the last
/// one back.
pub fn delete(name: &str) -> Result<(), String> {
    if is_builtin(name) {
        return Err(format!("`{name}` is built in; hide it instead"));
    }
    let path = user_path(name).filter(|p| p.is_file()).ok_or_else(|| format!("no user style named `{name}`"))?;
    let trash = user_dir().ok_or("no data folder")?.join(".trash");
    std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
    std::fs::rename(&path, trash.join(format!("{:016}__{name}.splash", now_ms()))).map_err(|e| e.to_string())
}

/// What the trash holds, newest first: (file, style name).
pub fn trash() -> Vec<(PathBuf, String)> {
    let Some(dir) = user_dir().map(|d| d.join(".trash")) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(PathBuf, String)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            let (_, name) = stem.split_once("__")?;
            Some((path.clone(), name.to_string()))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

/// Bring back the most recently deleted style. Returns its name.
pub fn undo_delete() -> Result<String, String> {
    let (path, name) = trash().into_iter().next().ok_or("nothing was deleted")?;
    let name = unique_name(&name);
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    write(&name, &text)?;
    let _ = std::fs::remove_file(path);
    Ok(name)
}

/// The text before the last write, if any (without going back to it).
pub fn previous(name: &str) -> Option<String> {
    let dir = history_dir(name)?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).collect();
    files.sort();
    std::fs::read_to_string(files.pop()?).ok()
}

/// Go back one version: the text before the last write. Returns it.
pub fn revert(name: &str) -> Result<String, String> {
    if is_builtin(name) {
        return Err(format!("`{name}` is built in and never changes"));
    }
    let dir = history_dir(name).ok_or("no data folder")?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map_err(|_| format!("`{name}` has no earlier version"))?.flatten().map(|e| e.path()).collect();
    files.sort();
    let last = files.pop().ok_or_else(|| format!("`{name}` has no earlier version"))?;
    let text = std::fs::read_to_string(&last).map_err(|e| e.to_string())?;
    let path = user_path(name).ok_or("no data folder")?;
    std::fs::write(&path, &text).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(last);
    Ok(text)
}

/// A fork goes back to its built-in (the title stays; the old text goes
/// to the history).
pub fn reset_to_original(name: &str) -> Result<(), String> {
    let text = read(name).ok_or_else(|| format!("no style named `{name}`"))?;
    let from = header(&text, "from").ok_or_else(|| format!("`{name}` was not copied from a built-in"))?;
    let title = header(&text, "title").unwrap_or_else(|| title_of(name));
    let fresh = starter(&title, Some(&from))?;
    write(name, &fresh)
}

// ---- editing one part ----

/// A style's statements: `(target, text)`, where target is the assigned
/// path (`mod.widgets.HwKey.draw_bg.ground`) and text the whole statement,
/// continuation lines included. Comments, `use` lines and the closing
/// `true` come back with an empty target.
pub fn statements(source: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in source.lines() {
        let starts = !line.starts_with(|c: char| c.is_whitespace() || c == '}' || c == ')' || c == ']');
        if starts || out.is_empty() {
            let target = if line.starts_with("mod.") {
                line.split_once('=').map(|(l, _)| l.trim().to_string()).unwrap_or_default()
            } else {
                String::new()
            };
            out.push((target, line.to_string()));
        } else {
            let last = out.last_mut().unwrap();
            last.1.push('\n');
            last.1.push_str(line);
        }
    }
    out
}

/// Set (or add) `target = value` in `source`: an existing assignment to
/// the same path is replaced in place; a new one goes after the last
/// assignment to the same template, else before the closing `true`.
pub fn upsert(source: &str, target: &str, value: &str) -> String {
    let statement = format!("{target} = {}", value.trim());
    let mut parts = statements(source);
    if let Some(at) = parts.iter().position(|(t, _)| t == target) {
        parts[at].1 = statement;
    } else {
        let template = template_of(target);
        let after = parts.iter().rposition(|(t, _)| !t.is_empty() && template_of(t) == template);
        let at = match after {
            Some(at) => at + 1,
            None => parts.iter().rposition(|(_, text)| text.trim() == "true").unwrap_or(parts.len()),
        };
        parts.insert(at, (target.to_string(), statement));
    }
    if !parts.iter().any(|(_, text)| text.trim() == "true") {
        parts.push((String::new(), "true".into()));
    }
    let mut out = parts.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("\n");
    out.push('\n');
    out
}

/// `mod.widgets.HwKey.draw_bg.ground` → `HwKey`.
fn template_of(target: &str) -> &str {
    target.strip_prefix("mod.widgets.").unwrap_or(target).split('.').next().unwrap_or("")
}

/// The value assigned to `target`, if the style sets it.
pub fn value_of(source: &str, target: &str) -> Option<String> {
    statements(source).into_iter().find(|(t, _)| t == target).and_then(|(_, text)| text.split_once('=').map(|(_, v)| v.trim().to_string()))
}
