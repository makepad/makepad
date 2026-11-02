//! Differential evidence. Compiler failures never count as agreement, even on both sides.

pub struct Output {
    pub exit: i32,
    pub text: String,
}

pub fn jit_error(text: &str) -> Option<String> {
    let verdict = text.lines().any(|line| line.starts_with("test result:"));
    text.lines().find(|line| {
        let s = line.to_ascii_lowercase();
        ["internal compiler error", "rc0002", "patchrefused", "patchreverted", "patch refused", "refused patch", "jit panic", "jit error"].iter().any(|p| s.contains(p))
            || (s.contains("makepad run bug") && !s.contains("if `cargo test` builds it"))
            || (!verdict && s.contains("panicked at") && ["relax/", "relax_", "relax-", "jit.rs"].iter().any(|p| s.contains(p)))
            || s.trim_start().starts_with("ice:") || s.contains("error: ice") || s.contains("[ice]")
    }).map(|s| s.trim().to_string())
}

/// Replace numeric durations, leaving counts, values and all non-timing text intact.
pub fn durations(line: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    while at < line.len() {
        let rest = &line[at..];
        let c = rest.chars().next().unwrap();
        if c.is_ascii_digit() && (at == 0 || !line[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')) {
            let n = rest.bytes().take_while(|c| c.is_ascii_digit() || *c == b'.').count();
            let tail = &rest[n..];
            let space = tail.bytes().take_while(|c| *c == b' ').count();
            let units = &tail[space..];
            if let Some(unit) = ["seconds", "second", "secs", "sec", "ns", "us", "µs", "ms", "s"].into_iter().find(|u| units.starts_with(u) && units[u.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_')) {
                out.push_str("<time>");
                at += n + space + unit.len();
                continue;
            }
        }
        out.push(c);
        at += c.len_utf8();
    }
    out
}

pub fn normalize(text: &str, roots: &[(&str, &str)]) -> Vec<String> {
    let mut roots = roots.to_vec();
    roots.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
    let mut lines: Vec<String> = text.lines().map(|line| {
        let mut line = line.to_string();
        for (path, name) in &roots { if !path.is_empty() { line = line.replace(path, name); } }
        durations(&line)
    }).collect();
    // Only libtest status rows may change order; printed values and diagnostics retain theirs.
    let mut at = 0;
    while at < lines.len() {
        let end = (at..lines.len()).find(|i| !test_row(&lines[*i])).unwrap_or(lines.len());
        lines[at..end].sort();
        at = end + 1;
    }
    lines
}

fn test_row(s: &str) -> bool {
    s.starts_with("test ") && s.split_once(" ... ").is_some_and(|(_, v)| matches!(v, "ok" | "FAILED" | "ignored"))
}

pub fn first_diff(tree: &[String], overlay: &[String]) -> Option<String> {
    (0..tree.len().max(overlay.len())).find_map(|i| {
        let a = tree.get(i).map_or("<end>", String::as_str);
        let b = overlay.get(i).map_or("<end>", String::as_str);
        (a != b).then(|| format!("line {}: tree={a:?}; overlay={b:?}", i + 1))
    })
}

/// Shadow evidence excludes build chatter; test result rows and failing names keep duplicates.
pub fn summary(exit: i32, text: &str) -> Vec<String> {
    let mut out = vec![format!("exit={exit}")];
    let mut results = Vec::new();
    let mut failures = Vec::new();
    let mut listing = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("test result:") { results.push(durations(trimmed)); listing = false; }
        else if trimmed == "failures:" { listing = true; }
        else if test_row(trimmed) && trimmed.ends_with(" ... FAILED") { failures.push(trimmed.to_string()); }
        else if trimmed.ends_with(" --- FAILED") { failures.push(trimmed.to_string()); }
        else if listing && line.starts_with("    ") && !trimmed.is_empty() && trimmed.split_whitespace().count() == 1 { failures.push(trimmed.to_string()); }
        else if !trimmed.is_empty() { listing = false; }
    }
    results.sort();
    failures.sort();
    out.extend(results);
    out.extend(failures);
    out
}
