//! Relaxed Rust (local/plans/relaxed-rust.md): which crates opt in to which relaxations, the
//! notes an accepted relaxation leaves, and the write-back (`makepad fix --to-strict`) that turns
//! the source into the plain Rust rustc accepts.
//!
//! Strict is the default. A crate opts in with `[package.metadata.makepad-run] relaxed = true` (every
//! shipped feature) or `relaxed = ["mut", "imports"]`, minus `relaxed-deny = [..]`; the session
//! override `MAKEPAD_RUN_RELAXED=all|none|<list>` (also `makepad run/check --relaxed[=<list>]`) replaces
//! every crate's setting. Each relaxation only accepts programs strict rejects; each accepted site
//! is recorded with the rustc error it replaces and, for write-back features, the edit.

use relax_session::sync::Mutex;

/// The shipped relaxation features (spec names).
pub const FEATURES: &[&str] = &["mut", "imports", "field-views", "two-phase"];

/// One accepted relaxation.
#[derive(Clone, Debug)]
pub struct Note {
    /// "file:line:col"
    pub at: String,
    pub feature: &'static str,
    /// the rustc error code strict mode reports
    pub code: &'static str,
    pub msg: String,
    /// write-back: insert `text` at byte `pos` of `file`
    pub fix: Option<(String, u32, String)>,
}

static NOTES: Mutex<Vec<Note>> = Mutex::new(Vec::new());
/// `makepad check --strict-report`: print every accepted site
pub static STRICT_REPORT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// `makepad fix --to-strict`: write the edits back into the sources
pub static FIX: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// End of a check: the report and / or the write-back the session asked for; a one-line
/// summary otherwise when anything was accepted.
pub fn finish_check() {
    use std::sync::atomic::Ordering;
    let notes = take_notes();
    if STRICT_REPORT.load(Ordering::Relaxed) {
        print_report(&notes);
    } else if !notes.is_empty() {
        eprintln!("makepad-run relaxed: {} site(s) accepted only in relaxed mode (makepad check --strict-report lists them)", notes.len());
    }
    if FIX.load(Ordering::Relaxed) {
        match apply_fixes(&notes) {
            Ok((f, e)) => eprintln!("makepad fix: {} edit(s) in {} file(s); {} site(s) without a write-back", e, f, notes.iter().filter(|n| n.fix.is_none()).count()),
            Err(e) => eprintln!("makepad fix: {}", e),
        }
    }
}
fn parse_list(v: &str) -> Vec<String> {
    match v.trim() {
        "" | "none" | "false" => Vec::new(),
        "all" | "true" | "1" => FEATURES.iter().map(|s| s.to_string()).collect(),
        l => l.split(',').map(|s| s.trim().to_string()).filter(|s| FEATURES.contains(&s.as_str())).collect(),
    }
}

/// The relaxation features crate `root` (its root source file) has on: the session override if
/// any, else its manifest's `[package.metadata.makepad-run]`.
pub static FEATURES_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn crate_features(options: &crate::options::Options, root: &str) -> Vec<String> {
    let t = std::time::Instant::now();
    let r = crate_features_t(options, root);
    FEATURES_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    r
}

fn crate_features_t(options: &crate::options::Options, root: &str) -> Vec<String> {
    if let Some(v) = &options.relaxed {
        return parse_list(v);
    }
    let mut dir = std::path::Path::new(root).parent();
    while let Some(d) = dir {
        let m = d.join("Cargo.toml");
        if crate::vfs::exists(&m) {
            return manifest_features(&crate::vfs::read_to_string(&m).unwrap_or_default());
        }
        dir = d.parent();
    }
    Vec::new()
}

/// `[package.metadata.makepad-run]`: `relaxed = true | [..]`, `relaxed-deny = [..]`.
fn manifest_features(toml: &str) -> Vec<String> {
    let mut in_sec = false;
    let mut on: Vec<String> = Vec::new();
    let mut deny: Vec<String> = Vec::new();
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_sec = l.trim_start_matches('[').trim_end_matches(']').trim() == "package.metadata.makepad-run";
            continue;
        }
        if !in_sec {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.split('#').next().unwrap_or("").trim();
        let list = |v: &str| -> Vec<String> {
            if v == "true" {
                return FEATURES.iter().map(|s| s.to_string()).collect();
            }
            v.trim_start_matches('[').trim_end_matches(']').split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| FEATURES.contains(&s.as_str())).collect()
        };
        match k.trim() {
            "relaxed" => on = list(v),
            "relaxed-deny" => deny = list(v),
            _ => {}
        }
    }
    on.retain(|f| !deny.contains(f));
    on
}

pub fn note(n: Note) {
    let mut v = NOTES.lock().unwrap_or_else(|e| e.into_inner());
    if !v.iter().any(|x| x.at == n.at && x.code == n.code && x.msg == n.msg) {
        v.push(n);
    }
}

pub fn take_notes() -> Vec<Note> {
    std::mem::take(&mut *NOTES.lock().unwrap_or_else(|e| e.into_inner()))
}

/// `makepad check --strict-report`: one line per accepted site, then totals per feature.
pub fn print_report(notes: &[Note]) {
    let mut v: Vec<&Note> = notes.iter().collect();
    v.sort_by(|a, b| a.at.cmp(&b.at));
    for n in &v {
        let policy = if n.fix.is_some() { "W" } else { "E" };
        eprintln!("{}  {}  {}  {}  {}", n.at, n.feature, n.code, policy, n.msg);
    }
    let mut totals: Vec<(&str, usize)> = Vec::new();
    for n in &v {
        match totals.iter_mut().find(|t| t.0 == n.feature) {
            Some(t) => t.1 += 1,
            None => totals.push((n.feature, 1)),
        }
    }
    let t: Vec<String> = totals.iter().map(|(f, c)| format!("{} {}", f, c)).collect();
    eprintln!("makepad-run strict-report: {} site(s) rustc rejects{}{}", v.len(), if t.is_empty() { "" } else { ": " }, t.join(", "));
}

/// `makepad fix --to-strict`: applies the write-back edits (each once, from the end of each file
/// backwards). Returns (files changed, edits).
pub fn apply_fixes(notes: &[Note]) -> std::io::Result<(usize, usize)> {
    let mut by_file: Vec<(String, Vec<(u32, String)>)> = Vec::new();
    for n in notes {
        let Some((f, pos, text)) = &n.fix else { continue };
        match by_file.iter_mut().find(|x| &x.0 == f) {
            Some(x) => {
                if !x.1.iter().any(|e| e.0 == *pos && &e.1 == text) {
                    x.1.push((*pos, text.clone()));
                }
            }
            None => by_file.push((f.clone(), vec![(*pos, text.clone())])),
        }
    }
    let (mut files, mut edits) = (0, 0);
    for (f, mut es) in by_file {
        let mut src = std::fs::read(&f)?;
        // from the end backwards; edits at one position end up in ascending text order
        es.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        for (pos, text) in &es {
            let p = (*pos as usize).min(src.len());
            src.splice(p..p, text.bytes());
            edits += 1;
        }
        let tmp = format!("{}.makepad-run-fix", f);
        std::fs::write(&tmp, &src)?;
        std::fs::rename(&tmp, &f)?;
        files += 1;
    }
    Ok((files, edits))
}
