//! Rust references to a named widget: the `id!(name)` and `ids!(a.b.name)`
//! lookups in the crate that owns the file under design. A rename or a
//! delete in the Splash source does not touch them, so the code that finds
//! the widget by name keeps looking for the old name and finds nothing. The
//! scan is a warning, not a gate: it names the places to fix, up front.

use std::path::{Path, PathBuf};

/// One Rust line that names the widget in a lookup macro.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    /// The file, relative to the crate root.
    pub path: String,
    /// One-based.
    pub line: usize,
}

/// The macros whose argument is a widget path.
const LOOKUPS: &[&str] = &["ids_array!(", "live_id!(", "ids!(", "id!("];

/// The crate root above `file`: the nearest ancestor holding a Cargo.toml.
fn crate_root(file: &Path) -> Option<PathBuf> {
    let mut dir = file.parent()?;
    loop {
        if dir.join("Cargo.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Every `.rs` file under `dir`, skipping build output.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            if name != "target" && !name.to_string_lossy().starts_with("target-") {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Whether a line names `name` as one segment of a lookup macro's path.
/// A bare `live_id!` is an id for anything (a menu row, a hotkey), so it
/// counts only on a line that looks a widget up with it.
pub fn line_names(line: &str, name: &str) -> bool {
    let widget_lookup = ["child(", "widget(", "widget_flood(", "widgets("].iter().any(|call| line.contains(call));
    for lookup in LOOKUPS {
        if *lookup == "live_id!(" && !widget_lookup {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find(lookup) {
            let inner = &rest[at + lookup.len()..];
            let end = inner.find(')').unwrap_or(inner.len());
            let names = inner[..end]
                .split(|c: char| c == '.' || c == ',' || c.is_whitespace())
                .any(|segment| segment == name);
            if names {
                return true;
            }
            rest = &inner[end..];
        }
    }
    false
}

/// The Rust lines in `file`'s crate that look the widget up by `name`.
pub fn rust_references(file: &str, name: &str) -> Vec<Reference> {
    let mut out = Vec::new();
    if name.is_empty() {
        return out;
    }
    let Some(root) = crate_root(Path::new(file)) else {
        return out;
    };
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    let design_file = Path::new(file);
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Another file that declares a widget of the same name looks up
        // its own, not this one.
        if path != design_file && declares(&text, name) {
            continue;
        }
        for (index, line) in text.lines().enumerate() {
            if line_names(line, name) {
                let rel = path.strip_prefix(&root).unwrap_or(&path);
                out.push(Reference {
                    path: rel.to_string_lossy().replace('\\', "/"),
                    line: index + 1,
                });
            }
        }
    }
    out
}

/// Whether `text` declares a Splash widget called `name` (`name := ...`).
fn declares(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        line.trim_start()
            .strip_prefix(name)
            .is_some_and(|rest| rest.trim_start().starts_with(":="))
    })
}

/// One line for the panel naming where `name` is looked up from Rust, or
/// nothing when it is not.
pub fn references_note(file: &str, name: &str) -> Option<String> {
    let refs = rust_references(file, name);
    if refs.is_empty() {
        return None;
    }
    let shown: Vec<String> =
        refs.iter().take(4).map(|r| format!("{}:{}", r.path, r.line)).collect();
    let more = refs.len().saturating_sub(shown.len());
    Some(format!(
        "note: Rust looks `{}` up by name at {}{}",
        name,
        shown.join(", "),
        if more > 0 { format!(" (+{} more)", more) } else { String::new() }
    ))
}
