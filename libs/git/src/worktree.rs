use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

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
            // Quick stat check: if mtime/size match the index, skip content hashing
            let metadata = fs::metadata(&file_path)?;
            let stat_matches = metadata.mtime() as u32 == idx_entry.mtime_sec
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
    collect_untracked(workdir, workdir, &index_map, &mut entries)?;

    // Sort by path for deterministic output
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    // Deduplicate: if a file appears as both Staged and Modified, keep both
    // (this is how git status works: file can be both staged and have unstaged changes)

    Ok(Status { entries })
}

/// Recursively collect untracked files.
fn collect_untracked(
    root: &Path,
    dir: &Path,
    index_map: &HashMap<&str, &IndexEntry>,
    entries: &mut Vec<StatusEntry>,
) -> Result<(), GitError> {
    let read_dir = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(GitError::Io(e)),
    };

    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip .git directory
        if name_str == ".git" {
            continue;
        }

        if path.is_dir() {
            collect_untracked(root, &path, index_map, entries)?;
        } else if path.is_file() {
            let rel_path = path.strip_prefix(root).map_err(|_| {
                GitError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "cannot compute relative path",
                ))
            })?;
            let rel_str = rel_path.to_string_lossy().to_string();
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
                ctime_sec: metadata.mtime() as u32,
                ctime_nsec: 0,
                mtime_sec: metadata.mtime() as u32,
                mtime_nsec: 0,
                dev: metadata.dev() as u32,
                ino: metadata.ino() as u32,
                mode: entry.mode,
                uid: metadata.uid(),
                gid: metadata.gid(),
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
        ctime_sec: metadata.mtime() as u32,
        ctime_nsec: 0,
        mtime_sec: metadata.mtime() as u32,
        mtime_nsec: 0,
        dev: metadata.dev() as u32,
        ino: metadata.ino() as u32,
        mode,
        uid: metadata.uid(),
        gid: metadata.gid(),
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
        let dir = tempfile::tempdir().unwrap();
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
}
