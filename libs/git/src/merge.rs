use std::collections::{HashMap, HashSet, VecDeque};

use crate::commit::Commit;
use crate::diff::{diff_lines, DiffOp};
use crate::error::GitError;
use crate::oid::ObjectId;
use crate::tree::Tree;

/// Result of a three-way file merge.
#[derive(Debug, Clone)]
pub enum MergeResult {
    /// Clean merge — no conflicts.
    Clean(String),
    /// Conflict — contains conflict markers.
    Conflict(String),
}

impl MergeResult {
    pub fn has_conflict(&self) -> bool {
        matches!(self, MergeResult::Conflict(_))
    }

    pub fn content(&self) -> &str {
        match self {
            MergeResult::Clean(s) => s,
            MergeResult::Conflict(s) => s,
        }
    }
}

/// A single file-level merge decision.
#[derive(Debug, Clone)]
pub enum TreeMergeEntry {
    /// Take this content (no conflict).
    Resolved {
        path: String,
        oid: ObjectId,
        mode: u32,
    },
    /// Both sides modified — needs content merge.
    BothModified {
        path: String,
        base_oid: ObjectId,
        ours_oid: ObjectId,
        theirs_oid: ObjectId,
        mode: u32,
    },
    /// File added on both sides with different content.
    AddAdd {
        path: String,
        ours_oid: ObjectId,
        theirs_oid: ObjectId,
        mode: u32,
    },
    /// Deleted on one side, modified on other — conflict.
    DeleteModify {
        path: String,
        surviving_oid: ObjectId,
        mode: u32,
        deleted_by_ours: bool,
    },
}

/// Find the merge base (lowest common ancestor) of two commits.
///
/// Uses BFS from both sides simultaneously. The merge base is the first
/// commit reachable from both.
pub fn find_merge_base(
    oid_a: &ObjectId,
    oid_b: &ObjectId,
    read_commit: &mut dyn FnMut(&ObjectId) -> Result<Commit, GitError>,
) -> Result<Option<ObjectId>, GitError> {
    if oid_a == oid_b {
        return Ok(Some(*oid_a));
    }

    let mut ancestors_a: HashSet<ObjectId> = HashSet::new();
    let mut ancestors_b: HashSet<ObjectId> = HashSet::new();
    let mut queue_a: VecDeque<ObjectId> = VecDeque::new();
    let mut queue_b: VecDeque<ObjectId> = VecDeque::new();

    ancestors_a.insert(*oid_a);
    ancestors_b.insert(*oid_b);
    queue_a.push_back(*oid_a);
    queue_b.push_back(*oid_b);

    // Alternate BFS expansion from both sides
    loop {
        let done_a = queue_a.is_empty();
        let done_b = queue_b.is_empty();

        if done_a && done_b {
            return Ok(None); // No common ancestor
        }

        // Expand A
        if !done_a {
            let oid = queue_a.pop_front().unwrap();
            if ancestors_b.contains(&oid) {
                return Ok(Some(oid));
            }
            let commit = read_commit(&oid)?;
            for parent in &commit.parents {
                if ancestors_a.insert(*parent) {
                    if ancestors_b.contains(parent) {
                        return Ok(Some(*parent));
                    }
                    queue_a.push_back(*parent);
                }
            }
        }

        // Expand B
        if !done_b {
            let oid = queue_b.pop_front().unwrap();
            if ancestors_a.contains(&oid) {
                return Ok(Some(oid));
            }
            let commit = read_commit(&oid)?;
            for parent in &commit.parents {
                if ancestors_b.insert(*parent) {
                    if ancestors_a.contains(parent) {
                        return Ok(Some(*parent));
                    }
                    queue_b.push_back(*parent);
                }
            }
        }
    }
}

/// Three-way merge of file content (line-based).
///
/// Given the base version and two derived versions, produces a merged result.
/// If there are conflicting changes (both sides changed the same lines differently),
/// conflict markers are inserted.
///
/// Uses a region-based approach: convert each diff into a list of regions on the
/// base text, then walk both region lists in parallel to detect overlaps.
pub fn merge3_text(base: &str, ours: &str, theirs: &str) -> MergeResult {
    let base_lines: Vec<&str> = if base.is_empty() {
        vec![]
    } else {
        base.lines().collect()
    };
    let ours_lines: Vec<&str> = if ours.is_empty() {
        vec![]
    } else {
        ours.lines().collect()
    };
    let theirs_lines: Vec<&str> = if theirs.is_empty() {
        vec![]
    } else {
        theirs.lines().collect()
    };

    let diff_ours = diff_lines(base, ours);
    let diff_theirs = diff_lines(base, theirs);

    let regions_ours = diff_to_regions(&diff_ours, &ours_lines);
    let regions_theirs = diff_to_regions(&diff_theirs, &theirs_lines);

    let mut result = String::new();
    let mut has_conflict = false;
    let mut base_pos = 0;
    let mut oi = 0; // index into regions_ours
    let mut ti = 0; // index into regions_theirs

    while base_pos < base_lines.len() || oi < regions_ours.len() || ti < regions_theirs.len() {
        // Get the next region from each side (if it starts at or before current base_pos)
        let our_region = if oi < regions_ours.len() {
            Some(&regions_ours[oi])
        } else {
            None
        };
        let their_region = if ti < regions_theirs.len() {
            Some(&regions_theirs[ti])
        } else {
            None
        };

        match (our_region, their_region) {
            (Some(or), Some(tr)) => {
                let o_start = or.base_start;
                let t_start = tr.base_start;

                // Emit unchanged base lines up to the earliest region
                let next_change = o_start.min(t_start);
                while base_pos < next_change && base_pos < base_lines.len() {
                    result.push_str(base_lines[base_pos]);
                    result.push('\n');
                    base_pos += 1;
                }

                if o_start == t_start && or.base_len == tr.base_len {
                    // Both sides changed the same base region
                    if or.new_lines == tr.new_lines {
                        // Same change — take it
                        for line in &or.new_lines {
                            result.push_str(line);
                            result.push('\n');
                        }
                    } else {
                        // CONFLICT
                        has_conflict = true;
                        result.push_str("<<<<<<< ours\n");
                        for line in &or.new_lines {
                            result.push_str(line);
                            result.push('\n');
                        }
                        result.push_str("=======\n");
                        for line in &tr.new_lines {
                            result.push_str(line);
                            result.push('\n');
                        }
                        result.push_str(">>>>>>> theirs\n");
                    }
                    base_pos = o_start + or.base_len;
                    oi += 1;
                    ti += 1;
                } else if o_start + or.base_len <= t_start {
                    // Our region ends before theirs starts — no overlap
                    for line in &or.new_lines {
                        result.push_str(line);
                        result.push('\n');
                    }
                    base_pos = o_start + or.base_len;
                    oi += 1;
                } else if t_start + tr.base_len <= o_start {
                    // Their region ends before ours starts — no overlap
                    for line in &tr.new_lines {
                        result.push_str(line);
                        result.push('\n');
                    }
                    base_pos = t_start + tr.base_len;
                    ti += 1;
                } else {
                    // Overlapping regions — conflict
                    has_conflict = true;
                    result.push_str("<<<<<<< ours\n");
                    for line in &or.new_lines {
                        result.push_str(line);
                        result.push('\n');
                    }
                    result.push_str("=======\n");
                    for line in &tr.new_lines {
                        result.push_str(line);
                        result.push('\n');
                    }
                    result.push_str(">>>>>>> theirs\n");
                    base_pos = (o_start + or.base_len).max(t_start + tr.base_len);
                    oi += 1;
                    ti += 1;
                }
            }
            (Some(or), None) => {
                let o_start = or.base_start;
                while base_pos < o_start && base_pos < base_lines.len() {
                    result.push_str(base_lines[base_pos]);
                    result.push('\n');
                    base_pos += 1;
                }
                for line in &or.new_lines {
                    result.push_str(line);
                    result.push('\n');
                }
                base_pos = o_start + or.base_len;
                oi += 1;
            }
            (None, Some(tr)) => {
                let t_start = tr.base_start;
                while base_pos < t_start && base_pos < base_lines.len() {
                    result.push_str(base_lines[base_pos]);
                    result.push('\n');
                    base_pos += 1;
                }
                for line in &tr.new_lines {
                    result.push_str(line);
                    result.push('\n');
                }
                base_pos = t_start + tr.base_len;
                ti += 1;
            }
            (None, None) => {
                // Emit remaining base lines
                while base_pos < base_lines.len() {
                    result.push_str(base_lines[base_pos]);
                    result.push('\n');
                    base_pos += 1;
                }
                break;
            }
        }
    }

    if has_conflict {
        MergeResult::Conflict(result)
    } else {
        MergeResult::Clean(result)
    }
}

/// A region describes a change: it replaces base_lines[base_start..base_start+base_len]
/// with new_lines.
#[derive(Debug)]
struct Region {
    base_start: usize,
    base_len: usize,
    new_lines: Vec<String>,
}

/// Convert diff ops + new file lines into a list of change regions.
fn diff_to_regions(ops: &[DiffOp], new_lines: &[&str]) -> Vec<Region> {
    let mut regions = Vec::new();
    let mut base_pos = 0;
    let mut new_pos = 0;
    let mut i = 0;

    while i < ops.len() {
        match &ops[i] {
            DiffOp::Equal { len, .. } => {
                base_pos += len;
                new_pos += len;
                i += 1;
            }
            DiffOp::Delete { len, .. } => {
                // Check if followed by Insert (= Replace)
                let del_start = base_pos;
                let del_len = *len;
                base_pos += del_len;
                i += 1;

                if i < ops.len() {
                    if let DiffOp::Insert { len: ins_len, .. } = &ops[i] {
                        let replacement: Vec<String> = new_lines[new_pos..new_pos + ins_len]
                            .iter()
                            .map(|s| s.to_string())
                            .collect();
                        regions.push(Region {
                            base_start: del_start,
                            base_len: del_len,
                            new_lines: replacement,
                        });
                        new_pos += ins_len;
                        i += 1;
                        continue;
                    }
                }
                // Pure delete
                regions.push(Region {
                    base_start: del_start,
                    base_len: del_len,
                    new_lines: vec![],
                });
            }
            DiffOp::Insert { len, .. } => {
                let insertion: Vec<String> = new_lines[new_pos..new_pos + len]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                regions.push(Region {
                    base_start: base_pos,
                    base_len: 0,
                    new_lines: insertion,
                });
                new_pos += len;
                i += 1;
            }
        }
    }

    regions
}

/// Three-way merge of two trees against a common base.
///
/// Returns a list of merge decisions for each file.
pub fn merge_trees(
    base_files: &HashMap<String, (ObjectId, u32)>,
    ours_files: &HashMap<String, (ObjectId, u32)>,
    theirs_files: &HashMap<String, (ObjectId, u32)>,
) -> Vec<TreeMergeEntry> {
    let mut result = Vec::new();

    // Collect all paths
    let mut all_paths: HashSet<&str> = HashSet::new();
    for path in base_files.keys() {
        all_paths.insert(path.as_str());
    }
    for path in ours_files.keys() {
        all_paths.insert(path.as_str());
    }
    for path in theirs_files.keys() {
        all_paths.insert(path.as_str());
    }

    for path in all_paths {
        let base = base_files.get(path);
        let ours = ours_files.get(path);
        let theirs = theirs_files.get(path);

        match (base, ours, theirs) {
            // Unchanged in both
            (Some((b_oid, _)), Some((o_oid, o_mode)), Some((t_oid, _)))
                if b_oid == o_oid && b_oid == t_oid =>
            {
                result.push(TreeMergeEntry::Resolved {
                    path: path.to_string(),
                    oid: *o_oid,
                    mode: *o_mode,
                });
            }
            // Only ours changed
            (Some((b_oid, _)), Some((o_oid, o_mode)), Some((t_oid, _)))
                if b_oid == t_oid && b_oid != o_oid =>
            {
                result.push(TreeMergeEntry::Resolved {
                    path: path.to_string(),
                    oid: *o_oid,
                    mode: *o_mode,
                });
            }
            // Only theirs changed
            (Some((b_oid, _)), Some((o_oid, _)), Some((t_oid, t_mode)))
                if b_oid == o_oid && b_oid != t_oid =>
            {
                result.push(TreeMergeEntry::Resolved {
                    path: path.to_string(),
                    oid: *t_oid,
                    mode: *t_mode,
                });
            }
            // Both changed to same thing
            (Some(_), Some((o_oid, o_mode)), Some((t_oid, _))) if o_oid == t_oid => {
                result.push(TreeMergeEntry::Resolved {
                    path: path.to_string(),
                    oid: *o_oid,
                    mode: *o_mode,
                });
            }
            // Both changed differently — needs content merge
            (Some((b_oid, _)), Some((o_oid, o_mode)), Some((t_oid, _))) => {
                result.push(TreeMergeEntry::BothModified {
                    path: path.to_string(),
                    base_oid: *b_oid,
                    ours_oid: *o_oid,
                    theirs_oid: *t_oid,
                    mode: *o_mode,
                });
            }
            // Ours deleted, theirs unchanged
            (Some((b_oid, _)), None, Some((t_oid, _))) if b_oid == t_oid => {
                // Clean delete by ours
            }
            // Theirs deleted, ours unchanged
            (Some((b_oid, _)), Some((o_oid, _)), None) if b_oid == o_oid => {
                // Clean delete by theirs
            }
            // Ours deleted, theirs modified — conflict
            (Some(_), None, Some((t_oid, t_mode))) => {
                result.push(TreeMergeEntry::DeleteModify {
                    path: path.to_string(),
                    surviving_oid: *t_oid,
                    mode: *t_mode,
                    deleted_by_ours: true,
                });
            }
            // Theirs deleted, ours modified — conflict
            (Some(_), Some((o_oid, o_mode)), None) => {
                result.push(TreeMergeEntry::DeleteModify {
                    path: path.to_string(),
                    surviving_oid: *o_oid,
                    mode: *o_mode,
                    deleted_by_ours: false,
                });
            }
            // Both deleted
            (Some(_), None, None) => {
                // Both agree: delete
            }
            // Added only by ours
            (None, Some((o_oid, o_mode)), None) => {
                result.push(TreeMergeEntry::Resolved {
                    path: path.to_string(),
                    oid: *o_oid,
                    mode: *o_mode,
                });
            }
            // Added only by theirs
            (None, None, Some((t_oid, t_mode))) => {
                result.push(TreeMergeEntry::Resolved {
                    path: path.to_string(),
                    oid: *t_oid,
                    mode: *t_mode,
                });
            }
            // Added by both with same content
            (None, Some((o_oid, o_mode)), Some((t_oid, _))) if o_oid == t_oid => {
                result.push(TreeMergeEntry::Resolved {
                    path: path.to_string(),
                    oid: *o_oid,
                    mode: *o_mode,
                });
            }
            // Added by both with different content — conflict
            (None, Some((o_oid, o_mode)), Some((t_oid, _))) => {
                result.push(TreeMergeEntry::AddAdd {
                    path: path.to_string(),
                    ours_oid: *o_oid,
                    theirs_oid: *t_oid,
                    mode: *o_mode,
                });
            }
            // Not present anywhere (shouldn't happen)
            (None, None, None) => {}
        }
    }

    result.sort_by(|a, b| {
        let pa = match a {
            TreeMergeEntry::Resolved { path, .. } => path,
            TreeMergeEntry::BothModified { path, .. } => path,
            TreeMergeEntry::AddAdd { path, .. } => path,
            TreeMergeEntry::DeleteModify { path, .. } => path,
        };
        let pb = match b {
            TreeMergeEntry::Resolved { path, .. } => path,
            TreeMergeEntry::BothModified { path, .. } => path,
            TreeMergeEntry::AddAdd { path, .. } => path,
            TreeMergeEntry::DeleteModify { path, .. } => path,
        };
        pa.cmp(pb)
    });

    result
}

/// Flatten a tree into a map of path -> (OID, mode).
pub fn flatten_tree_with_mode(
    tree: &Tree,
    prefix: &str,
    read_tree: &mut dyn FnMut(&ObjectId) -> Result<Tree, GitError>,
) -> Result<HashMap<String, (ObjectId, u32)>, GitError> {
    let mut result = HashMap::new();

    for entry in &tree.entries {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{}{}", prefix, entry.name)
        };

        if entry.is_tree() {
            let sub_tree = read_tree(&entry.oid)?;
            let sub_prefix = format!("{}/", path);
            let sub = flatten_tree_with_mode(&sub_tree, &sub_prefix, read_tree)?;
            result.extend(sub);
        } else {
            result.insert(path, (entry.oid, entry.mode));
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_base_linear() {
        // A -> B -> C
        let a = ObjectId::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let b = ObjectId::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        let c = ObjectId::from_hex("cccccccccccccccccccccccccccccccccccccccc").unwrap();

        let commits: HashMap<ObjectId, Commit> = [
            (
                a,
                Commit {
                    tree: ObjectId::ZERO,
                    parents: vec![],
                    author: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 1,
                        tz_offset: "+0000".into(),
                    },
                    committer: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 1,
                        tz_offset: "+0000".into(),
                    },
                    message: "a".into(),
                },
            ),
            (
                b,
                Commit {
                    tree: ObjectId::ZERO,
                    parents: vec![a],
                    author: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 2,
                        tz_offset: "+0000".into(),
                    },
                    committer: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 2,
                        tz_offset: "+0000".into(),
                    },
                    message: "b".into(),
                },
            ),
            (
                c,
                Commit {
                    tree: ObjectId::ZERO,
                    parents: vec![b],
                    author: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 3,
                        tz_offset: "+0000".into(),
                    },
                    committer: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 3,
                        tz_offset: "+0000".into(),
                    },
                    message: "c".into(),
                },
            ),
        ]
        .into_iter()
        .collect();

        let base = find_merge_base(&b, &c, &mut |oid| {
            commits
                .get(oid)
                .cloned()
                .ok_or(GitError::ObjectNotFound(oid.to_hex()))
        })
        .unwrap();
        assert_eq!(base, Some(b));
    }

    #[test]
    fn test_merge_base_fork() {
        // A -> B (ours)
        // A -> C (theirs)
        let a = ObjectId::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let b = ObjectId::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        let c = ObjectId::from_hex("cccccccccccccccccccccccccccccccccccccccc").unwrap();

        let commits: HashMap<ObjectId, Commit> = [
            (
                a,
                Commit {
                    tree: ObjectId::ZERO,
                    parents: vec![],
                    author: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 1,
                        tz_offset: "+0000".into(),
                    },
                    committer: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 1,
                        tz_offset: "+0000".into(),
                    },
                    message: "a".into(),
                },
            ),
            (
                b,
                Commit {
                    tree: ObjectId::ZERO,
                    parents: vec![a],
                    author: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 2,
                        tz_offset: "+0000".into(),
                    },
                    committer: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 2,
                        tz_offset: "+0000".into(),
                    },
                    message: "b".into(),
                },
            ),
            (
                c,
                Commit {
                    tree: ObjectId::ZERO,
                    parents: vec![a],
                    author: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 2,
                        tz_offset: "+0000".into(),
                    },
                    committer: crate::commit::Signature {
                        name: "T".into(),
                        email: "t@t".into(),
                        timestamp: 2,
                        tz_offset: "+0000".into(),
                    },
                    message: "c".into(),
                },
            ),
        ]
        .into_iter()
        .collect();

        let base = find_merge_base(&b, &c, &mut |oid| {
            commits
                .get(oid)
                .cloned()
                .ok_or(GitError::ObjectNotFound(oid.to_hex()))
        })
        .unwrap();
        assert_eq!(base, Some(a));
    }

    #[test]
    fn test_merge3_no_conflict() {
        let base = "line1\nline2\nline3\n";
        let ours = "line1\nline2 modified\nline3\n";
        let theirs = "line1\nline2\nline3 modified\n";

        let result = merge3_text(base, ours, theirs);
        assert!(!result.has_conflict());
    }

    #[test]
    fn test_merge3_same_change() {
        // Both sides make the same change
        let base = "line1\nline2\nline3\n";
        let ours = "line1\nchanged\nline3\n";
        let theirs = "line1\nchanged\nline3\n";

        let result = merge3_text(base, ours, theirs);
        assert!(!result.has_conflict());
        assert!(result.content().contains("changed"));
    }

    #[test]
    fn test_merge_trees_clean() {
        let base_oid = ObjectId::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let ours_oid = ObjectId::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        let theirs_oid = ObjectId::from_hex("cccccccccccccccccccccccccccccccccccccccc").unwrap();

        let mut base = HashMap::new();
        base.insert("file1.txt".to_string(), (base_oid, 0o100644));
        base.insert("file2.txt".to_string(), (base_oid, 0o100644));

        let mut ours = HashMap::new();
        ours.insert("file1.txt".to_string(), (ours_oid, 0o100644)); // modified
        ours.insert("file2.txt".to_string(), (base_oid, 0o100644)); // unchanged

        let mut theirs = HashMap::new();
        theirs.insert("file1.txt".to_string(), (base_oid, 0o100644)); // unchanged
        theirs.insert("file2.txt".to_string(), (theirs_oid, 0o100644)); // modified

        let result = merge_trees(&base, &ours, &theirs);
        assert_eq!(result.len(), 2);

        // file1 should take ours, file2 should take theirs
        for entry in &result {
            match entry {
                TreeMergeEntry::Resolved { path, oid, .. } => {
                    if path == "file1.txt" {
                        assert_eq!(*oid, ours_oid);
                    } else if path == "file2.txt" {
                        assert_eq!(*oid, theirs_oid);
                    }
                }
                _ => panic!("expected all resolved"),
            }
        }
    }
}
