//! The Architecture mode: ONE cached architecture plan —
//! `arch/<crate>/crate.toml` of the crate the active code tab belongs to,
//! else the platform's — parsed, validated and laid out on a worker
//! (`makepad_code_arch` + [`plan`] + [`layout`]), drawn by
//! [`view::ArchitectureView`] as lane bands with their titles, uniform
//! subsystem cards (kind icon, title, summary, budget chip, a kind stripe)
//! and orthogonal relations with arrowheads; hover reads a card's summary
//! and first story sentence and lights its one-hop neighbourhood, a click
//! pins the story, refs and relations in the side report.
use makepad_code_arch::{changes_since, validate, Changes, Limits, Validation};
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod layout;
pub mod plan;
pub mod view;

pub use view::{ArchitectureView, ArchitectureViewAction, ArchitectureViewRef};

/// The default plan when the active file names no crate with one.
pub const DEFAULT_PLAN: &str = "arch/platform/crate.toml";

/// A plan loaded, validated and laid out on the worker.
pub struct DesignBundle {
    /// The repository-relative plan path.
    pub path: PathBuf,
    pub scene: Arc<plan::DesignScene>,
    pub validation: Validation,
    pub changes: Changes,
}

/// The candidate plan paths for a repository-relative file path: the
/// deepest crate directory first, then its ancestors, then the default plan.
pub fn plan_candidates(selected_path: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = selected_path {
        let mut dir = Path::new(path).parent();
        while let Some(d) = dir {
            if d.as_os_str().is_empty() {
                break;
            }
            out.push(PathBuf::from("arch").join(d).join("crate.toml"));
            dir = d.parent();
        }
    }
    let default = PathBuf::from(DEFAULT_PLAN);
    if !out.contains(&default) {
        out.push(default);
    }
    out
}

/// The worker's job: the first candidate that exists, parsed, validated,
/// laid out, its freshness against the tree.
pub fn load_design(root: PathBuf, candidates: Vec<PathBuf>) -> Result<DesignBundle, String> {
    for rel in &candidates {
        let abs = root.join(rel);
        if !abs.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&abs).map_err(|e| format!("{}: {e}", rel.display()))?;
        let (plan, scene) = plan::design_scene(&text, &Limits::default()).map_err(|e| format!("{}: {e}", rel.display()))?;
        let validation = validate(&plan, &root);
        let changes = changes_since(&plan, &root);
        return Ok(DesignBundle { path: rel.clone(), scene: Arc::new(scene), validation, changes });
    }
    Err(format!("No architecture plan found ({DEFAULT_PLAN} is missing)"))
}

/// The freshness chip: `up to date`, or how many files changed.
pub fn freshness_text(changes: &Changes) -> String {
    let n = changes.added.len() + changes.removed.len() + changes.modified.len();
    if !changes.errors.is_empty() {
        "freshness unknown".to_string()
    } else if n == 0 {
        "up to date".to_string()
    } else if changes.affected_nodes.is_empty() {
        format!("stale · {n} file{} changed", if n == 1 { "" } else { "s" })
    } else {
        format!("stale · {n} file{} changed · {} subsystem{} affected", if n == 1 { "" } else { "s" }, changes.affected_nodes.len(), if changes.affected_nodes.len() == 1 { "" } else { "s" })
    }
}

/// Word-wrap `text` into at most `max_lines` lines of `avail` px, the last
/// line ellipsized when the text runs on.
pub fn wrap_lines(measure: &mut impl FnMut(&str) -> f64, text: &str, avail: f64, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        let candidate = if line.is_empty() { words[i].to_string() } else { format!("{line} {}", words[i]) };
        if measure(&candidate) <= avail || line.is_empty() {
            line = candidate;
            i += 1;
        } else {
            lines.push(std::mem::take(&mut line));
            if lines.len() == max_lines {
                break;
            }
        }
    }
    if !line.is_empty() && lines.len() < max_lines {
        lines.push(line);
    }
    if i < words.len() || lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            while !last.is_empty() && measure(&format!("{last}…")) > avail {
                last.pop();
            }
            last.push('…');
        }
    }
    lines
}

pub fn ellipsize_to(measure: &mut impl FnMut(&str) -> f64, text: &str, avail: f64) -> String {
    if measure(text) <= avail {
        return text.to_string();
    }
    let mut s = text.to_string();
    while !s.is_empty() && measure(&format!("{s}…")) > avail {
        s.pop();
    }
    format!("{s}…")
}

/// The side report of the Architecture mode: the plan's overview, or the
/// selected subsystem's story, refs and relations, as text lines.
pub fn report_lines(bundle: Option<&DesignBundle>, error: Option<&str>, loading: bool, selected: Option<usize>) -> Vec<String> {
    let Some(b) = bundle else {
        return match (error, loading) {
            (Some(e), _) => vec!["Architecture plan".into(), e.to_string()],
            (None, true) => vec!["Architecture plan".into(), "Loading…".into()],
            (None, false) => vec!["Architecture plan".into(), "No plan loaded".into()],
        };
    };
    let s = &b.scene;
    let mut rows = Vec::new();
    match selected.and_then(|i| s.nodes.get(i).map(|n| (i, n))) {
        Some((i, n)) => {
            rows.push(format!("{} · {}", n.title, n.kind.as_str()));
            if let Some(lane) = n.lane.and_then(|l| s.lanes.get(l)) {
                rows.push(format!("Lane: {}", lane.title));
            }
            if let Some(bd) = &n.budget {
                rows.push(format!("Budget: {bd}"));
            }
            rows.push(n.summary.clone());
            for line in n.story.lines().map(str::trim).filter(|l| !l.is_empty()) {
                rows.push(line.to_string());
            }
            let edges = s.edges_of(i);
            if !edges.is_empty() {
                rows.push(String::new());
                rows.push(format!("Relations ({})", edges.len()));
                for (ei, out) in edges {
                    let e = &s.edges[ei];
                    let other = if out { &s.nodes[e.to] } else { &s.nodes[e.from] };
                    let verb = e.kind.as_str().replace('_', " ");
                    let text = if out { format!("→ {verb} {}", other.title) } else { format!("← {} {verb}", other.title) };
                    rows.push(match &e.label {
                        Some(l) => format!("{text} · {l}"),
                        None => text,
                    });
                }
            }
            if !n.refs.is_empty() {
                rows.push(String::new());
                rows.push(format!("Code ({})", n.refs.len()));
                for r in &n.refs {
                    rows.push(format!("{} · {}", r.label(), r.path));
                }
            }
            if let Some(c) = &n.child {
                rows.push(format!("Child plan: {c}"));
            }
        }
        None => {
            rows.push(format!("{} · architecture plan", s.title));
            for line in s.overview.lines().map(str::trim).filter(|l| !l.is_empty()) {
                rows.push(line.to_string());
            }
            rows.push(String::new());
            rows.push(format!("Scope: {}", s.scope));
            rows.push(format!("Lanes: {}", s.lanes.len()));
            rows.push(format!("Subsystems: {}", s.nodes.len()));
            rows.push(format!("Relations: {}", s.edges.len()));
            rows.push(format!("Files: {}", s.file_count));
            rows.push(format!("Generated by: {} · {}", s.generator, s.prompt));
            rows.push(format!("Freshness: {}", freshness_text(&b.changes)));
            if !b.changes.affected_nodes.is_empty() {
                rows.push(format!("Affected: {}", b.changes.affected_nodes.join(", ")));
            }
            rows.push(format!("Plan: {}", b.path.display()));
            for w in b.validation.warnings.iter().take(4) {
                rows.push(format!("Warning · {}: {}", w.field, w.message));
            }
            if !s.lanes.is_empty() {
                rows.push(String::new());
                rows.push("Lanes".into());
                for li in &s.lane_order {
                    if let Some(l) = s.lanes.get(*li) {
                        let count = s.nodes.iter().filter(|n| n.lane == Some(*li)).count();
                        rows.push(format!("{} · {count}", l.title));
                    }
                }
            }
        }
    }
    rows
}

/// Register the plan view and its shaders.
pub fn script_mod(vm: &mut ScriptVm) {
    view::script_mod(vm);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_walk_up_to_the_crate_then_the_default() {
        let c = plan_candidates(Some("libs/regex/src/range.rs"));
        assert_eq!(c, vec![PathBuf::from("arch/libs/regex/src/crate.toml"), PathBuf::from("arch/libs/regex/crate.toml"), PathBuf::from("arch/libs/crate.toml"), PathBuf::from(DEFAULT_PLAN)]);
        assert_eq!(plan_candidates(None), vec![PathBuf::from(DEFAULT_PLAN)]);
        let c = plan_candidates(Some("platform/src/cx.rs"));
        assert_eq!(c.last(), Some(&PathBuf::from(DEFAULT_PLAN)));
        assert_eq!(c.iter().filter(|p| **p == PathBuf::from(DEFAULT_PLAN)).count(), 1);
    }

    #[test]
    fn freshness_reads_the_change_count() {
        let mut c = Changes::default();
        c.unchanged = true;
        assert_eq!(freshness_text(&c), "up to date");
        c.modified.push(PathBuf::from("a.rs"));
        c.unchanged = false;
        assert_eq!(freshness_text(&c), "stale · 1 file changed");
        c.added.push(PathBuf::from("b.rs"));
        c.affected_nodes.push("cx".into());
        assert_eq!(freshness_text(&c), "stale · 2 files changed · 1 subsystem affected");
    }

    #[test]
    fn wrapping_keeps_lines_within_the_width_and_ellipsizes_the_rest() {
        let mut measure = |t: &str| t.chars().count() as f64 * 6.0;
        let lines = wrap_lines(&mut measure, "one two three four five six seven", 60.0, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'));
        for l in &lines {
            assert!(measure(l) <= 60.0 + 6.0, "{l}");
        }
        assert_eq!(wrap_lines(&mut measure, "short", 60.0, 2), vec!["short".to_string()]);
        assert_eq!(ellipsize_to(&mut measure, "abcdefghijkl", 30.0), "abcd…");
    }

    #[test]
    fn the_platform_plan_loads_from_the_repository() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let root = root.canonicalize().unwrap_or(root);
        if !root.join(DEFAULT_PLAN).is_file() {
            return;
        }
        let b = load_design(root, plan_candidates(None)).expect("the platform plan loads");
        assert!(b.scene.nodes.len() >= 8, "{} nodes", b.scene.nodes.len());
        assert!(b.validation.is_valid(), "{:?}", b.validation.errors);
        for e in &b.scene.edges {
            assert!(e.from == e.to || e.points.len() >= 2, "edge {} unrouted", e.id);
        }
        let report = report_lines(Some(&b), None, false, None);
        assert!(report.iter().any(|l| l.starts_with("Subsystems: ")));
        let selected = report_lines(Some(&b), None, false, Some(0));
        assert!(selected[0].contains(&b.scene.nodes[0].title));
    }
}
