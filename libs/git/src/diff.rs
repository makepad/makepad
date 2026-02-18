use crate::error::GitError;
use crate::oid::ObjectId;
use crate::tree::{Tree, TreeEntry};

/// A single diff operation on lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp {
    Equal {
        old_index: usize,
        new_index: usize,
        len: usize,
    },
    Insert {
        new_index: usize,
        len: usize,
    },
    Delete {
        old_index: usize,
        len: usize,
    },
}

/// A diff between two files (blobs).
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub ops: Vec<DiffOp>,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

/// A change detected in a tree diff.
#[derive(Debug, Clone)]
pub enum TreeChange {
    Added {
        path: String,
        oid: ObjectId,
        mode: u32,
    },
    Deleted {
        path: String,
        oid: ObjectId,
        mode: u32,
    },
    Modified {
        path: String,
        old_oid: ObjectId,
        new_oid: ObjectId,
        old_mode: u32,
        new_mode: u32,
    },
}

/// Myers diff algorithm on line-split text.
///
/// Returns a list of DiffOps describing how to transform `old` into `new`.
pub fn diff_lines(old_text: &str, new_text: &str) -> Vec<DiffOp> {
    let old_lines: Vec<&str> = split_lines(old_text);
    let new_lines: Vec<&str> = split_lines(new_text);
    let n = old_lines.len();
    let m = new_lines.len();

    if n == 0 && m == 0 {
        return vec![];
    }
    if n == 0 {
        return vec![DiffOp::Insert {
            new_index: 0,
            len: m,
        }];
    }
    if m == 0 {
        return vec![DiffOp::Delete {
            old_index: 0,
            len: n,
        }];
    }

    // Myers algorithm - find shortest edit script
    let max_d = n + m;
    // v[k] = furthest reaching x on diagonal k
    // We offset k by max_d so indices are non-negative
    let v_size = 2 * max_d + 1;
    let mut v = vec![0i64; v_size];
    let mut trace: Vec<Vec<i64>> = Vec::new();

    let offset = max_d as i64;

    'outer: for d in 0..=(max_d as i64) {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let ki = (k + offset) as usize;
            let x: i64;
            if k == -d || (k != d && v[ki - 1] < v[ki + 1]) {
                x = v[ki + 1]; // move down
            } else {
                x = v[ki - 1] + 1; // move right
            }
            let mut x = x;
            let mut y = x - k;

            // Follow diagonal (matching lines)
            while (x as usize) < n
                && (y as usize) < m
                && old_lines[x as usize] == new_lines[y as usize]
            {
                x += 1;
                y += 1;
            }

            v[ki] = x;

            if (x as usize) >= n && (y as usize) >= m {
                break 'outer;
            }
            k += 2;
        }
    }

    // Backtrack to find the actual edit script
    let edits = backtrack(&trace, n, m, offset);
    compress_edits(&edits, n, m)
}

/// Represent individual edit steps
#[derive(Debug, Clone, Copy, PartialEq)]
enum Edit {
    Keep,   // diagonal move
    Insert, // down move
    Delete, // right move
}

fn backtrack(trace: &[Vec<i64>], n: usize, m: usize, offset: i64) -> Vec<Edit> {
    let mut x = n as i64;
    let mut y = m as i64;
    let mut edits = Vec::new();

    for d in (0..trace.len()).rev() {
        let v = &trace[d];
        let d = d as i64;
        let k = x - y;
        let ki = (k + offset) as usize;

        let prev_k;
        if k == -d || (k != d && v[ki - 1] < v[ki + 1]) {
            prev_k = k + 1; // came from above (insert)
        } else {
            prev_k = k - 1; // came from left (delete)
        }

        let prev_ki = (prev_k + offset) as usize;
        let prev_x = v[prev_ki];
        let prev_y = prev_x - prev_k;

        // Diagonal moves (matches)
        while x > prev_x && y > prev_y {
            edits.push(Edit::Keep);
            x -= 1;
            y -= 1;
        }

        if d > 0 {
            if prev_k == k + 1 {
                edits.push(Edit::Insert);
                y -= 1;
            } else {
                edits.push(Edit::Delete);
                x -= 1;
            }
        }
    }

    edits.reverse();
    edits
}

fn compress_edits(edits: &[Edit], _n: usize, _m: usize) -> Vec<DiffOp> {
    let mut ops = Vec::new();
    let mut old_idx = 0usize;
    let mut new_idx = 0usize;
    let mut i = 0;

    while i < edits.len() {
        match edits[i] {
            Edit::Keep => {
                let start_old = old_idx;
                let start_new = new_idx;
                let mut len = 0;
                while i < edits.len() && edits[i] == Edit::Keep {
                    len += 1;
                    old_idx += 1;
                    new_idx += 1;
                    i += 1;
                }
                ops.push(DiffOp::Equal {
                    old_index: start_old,
                    new_index: start_new,
                    len,
                });
            }
            Edit::Delete => {
                let start = old_idx;
                let mut len = 0;
                while i < edits.len() && edits[i] == Edit::Delete {
                    len += 1;
                    old_idx += 1;
                    i += 1;
                }
                ops.push(DiffOp::Delete {
                    old_index: start,
                    len,
                });
            }
            Edit::Insert => {
                let start = new_idx;
                let mut len = 0;
                while i < edits.len() && edits[i] == Edit::Insert {
                    len += 1;
                    new_idx += 1;
                    i += 1;
                }
                ops.push(DiffOp::Insert {
                    new_index: start,
                    len,
                });
            }
        }
    }
    ops
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![];
    }
    text.lines().collect()
}

/// Diff two blob byte arrays as text, returning a FileDiff.
pub fn diff_blobs(
    old_data: &[u8],
    new_data: &[u8],
    old_path: Option<String>,
    new_path: Option<String>,
    old_oid: Option<ObjectId>,
    new_oid: Option<ObjectId>,
) -> FileDiff {
    let old_text = String::from_utf8_lossy(old_data);
    let new_text = String::from_utf8_lossy(new_data);

    let ops = diff_lines(&old_text, &new_text);

    FileDiff {
        old_path,
        new_path,
        old_oid,
        new_oid,
        ops,
        old_lines: split_lines(&old_text)
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
        new_lines: split_lines(&new_text)
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// Compare two trees recursively and return a list of changes.
/// `read_tree_fn` is called to read sub-trees as needed.
pub fn diff_trees(
    old_tree: &Tree,
    new_tree: &Tree,
    prefix: &str,
    read_tree: &mut dyn FnMut(&ObjectId) -> Result<Tree, GitError>,
) -> Result<Vec<TreeChange>, GitError> {
    let mut changes = Vec::new();

    // Build maps of name -> entry for both trees
    let old_map: std::collections::HashMap<&str, &TreeEntry> = old_tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();
    let new_map: std::collections::HashMap<&str, &TreeEntry> = new_tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();

    // Entries in old but not in new (deleted)
    for (name, old_entry) in &old_map {
        if !new_map.contains_key(name) {
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}{}", prefix, name)
            };
            if old_entry.is_tree() {
                // Recursively list all files under deleted directory
                let sub_tree = read_tree(&old_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                collect_tree_entries(&sub_tree, &sub_prefix, read_tree, &mut |p, oid, mode| {
                    changes.push(TreeChange::Deleted { path: p, oid, mode });
                })?;
            } else {
                changes.push(TreeChange::Deleted {
                    path,
                    oid: old_entry.oid,
                    mode: old_entry.mode,
                });
            }
        }
    }

    // Entries in new but not in old (added)
    for (name, new_entry) in &new_map {
        if !old_map.contains_key(name) {
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}{}", prefix, name)
            };
            if new_entry.is_tree() {
                let sub_tree = read_tree(&new_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                collect_tree_entries(&sub_tree, &sub_prefix, read_tree, &mut |p, oid, mode| {
                    changes.push(TreeChange::Added { path: p, oid, mode });
                })?;
            } else {
                changes.push(TreeChange::Added {
                    path,
                    oid: new_entry.oid,
                    mode: new_entry.mode,
                });
            }
        }
    }

    // Entries in both (check for modifications)
    for (name, old_entry) in &old_map {
        if let Some(new_entry) = new_map.get(name) {
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}{}", prefix, name)
            };

            if old_entry.oid == new_entry.oid && old_entry.mode == new_entry.mode {
                continue; // identical
            }

            if old_entry.is_tree() && new_entry.is_tree() {
                // Both are trees — recurse
                let old_sub = read_tree(&old_entry.oid)?;
                let new_sub = read_tree(&new_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                let sub_changes = diff_trees(&old_sub, &new_sub, &sub_prefix, read_tree)?;
                changes.extend(sub_changes);
            } else if old_entry.is_tree() && !new_entry.is_tree() {
                // Tree replaced by file: delete all tree contents, add file
                let sub_tree = read_tree(&old_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                collect_tree_entries(&sub_tree, &sub_prefix, read_tree, &mut |p, oid, mode| {
                    changes.push(TreeChange::Deleted { path: p, oid, mode });
                })?;
                changes.push(TreeChange::Added {
                    path,
                    oid: new_entry.oid,
                    mode: new_entry.mode,
                });
            } else if !old_entry.is_tree() && new_entry.is_tree() {
                // File replaced by tree: delete file, add all tree contents
                changes.push(TreeChange::Deleted {
                    path: path.clone(),
                    oid: old_entry.oid,
                    mode: old_entry.mode,
                });
                let sub_tree = read_tree(&new_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                collect_tree_entries(&sub_tree, &sub_prefix, read_tree, &mut |p, oid, mode| {
                    changes.push(TreeChange::Added { path: p, oid, mode });
                })?;
            } else {
                // Both are files, content or mode changed
                changes.push(TreeChange::Modified {
                    path,
                    old_oid: old_entry.oid,
                    new_oid: new_entry.oid,
                    old_mode: old_entry.mode,
                    new_mode: new_entry.mode,
                });
            }
        }
    }

    changes.sort_by(|a, b| {
        let pa = match a {
            TreeChange::Added { path, .. } => path,
            TreeChange::Deleted { path, .. } => path,
            TreeChange::Modified { path, .. } => path,
        };
        let pb = match b {
            TreeChange::Added { path, .. } => path,
            TreeChange::Deleted { path, .. } => path,
            TreeChange::Modified { path, .. } => path,
        };
        pa.cmp(pb)
    });

    Ok(changes)
}

/// Recursively collect all file entries in a tree.
fn collect_tree_entries(
    tree: &Tree,
    prefix: &str,
    read_tree: &mut dyn FnMut(&ObjectId) -> Result<Tree, GitError>,
    emit: &mut dyn FnMut(String, ObjectId, u32),
) -> Result<(), GitError> {
    for entry in &tree.entries {
        let path = format!("{}{}", prefix, entry.name);
        if entry.is_tree() {
            let sub = read_tree(&entry.oid)?;
            let sub_prefix = format!("{}/", path);
            collect_tree_entries(&sub, &sub_prefix, read_tree, emit)?;
        } else {
            emit(path, entry.oid, entry.mode);
        }
    }
    Ok(())
}

/// Generate a unified diff string from a FileDiff.
pub fn format_unified_diff(diff: &FileDiff, context_lines: usize) -> String {
    let mut output = String::new();

    let old_path = diff.old_path.as_deref().unwrap_or("/dev/null");
    let new_path = diff.new_path.as_deref().unwrap_or("/dev/null");
    output.push_str(&format!("--- a/{}\n", old_path));
    output.push_str(&format!("+++ b/{}\n", new_path));

    // Convert ops into hunks with context
    let hunks = build_hunks(&diff.ops, &diff.old_lines, &diff.new_lines, context_lines);
    for hunk in &hunks {
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start + 1,
            hunk.old_count,
            hunk.new_start + 1,
            hunk.new_count
        ));
        for line in &hunk.lines {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<String>,
}

fn build_hunks(
    ops: &[DiffOp],
    old_lines: &[String],
    new_lines: &[String],
    context: usize,
) -> Vec<Hunk> {
    // First, expand ops into individual line edits
    let mut line_edits: Vec<(char, &str)> = Vec::new(); // (' ', '+', '-')
    let mut old_idx = 0;
    let mut new_idx = 0;

    for op in ops {
        match op {
            DiffOp::Equal { len, .. } => {
                for i in 0..*len {
                    line_edits.push((' ', &old_lines[old_idx + i]));
                }
                old_idx += len;
                new_idx += len;
            }
            DiffOp::Delete { len, .. } => {
                for i in 0..*len {
                    line_edits.push(('-', &old_lines[old_idx + i]));
                }
                old_idx += len;
            }
            DiffOp::Insert { len, .. } => {
                for i in 0..*len {
                    line_edits.push(('+', &new_lines[new_idx + i]));
                }
                new_idx += len;
            }
        }
    }

    if line_edits.is_empty() {
        return vec![];
    }

    // Find ranges of changes and group with context
    let mut change_positions: Vec<usize> = Vec::new();
    for (i, (kind, _)) in line_edits.iter().enumerate() {
        if *kind != ' ' {
            change_positions.push(i);
        }
    }

    if change_positions.is_empty() {
        return vec![];
    }

    // Group changes that overlap when context is added
    let mut groups: Vec<(usize, usize)> = Vec::new(); // (start, end) in line_edits
    let mut group_start = change_positions[0].saturating_sub(context);
    let mut group_end = (change_positions[0] + context + 1).min(line_edits.len());

    for &pos in &change_positions[1..] {
        let new_start = pos.saturating_sub(context);
        let new_end = (pos + context + 1).min(line_edits.len());
        if new_start <= group_end {
            group_end = new_end;
        } else {
            groups.push((group_start, group_end));
            group_start = new_start;
            group_end = new_end;
        }
    }
    groups.push((group_start, group_end));

    // Build hunks from groups
    let mut hunks = Vec::new();
    for (start, end) in groups {
        let mut hunk_lines = Vec::new();
        let mut old_count = 0;
        let mut new_count = 0;

        // Calculate old_start and new_start by counting through line_edits
        let mut oi = 0;
        let mut ni = 0;
        for i in 0..start {
            match line_edits[i].0 {
                ' ' => {
                    oi += 1;
                    ni += 1;
                }
                '-' => {
                    oi += 1;
                }
                '+' => {
                    ni += 1;
                }
                _ => {}
            }
        }
        let old_start = oi;
        let new_start = ni;

        for i in start..end {
            let (kind, text) = &line_edits[i];
            hunk_lines.push(format!("{}{}", kind, text));
            match kind {
                ' ' => {
                    old_count += 1;
                    new_count += 1;
                }
                '-' => {
                    old_count += 1;
                }
                '+' => {
                    new_count += 1;
                }
                _ => {}
            }
        }

        hunks.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines: hunk_lines,
        });
    }

    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_identical() {
        let ops = diff_lines("hello\nworld\n", "hello\nworld\n");
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], DiffOp::Equal { len: 2, .. }));
    }

    #[test]
    fn test_diff_insert() {
        let ops = diff_lines("a\nc\n", "a\nb\nc\n");
        // Should have: Equal(a), Insert(b), Equal(c)
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], DiffOp::Equal { len: 1, .. }));
        assert!(matches!(ops[1], DiffOp::Insert { len: 1, .. }));
        assert!(matches!(ops[2], DiffOp::Equal { len: 1, .. }));
    }

    #[test]
    fn test_diff_delete() {
        let ops = diff_lines("a\nb\nc\n", "a\nc\n");
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], DiffOp::Equal { len: 1, .. }));
        assert!(matches!(ops[1], DiffOp::Delete { len: 1, .. }));
        assert!(matches!(ops[2], DiffOp::Equal { len: 1, .. }));
    }

    #[test]
    fn test_diff_replace() {
        let ops = diff_lines("a\nb\nc\n", "a\nX\nc\n");
        // Should be: Equal(a), Delete(b), Insert(X), Equal(c)
        assert_eq!(ops.len(), 4);
        assert!(matches!(ops[0], DiffOp::Equal { len: 1, .. }));
        assert!(matches!(ops[1], DiffOp::Delete { len: 1, .. }));
        assert!(matches!(ops[2], DiffOp::Insert { len: 1, .. }));
        assert!(matches!(ops[3], DiffOp::Equal { len: 1, .. }));
    }

    #[test]
    fn test_diff_empty_old() {
        let ops = diff_lines("", "a\nb\n");
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], DiffOp::Insert { len: 2, .. }));
    }

    #[test]
    fn test_diff_empty_new() {
        let ops = diff_lines("a\nb\n", "");
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], DiffOp::Delete { len: 2, .. }));
    }

    #[test]
    fn test_diff_both_empty() {
        let ops = diff_lines("", "");
        assert!(ops.is_empty());
    }
}
