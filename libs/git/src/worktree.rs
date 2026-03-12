use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::error::GitError;
use crate::index::{Index, IndexEntry};
use crate::object::{write_loose_object, ObjectKind};
use crate::oid::{hash_object, ObjectId};
use crate::tree::Tree;

/// Status of a file in the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    /// In index, modified in worktree (unstaged change)
    Modified,
    /// In index, deleted from worktree
    Deleted,
    /// In worktree, not in index
    Untracked,
    /// Staged change: different from HEAD
    Staged,
    /// Staged for deletion (in HEAD but removed from index)
    StagedDeleted,
    /// New file staged (in index but not in HEAD)
    StagedNew,
}

/// A single entry in the status result.
#[derive(Debug, Clone)]
pub struct StatusEntry {
    pub path: String,
    pub status: FileStatus,
}

/// Full status of a working tree.
#[derive(Debug, Clone)]
pub struct Status {
    pub entries: Vec<StatusEntry>,
}

/// Options controlling status traversal behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatusOptions {
    pub skip_hidden: bool,
    pub skip_target_dirs: bool,
    pub skip_worktree_content_compare: bool,
}

#[derive(Debug, Clone)]
struct IgnoreRule {
    base_rel: String,
    pattern: String,
    negated: bool,
    dir_only: bool,
    anchored: bool,
    has_slash: bool,
}

#[derive(Debug, Default)]
struct IgnoreStack {
    rules: Vec<IgnoreRule>,
    repo_exclude_loaded: bool,
}

/// Compute full working tree status by comparing HEAD tree, index, and worktree.
///
/// - `head_files`: flat map of path -> OID from the HEAD commit's tree (recursively flattened).
///   Pass an empty map for the initial commit (no HEAD).
/// - `index`: the current index
/// - `workdir`: the working directory root
pub fn compute_status(
    head_files: &HashMap<String, ObjectId>,
    index: &Index,
    workdir: &Path,
) -> Result<Status, GitError> {
    compute_status_with_options(head_files, index, workdir, StatusOptions::default())
}

/// Compute full working tree status with traversal options.
pub fn compute_status_with_options(
    head_files: &HashMap<String, ObjectId>,
    index: &Index,
    workdir: &Path,
    options: StatusOptions,
) -> Result<Status, GitError> {
    let mut entries = Vec::new();

    // Build index map (only stage 0 entries)
    let index_map: HashMap<&str, &IndexEntry> = index
        .entries
        .iter()
        .filter(|e| e.stage() == 0)
        .map(|e| (e.path.as_str(), e))
        .collect();

    // 1. Compare index vs HEAD (staged changes)
    for (path, idx_entry) in &index_map {
        match head_files.get(*path) {
            Some(head_oid) => {
                if idx_entry.oid != *head_oid {
                    entries.push(StatusEntry {
                        path: path.to_string(),
                        status: FileStatus::Staged,
                    });
                }
            }
            None => {
                entries.push(StatusEntry {
                    path: path.to_string(),
                    status: FileStatus::StagedNew,
                });
            }
        }
    }

    // Files in HEAD but not in index (staged deletion)
    for path in head_files.keys() {
        if !index_map.contains_key(path.as_str()) {
            entries.push(StatusEntry {
                path: path.clone(),
                status: FileStatus::StagedDeleted,
            });
        }
    }

    // 2. Compare worktree vs index (unstaged changes)
    for (path, idx_entry) in &index_map {
        let file_path = workdir.join(path);
        if !file_path.exists() {
            // File deleted from worktree but still in index
            entries.push(StatusEntry {
                path: path.to_string(),
                status: FileStatus::Deleted,
            });
        } else {
            if options.skip_worktree_content_compare {
                continue;
            }
            // Quick stat check: if mtime/size match the index, skip content hashing
            let metadata = fs::metadata(&file_path)?;
            let stat_matches = metadata_mtime_sec(&metadata) == idx_entry.mtime_sec
                && metadata.len() as u32 == idx_entry.file_size;

            if !stat_matches {
                // Content may have changed — hash and compare
                let content = fs::read(&file_path)?;
                let worktree_oid = hash_object("blob", &content);
                if worktree_oid != idx_entry.oid {
                    entries.push(StatusEntry {
                        path: path.to_string(),
                        status: FileStatus::Modified,
                    });
                }
            }
        }
    }

    // 3. Untracked files
    let mut ignore_stack = IgnoreStack::default();
    collect_untracked(
        workdir,
        workdir,
        &index_map,
        &mut entries,
        options,
        &mut ignore_stack,
    )?;

    // Sort by path for deterministic output
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    // Deduplicate: if a file appears as both Staged and Modified, keep both
    // (this is how git status works: file can be both staged and have unstaged changes)

    Ok(Status { entries })
}

/// Compute working-tree-only status (without reading HEAD tree objects).
///
/// This reports:
/// - `Modified` and `Deleted` by comparing worktree vs index
/// - `Untracked` by scanning files not present in the index
///
/// It intentionally omits staged states because those require HEAD tree data.
pub fn compute_status_worktree_only(index: &Index, workdir: &Path) -> Result<Status, GitError> {
    compute_status_worktree_only_with_options(index, workdir, StatusOptions::default())
}

/// Compute working-tree-only status with traversal options.
pub fn compute_status_worktree_only_with_options(
    index: &Index,
    workdir: &Path,
    options: StatusOptions,
) -> Result<Status, GitError> {
    let mut entries = Vec::new();

    // Build index map (only stage 0 entries)
    let index_map: HashMap<&str, &IndexEntry> = index
        .entries
        .iter()
        .filter(|e| e.stage() == 0)
        .map(|e| (e.path.as_str(), e))
        .collect();

    // Compare worktree vs index (unstaged changes)
    for (path, idx_entry) in &index_map {
        let file_path = workdir.join(path);
        if !file_path.exists() {
            entries.push(StatusEntry {
                path: path.to_string(),
                status: FileStatus::Deleted,
            });
            continue;
        }

        if options.skip_worktree_content_compare {
            continue;
        }

        // Quick stat check: if mtime/size match the index, skip content hashing
        let metadata = fs::metadata(&file_path)?;
        let stat_matches = metadata_mtime_sec(&metadata) == idx_entry.mtime_sec
            && metadata.len() as u32 == idx_entry.file_size;

        if !stat_matches {
            let content = fs::read(&file_path)?;
            let worktree_oid = hash_object("blob", &content);
            if worktree_oid != idx_entry.oid {
                entries.push(StatusEntry {
                    path: path.to_string(),
                    status: FileStatus::Modified,
                });
            }
        }
    }

    // Untracked files
    let mut ignore_stack = IgnoreStack::default();
    collect_untracked(
        workdir,
        workdir,
        &index_map,
        &mut entries,
        options,
        &mut ignore_stack,
    )?;

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Status { entries })
}

/// Compute status for a single relative path.
///
/// This avoids scanning the full worktree and is intended for event-driven UIs.
pub fn compute_status_for_path_with_options(
    head_oid: Option<ObjectId>,
    index: &Index,
    workdir: &Path,
    path: &str,
    options: StatusOptions,
) -> Result<Option<FileStatus>, GitError> {
    let path = normalize_status_path(path);
    if path.is_empty() {
        return Ok(None);
    }

    let idx_entry = index
        .entries
        .iter()
        .find(|entry| entry.stage() == 0 && entry.path == path);

    let mut status = if let Some(idx_entry) = idx_entry {
        let staged = match head_oid {
            Some(oid) if idx_entry.oid != oid => Some(FileStatus::Staged),
            None => Some(FileStatus::StagedNew),
            _ => None,
        };
        let unstaged = worktree_vs_index_status_for_path(idx_entry, workdir, &path, options)?;
        // Keep staged state when both staged + unstaged are present.
        staged.or(unstaged)
    } else if head_oid.is_some() {
        Some(FileStatus::StagedDeleted)
    } else {
        None
    };

    if status.is_none() {
        status = compute_untracked_status_for_path(index, workdir, &path, options)?;
    }

    Ok(status)
}

/// Compute worktree-only status for a single relative path.
///
/// Unlike `compute_status_for_path_with_options`, this omits staged states.
pub fn compute_status_for_path_worktree_only_with_options(
    index: &Index,
    workdir: &Path,
    path: &str,
    options: StatusOptions,
) -> Result<Option<FileStatus>, GitError> {
    let path = normalize_status_path(path);
    if path.is_empty() {
        return Ok(None);
    }

    let idx_entry = index
        .entries
        .iter()
        .find(|entry| entry.stage() == 0 && entry.path == path);
    let mut status = None;
    if let Some(idx_entry) = idx_entry {
        status = worktree_vs_index_status_for_path(idx_entry, workdir, &path, options)?;
    }
    if status.is_none() {
        status = compute_untracked_status_for_path(index, workdir, &path, options)?;
    }
    Ok(status)
}

fn normalize_status_path(path: &str) -> String {
    let mut path = path.replace('\\', "/");
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped.to_string();
    }
    path.trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn worktree_vs_index_status_for_path(
    idx_entry: &IndexEntry,
    workdir: &Path,
    path: &str,
    options: StatusOptions,
) -> Result<Option<FileStatus>, GitError> {
    let file_path = workdir.join(path);
    if !file_path.exists() {
        return Ok(Some(FileStatus::Deleted));
    }
    if options.skip_worktree_content_compare {
        return Ok(None);
    }

    let metadata = fs::metadata(&file_path)?;
    let stat_matches = metadata_mtime_sec(&metadata) == idx_entry.mtime_sec
        && metadata.len() as u32 == idx_entry.file_size;
    if stat_matches {
        return Ok(None);
    }

    let content = fs::read(&file_path)?;
    let worktree_oid = hash_object("blob", &content);
    if worktree_oid != idx_entry.oid {
        Ok(Some(FileStatus::Modified))
    } else {
        Ok(None)
    }
}

fn compute_untracked_status_for_path(
    index: &Index,
    workdir: &Path,
    path: &str,
    options: StatusOptions,
) -> Result<Option<FileStatus>, GitError> {
    let file_path = workdir.join(path);
    let metadata = match fs::symlink_metadata(&file_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(GitError::Io(err)),
    };
    let file_type = metadata.file_type();
    let is_dir = file_type.is_dir();
    let is_file_like = file_type.is_file() || file_type.is_symlink();
    if !is_dir && !is_file_like {
        return Ok(None);
    }

    if options.skip_hidden && path.split('/').any(|segment| segment.starts_with('.')) {
        return Ok(None);
    }
    if options.skip_target_dirs && path.split('/').any(|segment| segment == "target") {
        return Ok(None);
    }

    if is_dir {
        let prefix = format!("{}/", path);
        if index
            .entries
            .iter()
            .any(|entry| entry.stage() == 0 && entry.path.starts_with(&prefix))
        {
            return Ok(None);
        }
    } else if index
        .entries
        .iter()
        .any(|entry| entry.stage() == 0 && entry.path == path)
    {
        return Ok(None);
    }

    if is_ignored_path(workdir, path, is_dir)? {
        return Ok(None);
    }

    Ok(Some(FileStatus::Untracked))
}

fn is_ignored_path(root: &Path, rel_path: &str, is_dir: bool) -> Result<bool, GitError> {
    let mut ignore_stack = IgnoreStack::default();
    ignore_stack.ensure_repo_exclude_loaded(root)?;

    let mut base_dir = root.to_path_buf();
    ignore_stack.push_ignore_file(root, &base_dir, &base_dir.join(".gitignore"))?;
    let mut components = rel_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            break;
        }
        base_dir.push(component);
        ignore_stack.push_ignore_file(root, &base_dir, &base_dir.join(".gitignore"))?;
    }

    Ok(ignore_stack.is_ignored(rel_path, is_dir))
}

fn path_to_rel_slash(root: &Path, path: &Path) -> Result<String, GitError> {
    let rel = path.strip_prefix(root).map_err(|_| {
        GitError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "cannot compute relative path",
        ))
    })?;
    let mut out = String::new();
    for (idx, comp) in rel.components().enumerate() {
        if idx > 0 {
            out.push('/');
        }
        out.push_str(&comp.as_os_str().to_string_lossy());
    }
    Ok(out)
}

fn parse_ignore_line(line: &str) -> Option<(String, bool, bool, bool, bool)> {
    let mut raw = line.trim_end_matches('\r');
    if raw.is_empty() {
        return None;
    }

    if raw.starts_with('#') {
        return None;
    }

    let mut negated = false;
    if raw.starts_with("\\#") || raw.starts_with("\\!") {
        raw = &raw[1..];
    } else if raw.starts_with('!') {
        negated = true;
        raw = &raw[1..];
    }

    if raw.is_empty() {
        return None;
    }

    let mut anchored = false;
    if raw.starts_with('/') {
        anchored = true;
        raw = raw.trim_start_matches('/');
    }

    if raw.is_empty() {
        return None;
    }

    let mut dir_only = false;
    if raw.ends_with('/') {
        dir_only = true;
        raw = raw.trim_end_matches('/');
    }

    if raw.is_empty() {
        return None;
    }

    let mut pattern = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                pattern.push(next);
            } else {
                pattern.push('\\');
            }
        } else {
            pattern.push(ch);
        }
    }

    if pattern.is_empty() {
        return None;
    }

    let has_slash = pattern.contains('/');
    Some((pattern, negated, dir_only, anchored, has_slash))
}

fn strip_rule_base<'a>(base_rel: &str, rel_path: &'a str) -> Option<&'a str> {
    if base_rel.is_empty() {
        return Some(rel_path);
    }
    if rel_path == base_rel {
        return Some("");
    }
    rel_path
        .strip_prefix(base_rel)
        .and_then(|rest| rest.strip_prefix('/'))
}

fn glob_match(pattern: &str, text: &str, slash_sensitive: bool) -> bool {
    fn rec(
        p: &[u8],
        t: &[u8],
        slash_sensitive: bool,
        pi: usize,
        ti: usize,
        memo: &mut [Option<bool>],
    ) -> bool {
        let width = t.len() + 1;
        let key = pi * width + ti;
        if let Some(cached) = memo[key] {
            return cached;
        }

        let result = if pi == p.len() {
            ti == t.len()
        } else {
            match p[pi] {
                b'*' => {
                    let mut next_pi = pi + 1;
                    let is_double = next_pi < p.len() && p[next_pi] == b'*';
                    if is_double {
                        while next_pi < p.len() && p[next_pi] == b'*' {
                            next_pi += 1;
                        }
                        let mut matched = false;
                        let mut k = ti;
                        while k <= t.len() {
                            if rec(p, t, slash_sensitive, next_pi, k, memo) {
                                matched = true;
                                break;
                            }
                            k += 1;
                        }
                        matched
                    } else {
                        let mut matched = false;
                        let mut k = ti;
                        loop {
                            if rec(p, t, slash_sensitive, next_pi, k, memo) {
                                matched = true;
                                break;
                            }
                            if k == t.len() {
                                break;
                            }
                            if slash_sensitive && t[k] == b'/' {
                                break;
                            }
                            k += 1;
                        }
                        matched
                    }
                }
                b'?' => {
                    if ti < t.len() && (!slash_sensitive || t[ti] != b'/') {
                        rec(p, t, slash_sensitive, pi + 1, ti + 1, memo)
                    } else {
                        false
                    }
                }
                ch => {
                    if ti < t.len() && ch == t[ti] {
                        rec(p, t, slash_sensitive, pi + 1, ti + 1, memo)
                    } else {
                        false
                    }
                }
            }
        };

        memo[key] = Some(result);
        result
    }

    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let mut memo = vec![None; (p.len() + 1) * (t.len() + 1)];
    rec(p, t, slash_sensitive, 0, 0, &mut memo)
}

fn path_matches_rule_pattern(rule: &IgnoreRule, rel_in_base: &str) -> bool {
    if rule.anchored {
        return glob_match(&rule.pattern, rel_in_base, true);
    }

    if rule.has_slash {
        if glob_match(&rule.pattern, rel_in_base, true) {
            return true;
        }
        let mut tail = rel_in_base;
        while let Some(pos) = tail.find('/') {
            tail = &tail[pos + 1..];
            if glob_match(&rule.pattern, tail, true) {
                return true;
            }
        }
        return false;
    }

    rel_in_base
        .split('/')
        .any(|seg| glob_match(&rule.pattern, seg, false))
}

fn rule_matches(rule: &IgnoreRule, rel_path: &str, is_dir: bool) -> bool {
    let Some(rel_in_base) = strip_rule_base(&rule.base_rel, rel_path) else {
        return false;
    };
    if rel_in_base.is_empty() {
        return false;
    }

    if rule.dir_only {
        if is_dir && path_matches_rule_pattern(rule, rel_in_base) {
            return true;
        }
        let mut idx = 0usize;
        while let Some(pos) = rel_in_base[idx..].find('/') {
            let end = idx + pos;
            let candidate = &rel_in_base[..end];
            if path_matches_rule_pattern(rule, candidate) {
                return true;
            }
            idx = end + 1;
        }
        return false;
    }

    path_matches_rule_pattern(rule, rel_in_base)
}

impl IgnoreStack {
    fn push_ignore_file(
        &mut self,
        root: &Path,
        base_dir: &Path,
        file_path: &Path,
    ) -> Result<(), GitError> {
        let content = match fs::read_to_string(file_path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(GitError::Io(err)),
        };

        let base_rel = path_to_rel_slash(root, base_dir)?;
        for line in content.lines() {
            let Some((pattern, negated, dir_only, anchored, has_slash)) = parse_ignore_line(line)
            else {
                continue;
            };
            self.rules.push(IgnoreRule {
                base_rel: base_rel.clone(),
                pattern,
                negated,
                dir_only,
                anchored,
                has_slash,
            });
        }
        Ok(())
    }

    fn ensure_repo_exclude_loaded(&mut self, root: &Path) -> Result<(), GitError> {
        if self.repo_exclude_loaded {
            return Ok(());
        }
        self.repo_exclude_loaded = true;
        let exclude_path = root.join(".git").join("info").join("exclude");
        self.push_ignore_file(root, root, &exclude_path)
    }

    fn is_ignored(&self, rel_path: &str, is_dir: bool) -> bool {
        let mut ignored = false;
        for rule in &self.rules {
            if rule_matches(rule, rel_path, is_dir) {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

/// Recursively collect untracked files.
fn collect_untracked(
    root: &Path,
    dir: &Path,
    index_map: &HashMap<&str, &IndexEntry>,
    entries: &mut Vec<StatusEntry>,
    options: StatusOptions,
    ignore_stack: &mut IgnoreStack,
) -> Result<(), GitError> {
    let saved_rules_len = ignore_stack.rules.len();
    if dir == root {
        ignore_stack.ensure_repo_exclude_loaded(root)?;
    }
    ignore_stack.push_ignore_file(root, dir, &dir.join(".gitignore"))?;

    let mut dir_entries: Vec<(std::ffi::OsString, std::path::PathBuf)> = Vec::new();
    let read_dir = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(GitError::Io(e)),
    };
    for entry in read_dir {
        let entry = entry?;
        dir_entries.push((entry.file_name(), entry.path()));
    }

    // Iterate over plain paths so read_dir handles are dropped before recursion.
    for (name, path) in dir_entries {
        let name_str = name.to_string_lossy();

        // Skip .git directory
        if name_str == ".git" {
            continue;
        }
        if options.skip_hidden && name_str.starts_with('.') {
            continue;
        }
        if options.skip_target_dirs && path.is_dir() && name_str == "target" {
            continue;
        }

        let is_dir = path.is_dir();
        let is_file = path.is_file();
        if !is_dir && !is_file {
            continue;
        }

        let rel_str = path_to_rel_slash(root, &path)?;
        if ignore_stack.is_ignored(&rel_str, is_dir) {
            continue;
        }

        if is_dir {
            collect_untracked(root, &path, index_map, entries, options, ignore_stack)?;
        } else if is_file {
            if !index_map.contains_key(rel_str.as_str()) {
                entries.push(StatusEntry {
                    path: rel_str,
                    status: FileStatus::Untracked,
                });
            }
        }
    }
    ignore_stack.rules.truncate(saved_rules_len);
    Ok(())
}

/// Flatten a tree into a map of path -> OID for all blobs (recursively).
pub fn flatten_tree(
    tree: &Tree,
    prefix: &str,
    read_tree: &mut dyn FnMut(&ObjectId) -> Result<Tree, GitError>,
) -> Result<HashMap<String, ObjectId>, GitError> {
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
            let sub_entries = flatten_tree(&sub_tree, &sub_prefix, read_tree)?;
            result.extend(sub_entries);
        } else {
            result.insert(path, entry.oid);
        }
    }

    Ok(result)
}

/// Checkout a tree to the working directory and rebuild the index.
///
/// This writes all files from the tree to the workdir and creates a matching index.
pub fn checkout_tree(
    git_dir: &Path,
    workdir: &Path,
    tree: &Tree,
    prefix: &str,
    read_tree: &mut dyn FnMut(&ObjectId) -> Result<Tree, GitError>,
    read_blob: &mut dyn FnMut(&ObjectId) -> Result<Vec<u8>, GitError>,
    index_entries: &mut Vec<IndexEntry>,
) -> Result<(), GitError> {
    for entry in &tree.entries {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{}{}", prefix, entry.name)
        };

        if entry.is_tree() {
            let sub_tree = read_tree(&entry.oid)?;
            let dir_path = workdir.join(&path);
            fs::create_dir_all(&dir_path)?;
            let sub_prefix = format!("{}/", path);
            checkout_tree(
                git_dir,
                workdir,
                &sub_tree,
                &sub_prefix,
                read_tree,
                read_blob,
                index_entries,
            )?;
        } else {
            // Write file
            let data = read_blob(&entry.oid)?;
            let file_path = workdir.join(&path);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file_path, &data)?;

            // Set executable permission if needed
            #[cfg(unix)]
            if entry.mode == 0o100755 {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o755);
                fs::set_permissions(&file_path, perms)?;
            }

            // Create index entry
            let metadata = fs::metadata(&file_path)?;
            let ie = IndexEntry {
                ctime_sec: metadata_mtime_sec(&metadata),
                ctime_nsec: 0,
                mtime_sec: metadata_mtime_sec(&metadata),
                mtime_nsec: 0,
                dev: metadata_dev(&metadata),
                ino: metadata_ino(&metadata),
                mode: entry.mode,
                uid: metadata_uid(&metadata),
                gid: metadata_gid(&metadata),
                file_size: metadata.len() as u32,
                oid: entry.oid,
                flags: (path.len().min(0xFFF)) as u16,
                path,
            };
            index_entries.push(ie);
        }
    }
    Ok(())
}

/// Add a file to the index: hash the file content, write the blob object,
/// and update or insert the index entry.
pub fn stage_file(
    git_dir: &Path,
    workdir: &Path,
    index: &mut Index,
    path: &str,
) -> Result<(), GitError> {
    let file_path = workdir.join(path);
    let content = fs::read(&file_path)?;
    let oid = write_loose_object(git_dir, ObjectKind::Blob, &content)?;
    let metadata = fs::metadata(&file_path)?;

    let mode = if metadata.permissions().readonly() {
        0o100644
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = metadata.permissions().mode();
            if m & 0o111 != 0 {
                0o100755
            } else {
                0o100644
            }
        }
        #[cfg(not(unix))]
        {
            0o100644
        }
    };

    let entry = IndexEntry {
        ctime_sec: metadata_mtime_sec(&metadata),
        ctime_nsec: 0,
        mtime_sec: metadata_mtime_sec(&metadata),
        mtime_nsec: 0,
        dev: metadata_dev(&metadata),
        ino: metadata_ino(&metadata),
        mode,
        uid: metadata_uid(&metadata),
        gid: metadata_gid(&metadata),
        file_size: metadata.len() as u32,
        oid,
        flags: (path.len().min(0xFFF)) as u16,
        path: path.to_string(),
    };

    // Update existing entry or insert in sorted order
    match index.entries.binary_search_by(|e| e.path.cmp(&entry.path)) {
        Ok(idx) => index.entries[idx] = entry,
        Err(idx) => index.entries.insert(idx, entry),
    }

    Ok(())
}

/// Remove a file from the index.
pub fn unstage_file(index: &mut Index, path: &str) {
    index.entries.retain(|e| e.path != path);
}

/// Remove files from the worktree that are not in the given tree.
/// Used during checkout to clean up files from the old branch.
pub fn remove_worktree_files(
    workdir: &Path,
    old_files: &HashMap<String, ObjectId>,
    new_files: &HashMap<String, ObjectId>,
) -> Result<(), GitError> {
    for path in old_files.keys() {
        if !new_files.contains_key(path) {
            let file_path = workdir.join(path);
            if file_path.exists() {
                fs::remove_file(&file_path)?;
            }
            // Try to remove empty parent directories
            if let Some(parent) = file_path.parent() {
                let _ = remove_empty_dirs(parent, workdir);
            }
        }
    }
    Ok(())
}

/// Remove empty directories up to (but not including) the stop directory.
fn remove_empty_dirs(dir: &Path, stop: &Path) -> Result<(), std::io::Error> {
    if dir == stop {
        return Ok(());
    }
    if let Ok(mut entries) = fs::read_dir(dir) {
        if entries.next().is_none() {
            fs::remove_dir(dir)?;
            if let Some(parent) = dir.parent() {
                let _ = remove_empty_dirs(parent, stop);
            }
        }
    }
    Ok(())
}

fn metadata_mtime_sec(metadata: &fs::Metadata) -> u32 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

#[cfg(unix)]
fn metadata_dev(metadata: &fs::Metadata) -> u32 {
    metadata.dev() as u32
}

#[cfg(windows)]
fn metadata_dev(metadata: &fs::Metadata) -> u32 {
    let _ = metadata;
    0
}

#[cfg(not(any(unix, windows)))]
fn metadata_dev(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn metadata_ino(metadata: &fs::Metadata) -> u32 {
    metadata.ino() as u32
}

#[cfg(windows)]
fn metadata_ino(metadata: &fs::Metadata) -> u32 {
    let _ = metadata;
    0
}

#[cfg(not(any(unix, windows)))]
fn metadata_ino(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn metadata_uid(metadata: &fs::Metadata) -> u32 {
    metadata.uid()
}

#[cfg(windows)]
fn metadata_uid(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(not(any(unix, windows)))]
fn metadata_uid(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn metadata_gid(metadata: &fs::Metadata) -> u32 {
    metadata.gid()
}

#[cfg(windows)]
fn metadata_gid(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(not(any(unix, windows)))]
fn metadata_gid(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_flatten_tree() {
        use crate::tree::TreeEntry;

        let sub_tree = Tree {
            entries: vec![TreeEntry {
                mode: 0o100644,
                name: "nested.txt".into(),
                oid: ObjectId::from_hex("3b18e512dba79e4c8300dd08aeb37f8e728b8dad").unwrap(),
            }],
        };
        let sub_oid = ObjectId::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();

        let root_tree = Tree {
            entries: vec![
                TreeEntry {
                    mode: 0o100644,
                    name: "file.txt".into(),
                    oid: ObjectId::from_hex("3b18e512dba79e4c8300dd08aeb37f8e728b8dad").unwrap(),
                },
                TreeEntry {
                    mode: 0o040000,
                    name: "dir".into(),
                    oid: sub_oid,
                },
            ],
        };

        let sub_tree_clone = sub_tree.clone();
        let files = flatten_tree(&root_tree, "", &mut |oid| {
            if *oid == sub_oid {
                Ok(sub_tree_clone.clone())
            } else {
                Err(GitError::ObjectNotFound(oid.to_hex()))
            }
        })
        .unwrap();

        assert_eq!(files.len(), 2);
        assert!(files.contains_key("file.txt"));
        assert!(files.contains_key("dir/nested.txt"));
    }

    #[test]
    fn test_compute_status_untracked() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();
        fs::write(workdir.join("new_file.txt"), "untracked\n").unwrap();

        let head_files = HashMap::new();
        let index = Index {
            version: 2,
            entries: vec![],
        };

        let status = compute_status(&head_files, &index, workdir).unwrap();
        assert_eq!(status.entries.len(), 1);
        assert_eq!(status.entries[0].path, "new_file.txt");
        assert_eq!(status.entries[0].status, FileStatus::Untracked);
    }

    #[test]
    fn test_compute_status_for_path_reports_untracked() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();
        fs::write(workdir.join("new_file.txt"), "untracked\n").unwrap();

        let index = Index {
            version: 2,
            entries: vec![],
        };
        let status = compute_status_for_path_with_options(
            None,
            &index,
            workdir,
            "new_file.txt",
            StatusOptions::default(),
        )
        .unwrap();
        assert_eq!(status, Some(FileStatus::Untracked));
    }

    #[test]
    fn test_compute_status_for_path_reports_staged_before_unstaged() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();
        fs::write(workdir.join("tracked.txt"), "worktree\n").unwrap();

        let index = Index {
            version: 2,
            entries: vec![IndexEntry {
                ctime_sec: 0,
                ctime_nsec: 0,
                mtime_sec: 0,
                mtime_nsec: 0,
                dev: 0,
                ino: 0,
                mode: 0o100644,
                uid: 0,
                gid: 0,
                file_size: 0,
                oid: hash_object("blob", b"index\n"),
                flags: "tracked.txt".len() as u16,
                path: "tracked.txt".to_string(),
            }],
        };

        let status = compute_status_for_path_with_options(
            Some(hash_object("blob", b"head\n")),
            &index,
            workdir,
            "tracked.txt",
            StatusOptions::default(),
        )
        .unwrap();
        assert_eq!(status, Some(FileStatus::Staged));
    }

    #[test]
    fn test_compute_status_for_path_worktree_only_reports_deleted() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();

        let index = Index {
            version: 2,
            entries: vec![IndexEntry {
                ctime_sec: 0,
                ctime_nsec: 0,
                mtime_sec: 0,
                mtime_nsec: 0,
                dev: 0,
                ino: 0,
                mode: 0o100644,
                uid: 0,
                gid: 0,
                file_size: 0,
                oid: hash_object("blob", b"index\n"),
                flags: "deleted.txt".len() as u16,
                path: "deleted.txt".to_string(),
            }],
        };

        let status = compute_status_for_path_worktree_only_with_options(
            &index,
            workdir,
            "deleted.txt",
            StatusOptions::default(),
        )
        .unwrap();
        assert_eq!(status, Some(FileStatus::Deleted));
    }

    #[test]
    fn test_compute_status_worktree_only_modified_and_untracked() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();

        fs::write(workdir.join("tracked.txt"), "new content\n").unwrap();
        fs::write(workdir.join("untracked.txt"), "hello\n").unwrap();

        let index = Index {
            version: 2,
            entries: vec![IndexEntry {
                ctime_sec: 0,
                ctime_nsec: 0,
                mtime_sec: 0,
                mtime_nsec: 0,
                dev: 0,
                ino: 0,
                mode: 0o100644,
                uid: 0,
                gid: 0,
                file_size: 0,
                oid: hash_object("blob", b"old content\n"),
                flags: "tracked.txt".len() as u16,
                path: "tracked.txt".to_string(),
            }],
        };

        let status = compute_status_worktree_only(&index, workdir).unwrap();
        assert!(status
            .entries
            .iter()
            .any(|e| e.path == "tracked.txt" && e.status == FileStatus::Modified));
        assert!(status
            .entries
            .iter()
            .any(|e| e.path == "untracked.txt" && e.status == FileStatus::Untracked));
    }

    #[test]
    fn test_compute_status_respects_gitignore_untracked() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();

        fs::write(workdir.join(".gitignore"), "*.log\nbuild/\n").unwrap();
        fs::write(workdir.join("keep.txt"), "keep\n").unwrap();
        fs::write(workdir.join("ignored.log"), "ignored\n").unwrap();
        fs::create_dir_all(workdir.join("build")).unwrap();
        fs::write(workdir.join("build").join("out.txt"), "ignored\n").unwrap();

        let head_files = HashMap::new();
        let index = Index {
            version: 2,
            entries: vec![],
        };

        let status = compute_status(&head_files, &index, workdir).unwrap();
        assert!(status
            .entries
            .iter()
            .any(|e| e.path == "keep.txt" && e.status == FileStatus::Untracked));
        assert!(!status
            .entries
            .iter()
            .any(|e| e.path == "ignored.log" && e.status == FileStatus::Untracked));
        assert!(!status
            .entries
            .iter()
            .any(|e| e.path == "build/out.txt" && e.status == FileStatus::Untracked));
    }

    #[test]
    fn test_compute_status_gitignore_negation() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();

        fs::write(workdir.join(".gitignore"), "*.log\n!important.log\n").unwrap();
        fs::write(workdir.join("ignored.log"), "ignored\n").unwrap();
        fs::write(workdir.join("important.log"), "keep\n").unwrap();

        let head_files = HashMap::new();
        let index = Index {
            version: 2,
            entries: vec![],
        };

        let status = compute_status(&head_files, &index, workdir).unwrap();
        assert!(status
            .entries
            .iter()
            .any(|e| e.path == "important.log" && e.status == FileStatus::Untracked));
        assert!(!status
            .entries
            .iter()
            .any(|e| e.path == "ignored.log" && e.status == FileStatus::Untracked));
    }

    #[test]
    fn test_compute_status_respects_git_info_exclude() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();
        fs::create_dir_all(workdir.join(".git").join("info")).unwrap();
        fs::write(workdir.join(".git").join("info").join("exclude"), "*.tmp\n").unwrap();
        fs::write(workdir.join("ignored.tmp"), "ignored\n").unwrap();
        fs::write(workdir.join("keep.txt"), "keep\n").unwrap();

        let head_files = HashMap::new();
        let index = Index {
            version: 2,
            entries: vec![],
        };

        let status = compute_status(&head_files, &index, workdir).unwrap();
        assert!(status
            .entries
            .iter()
            .any(|e| e.path == "keep.txt" && e.status == FileStatus::Untracked));
        assert!(!status
            .entries
            .iter()
            .any(|e| e.path == "ignored.tmp" && e.status == FileStatus::Untracked));
    }
}
