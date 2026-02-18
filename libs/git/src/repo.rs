use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::commit::{parse_commit, serialize_commit, Commit, Signature};
use crate::diff::{self, TreeChange};
use crate::error::GitError;
use crate::index::{read_index, write_index, Index, IndexEntry};
use crate::merge::{self, MergeResult, TreeMergeEntry};
use crate::object::{read_loose_object, write_loose_object, Object, ObjectKind};
use crate::oid::ObjectId;
use crate::pack::{find_packs, read_pack_object, PackIndex, PackLookup};
use crate::refs::{self, RefTarget};
use crate::tree::{parse_tree, serialize_tree, Tree};
use crate::worktree;

/// A handle to a git repository on disk.
pub struct Repository {
    /// The working directory (parent of .git)
    pub workdir: PathBuf,
    /// The .git directory
    pub git_dir: PathBuf,
    /// Loaded pack indices (lazy — populated on first object lookup miss)
    packs: Option<Vec<PackIndex>>,
    /// Alternate object directories (from .git/objects/info/alternates)
    alternates: Option<Vec<PathBuf>>,
    /// Pack indices from alternate object stores
    alternate_packs: Option<Vec<PackIndex>>,
}

impl Repository {
    /// Open an existing repository. Walks up from `path` to find .git.
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let mut current = path.canonicalize().map_err(GitError::Io)?;
        loop {
            let git_dir = current.join(".git");
            if git_dir.is_dir() {
                return Ok(Repository {
                    workdir: current,
                    git_dir,
                    packs: None,
                    alternates: None,
                    alternate_packs: None,
                });
            }
            if !current.pop() {
                return Err(GitError::InvalidRef(format!(
                    "not a git repository: {}",
                    path.display()
                )));
            }
        }
    }

    /// Open with an explicit git_dir (for bare repos or testing).
    pub fn open_git_dir(git_dir: PathBuf, workdir: PathBuf) -> Self {
        Repository {
            workdir,
            git_dir,
            packs: None,
            alternates: None,
            alternate_packs: None,
        }
    }

    // --- Object Operations ---

    /// Read any object by OID. Tries loose objects first, then pack files,
    /// then alternates.
    pub fn read_object(&mut self, oid: &ObjectId) -> Result<Object, GitError> {
        // Try loose first
        match read_loose_object(&self.git_dir, oid) {
            Ok(obj) => return Ok(obj),
            Err(GitError::ObjectNotFound(_)) => {}
            Err(e) => return Err(e),
        }

        // Try local pack files
        self.ensure_packs()?;
        if let Some(packs) = &self.packs {
            for pack in packs {
                if let Some(offset) = pack.find_offset(oid) {
                    return read_pack_object(&pack.pack_path, offset, pack);
                }
            }
        }

        // Try alternates (loose + packs)
        self.ensure_alternates()?;
        if let Some(alt_dirs) = &self.alternates {
            for alt_dir in alt_dirs {
                // alt_dir is an objects/ directory — its parent is the git_dir
                match read_loose_object_from_objects_dir(alt_dir, oid) {
                    Ok(obj) => return Ok(obj),
                    Err(GitError::ObjectNotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        if let Some(alt_packs) = &self.alternate_packs {
            for pack in alt_packs {
                if let Some(offset) = pack.find_offset(oid) {
                    return read_pack_object(&pack.pack_path, offset, pack);
                }
            }
        }

        Err(GitError::ObjectNotFound(oid.to_hex()))
    }

    /// Write a raw object. Returns its OID.
    pub fn write_object(&self, kind: ObjectKind, data: &[u8]) -> Result<ObjectId, GitError> {
        write_loose_object(&self.git_dir, kind, data)
    }

    /// Read and parse a tree object.
    pub fn read_tree(&mut self, oid: &ObjectId) -> Result<Tree, GitError> {
        let obj = self.read_object(oid)?;
        if obj.kind != ObjectKind::Tree {
            return Err(GitError::InvalidObject(format!(
                "expected tree, got {:?}",
                obj.kind
            )));
        }
        parse_tree(&obj.data)
    }

    /// Write a tree object. Returns its OID.
    pub fn write_tree(&self, tree: &Tree) -> Result<ObjectId, GitError> {
        let data = serialize_tree(tree);
        self.write_object(ObjectKind::Tree, &data)
    }

    /// Read and parse a commit object.
    pub fn read_commit(&mut self, oid: &ObjectId) -> Result<Commit, GitError> {
        let obj = self.read_object(oid)?;
        if obj.kind != ObjectKind::Commit {
            return Err(GitError::InvalidObject(format!(
                "expected commit, got {:?}",
                obj.kind
            )));
        }
        parse_commit(&obj.data)
    }

    /// Write a commit object. Returns its OID.
    pub fn write_commit(&self, commit: &Commit) -> Result<ObjectId, GitError> {
        let data = serialize_commit(commit);
        self.write_object(ObjectKind::Commit, &data)
    }

    /// Read a blob's data.
    pub fn read_blob(&mut self, oid: &ObjectId) -> Result<Vec<u8>, GitError> {
        let obj = self.read_object(oid)?;
        if obj.kind != ObjectKind::Blob {
            return Err(GitError::InvalidObject(format!(
                "expected blob, got {:?}",
                obj.kind
            )));
        }
        Ok(obj.data)
    }

    /// Write a blob. Returns its OID.
    pub fn write_blob(&self, data: &[u8]) -> Result<ObjectId, GitError> {
        self.write_object(ObjectKind::Blob, data)
    }

    // --- Ref Operations ---

    /// Read HEAD (symbolic or direct).
    pub fn head(&self) -> Result<RefTarget, GitError> {
        refs::read_head(&self.git_dir)
    }

    /// Resolve HEAD to a concrete OID.
    pub fn head_oid(&self) -> Result<ObjectId, GitError> {
        refs::resolve_head(&self.git_dir)
    }

    /// Get the current branch name (e.g. "main"), or None if HEAD is detached.
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        match self.head()? {
            RefTarget::Symbolic(refname) => {
                Ok(refname.strip_prefix("refs/heads/").map(|s| s.to_string()))
            }
            RefTarget::Direct(_) => Ok(None),
        }
    }

    /// Resolve a ref name to an OID.
    pub fn resolve_ref(&self, name: &str) -> Result<ObjectId, GitError> {
        refs::resolve_ref(&self.git_dir, name)
    }

    /// List all branches.
    pub fn list_branches(&self) -> Result<Vec<refs::Ref>, GitError> {
        refs::list_refs(&self.git_dir, "refs/heads/")
    }

    /// List all tags.
    pub fn list_tags(&self) -> Result<Vec<refs::Ref>, GitError> {
        refs::list_refs(&self.git_dir, "refs/tags/")
    }

    /// Create a branch pointing at the given OID.
    pub fn create_branch(&self, name: &str, oid: &ObjectId) -> Result<(), GitError> {
        let refname = format!("refs/heads/{}", name);
        refs::write_ref(&self.git_dir, &refname, oid)
    }

    /// Delete a branch.
    pub fn delete_branch(&self, name: &str) -> Result<(), GitError> {
        let refname = format!("refs/heads/{}", name);
        refs::delete_ref(&self.git_dir, &refname)
    }

    /// Update HEAD to point at a branch.
    pub fn set_head_branch(&self, branch: &str) -> Result<(), GitError> {
        let target = RefTarget::Symbolic(format!("refs/heads/{}", branch));
        refs::update_head(&self.git_dir, &target)
    }

    /// Detach HEAD to a specific OID.
    pub fn detach_head(&self, oid: &ObjectId) -> Result<(), GitError> {
        let target = RefTarget::Direct(*oid);
        refs::update_head(&self.git_dir, &target)
    }

    // --- Index Operations ---

    /// Read the index (staging area).
    pub fn read_index(&self) -> Result<Index, GitError> {
        read_index(&self.git_dir)
    }

    /// Write the index.
    pub fn write_index(&self, index: &Index) -> Result<(), GitError> {
        write_index(&self.git_dir, index)
    }

    /// Build a tree hierarchy from the flat index and write all tree objects.
    /// Returns the root tree OID.
    pub fn index_to_tree(&mut self, index: &Index) -> Result<ObjectId, GitError> {
        // Group entries by directory
        let entries: Vec<(&str, &IndexEntry)> = index
            .entries
            .iter()
            .filter(|e| e.stage() == 0) // Only stage 0 entries
            .map(|e| (e.path.as_str(), e))
            .collect();
        self.build_tree_recursive(&entries, "")
    }

    fn build_tree_recursive(
        &mut self,
        entries: &[(&str, &IndexEntry)],
        prefix: &str,
    ) -> Result<ObjectId, GitError> {
        use crate::tree::TreeEntry;
        use std::collections::BTreeMap;

        let mut tree_entries: Vec<TreeEntry> = Vec::new();
        let mut subtrees: BTreeMap<String, Vec<(&str, &IndexEntry)>> = BTreeMap::new();

        for &(path, entry) in entries {
            let relative = if prefix.is_empty() {
                path
            } else if let Some(r) = path.strip_prefix(prefix) {
                r
            } else {
                continue;
            };

            if let Some(slash_pos) = relative.find('/') {
                let dir_name = &relative[..slash_pos];
                subtrees
                    .entry(dir_name.to_string())
                    .or_default()
                    .push((path, entry));
            } else {
                // Direct file entry
                tree_entries.push(TreeEntry {
                    mode: entry.mode,
                    name: relative.to_string(),
                    oid: entry.oid,
                });
            }
        }

        // Recursively build subtrees
        for (dir_name, sub_entries) in &subtrees {
            let sub_prefix = if prefix.is_empty() {
                format!("{}/", dir_name)
            } else {
                format!("{}{}/", prefix, dir_name)
            };
            let sub_oid = self.build_tree_recursive(sub_entries, &sub_prefix)?;
            tree_entries.push(TreeEntry {
                mode: 0o040000,
                name: dir_name.clone(),
                oid: sub_oid,
            });
        }

        let tree = Tree {
            entries: tree_entries,
        };
        self.write_tree(&tree)
    }

    // --- High-Level Operations ---

    /// Create a commit from the current index state.
    pub fn commit(&mut self, message: &str, author: Signature) -> Result<ObjectId, GitError> {
        let index = self.read_index()?;
        let tree_oid = self.index_to_tree(&index)?;

        // Get parent (current HEAD), if any
        let parents = match self.head_oid() {
            Ok(oid) => vec![oid],
            Err(GitError::RefNotFound(_)) => vec![], // initial commit
            Err(e) => return Err(e),
        };

        let commit = Commit {
            tree: tree_oid,
            parents,
            author: author.clone(),
            committer: author,
            message: message.to_string(),
        };

        let commit_oid = self.write_commit(&commit)?;

        // Update the current branch ref (or HEAD if detached)
        match self.head()? {
            RefTarget::Symbolic(refname) => {
                refs::write_ref(&self.git_dir, &refname, &commit_oid)?;
            }
            RefTarget::Direct(_) => {
                self.detach_head(&commit_oid)?;
            }
        }

        Ok(commit_oid)
    }

    /// Walk commit history from a starting OID.
    pub fn log(
        &mut self,
        start: &ObjectId,
        max_count: usize,
    ) -> Result<Vec<(ObjectId, Commit)>, GitError> {
        let mut result = Vec::new();
        let mut queue = vec![*start];
        let mut seen = std::collections::HashSet::new();

        while let Some(oid) = queue.pop() {
            if !seen.insert(oid) {
                continue;
            }
            if result.len() >= max_count {
                break;
            }

            let commit = self.read_commit(&oid)?;
            for parent in &commit.parents {
                queue.push(*parent);
            }
            result.push((oid, commit));
        }

        Ok(result)
    }

    // --- Diff Operations ---

    /// Diff two trees, returning the list of changes.
    pub fn diff_trees(
        &mut self,
        old_tree_oid: &ObjectId,
        new_tree_oid: &ObjectId,
    ) -> Result<Vec<TreeChange>, GitError> {
        let old_tree = self.read_tree(old_tree_oid)?;
        let new_tree = self.read_tree(new_tree_oid)?;
        // We need a closure that can call self.read_tree, but self is already borrowed.
        // Workaround: use the lower-level read_object to avoid borrowing self in closure.
        let git_dir = self.git_dir.clone();
        let packs_ref = &self.packs;
        let alts = &self.alternates;
        diff::diff_trees(&old_tree, &new_tree, "", &mut |oid| {
            let obj = read_tree_standalone(&git_dir, oid, packs_ref, alts)?;
            Ok(obj)
        })
    }

    /// Diff two commits (compares their trees).
    pub fn diff_commits(
        &mut self,
        old_commit_oid: &ObjectId,
        new_commit_oid: &ObjectId,
    ) -> Result<Vec<TreeChange>, GitError> {
        let old_commit = self.read_commit(old_commit_oid)?;
        let new_commit = self.read_commit(new_commit_oid)?;
        self.diff_trees(&old_commit.tree, &new_commit.tree)
    }

    // --- Worktree Operations ---

    /// Compute the full status of the working tree.
    pub fn status(&mut self) -> Result<worktree::Status, GitError> {
        let index = self.read_index()?;

        // Get HEAD tree files (empty map if no HEAD yet)
        let head_files = match self.head_oid() {
            Ok(head_oid) => {
                let commit = self.read_commit(&head_oid)?;
                let tree = self.read_tree(&commit.tree)?;
                let git_dir = self.git_dir.clone();
                let packs = &self.packs;
                let alts = &self.alternates;
                worktree::flatten_tree(&tree, "", &mut |oid| {
                    read_tree_standalone(&git_dir, oid, packs, alts)
                })?
            }
            Err(GitError::RefNotFound(_)) => HashMap::new(),
            Err(e) => return Err(e),
        };

        worktree::compute_status(&head_files, &index, &self.workdir)
    }

    /// Stage a file (add to index).
    pub fn stage_file(&mut self, path: &str) -> Result<(), GitError> {
        let mut index = self.read_index()?;
        worktree::stage_file(&self.git_dir, &self.workdir, &mut index, path)?;
        self.write_index(&index)
    }

    /// Unstage a file (remove from index).
    pub fn unstage_file(&mut self, path: &str) -> Result<(), GitError> {
        let mut index = self.read_index()?;
        worktree::unstage_file(&mut index, path);
        self.write_index(&index)
    }

    /// Checkout a branch: update HEAD, write tree to workdir, rebuild index.
    pub fn checkout_branch(&mut self, branch: &str) -> Result<(), GitError> {
        let refname = format!("refs/heads/{}", branch);
        let target_oid = refs::resolve_ref(&self.git_dir, &refname)?;
        let target_commit = self.read_commit(&target_oid)?;
        let target_tree = self.read_tree(&target_commit.tree)?;

        // Get current HEAD tree files for cleanup
        let old_files = match self.head_oid() {
            Ok(head_oid) => {
                let commit = self.read_commit(&head_oid)?;
                let tree = self.read_tree(&commit.tree)?;
                let git_dir = self.git_dir.clone();
                let packs = &self.packs;
                let alts = &self.alternates;
                worktree::flatten_tree(&tree, "", &mut |oid| {
                    read_tree_standalone(&git_dir, oid, packs, alts)
                })?
            }
            Err(GitError::RefNotFound(_)) => HashMap::new(),
            Err(e) => return Err(e),
        };

        // Get new tree files
        let git_dir = self.git_dir.clone();
        let packs = &self.packs;
        let alts = &self.alternates;
        let new_files = worktree::flatten_tree(&target_tree, "", &mut |oid| {
            read_tree_standalone(&git_dir, oid, packs, alts)
        })?;

        // Remove files that are in old but not in new
        worktree::remove_worktree_files(&self.workdir, &old_files, &new_files)?;

        // Checkout new tree
        let mut index_entries = Vec::new();
        let git_dir = self.git_dir.clone();
        let workdir = self.workdir.clone();
        worktree::checkout_tree(
            &git_dir,
            &workdir,
            &target_tree,
            "",
            &mut |oid| read_tree_standalone(&git_dir, oid, &self.packs, &self.alternates),
            &mut |oid| {
                let obj = read_object_standalone(&git_dir, oid, &self.packs, &self.alternates)?;
                if obj.kind != ObjectKind::Blob {
                    return Err(GitError::InvalidObject("expected blob".into()));
                }
                Ok(obj.data)
            },
            &mut index_entries,
        )?;

        // Sort index entries and write
        index_entries.sort_by(|a, b| a.path.cmp(&b.path));
        let index = Index {
            version: 2,
            entries: index_entries,
        };
        self.write_index(&index)?;

        // Update HEAD
        self.set_head_branch(branch)?;

        Ok(())
    }

    // --- Merge Operations ---

    /// Find the merge base of two commits.
    pub fn merge_base(
        &mut self,
        oid_a: &ObjectId,
        oid_b: &ObjectId,
    ) -> Result<Option<ObjectId>, GitError> {
        self.ensure_packs()?;
        let git_dir = self.git_dir.clone();
        let packs = &self.packs;
        let alts = &self.alternates;
        merge::find_merge_base(oid_a, oid_b, &mut |oid| {
            let obj = read_object_standalone(&git_dir, oid, packs, alts)?;
            if obj.kind != ObjectKind::Commit {
                return Err(GitError::InvalidObject("expected commit".into()));
            }
            parse_commit(&obj.data)
        })
    }

    /// Perform a merge of the given branch into the current branch.
    ///
    /// Returns Ok(commit_oid) for a clean merge, or Err with conflict info.
    /// For conflicting merges, the index is left with conflict entries (stages 1-3)
    /// and the working tree has conflict markers.
    pub fn merge_branch(
        &mut self,
        branch: &str,
        author: Signature,
    ) -> Result<MergeResult, GitError> {
        let ours_oid = self.head_oid()?;
        let refname = format!("refs/heads/{}", branch);
        let theirs_oid = refs::resolve_ref(&self.git_dir, &refname)?;

        // Fast-forward check
        let base_oid = self
            .merge_base(&ours_oid, &theirs_oid)?
            .ok_or_else(|| GitError::InvalidRef("no common ancestor".into()))?;

        if base_oid == theirs_oid {
            // Already up to date
            return Ok(MergeResult::Clean("Already up to date.".into()));
        }

        if base_oid == ours_oid {
            // Fast-forward: just move the branch pointer
            match self.head()? {
                RefTarget::Symbolic(refname) => {
                    refs::write_ref(&self.git_dir, &refname, &theirs_oid)?;
                }
                RefTarget::Direct(_) => {
                    self.detach_head(&theirs_oid)?;
                }
            }
            // Checkout the new tree
            let theirs_commit = self.read_commit(&theirs_oid)?;
            let theirs_tree = self.read_tree(&theirs_commit.tree)?;

            let old_files = {
                let ours_commit = self.read_commit(&ours_oid)?;
                let ours_tree = self.read_tree(&ours_commit.tree)?;
                let git_dir = self.git_dir.clone();
                let packs = &self.packs;
                let alts = &self.alternates;
                worktree::flatten_tree(&ours_tree, "", &mut |oid| {
                    read_tree_standalone(&git_dir, oid, packs, alts)
                })?
            };

            let git_dir = self.git_dir.clone();
            let packs = &self.packs;
            let alts = &self.alternates;
            let new_files = worktree::flatten_tree(&theirs_tree, "", &mut |oid| {
                read_tree_standalone(&git_dir, oid, packs, alts)
            })?;

            worktree::remove_worktree_files(&self.workdir, &old_files, &new_files)?;

            let mut index_entries = Vec::new();
            let git_dir = self.git_dir.clone();
            let workdir = self.workdir.clone();
            worktree::checkout_tree(
                &git_dir,
                &workdir,
                &theirs_tree,
                "",
                &mut |oid| read_tree_standalone(&git_dir, oid, &self.packs, &self.alternates),
                &mut |oid| {
                    let obj = read_object_standalone(&git_dir, oid, &self.packs, &self.alternates)?;
                    Ok(obj.data)
                },
                &mut index_entries,
            )?;
            index_entries.sort_by(|a, b| a.path.cmp(&b.path));
            self.write_index(&Index {
                version: 2,
                entries: index_entries,
            })?;

            return Ok(MergeResult::Clean(format!(
                "Fast-forward to {}",
                theirs_oid
            )));
        }

        // True three-way merge
        let base_commit = self.read_commit(&base_oid)?;
        let ours_commit = self.read_commit(&ours_oid)?;
        let theirs_commit = self.read_commit(&theirs_oid)?;

        let base_tree = self.read_tree(&base_commit.tree)?;
        let ours_tree = self.read_tree(&ours_commit.tree)?;
        let theirs_tree = self.read_tree(&theirs_commit.tree)?;

        let git_dir = self.git_dir.clone();
        let packs = &self.packs;
        let alts = &self.alternates;
        let base_files = merge::flatten_tree_with_mode(&base_tree, "", &mut |oid| {
            read_tree_standalone(&git_dir, oid, packs, alts)
        })?;
        let ours_files = merge::flatten_tree_with_mode(&ours_tree, "", &mut |oid| {
            read_tree_standalone(&git_dir, oid, packs, alts)
        })?;
        let theirs_files = merge::flatten_tree_with_mode(&theirs_tree, "", &mut |oid| {
            read_tree_standalone(&git_dir, oid, packs, alts)
        })?;

        let merge_entries = merge::merge_trees(&base_files, &ours_files, &theirs_files);

        let mut has_conflicts = false;
        let mut index = Index {
            version: 2,
            entries: Vec::new(),
        };

        for entry in &merge_entries {
            match entry {
                TreeMergeEntry::Resolved { path, oid, mode } => {
                    // Write blob to worktree
                    let data = self.read_blob(oid)?;
                    let file_path = self.workdir.join(path);
                    if let Some(parent) = file_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&file_path, &data)?;

                    index.entries.push(make_index_entry(path, *oid, *mode));
                }
                TreeMergeEntry::BothModified {
                    path,
                    base_oid,
                    ours_oid,
                    theirs_oid,
                    mode,
                } => {
                    let base_data = self.read_blob(base_oid)?;
                    let ours_data = self.read_blob(ours_oid)?;
                    let theirs_data = self.read_blob(theirs_oid)?;

                    let base_text = String::from_utf8_lossy(&base_data);
                    let ours_text = String::from_utf8_lossy(&ours_data);
                    let theirs_text = String::from_utf8_lossy(&theirs_data);

                    let merge_result = merge::merge3_text(&base_text, &ours_text, &theirs_text);

                    let file_path = self.workdir.join(path);
                    if let Some(parent) = file_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&file_path, merge_result.content())?;

                    if merge_result.has_conflict() {
                        has_conflicts = true;
                        // Write conflict stages (1=base, 2=ours, 3=theirs)
                        let mut e1 = make_index_entry(path, *base_oid, *mode);
                        e1.flags = (e1.flags & 0x0FFF) | (1 << 12);
                        let mut e2 = make_index_entry(path, *ours_oid, *mode);
                        e2.flags = (e2.flags & 0x0FFF) | (2 << 12);
                        let mut e3 = make_index_entry(path, *theirs_oid, *mode);
                        e3.flags = (e3.flags & 0x0FFF) | (3 << 12);
                        index.entries.push(e1);
                        index.entries.push(e2);
                        index.entries.push(e3);
                    } else {
                        // Write merged content as blob
                        let merged_oid = self.write_blob(merge_result.content().as_bytes())?;
                        index
                            .entries
                            .push(make_index_entry(path, merged_oid, *mode));
                    }
                }
                TreeMergeEntry::AddAdd {
                    path,
                    ours_oid,
                    theirs_oid,
                    mode,
                } => {
                    has_conflicts = true;
                    let ours_data = self.read_blob(ours_oid)?;
                    let theirs_data = self.read_blob(theirs_oid)?;

                    // Write conflict markers
                    let content = format!(
                        "<<<<<<< ours\n{}=======\n{}>>>>>>> theirs\n",
                        String::from_utf8_lossy(&ours_data),
                        String::from_utf8_lossy(&theirs_data),
                    );
                    let file_path = self.workdir.join(path);
                    if let Some(parent) = file_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&file_path, &content)?;

                    let mut e2 = make_index_entry(path, *ours_oid, *mode);
                    e2.flags = (e2.flags & 0x0FFF) | (2 << 12);
                    let mut e3 = make_index_entry(path, *theirs_oid, *mode);
                    e3.flags = (e3.flags & 0x0FFF) | (3 << 12);
                    index.entries.push(e2);
                    index.entries.push(e3);
                }
                TreeMergeEntry::DeleteModify {
                    path,
                    surviving_oid,
                    mode,
                    deleted_by_ours,
                } => {
                    has_conflicts = true;
                    let data = self.read_blob(surviving_oid)?;
                    let file_path = self.workdir.join(path);
                    if let Some(parent) = file_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&file_path, &data)?;

                    let stage = if *deleted_by_ours { 3 } else { 2 };
                    let mut e = make_index_entry(path, *surviving_oid, *mode);
                    e.flags = (e.flags & 0x0FFF) | (stage << 12);
                    index.entries.push(e);
                }
            }
        }

        index
            .entries
            .sort_by(|a, b| a.path.cmp(&b.path).then(a.stage().cmp(&b.stage())));
        self.write_index(&index)?;

        if has_conflicts {
            // Write MERGE_HEAD so git knows we're in a merge
            let merge_head_path = self.git_dir.join("MERGE_HEAD");
            std::fs::write(&merge_head_path, format!("{}\n", theirs_oid))?;
            let merge_msg_path = self.git_dir.join("MERGE_MSG");
            std::fs::write(
                &merge_msg_path,
                format!("Merge branch '{}'\n\nConflicts:\n", branch),
            )?;

            Ok(MergeResult::Conflict(
                "Merge conflict — resolve and commit.".into(),
            ))
        } else {
            // Clean merge — create merge commit
            let tree_oid = self.index_to_tree(&index)?;
            let commit = Commit {
                tree: tree_oid,
                parents: vec![ours_oid, theirs_oid],
                author: author.clone(),
                committer: author,
                message: format!("Merge branch '{}'\n", branch),
            };
            let commit_oid = self.write_commit(&commit)?;

            match self.head()? {
                RefTarget::Symbolic(refname) => {
                    refs::write_ref(&self.git_dir, &refname, &commit_oid)?;
                }
                RefTarget::Direct(_) => {
                    self.detach_head(&commit_oid)?;
                }
            }

            Ok(MergeResult::Clean(format!(
                "Merge made by three-way strategy. Commit: {}",
                commit_oid
            )))
        }
    }

    // --- Private ---

    fn ensure_packs(&mut self) -> Result<(), GitError> {
        if self.packs.is_none() {
            let mut packs = find_packs(&self.git_dir)?;
            // Also load packs from alternates so all standalone reads work
            let alt_file = self.git_dir.join("objects/info/alternates");
            if let Ok(content) = std::fs::read_to_string(&alt_file) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let path = PathBuf::from(line);
                    let pack_dir = path.join("pack");
                    if pack_dir.is_dir() {
                        if let Ok(entries) = std::fs::read_dir(&pack_dir) {
                            for entry in entries.flatten() {
                                let p = entry.path();
                                if p.extension().map(|e| e == "idx").unwrap_or(false) {
                                    if let Ok(idx) = crate::pack::read_pack_index(&p) {
                                        packs.push(idx);
                                    }
                                }
                            }
                        }
                    }
                    // Store alternate dir for loose object lookups
                    if self.alternates.is_none() {
                        self.alternates = Some(Vec::new());
                    }
                    if let Some(ref mut alts) = self.alternates {
                        alts.push(path);
                    }
                }
            }
            self.packs = Some(packs);
        }
        Ok(())
    }

    fn ensure_alternates(&mut self) -> Result<(), GitError> {
        if self.alternates.is_some() {
            return Ok(());
        }
        let alt_file = self.git_dir.join("objects/info/alternates");
        let content = match std::fs::read_to_string(&alt_file) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.alternates = Some(Vec::new());
                self.alternate_packs = Some(Vec::new());
                return Ok(());
            }
            Err(e) => return Err(GitError::Io(e)),
        };
        let mut dirs = Vec::new();
        let mut packs = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let path = PathBuf::from(line);
            if path.is_dir() {
                // Load packs from this alternate
                let pack_dir = path.join("pack");
                if pack_dir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&pack_dir) {
                        for entry in entries {
                            if let Ok(entry) = entry {
                                let p = entry.path();
                                if p.extension().map(|e| e == "idx").unwrap_or(false) {
                                    if let Ok(idx) = crate::pack::read_pack_index(&p) {
                                        packs.push(idx);
                                    }
                                }
                            }
                        }
                    }
                }
                dirs.push(path);
            }
        }
        self.alternates = Some(dirs);
        self.alternate_packs = Some(packs);
        Ok(())
    }
}

/// Read a loose object directly from an objects/ directory (for alternates).
fn read_loose_object_from_objects_dir(
    objects_dir: &Path,
    oid: &ObjectId,
) -> Result<Object, GitError> {
    let (dir, file) = oid.loose_path_components();
    let path = objects_dir.join(&dir).join(&file);

    let compressed = std::fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::ObjectNotFound(oid.to_hex())
        } else {
            GitError::Io(e)
        }
    })?;

    use flate2::read::ZlibDecoder;
    use std::io::Read;
    let mut decoder = ZlibDecoder::new(&compressed[..]);
    let mut raw = Vec::new();
    decoder
        .read_to_end(&mut raw)
        .map_err(|e| GitError::InvalidObject(format!("zlib failed for {}: {}", oid, e)))?;

    let null_pos = raw
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| GitError::InvalidObject(format!("no null in header for {}", oid)))?;
    let header = std::str::from_utf8(&raw[..null_pos])
        .map_err(|_| GitError::InvalidObject(format!("bad header for {}", oid)))?;
    let space_pos = header
        .find(' ')
        .ok_or_else(|| GitError::InvalidObject(format!("no space in header for {}", oid)))?;
    let kind = ObjectKind::from_str(&header[..space_pos])?;
    let data = raw[null_pos + 1..].to_vec();
    Ok(Object { kind, data })
}

/// Read an object without requiring &mut Repository — used in closures.
/// Checks local loose, local packs (which includes alternate packs after ensure_packs),
/// and alternate loose dirs.
fn read_object_standalone(
    git_dir: &Path,
    oid: &ObjectId,
    packs: &Option<Vec<PackIndex>>,
    alternates: &Option<Vec<PathBuf>>,
) -> Result<Object, GitError> {
    match read_loose_object(git_dir, oid) {
        Ok(obj) => return Ok(obj),
        Err(GitError::ObjectNotFound(_)) => {}
        Err(e) => return Err(e),
    }
    if let Some(packs) = packs {
        for pack in packs {
            if let Some(offset) = pack.find_offset(oid) {
                return read_pack_object(&pack.pack_path, offset, pack);
            }
        }
    }
    if let Some(alt_dirs) = alternates {
        for alt_dir in alt_dirs {
            match read_loose_object_from_objects_dir(alt_dir, oid) {
                Ok(obj) => return Ok(obj),
                Err(GitError::ObjectNotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
    }
    Err(GitError::ObjectNotFound(oid.to_hex()))
}

/// Read and parse a tree object without requiring &mut Repository.
fn read_tree_standalone(
    git_dir: &Path,
    oid: &ObjectId,
    packs: &Option<Vec<PackIndex>>,
    alternates: &Option<Vec<PathBuf>>,
) -> Result<Tree, GitError> {
    let obj = read_object_standalone(git_dir, oid, packs, alternates)?;
    if obj.kind != ObjectKind::Tree {
        return Err(GitError::InvalidObject(format!(
            "expected tree, got {:?}",
            obj.kind
        )));
    }
    parse_tree(&obj.data)
}

/// Create a minimal IndexEntry for merge results.
fn make_index_entry(path: &str, oid: ObjectId, mode: u32) -> IndexEntry {
    IndexEntry {
        ctime_sec: 0,
        ctime_nsec: 0,
        mtime_sec: 0,
        mtime_nsec: 0,
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        oid,
        flags: (path.len().min(0xFFF)) as u16,
        path: path.to_string(),
    }
}
