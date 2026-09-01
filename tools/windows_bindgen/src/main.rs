//! Regenerate the vendored `windows` crate bindings.
//!
//! `libs/windows/windows-rs` is upstream `windows` 0.62.2 with ONE generated
//! file, `src/Windows/mod.rs`, holding exactly the APIs this repo uses (plus
//! their metadata dependencies) instead of the 10M-line full crate. This tool
//! is the only way that file changes:
//!
//! ```text
//! cd tools/windows_bindgen && cargo run --release
//! ```
//!
//! It runs `windows-bindgen` 0.62.1 in `--package --implement` mode over
//! `filter.txt` (one fully-qualified type, function, constant or namespace per
//! line; `#` comments allowed), closes the set over every dependency the
//! generator would otherwise skip a member for, folds the per-namespace files
//! into the single nested-module file the crate `include!`s (keeping every
//! `#[cfg(feature = "...")]` gate, module-level ones included, so the feature
//! algebra is exactly upstream's), and normalizes
//! the two spots where the published 0.62.1 generator predates the vendored
//! windows-core 0.62.2 (`Error::from_thread`, `imp::array_proxy`). Item-level
//! A consumer enables the features for the namespaces it uses, as with the
//! upstream crate.
//!
//! To use a new Windows API: add it (or its namespace) to `filter.txt`, run
//! this, and `cargo check --target x86_64-pc-windows-msvc` the consumers
//! (platform, platform/video, platform/network, libs/system_speech, mpterm).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let tool_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = tool_dir
        .parent()
        .and_then(Path::parent)
        .expect("tools/windows_bindgen sits two levels below the repo root");
    let filter = tool_dir.join("filter.txt");
    let out_file = repo.join("libs/windows/windows-rs/src/Windows/mod.rs");
    let pkg_dir = tool_dir.join("target/bindgen-package");

    let filter_text = fs::read_to_string(&filter).expect("read filter.txt");
    let mut wanted: BTreeSet<String> = filter_text
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    eprintln!("windows-bindgen: {} filter entries", wanted.len());

    // A member whose parameter or return type is outside the set is skipped by
    // the generator — but its `_Impl` vtable slot is still emitted, and the two
    // disagree (a `usize` placeholder against a fn item). Closing the set over
    // those dependencies removes every skip, so the output is self-consistent.
    for round in 1..=20 {
        let _ = fs::remove_dir_all(&pkg_dir);
        fs::create_dir_all(&pkg_dir).expect("create package dir");
        let warnings = generate(&pkg_dir, &wanted);
        let missing = missing_dependencies(&warnings.to_string());
        if missing.is_empty() {
            break;
        }
        eprintln!("round {round}: {} skipped members, adding {} dependency types", warnings.len(), missing.len());
        let before = wanted.len();
        wanted.extend(missing);
        if wanted.len() == before {
            panic!("the generator keeps skipping members it was already given:\n{warnings}");
        }
    }

    let flat = normalize(&flatten(&pkg_dir.join("src/Windows")));
    fs::write(&out_file, &flat).expect("write mod.rs");
    eprintln!("wrote {} ({} lines)", out_file.display(), flat.lines().count());
}

fn generate(pkg_dir: &Path, wanted: &BTreeSet<String>) -> windows_bindgen::Warnings {
    let mut args: Vec<String> = vec![
        "--in".into(),
        "default".into(),
        "--package".into(),
        "--no-toml".into(),
        // Upstream generates the `*_Impl` traits too; consumers implement COM
        // interfaces (drop targets, MF callbacks) through them.
        "--implement".into(),
        // Upstream links Win32 imports through windows_core, not windows_link;
        // the vendored crate has no windows-link dependency.
        "--link".into(),
        "windows_core".into(),
        // Upstream's own rustfmt.toml: one item per line, 800 columns. The repo
        // root's rustfmt.toml disables formatting; override it here or the
        // generator falls back to raw token soup.
        "--rustfmt".into(),
        "disable_all_formatting=false,max_width=800,newline_style=Unix".into(),
        "--out".into(),
        pkg_dir.to_string_lossy().into_owned(),
        "--filter".into(),
    ];
    args.extend(wanted.iter().cloned());
    windows_bindgen::bindgen(args.iter().map(String::as_str))
}

/// The types named under "due to missing dependencies:" in the warnings.
fn missing_dependencies(warnings: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_list = false;
    for line in warnings.lines() {
        if line.contains("due to missing dependencies") {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(name) = line.strip_prefix("  Windows.") {
                out.insert(format!("Windows.{}", name.trim()));
                continue;
            }
            in_list = false;
        }
    }
    out
}

/// Fold one package-mode namespace directory: its items first, then each
/// submodule as `#[cfg(feature = "..")] pub mod Name{ ... }`, recursively. The
/// per-file header (generator comment, `#![allow(..)]`) is dropped; the
/// module declarations become inline modules with the same gate; everything
/// else is kept verbatim.
fn flatten(dir: &Path) -> String {
    let text = fs::read_to_string(dir.join("mod.rs")).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() && (lines[i].starts_with("// Bindings generated") || lines[i].trim().is_empty()) {
        i += 1;
    }
    if i < lines.len() && lines[i].starts_with("#![allow(") {
        // One line at max_width=800, several at rustfmt defaults.
        while i < lines.len() && !lines[i].trim_end().ends_with(")]") {
            i += 1;
        }
        i += 1;
        while i < lines.len() && lines[i].trim().is_empty() {
            i += 1;
        }
    }
    let mut items = String::new();
    let mut mods: Vec<(Option<&str>, &str)> = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        if let Some(name) = submodule_decl(line) {
            mods.push((None, name));
            i += 1;
            continue;
        }
        if line.starts_with("#[cfg(") && line.ends_with(")]") {
            if let Some(name) = lines.get(i + 1).and_then(|l| submodule_decl(l)) {
                mods.push((Some(line), name));
                i += 2;
                continue;
            }
        }
        items.push_str(line);
        items.push('\n');
        i += 1;
    }
    let mut out = items;
    for (cfg, name) in mods {
        if let Some(cfg) = cfg {
            out.push_str(cfg);
            out.push('\n');
        }
        out.push_str("pub mod ");
        out.push_str(name);
        out.push_str("{\n");
        out.push_str(&flatten(&dir.join(name)));
        out.push_str("}\n");
    }
    out
}

fn submodule_decl(line: &str) -> Option<&str> {
    line.strip_prefix("pub mod ")?.strip_suffix(';')
}

/// The published windows-bindgen 0.62.1 targets windows-core 0.62.1; the
/// vendored core is 0.62.2, which renamed the last-error constructor and moved
/// the implement-side array proxy. These are the only two differences the
/// generated code shows against the upstream 0.62.2 sources.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let line = line.replace("windows_core::Error::from_win32", "windows_core::Error::from_thread");
        let line = rewrite_array_proxy(&line);
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// `windows_core::ArrayProxy::from_raw_parts(ARGS).as_array()` ->
/// `&mut windows_core::imp::array_proxy(ARGS)` (what upstream 0.62.2 emits).
fn rewrite_array_proxy(line: &str) -> String {
    const OLD: &str = "windows_core::ArrayProxy::from_raw_parts(";
    const TAIL: &str = ").as_array()";
    let mut rest = line;
    let mut out = String::new();
    while let Some(start) = rest.find(OLD) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OLD.len()..];
        let Some(end) = after.find(TAIL) else {
            panic!("unexpected array proxy form: {line}");
        };
        out.push_str("&mut windows_core::imp::array_proxy(");
        out.push_str(&after[..end]);
        out.push(')');
        rest = &after[end + TAIL.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_proxy_call_sites_take_the_upstream_form() {
        let line = "IPropertyValue_Impl::GetUInt8Array(this, windows_core::ArrayProxy::from_raw_parts(core::mem::transmute_copy(&value), value_array_size).as_array()).into()";
        assert_eq!(
            rewrite_array_proxy(line),
            "IPropertyValue_Impl::GetUInt8Array(this, &mut windows_core::imp::array_proxy(core::mem::transmute_copy(&value), value_array_size)).into()"
        );
    }

    #[test]
    fn missing_dependency_lists_are_parsed() {
        let text = "skipping `A.B` due to missing dependencies:\n  Windows.Win32.System.Search.Common.CONDITION_OPERATION\n  Windows.Foundation.X\nskipping `C` due to missing dependencies:\n  Windows.Foundation.X\n";
        let missing = missing_dependencies(text);
        assert_eq!(missing.len(), 2);
        assert!(missing.contains("Windows.Foundation.X"));
    }
}
