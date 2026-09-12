//! Linked worktrees created, listed, removed and pruned on disk without a
//! git binary, using exactly git's layout:
//!
//! ```text
//! <common>/worktrees/<name>/HEAD       ref: refs/heads/<branch>
//! <common>/worktrees/<name>/commondir  ../..
//! <common>/worktrees/<name>/gitdir     <absolute worktree path>/.git
//! <common>/worktrees/<name>/index      built from the checked-out tree
//! <worktree>/.git                      gitdir: <common>/worktrees/<name>
//! ```
//!
//! Populating the new working directory from its tree is the only checkout
//! this module performs; branches are created but never deleted here.
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    error::GitError,
    index::Index,
    object::ObjectKind,
    oid::ObjectId,
    refs::{self, RefTarget},
    repo::{read_object_standalone, read_tree_standalone, resolve_gitfile, Repository},
    worktree,
};

/// Which branch a new linked worktree checks out.
#[derive(Debug, Clone)]
pub enum WorktreeBranch<'a> {
    /// An existing `refs/heads/<name>` that no worktree has checked out.
    Existing(&'a str),
    /// Create `refs/heads/<name>` at `start` first; the branch must not exist.
    NewFrom { name: &'a str, start: ObjectId },
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedWorktree {
    /// Registration name under `<common>/worktrees/`; empty for the main
    /// worktree, which has no registration.
    pub name: String,
    /// The working directory as recorded (main: the common dir's parent).
    pub path: PathBuf,
    /// The private git directory: `<common>/worktrees/<name>`, or the common
    /// dir itself for the main worktree.
    pub git_dir: PathBuf,
    /// Resolved HEAD, `None` when HEAD is unborn or unreadable.
    pub head: Option<ObjectId>,
    /// Short branch name when HEAD is symbolic to `refs/heads/…`; `None`
    /// when detached.
    pub branch: Option<String>,
    /// `Some(reason)` when `<common>/worktrees/<name>/locked` exists (the
    /// reason may be empty).
    pub locked: Option<String>,
    /// `Some(reason)` when the registration no longer describes a worktree:
    /// its `gitdir` file is missing or points nowhere. Locked entries are
    /// never prunable.
    pub prunable: Option<String>,
    pub main: bool,
}

impl Repository {
    /// `git worktree add`: register `<common>/worktrees/<name>`, point
    /// `path/.git` at it, and populate `path` from the branch tip. `path`
    /// must not exist or must be an empty directory. `name` defaults to the
    /// path's file name, de-duplicated with a numeric suffix like git does.
    /// A branch already checked out by any worktree is refused.
    pub fn worktree_add(
        &mut self,
        path: &Path,
        branch: WorktreeBranch<'_>,
        name: Option<&str>,
    ) -> Result<LinkedWorktree, GitError> {
        let path = absolute(path)?;
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => {
                if fs::read_dir(&path)?.next().is_some() {
                    return Err(GitError::InvalidRef(format!(
                        "worktree destination is not empty: {}",
                        path.display()
                    )));
                }
            }
            Ok(_) => {
                return Err(GitError::InvalidRef(format!(
                    "worktree destination exists and is not a directory: {}",
                    path.display()
                )))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(GitError::Io(e)),
        }
        let (branch_name, tip, create) = match branch {
            WorktreeBranch::Existing(name) => {
                validate_branch_name(name)?;
                let refname = format!("refs/heads/{name}");
                let tip = refs::resolve_ref_in(&self.git_dir, &self.common_dir, &refname)?;
                (name.to_string(), tip, false)
            }
            WorktreeBranch::NewFrom { name, start } => {
                validate_branch_name(name)?;
                let refname = format!("refs/heads/{name}");
                if refs::read_ref_in(&self.git_dir, &self.common_dir, &refname)?.is_some() {
                    return Err(GitError::InvalidRef(format!(
                        "branch already exists: {name}"
                    )));
                }
                (name.to_string(), start, true)
            }
        };
        let refname = format!("refs/heads/{branch_name}");
        for existing in self.worktree_list()? {
            if existing.branch.as_deref() == Some(branch_name.as_str()) {
                return Err(GitError::InvalidRef(format!(
                    "branch {branch_name} is already checked out at {}",
                    existing.path.display()
                )));
            }
        }
        // Verify the start point is a commit before writing anything.
        let commit = self.read_commit(&tip)?;
        let tree = self.read_tree(&commit.tree)?;

        let registrations = self.common_dir.join("worktrees");
        fs::create_dir_all(&registrations)?;
        let base = match name {
            Some(name) => name.to_string(),
            None => path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
                .unwrap_or_default(),
        };
        let base = sanitize_name(&base);
        let mut registration_name = base.clone();
        let mut counter = 0u32;
        while registrations.join(&registration_name).exists() {
            counter += 1;
            registration_name = format!("{base}{counter}");
        }
        let private = registrations.join(&registration_name);
        let created_workdir = !path.exists();
        let result = (|| -> Result<(), GitError> {
            fs::create_dir(&private)?;
            fs::write(private.join("HEAD"), format!("ref: {refname}\n"))?;
            fs::write(private.join("commondir"), "../..\n")?;
            fs::write(
                private.join("gitdir"),
                format!("{}\n", path.join(".git").display()),
            )?;
            if create {
                refs::write_ref_in(&self.git_dir, &self.common_dir, &refname, &tip)?;
            }
            fs::create_dir_all(&path)?;
            fs::write(path.join(".git"), format!("gitdir: {}\n", private.display()))?;
            self.ensure_object_sources()?;
            let mut entries = Vec::new();
            let git_dir = self.common_dir.clone();
            let packs = &self.packs;
            let alts = &self.alternates;
            worktree::checkout_tree(
                &git_dir,
                &path,
                &tree,
                "",
                &mut |oid| read_tree_standalone(&git_dir, oid, packs, alts),
                &mut |oid| {
                    let obj = read_object_standalone(&git_dir, oid, packs, alts)?;
                    if obj.kind != ObjectKind::Blob {
                        return Err(GitError::InvalidObject("expected blob".into()));
                    }
                    Ok(obj.data)
                },
                &mut entries,
            )?;
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            crate::index::write_index(&private, &Index { version: 2, entries })?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&private);
            if created_workdir {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(path.join(".git"));
            }
            return Err(error);
        }
        Ok(LinkedWorktree {
            name: registration_name,
            path,
            git_dir: private,
            head: Some(tip),
            branch: Some(branch_name),
            locked: None,
            prunable: None,
            main: false,
        })
    }

    /// `git worktree list --porcelain`: the main worktree first, then every
    /// registration under `<common>/worktrees/` in name order, with its
    /// `locked` and `prunable` state.
    pub fn worktree_list(&self) -> Result<Vec<LinkedWorktree>, GitError> {
        let mut result = Vec::new();
        let main_path = if self.common_dir.file_name().is_some_and(|n| n == ".git") {
            self.common_dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.common_dir.clone())
        } else {
            self.common_dir.clone()
        };
        let (head, branch) = head_state(&self.common_dir, &self.common_dir);
        result.push(LinkedWorktree {
            name: String::new(),
            path: main_path,
            git_dir: self.common_dir.clone(),
            head,
            branch,
            locked: None,
            prunable: None,
            main: true,
        });
        let registrations = self.common_dir.join("worktrees");
        let mut names: Vec<String> = match fs::read_dir(&registrations) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(GitError::Io(e)),
        };
        names.sort();
        for name in names {
            let private = registrations.join(&name);
            let locked = match fs::read_to_string(private.join("locked")) {
                Ok(reason) => Some(reason.trim_end().to_string()),
                Err(_) => None,
            };
            let (path, prunable) = match fs::read_to_string(private.join("gitdir")) {
                Ok(content) => {
                    let pointer = content.lines().next().unwrap_or("").trim();
                    if pointer.is_empty() {
                        (private.clone(), Some("gitdir file is empty".to_string()))
                    } else {
                        let pointer = PathBuf::from(pointer);
                        let workdir = pointer
                            .parent()
                            .map(Path::to_path_buf)
                            .unwrap_or_else(|| pointer.clone());
                        let prunable = if !pointer.exists() {
                            Some("gitdir file points to non-existent location".to_string())
                        } else {
                            match resolve_gitfile(&pointer) {
                                Ok(target) if same_dir(&target, &private) => None,
                                _ => Some(
                                    "gitdir file points to a directory that is not this worktree"
                                        .to_string(),
                                ),
                            }
                        };
                        (workdir, prunable)
                    }
                }
                Err(_) => (private.clone(), Some("gitdir file does not exist".to_string())),
            };
            let prunable = if locked.is_some() { None } else { prunable };
            let (head, branch) = head_state(&private, &self.common_dir);
            result.push(LinkedWorktree {
                name,
                path,
                git_dir: private,
                head,
                branch,
                locked,
                prunable,
                main: false,
            });
        }
        Ok(result)
    }

    /// `git worktree remove`: delete a linked worktree's working directory
    /// and its registration. Without `force` a worktree with modified or
    /// untracked files, or a locked one, is refused. The branch is never
    /// touched. `name_or_path` is the registration name or the working
    /// directory.
    pub fn worktree_remove(&mut self, name_or_path: &str, force: bool) -> Result<(), GitError> {
        let entry = self.find_worktree(name_or_path)?;
        if entry.main {
            return Err(GitError::InvalidRef(
                "the main working tree cannot be removed".into(),
            ));
        }
        if entry.locked.is_some() && !force {
            return Err(GitError::InvalidRef(format!(
                "worktree {} is locked; use force to remove it",
                entry.name
            )));
        }
        let workdir_present = entry.path.join(".git").is_file();
        if workdir_present {
            // Only ever delete a directory whose .git file points back here.
            let target = resolve_gitfile(&entry.path.join(".git"))?;
            if !same_dir(&target, &entry.git_dir) {
                return Err(GitError::InvalidRef(format!(
                    "{} does not belong to worktree {}",
                    entry.path.display(),
                    entry.name
                )));
            }
            if !force {
                let mut linked =
                    Repository::open_git_dir(entry.git_dir.clone(), entry.path.clone())?;
                let status = linked.status()?;
                if let Some(first) = status.entries.first() {
                    return Err(GitError::InvalidRef(format!(
                        "worktree {} contains modified or untracked files ({}); use force to remove it",
                        entry.name, first.path
                    )));
                }
            }
            fs::remove_dir_all(&entry.path)?;
        }
        fs::remove_dir_all(&entry.git_dir)?;
        Ok(())
    }

    /// `git worktree prune`: drop registrations whose working directory is
    /// gone. Returns the pruned names.
    pub fn worktree_prune(&mut self) -> Result<Vec<String>, GitError> {
        let mut pruned = Vec::new();
        for entry in self.worktree_list()? {
            if entry.main || entry.prunable.is_none() {
                continue;
            }
            fs::remove_dir_all(&entry.git_dir)?;
            pruned.push(entry.name);
        }
        Ok(pruned)
    }

    fn find_worktree(&self, name_or_path: &str) -> Result<LinkedWorktree, GitError> {
        let list = self.worktree_list()?;
        if let Some(entry) = list.iter().find(|e| !e.main && e.name == name_or_path) {
            return Ok(entry.clone());
        }
        let wanted = Path::new(name_or_path);
        let canonical = wanted.canonicalize().ok();
        for entry in &list {
            if entry.path == wanted
                || canonical
                    .as_ref()
                    .is_some_and(|c| entry.path.canonicalize().ok().as_ref() == Some(c))
            {
                return Ok(entry.clone());
            }
        }
        Err(GitError::InvalidRef(format!(
            "no such worktree: {name_or_path}"
        )))
    }
}

fn head_state(git_dir: &Path, common_dir: &Path) -> (Option<ObjectId>, Option<String>) {
    let branch = match refs::read_head(git_dir) {
        Ok(RefTarget::Symbolic(name)) => name.strip_prefix("refs/heads/").map(str::to_string),
        _ => None,
    };
    let head = refs::resolve_head_in(git_dir, common_dir).ok();
    (head, branch)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn absolute(path: &Path) -> Result<PathBuf, GitError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    // Canonicalize the deepest existing ancestor so the recorded gitdir
    // pointer survives symlinked temp directories.
    let mut existing = path.clone();
    let mut tail = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => return Ok(path),
        }
        if !existing.pop() {
            return Ok(path);
        }
    }
    let mut out = existing.canonicalize()?;
    for name in tail.into_iter().rev() {
        out.push(name);
    }
    Ok(out)
}

fn validate_branch_name(name: &str) -> Result<(), GitError> {
    let bad = name.is_empty()
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name
            .bytes()
            .any(|b| b.is_ascii_control() || matches!(b, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\'));
    if bad {
        return Err(GitError::InvalidRef(format!("invalid branch name: {name}")));
    }
    Ok(())
}

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        "worktree".to_string()
    } else {
        trimmed.to_string()
    }
}
