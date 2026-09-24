use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::error::GitError;
use crate::ignore::{GitIgnore, IgnoreError};
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
    compute_status_cancellable(head_files, index, workdir, options, &|| false)
}

/// [`compute_status_with_options`] that stops with [`GitError::Cancelled`]
/// as soon as `cancel` returns true; it is polled per file and per folder.
pub fn compute_status_cancellable(
    head_files: &HashMap<String, ObjectId>,
    index: &Index,
    workdir: &Path,
    options: StatusOptions,
    cancel: &dyn Fn() -> bool,
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
        if cancel() {
            return Err(GitError::Cancelled);
        }
        if let Some(status) = worktree_vs_index_status_for_path(idx_entry, workdir, path, options)? {
            entries.push(StatusEntry {
                path: path.to_string(),
                status,
            });
        }
    }

    // 3. Untracked files
    let mut ignore = status_ignore(workdir)?;
    collect_untracked(
        workdir,
        workdir,
        &index_map,
        &mut entries,
        options,
        &mut ignore,
        cancel,
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
    compute_status_worktree_only_cancellable(index, workdir, options, &|| false)
}

/// [`compute_status_worktree_only_with_options`] that stops with
/// [`GitError::Cancelled`] as soon as `cancel` returns true.
pub fn compute_status_worktree_only_cancellable(
    index: &Index,
    workdir: &Path,
    options: StatusOptions,
    cancel: &dyn Fn() -> bool,
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
        if cancel() {
            return Err(GitError::Cancelled);
        }
        if let Some(status) = worktree_vs_index_status_for_path(idx_entry, workdir, path, options)? {
            entries.push(StatusEntry {
                path: path.to_string(),
                status,
            });
        }
    }

    // Untracked files
    let mut ignore = status_ignore(workdir)?;
    collect_untracked(
        workdir,
        workdir,
        &index_map,
        &mut entries,
        options,
        &mut ignore,
        cancel,
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

const GITLINK_MODE: u32 = 0o160000;
const SYMLINK_MODE: u32 = 0o120000;

/// The bytes git stores as a symlink's blob: its target path.
fn symlink_text(path: &Path) -> Result<Vec<u8>, GitError> {
    let target = fs::read_link(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(target.into_os_string().into_vec())
    }
    #[cfg(not(unix))]
    {
        Ok(target.to_string_lossy().replace('\\', "/").into_bytes())
    }
}

fn worktree_vs_index_status_for_path(
    idx_entry: &IndexEntry,
    workdir: &Path,
    path: &str,
    options: StatusOptions,
) -> Result<Option<FileStatus>, GitError> {
    let file_path = workdir.join(path);
    // lstat, as git does: a tracked symlink is compared as its link text
    // and a dangling one is not deleted.
    let metadata = match fs::symlink_metadata(&file_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(FileStatus::Deleted));
        }
        // A parent that became a file (ENOTDIR) also means the path is gone.
        Err(_) if !file_path.parent().is_some_and(Path::is_dir) => {
            return Ok(Some(FileStatus::Deleted));
        }
        Err(err) => return Err(GitError::Io(err)),
    };
    let file_type = metadata.file_type();
    if idx_entry.mode == GITLINK_MODE {
        // A submodule: its checked-out commit is not compared here.
        return Ok((!file_type.is_dir()).then_some(FileStatus::Modified));
    }
    if file_type.is_dir() {
        // A tracked file replaced by a folder is deleted; the folder's files
        // are reported as untracked.
        return Ok(Some(FileStatus::Deleted));
    }
    if options.skip_worktree_content_compare {
        return Ok(None);
    }
    let index_is_link = idx_entry.mode & 0o170000 == SYMLINK_MODE;
    // Where symlinks are checked out as plain files holding the link text
    // (Windows without symlink support) the content comparison decides.
    if cfg!(unix) && file_type.is_symlink() != index_is_link {
        return Ok(Some(FileStatus::Modified));
    }

    let stat_matches = metadata_mtime_sec(&metadata) == idx_entry.mtime_sec
        && metadata.len() as u32 == idx_entry.file_size;
    if stat_matches {
        return Ok(None);
    }

    let content = if file_type.is_symlink() {
        symlink_text(&file_path)?
    } else {
        fs::read(&file_path)?
    };
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
    status_ignore(root)?
        .ignored(rel_path, is_dir)
        .map_err(ignore_error)
}

/// The ignore rules `git status` applies: per-directory `.gitignore`,
/// `info/exclude` and the user's excludes file.
fn status_ignore(root: &Path) -> Result<GitIgnore, GitError> {
    GitIgnore::with_user_excludes(root).map_err(ignore_error)
}

fn ignore_error(error: IgnoreError) -> GitError {
    match error {
        IgnoreError::Io(message) => GitError::Io(std::io::Error::other(message)),
        IgnoreError::Git(message) => GitError::InvalidIndex(message),
    }
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

/// Recursively collect untracked files.
fn collect_untracked(
    root: &Path,
    dir: &Path,
    index_map: &HashMap<&str, &IndexEntry>,
    entries: &mut Vec<StatusEntry>,
    options: StatusOptions,
    ignore: &mut GitIgnore,
    cancel: &dyn Fn() -> bool,
) -> Result<(), GitError> {
    if cancel() {
        return Err(GitError::Cancelled);
    }

    let mut dir_entries: Vec<(std::ffi::OsString, std::path::PathBuf, fs::FileType)> = Vec::new();
    let read_dir = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(GitError::Io(e)),
    };
    for entry in read_dir {
        let entry = entry?;
        // The entry's own type: a symlink is never followed, as in git.
        let file_type = entry.file_type()?;
        dir_entries.push((entry.file_name(), entry.path(), file_type));
    }

    // Iterate over plain paths so read_dir handles are dropped before recursion.
    for (name, path, file_type) in dir_entries {
        let name_str = name.to_string_lossy();

        // Skip .git directory
        if name_str == ".git" {
            continue;
        }
        if options.skip_hidden && name_str.starts_with('.') {
            continue;
        }
        let is_dir = file_type.is_dir();
        let is_file = file_type.is_file() || file_type.is_symlink();
        if options.skip_target_dirs && is_dir && name_str == "target" {
            continue;
        }
        if !is_dir && !is_file {
            continue;
        }

        let rel_str = path_to_rel_slash(root, &path)?;
        if ignore.ignored(&rel_str, is_dir).map_err(ignore_error)? {
            continue;
        }

        if is_dir {
            if index_map.contains_key(rel_str.as_str()) {
                // A tracked submodule; its files belong to its own repository.
                continue;
            }
            if fs::symlink_metadata(path.join(".git")).is_ok() {
                // A nested repository nothing of ours is tracked in is one
                // untracked entry, "dir/", as git reports it.
                let prefix = format!("{rel_str}/");
                if !index_map.keys().any(|tracked| tracked.starts_with(&prefix)) {
                    entries.push(StatusEntry {
                        path: prefix,
                        status: FileStatus::Untracked,
                    });
                    continue;
                }
            }
            collect_untracked(root, &path, index_map, entries, options, ignore, cancel)?;
        } else if is_file {
            if !index_map.contains_key(rel_str.as_str()) {
                entries.push(StatusEntry {
                    path: rel_str,
                    status: FileStatus::Untracked,
                });
            }
        }
    }
    Ok(())
}

/// The on-disk path for a repository-relative `path` that is about to be
/// written or removed, refusing anything that could land outside `workdir`
/// or inside `.git`: every '/'-separated component must pass
/// [`crate::tree::validate_entry_name`], and no parent folder below
/// `workdir` may be a symlink (writing through one would touch whatever it
/// points at). A symlink as the final component is the caller's business:
/// removing it removes the link, and writers unlink it before writing.
pub fn checked_worktree_path(workdir: &Path, path: &str) -> Result<PathBuf, GitError> {
    let mut file_path = workdir.to_path_buf();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        crate::tree::validate_entry_name(part)
            .map_err(|_| GitError::InvalidObject(format!("unsafe worktree path {:?}", path)))?;
        file_path.push(part);
        if parts.peek().is_some() {
            if let Ok(meta) = fs::symlink_metadata(&file_path) {
                if meta.file_type().is_symlink() {
                    return Err(GitError::InvalidObject(format!(
                        "{path}: folder {part} is a symlink; refusing to write or remove through it"
                    )));
                }
            }
        }
    }
    Ok(file_path)
}

/// Replace a symlink at `file_path` (the link itself, never its target) so
/// a following `fs::write` creates a plain file instead of writing
/// through the link.
pub(crate) fn unlink_if_symlink(file_path: &Path) -> Result<(), GitError> {
    if fs::symlink_metadata(file_path).is_ok_and(|m| m.file_type().is_symlink()) {
        fs::remove_file(file_path)?;
    }
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
            let dir_path = checked_worktree_path(workdir, &path)?;
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
            let file_path = checked_worktree_path(workdir, &path)?;
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            unlink_if_symlink(&file_path)?;
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
            index_entries.push(index_entry_from_metadata(path, entry.oid, entry.mode, &metadata));
        }
    }
    Ok(())
}

/// An index entry for a working-tree file whose stat data was just read.
pub fn index_entry_from_metadata(
    path: String,
    oid: ObjectId,
    mode: u32,
    metadata: &fs::Metadata,
) -> IndexEntry {
    IndexEntry {
        ctime_sec: metadata_mtime_sec(metadata),
        ctime_nsec: 0,
        mtime_sec: metadata_mtime_sec(metadata),
        mtime_nsec: 0,
        dev: metadata_dev(metadata),
        ino: metadata_ino(metadata),
        mode,
        uid: metadata_uid(metadata),
        gid: metadata_gid(metadata),
        file_size: metadata.len() as u32,
        oid,
        flags: (path.len().min(0xFFF)) as u16,
        path,
    }
}

/// Write one blob to `workdir/path` (creating parents) with the mode's
/// executable bit and return its fresh index entry.
pub fn write_worktree_file(
    workdir: &Path,
    path: &str,
    oid: ObjectId,
    mode: u32,
    data: &[u8],
) -> Result<IndexEntry, GitError> {
    let file_path = checked_worktree_path(workdir, path)?;
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    unlink_if_symlink(&file_path)?;
    fs::write(&file_path, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bits = if mode == 0o100755 { 0o755 } else { 0o644 };
        fs::set_permissions(&file_path, fs::Permissions::from_mode(bits))?;
    }
    let metadata = fs::metadata(&file_path)?;
    Ok(index_entry_from_metadata(path.to_string(), oid, mode, &metadata))
}

/// The blob id a working-tree file would hash to, streamed so large files
/// are never held in memory.
pub fn hash_file_blob(path: &Path) -> Result<ObjectId, GitError> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut hasher = crate::sha1::Sha1::new();
    hasher.update(format!("blob {}\0", len).as_bytes());
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(ObjectId::from_bytes(hasher.finalize()))
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
    // Check every path before removing any, so an unsafe one stops the
    // whole clean-up instead of leaving it half done.
    let mut doomed = Vec::new();
    for path in old_files.keys() {
        if !new_files.contains_key(path) {
            doomed.push(checked_worktree_path(workdir, path)?);
        }
    }
    for file_path in doomed {
        if file_path.exists() {
            fs::remove_file(&file_path)?;
        }
        // Try to remove empty parent directories
        if let Some(parent) = file_path.parent() {
            remove_empty_dirs(parent, workdir);
        }
    }
    Ok(())
}

/// Remove empty directories up to (but not including) the stop directory.
pub(crate) fn remove_empty_dirs(dir: &Path, stop: &Path) {
    let _ = remove_empty_dirs_inner(dir, stop);
}

fn remove_empty_dirs_inner(dir: &Path, stop: &Path) -> Result<(), std::io::Error> {
    if dir == stop {
        return Ok(());
    }
    if let Ok(mut entries) = fs::read_dir(dir) {
        if entries.next().is_none() {
            fs::remove_dir(dir)?;
            if let Some(parent) = dir.parent() {
                let _ = remove_empty_dirs_inner(parent, stop);
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

    #[test]
    fn unsafe_worktree_paths_are_refused_before_writing() {
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();
        let oid = ObjectId::from_hex("2015f8e40c38d86ca88808c6f031bb22544e92cf").unwrap();
        for path in [
            "", "..", "../escape.txt", "a/../../escape.txt", ".git/config", "sub/.GIT/HEAD",
            "a//b", "a/./b", "a\\..\\b", "trailing/",
        ] {
            assert!(checked_worktree_path(workdir, path).is_err(), "{path:?}");
            assert!(write_worktree_file(workdir, path, oid, 0o100644, b"x").is_err(), "{path:?}");
        }
        assert!(checked_worktree_path(workdir, "dir/file.txt").is_ok());
        assert!(!workdir.parent().unwrap().join("escape.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_parent_folder_is_refused() {
        let dir = crate::test_support::tempdir().unwrap();
        let outside = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();
        std::os::unix::fs::symlink(outside.path(), workdir.join("link")).unwrap();
        let oid = ObjectId::from_hex("2015f8e40c38d86ca88808c6f031bb22544e92cf").unwrap();
        assert!(write_worktree_file(workdir, "link/file.txt", oid, 0o100644, b"x").is_err());
        assert!(!outside.path().join("file.txt").exists());

        // Removing through the link is refused too, and nothing is removed.
        fs::write(outside.path().join("keep.txt"), "keep").unwrap();
        let mut old_files = HashMap::new();
        old_files.insert("link/keep.txt".to_string(), oid);
        assert!(remove_worktree_files(workdir, &old_files, &HashMap::new()).is_err());
        assert!(outside.path().join("keep.txt").exists());

        // A symlink as the file itself is replaced, not written through.
        fs::write(outside.path().join("target.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path().join("target.txt"), workdir.join("f.txt"))
            .unwrap();
        write_worktree_file(workdir, "f.txt", oid, 0o100644, b"inside").unwrap();
        assert_eq!(fs::read_to_string(outside.path().join("target.txt")).unwrap(), "outside");
        assert_eq!(fs::read_to_string(workdir.join("f.txt")).unwrap(), "inside");
    }

    /// What `git status` reports for symlinks, nested repositories, tracked
    /// submodules and root-level `**/dir/` rules.
    #[cfg(unix)]
    #[test]
    fn status_lstat_symlinks_nested_repos_and_submodules() {
        use std::os::unix::fs::symlink;
        let dir = crate::test_support::tempdir().unwrap();
        let workdir = dir.path();
        fs::create_dir_all(workdir.join(".git")).unwrap();
        fs::write(workdir.join(".gitignore"), "**/target/\n").unwrap();
        fs::create_dir_all(workdir.join("target/debug")).unwrap();
        fs::write(workdir.join("target/debug/out"), "x").unwrap();
        fs::create_dir_all(workdir.join("real/deep")).unwrap();
        fs::write(workdir.join("real/deep/file.txt"), "x").unwrap();
        // Untracked symlink to a folder: one entry, never followed.
        symlink(workdir.join("real"), workdir.join("link_dir")).unwrap();
        // Tracked symlink to a folder, unchanged, and a dangling tracked one.
        symlink("real", workdir.join("tracked_link")).unwrap();
        symlink("missing", workdir.join("dangling")).unwrap();
        // A nested repository and a tracked submodule.
        fs::create_dir_all(workdir.join("nested/.git")).unwrap();
        fs::write(workdir.join("nested/inner.txt"), "x").unwrap();
        fs::create_dir_all(workdir.join("sub")).unwrap();
        fs::write(workdir.join("sub/.git"), "gitdir: elsewhere\n").unwrap();
        fs::write(workdir.join("sub/inner.txt"), "x").unwrap();

        let link_entry = |name: &str, target: &str| {
            let meta = fs::symlink_metadata(workdir.join(name)).unwrap();
            index_entry_from_metadata(
                name.to_string(),
                hash_object("blob", target.as_bytes()),
                SYMLINK_MODE,
                &meta,
            )
        };
        let mut sub = link_entry("tracked_link", "real");
        sub.path = "sub".into();
        sub.mode = GITLINK_MODE;
        let mut stale = link_entry("dangling", "missing");
        stale.mtime_sec = 0; // force the content comparison
        let index = Index {
            version: 2,
            entries: vec![link_entry("tracked_link", "real"), stale, sub],
        };
        let head_files: HashMap<String, ObjectId> =
            index.entries.iter().map(|e| (e.path.clone(), e.oid)).collect();
        let status = compute_status(&head_files, &index, workdir).unwrap();
        let got: Vec<(&str, &FileStatus)> =
            status.entries.iter().map(|e| (e.path.as_str(), &e.status)).collect();
        assert_eq!(
            got,
            vec![
                (".gitignore", &FileStatus::Untracked),
                ("link_dir", &FileStatus::Untracked),
                ("nested/", &FileStatus::Untracked),
                ("real/deep/file.txt", &FileStatus::Untracked),
            ]
        );

        // Retargeting a tracked symlink is a modification.
        fs::remove_file(workdir.join("tracked_link")).unwrap();
        symlink("real/deep", workdir.join("tracked_link")).unwrap();
        let status = compute_status(&head_files, &index, workdir).unwrap();
        assert!(status
            .entries
            .iter()
            .any(|e| e.path == "tracked_link" && e.status == FileStatus::Modified));
    }

    #[test]
    fn status_stops_when_cancelled() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::write(dir.path().join("file.txt"), "x").unwrap();
        let index = Index { version: 2, entries: vec![] };
        let result =
            compute_status_cancellable(&HashMap::new(), &index, dir.path(), StatusOptions::default(), &|| true);
        assert!(matches!(result, Err(GitError::Cancelled)));
    }
}
