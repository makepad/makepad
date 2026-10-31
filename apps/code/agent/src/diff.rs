//! Line diffs as hunks, and applying a chosen subset of them.

use super::event::{Diff, DiffLine, Hunk};
use makepad_git::DiffOp;

const CONTEXT: usize = 3;

/// Ops [first, last] (inclusive) that form one hunk's changes.
fn clusters(ops: &[DiffOp]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut pending_equal = usize::MAX; // length of the Equal run since the last change
    for (i, op) in ops.iter().enumerate() {
        match op {
            DiffOp::Equal { len, .. } => {
                pending_equal = if pending_equal == usize::MAX { *len } else { pending_equal + len };
                let _ = i;
            }
            _ => {
                match out.last_mut() {
                    Some(last) if pending_equal <= 2 * CONTEXT => last.1 = i,
                    _ => out.push((i, i)),
                }
                pending_equal = 0;
            }
        }
    }
    out
}

fn lines_of(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn trim_eol(s: &str) -> String {
    s.trim_end_matches(['\n', '\r']).to_string()
}

/// The hunks that turn `old` into `new`, with three lines of context.
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let ops = makepad_git::diff_lines(old, new);
    let (a, b) = (lines_of(old), lines_of(new));
    // (old, new) line position at the start of every op.
    let mut at = Vec::with_capacity(ops.len());
    let (mut o, mut n) = (0usize, 0usize);
    for op in &ops {
        at.push((o, n));
        match op {
            DiffOp::Equal { len, .. } => {
                o += len;
                n += len;
            }
            DiffOp::Delete { len, .. } => o += len,
            DiffOp::Insert { len, .. } => n += len,
        }
    }
    let mut out = Vec::new();
    for (first, last) in clusters(&ops) {
        let mut lead = 0;
        if first > 0 {
            if let DiffOp::Equal { len, .. } = &ops[first - 1] {
                lead = (*len).min(CONTEXT);
            }
        }
        let (o0, n0) = at[first];
        let mut hunk = Hunk {
            old_start: (o0 - lead + 1) as u32,
            new_start: (n0 - lead + 1) as u32,
            lines: Vec::new(),
        };
        for k in 0..lead {
            hunk.lines.push((DiffLine::Context, trim_eol(a[o0 - lead + k])));
        }
        for op in &ops[first..=last] {
            match op {
                DiffOp::Equal { old_index, len, .. } => {
                    for k in 0..*len {
                        hunk.lines.push((DiffLine::Context, trim_eol(a[old_index + k])));
                    }
                }
                DiffOp::Delete { old_index, len } => {
                    for k in 0..*len {
                        hunk.lines.push((DiffLine::Removed, trim_eol(a[old_index + k])));
                    }
                }
                DiffOp::Insert { new_index, len } => {
                    for k in 0..*len {
                        hunk.lines.push((DiffLine::Added, trim_eol(b[new_index + k])));
                    }
                }
            }
        }
        if let Some(DiffOp::Equal { old_index, len, .. }) = ops.get(last + 1) {
            for k in 0..(*len).min(CONTEXT) {
                hunk.lines.push((DiffLine::Context, trim_eol(a[old_index + k])));
            }
        }
        out.push(hunk);
    }
    out
}

pub fn diff(path: &str, old: &str, new: &str) -> Diff {
    Diff { path: path.to_string(), hunks: hunks(old, new) }
}

/// `old` with only the hunks in `selected` applied (indexes into `hunks`).
pub fn apply_selected(old: &str, new: &str, selected: &[usize]) -> String {
    let ops = makepad_git::diff_lines(old, new);
    let (a, b) = (lines_of(old), lines_of(new));
    let groups = clusters(&ops);
    let mut out = String::with_capacity(old.len());
    for (i, op) in ops.iter().enumerate() {
        let hunk = groups.iter().position(|&(f, l)| i >= f && i <= l);
        let take = hunk.is_some_and(|h| selected.contains(&h));
        match op {
            DiffOp::Equal { old_index, len, .. } => {
                for k in 0..*len {
                    out.push_str(a[old_index + k]);
                }
            }
            DiffOp::Delete { old_index, len } => {
                if !take {
                    for k in 0..*len {
                        out.push_str(a[old_index + k]);
                    }
                }
            }
            DiffOp::Insert { new_index, len } => {
                if take {
                    for k in 0..*len {
                        out.push_str(b[new_index + k]);
                    }
                }
            }
        }
    }
    out
}

/// (added, removed) line counts.
pub fn counts(d: &Diff) -> (usize, usize) {
    let mut add = 0;
    let mut del = 0;
    for h in &d.hunks {
        for (k, _) in &h.lines {
            match k {
                DiffLine::Added => add += 1,
                DiffLine::Removed => del += 1,
                DiffLine::Context => {}
            }
        }
    }
    (add, del)
}

/// The diff an edit tool call would make, so the person can pick hunks
/// before anything is written. `None` when the call cannot be previewed.
pub fn preview_edit(root: &std::path::Path, name: &str, args: &[(String, String)]) -> Option<Diff> {
    let arg = |keys: &[&str]| keys.iter().find_map(|k| args.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()));
    let path = arg(&["path", "file_path"])?;
    let rel = std::path::Path::new(path);
    if rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return None;
    }
    let abs = if rel.is_absolute() { rel.to_path_buf() } else { root.join(rel) };
    let shown = abs.strip_prefix(root).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| path.to_string());
    match name {
        "edit_file" => {
            let before = std::fs::read_to_string(&abs).ok()?;
            let all = arg(&["replace_all"]).is_some_and(|v| v == "true");
            let after = crate::tools::replace_unique(&before, arg(&["old_string"]).unwrap_or(""), arg(&["new_string"]).unwrap_or(""), all).ok()?;
            Some(diff(&shown, &before, &after))
        }
        "write_file" => {
            let before = std::fs::read_to_string(&abs).unwrap_or_default();
            let mut after = arg(&["content"]).unwrap_or("").to_string();
            if !after.ends_with('\n') {
                after.push('\n');
            }
            Some(diff(&shown, &before, &after))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn one_change_has_context_and_positions() {
        let old = numbered(20);
        let new = old.replace("line 10\n", "ten\n");
        let h = hunks(&old, &new);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].old_start, 7);
        assert_eq!(h[0].new_start, 7);
        let kinds: Vec<_> = h[0].lines.iter().map(|l| l.0).collect();
        assert_eq!(kinds.iter().filter(|k| **k == DiffLine::Context).count(), 6);
        assert!(h[0].lines.contains(&(DiffLine::Removed, "line 10".into())));
        assert!(h[0].lines.contains(&(DiffLine::Added, "ten".into())));
    }

    #[test]
    fn far_apart_changes_are_two_hunks_and_apply_subset() {
        let old = numbered(40);
        let new = old.replace("line 3\n", "three\n").replace("line 35\n", "thirty-five\n");
        let h = hunks(&old, &new);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].old_start, 1);
        let only_second = apply_selected(&old, &new, &[1]);
        assert!(only_second.contains("line 3\n") && only_second.contains("thirty-five\n"));
        assert!(!only_second.contains("three\n"));
        assert_eq!(apply_selected(&old, &new, &[0, 1]), new);
        assert_eq!(apply_selected(&old, &new, &[]), old);
    }

    #[test]
    fn change_at_file_start_and_end() {
        let old = numbered(5);
        let new = format!("zero\n{}", old.replace("line 5\n", "five\n"));
        let h = hunks(&old, &new);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].old_start, 1);
        assert_eq!(apply_selected(&old, &new, &[0]), new);
    }

    #[test]
    fn new_file_is_one_added_hunk() {
        let h = hunks("", "a\nb\n");
        assert_eq!(h.len(), 1);
        assert_eq!(counts(&Diff { path: "x".into(), hunks: h }), (2, 0));
    }

    #[test]
    fn crlf_survives_partial_apply() {
        let old = "a\r\nb\r\nc\r\n";
        let new = "a\r\nB\r\nc\r\n";
        assert_eq!(apply_selected(old, new, &[0]), new);
        assert_eq!(apply_selected(old, new, &[]), old);
    }
}
