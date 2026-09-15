use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::commit::{parse_commit, serialize_commit, Commit, Signature};
use crate::diff::{self, TreeChange};
use crate::error::GitError;
use crate::index::{read_index, write_index, Index, IndexEntry};
use crate::merge::{self, MergeResult, TreeMergeEntry};
use crate::object::{read_loose_object, write_loose_object, Object, ObjectKind};
use crate::oid::ObjectId;
use crate::pack::{find_packs, find_packs_in_objects_dir, read_pack_object, PackIndex, PackLookup};
use crate::refs::{self, RefTarget};
use crate::tree::{parse_tree, serialize_tree, Tree};
use crate::worktree;

/// A handle to a git repository on disk.
pub struct Repository {
    /// The working directory (parent of .git)
    pub workdir: PathBuf,
    /// The worktree-private git directory: `HEAD`, the index, merge state and
    /// the per-worktree ref namespaces. For an ordinary repository this is
    /// `.git`; for a linked worktree it is `<common>/worktrees/<name>`.
    pub git_dir: PathBuf,
    /// The shared git directory: objects, packs, alternates, shared refs and
    /// `packed-refs`. Equal to `git_dir` for an ordinary repository.
    pub common_dir: PathBuf,
    /// Loaded pack indices (lazy — populated on first object lookup miss)
    pub(crate) packs: Option<Vec<PackIndex>>,
    /// Alternate object directories (from .git/objects/info/alternates)
    pub(crate) alternates: Option<Vec<PathBuf>>,
    /// Pack indices from alternate object stores
    alternate_packs: Option<Vec<PackIndex>>,
    read_cache: Option<ReadCache>,
    blob_reads: u64,
}

struct ReadCache {
    account: crate::memory::MemoryAccount,
    budget: usize,
    used: usize,
    entries: HashMap<ObjectId, (Object, crate::memory::Reservation)>,
    order: std::collections::VecDeque<ObjectId>,
    hits: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RepositoryReadStats {
    pub blob_reads: u64,
    pub cache_hits: u64,
    pub cache_bytes: usize,
    pub pack_index_bytes: usize,
    pub pack_load_ms: f64,
    pub inflate_ms: f64,
    pub pack_bytes_read: u64,
    pub objects_inflated: u64,
}

/// The directories a working directory maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPaths {
    pub workdir: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

/// Resolve the `.git` entry directly inside `root`. `Ok(None)` means `root`
/// has no `.git` entry at all; a `.git` file or directory that is malformed
/// is an error, never a reason to look further up.
pub fn repository_paths(root: &Path) -> Result<Option<RepositoryPaths>, GitError> {
    let dot_git = root.join(".git");
    let link = match std::fs::symlink_metadata(&dot_git) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(GitError::Io(e)),
    };
    // A `.git` entry exists; from here on every problem is an error, never a
    // reason to look further up. A dangling symlink is such a problem.
    let metadata = std::fs::metadata(&dot_git).map_err(|e| {
        if link.file_type().is_symlink() {
            GitError::InvalidRef(format!("dangling .git symlink: {}", dot_git.display()))
        } else {
            GitError::Io(e)
        }
    })?;
    let git_dir = if metadata.is_dir() {
        dot_git
    } else if metadata.is_file() {
        resolve_gitfile(&dot_git)?
    } else {
        return Err(GitError::InvalidRef(format!(
            "unsupported .git entry: {}",
            dot_git.display()
        )));
    };
    let common_dir = resolve_commondir(&git_dir)?;
    Ok(Some(RepositoryPaths {
        workdir: root.to_path_buf(),
        git_dir,
        common_dir,
    }))
}

/// Resolve a `.git` file (`gitdir: <path>`) to the git directory it points
/// at. A relative path is taken from the file's parent directory. The
/// destination must exist and be a git directory; chained gitfiles and
/// self references are rejected.
pub fn resolve_gitfile(path: &Path) -> Result<PathBuf, GitError> {
    let content = std::fs::read_to_string(path)?;
    let line = content.lines().next().unwrap_or("").trim();
    let target = line.strip_prefix("gitdir:").map(str::trim).ok_or_else(|| {
        GitError::InvalidRef(format!("{}: not a gitfile", path.display()))
    })?;
    if target.is_empty() {
        return Err(GitError::InvalidRef(format!(
            "{}: empty gitdir pointer",
            path.display()
        )));
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let target = Path::new(target);
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        base.join(target)
    };
    // Filesystem semantics: `..` and symlinks resolve the way the kernel
    // resolves them, and the destination has to exist.
    let resolved = joined.canonicalize().map_err(|_| {
        GitError::InvalidRef(format!(
            "{}: gitdir destination missing: {}",
            path.display(),
            joined.display()
        ))
    })?;
    if !resolved.is_dir() {
        return Err(GitError::InvalidRef(format!(
            "{}: gitdir points at a file (chained gitfiles are not supported)",
            path.display()
        )));
    }
    let self_reference = base.canonicalize().map(|b| b == resolved).unwrap_or(false);
    let looks_like_git_dir = resolved.join("HEAD").is_file()
        && (resolved.join("objects").is_dir() || resolved.join("commondir").is_file());
    if self_reference || !looks_like_git_dir {
        return Err(GitError::InvalidRef(format!(
            "{}: gitdir destination is not a git directory: {}",
            path.display(),
            resolved.display()
        )));
    }
    Ok(resolved)
}

/// Resolve the shared git directory of `git_dir`: the `commondir` file of a
/// linked worktree, or `git_dir` itself when there is none.
pub fn resolve_commondir(git_dir: &Path) -> Result<PathBuf, GitError> {
    let file = git_dir.join("commondir");
    let content = match std::fs::read_to_string(&file) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(git_dir.to_path_buf()),
        Err(e) => return Err(GitError::Io(e)),
    };
    let target = content.lines().next().unwrap_or("").trim();
    if target.is_empty() {
        return Err(GitError::InvalidRef(format!(
            "{}: empty commondir pointer",
            file.display()
        )));
    }
    let target = Path::new(target);
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        git_dir.join(target)
    };
    let resolved = joined.canonicalize().map_err(|_| {
        GitError::InvalidRef(format!(
            "{}: commondir destination missing: {}",
            file.display(),
            joined.display()
        ))
    })?;
    if !resolved.is_dir() {
        return Err(GitError::InvalidRef(format!(
            "{}: commondir destination is not a directory: {}",
            file.display(),
            resolved.display()
        )));
    }
    // A pointer file that resolves back to the private directory is a cycle
    // (`commondir = .`), and so is a destination with its own pointer.
    if git_dir.canonicalize().map(|g| g == resolved).unwrap_or(false) {
        return Err(GitError::InvalidRef(format!(
            "{}: commondir points at the private git directory itself",
            file.display()
        )));
    }
    if resolved.join("commondir").is_file() {
        return Err(GitError::InvalidRef(format!(
            "{}: chained commondir pointers are not supported",
            file.display()
        )));
    }
    let has_refs = resolved.join("refs").is_dir() || resolved.join("packed-refs").is_file();
    if !resolved.join("objects").is_dir() || !has_refs {
        return Err(GitError::InvalidRef(format!(
            "{}: commondir destination is not a git directory: {}",
            file.display(),
            resolved.display()
        )));
    }
    Ok(resolved)
}

/// Lexically remove `.` components and cancel `..` against a preceding
/// normal component only; leading or unmatched `..` are kept. Used for paths
/// that may not exist yet; existing paths go through `canonicalize`.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let cancels = matches!(
                    out.components().next_back(),
                    Some(Component::Normal(_))
                );
                if cancels {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve one line of `objects/info/alternates`; relative entries are taken
/// from the common `objects` directory. An existing destination resolves with
/// filesystem semantics; a missing one keeps its unresolved shape.
fn resolve_alternate(common_dir: &Path, line: &str) -> PathBuf {
    let path = Path::new(line);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        common_dir.join("objects").join(path)
    };
    joined.canonicalize().unwrap_or_else(|_| normalize_path(&joined))
}

/// Every place objects can come from: the common `objects` directory, the
/// alternates it names (one level, as git's reader does) and all their packs.
pub(crate) struct ObjectSources {
    /// `objects` directories to try for loose objects, primary first.
    pub loose_dirs: Vec<PathBuf>,
    /// The alternate `objects` directories only.
    pub alternate_dirs: Vec<PathBuf>,
    /// Packs of the primary store and of every alternate.
    pub packs: Vec<PackIndex>,
}

impl ObjectSources {
    pub(crate) fn read(&self, oid: &ObjectId) -> Result<Object, GitError> {
        for dir in &self.loose_dirs {
            match read_loose_object_from_objects_dir(dir, oid) {
                Ok(obj) => return Ok(obj),
                Err(GitError::ObjectNotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        for pack in &self.packs {
            if let Some(offset) = pack.find_offset(oid) {
                return read_pack_object(&pack.pack_path, offset, pack);
            }
        }
        Err(GitError::ObjectNotFound(oid.to_hex()))
    }
}

pub(crate) fn object_sources(common_dir: &Path) -> Result<ObjectSources, GitError> {
    let primary = common_dir.join("objects");
    let mut loose_dirs = vec![primary.clone()];
    let mut alternate_dirs = Vec::new();
    let mut packs = find_packs(common_dir)?;
    let alt_file = primary.join("info").join("alternates");
    match std::fs::read_to_string(&alt_file) {
        Ok(content) => {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let dir = resolve_alternate(common_dir, line);
                if !dir.is_dir() {
                    continue;
                }
                packs.extend(find_packs_in_objects_dir(&dir)?);
                loose_dirs.push(dir.clone());
                alternate_dirs.push(dir);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(GitError::Io(e)),
    }
    Ok(ObjectSources {
        loose_dirs,
        alternate_dirs,
        packs,
    })
}

impl Repository {
    fn from_paths(paths: RepositoryPaths) -> Self {
        Repository {
            workdir: paths.workdir,
            git_dir: paths.git_dir,
            common_dir: paths.common_dir,
            packs: None,
            alternates: None,
            alternate_packs: None,
            read_cache: None,
            blob_reads: 0,
        }
    }

    /// Open an existing repository. Walks up from `path` to find `.git`,
    /// which may be a directory or a linked worktree's gitfile; an explicit
    /// `.git` path is accepted as well. A malformed `.git` entry is an error
    /// and never falls through to an ancestor repository.
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let start = path.canonicalize().map_err(GitError::Io)?;
        if start.file_name().is_some_and(|name| name == ".git") {
            if let Some(workdir) = start.parent() {
                if let Some(paths) = repository_paths(workdir)? {
                    return Ok(Self::from_paths(paths));
                }
            }
        }
        let mut current = start;
        loop {
            if let Some(paths) = repository_paths(&current)? {
                return Ok(Self::from_paths(paths));
            }
            if !current.pop() {
                return Err(GitError::InvalidRef(format!(
                    "not a git repository: {}",
                    path.display()
                )));
            }
        }
    }

    /// Open with an explicit git_dir (for bare repos, linked worktrees or
    /// testing). The shared directory comes from `git_dir`'s `commondir`.
    pub fn open_git_dir(git_dir: PathBuf, workdir: PathBuf) -> Result<Self, GitError> {
        let common_dir = resolve_commondir(&git_dir)?;
        Ok(Self::from_paths(RepositoryPaths {
            workdir,
            git_dir,
            common_dir,
        }))
    }

    // --- Object Operations ---

    /// Read any object by OID. Tries loose objects first, then pack files,
    /// then alternates.
    /// Configure a persistent worker reader. Pack reads use bounded windows;
    /// decoded read-cache entries reserve from the same history account.
    pub fn set_read_cache_budget(&mut self, account: crate::memory::MemoryAccount, bytes: usize) -> Result<(), GitError> {
        self.read_cache = Some(ReadCache { account: account.clone(), budget: bytes, used: 0, entries: HashMap::new(), order: std::collections::VecDeque::new(), hits: 0 });
        for pack in self.packs.iter_mut().chain(self.alternate_packs.iter_mut()).flatten() { pack.set_read_budget(account.clone(), bytes)?; }
        Ok(())
    }
    pub fn read_stats(&self) -> RepositoryReadStats {
        let mut stats = RepositoryReadStats { blob_reads: self.blob_reads, ..Default::default() };
        if let Some(cache) = &self.read_cache { stats.cache_hits = cache.hits; stats.cache_bytes = cache.used; }
        for pack in self.packs.iter().chain(self.alternate_packs.iter()).flatten() {
            let p = pack.read_phases(); stats.pack_load_ms += p.pack_load_ms; stats.inflate_ms += p.inflate_ms; stats.pack_bytes_read += p.pack_bytes_read; stats.objects_inflated += p.objects_inflated; stats.pack_index_bytes += pack.retained_bytes();
        }
        stats
    }
    pub fn clear_read_cache(&mut self) {
        if let Some(cache) = &mut self.read_cache { cache.entries = HashMap::new(); cache.order = std::collections::VecDeque::new(); cache.used = 0; }
    }
    pub fn read_object(&mut self, oid: &ObjectId) -> Result<Object, GitError> {
        if let Some(cache) = &mut self.read_cache {
            if let Some((obj, _)) = cache.entries.get(oid) { cache.hits += 1; return Ok(obj.clone()); }
        }
        let object = self.read_object_uncached(oid)?;
        if let Some(cache) = &mut self.read_cache {
            let bytes = object.data.len().saturating_add(256);
            if bytes <= cache.budget {
                while cache.used.saturating_add(bytes) > cache.budget {
                    let Some(old) = cache.order.pop_front() else { break };
                    if let Some((_, lease)) = cache.entries.remove(&old) { cache.used = cache.used.saturating_sub(lease.bytes()); }
                }
                if let Some(lease) = cache.account.try_reserve(bytes) {
                    cache.entries.insert(*oid, (object.clone(), lease)); cache.order.push_back(*oid); cache.used += bytes;
                }
            }
        }
        Ok(object)
    }
    fn read_object_uncached(&mut self, oid: &ObjectId) -> Result<Object, GitError> {
        // Try loose first
        match read_loose_object(&self.common_dir, oid) {
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
        write_loose_object(&self.common_dir, kind, data)
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
        self.blob_reads += 1;
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
        refs::resolve_head_in(&self.git_dir, &self.common_dir)
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
        refs::resolve_ref_in(&self.git_dir, &self.common_dir, name)
    }

    /// List all branches.
    pub fn list_branches(&self) -> Result<Vec<refs::Ref>, GitError> {
        refs::list_refs_in(&self.git_dir, &self.common_dir, "refs/heads/")
    }

    /// List all tags.
    pub fn list_tags(&self) -> Result<Vec<refs::Ref>, GitError> {
        refs::list_refs_in(&self.git_dir, &self.common_dir, "refs/tags/")
    }

    /// Create a branch pointing at the given OID.
    pub fn create_branch(&self, name: &str, oid: &ObjectId) -> Result<(), GitError> {
        let refname = format!("refs/heads/{}", name);
        refs::write_ref_in(&self.git_dir, &self.common_dir, &refname, oid)
    }

    /// Delete a branch.
    pub fn delete_branch(&self, name: &str) -> Result<(), GitError> {
        let refname = format!("refs/heads/{}", name);
        refs::delete_ref_in(&self.git_dir, &self.common_dir, &refname)
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
        // A repository (or a fresh linked worktree) without an index file has
        // an empty index.
        match read_index(&self.git_dir) {
            Err(GitError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index {
                version: 2,
                entries: Vec::new(),
            }),
            other => other,
        }
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
                refs::write_ref_in(&self.git_dir, &self.common_dir, &refname, &commit_oid)?;
            }
            RefTarget::Direct(_) => {
                self.detach_head(&commit_oid)?;
            }
        }

        Ok(commit_oid)
    }

    /// Walk commit history from a starting OID in topological order: every
    /// commit is emitted before any of its parents, and among commits with no
    /// ordering constraint the newer committer timestamp comes first. This is
    /// `git log --topo-order` with `--date-order` tie-breaking: a merge diamond
    /// `c3(c1, c2) -> c0` yields `c3, c2, c1, c0` (or `c3, c1, c2, c0`),
    /// never the shared ancestor before one of its children.
    ///
    /// The order is exact whatever the committer clocks say: the walk first
    /// discovers every commit reachable from `start`, reading each object
    /// once, then releases a commit only when every reachable child has been
    /// emitted (Kahn's algorithm over a newest-first heap). No clock-skew
    /// heuristic bounds the read set, because none can: a child may carry any
    /// timestamp, and proving a commit childless requires knowing the whole
    /// reachable set. Callers that scrub history keep the result (or the
    /// `Timeline` built from it) rather than calling this per step; a 3,000
    /// commit history walks in tens of milliseconds in release builds.
    ///
    /// Shallow boundaries (`<common>/shallow`) are honoured: a listed commit
    /// is treated as parentless and nothing beyond it is read. A parent
    /// object missing for any other reason is an error, never a silent gap.
    /// `max_count` bounds the emitted list; zero yields an empty list.
    pub fn log(
        &mut self,
        start: &ObjectId,
        max_count: usize,
    ) -> Result<Vec<(ObjectId, Commit)>, GitError> {
        use std::cmp::Ordering as CmpOrdering;
        use std::collections::{hash_map::Entry, BinaryHeap, HashMap};

        /// Heap entry ordered so that the newest committer time pops first;
        /// the discovery sequence breaks timestamp ties (earlier first).
        #[derive(PartialEq, Eq)]
        struct Newest {
            timestamp: i64,
            seq: u64,
            oid: ObjectId,
        }
        impl PartialOrd for Newest {
            fn partial_cmp(&self, o: &Self) -> Option<CmpOrdering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Newest {
            fn cmp(&self, o: &Self) -> CmpOrdering {
                self.timestamp
                    .cmp(&o.timestamp)
                    .then_with(|| o.seq.cmp(&self.seq))
            }
        }

        let mut result = Vec::new();
        if max_count == 0 {
            return Ok(result);
        }
        let shallow = self.shallow_boundary()?;

        // Phase 1: discover the reachable set. `pending_children` counts, per
        // commit, the parent edges pointing at it from reachable commits; a
        // duplicated parent entry counts twice and is released twice.
        let mut commits: HashMap<ObjectId, (u64, Commit)> = HashMap::new();
        let mut pending_children: HashMap<ObjectId, usize> = HashMap::new();
        let mut stack: Vec<ObjectId> = vec![*start];
        commits.insert(*start, (0, self.read_commit(start)?));
        pending_children.insert(*start, 0);
        let mut next_seq = 1u64;
        while let Some(oid) = stack.pop() {
            if shallow.contains(&oid) {
                continue;
            }
            let parents = commits[&oid].1.parents.clone();
            for parent in parents {
                *pending_children.entry(parent).or_insert(0) += 1;
                if let Entry::Vacant(v) = commits.entry(parent) {
                    v.insert((next_seq, self.read_commit(&parent)?));
                    next_seq += 1;
                    stack.push(parent);
                }
            }
        }

        // Phase 2: release commits newest-first once no reachable child is
        // left. `start` is the only commit without a child in the set.
        let mut ready: BinaryHeap<Newest> = BinaryHeap::new();
        ready.push(Newest { timestamp: commits[start].1.committer.timestamp, seq: 0, oid: *start });
        while let Some(next) = ready.pop() {
            let (_, commit) = commits.remove(&next.oid).expect("discovered commit");
            if !shallow.contains(&next.oid) {
                for parent in &commit.parents {
                    let left = pending_children.get_mut(parent).expect("counted edge");
                    *left -= 1;
                    if *left == 0 {
                        let (seq, pc) = &commits[parent];
                        ready.push(Newest { timestamp: pc.committer.timestamp, seq: *seq, oid: *parent });
                    }
                }
            }
            result.push((next.oid, commit));
            if result.len() >= max_count {
                break;
            }
        }
        Ok(result)
    }

    /// The commits listed in `<common>/shallow`, treated as parentless by
    /// history walks. Empty for a full clone.
    fn shallow_boundary(&self) -> Result<std::collections::HashSet<ObjectId>, GitError> {
        let path = self.common_dir.join("shallow");
        let mut set = std::collections::HashSet::new();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                for line in text.lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        set.insert(ObjectId::from_hex(line)?);
                    }
                }
                Ok(set)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(set),
            Err(e) => Err(GitError::Io(e)),
        }
    }

    // --- Diff Operations ---

    /// Blob-free, admitted tree comparison. `None` is the empty tree. All
    /// decoding/path/result storage competes in `account`; a refusal returns
    /// explicit incomplete coverage. Cancellation discards the whole result.
    /// Uses uncached metadata reads without changing this reader's cache policy.
    pub fn diff_trees_bounded(
        &mut self,
        old: Option<ObjectId>,
        new: Option<ObjectId>,
        limits: &diff::TreeDiffLimits,
        account: &crate::memory::MemoryAccount,
        cancel: &dyn Fn() -> bool,
    ) -> Result<diff::BoundedTreeDiff, GitError> {
        diff::bounded_tree_diff(&self.common_dir, old, new, limits, account, cancel, &mut self.blob_reads)
    }

    /// Compare commit trees with the same bounds. `None` denotes an empty
    /// endpoint, including the reference for a root commit. Missing commits
    /// (e.g. shallow parents) remain errors, never empty endpoints.
    pub fn diff_commits_bounded(
        &mut self,
        old: Option<ObjectId>,
        new: Option<ObjectId>,
        limits: &diff::TreeDiffLimits,
        account: &crate::memory::MemoryAccount,
        cancel: &dyn Fn() -> bool,
    ) -> Result<diff::BoundedTreeDiff, GitError> {
        if cancel() { return Err(GitError::Cancelled); }
        if old == new { return Ok(diff::BoundedTreeDiff::empty(account)); }
        let scratch = crate::bounded_read::Scratch::new(account, limits.max_scratch_bytes);
        let mut bytes_read = 0;
        let mut tree = |oid: Option<ObjectId>| -> Result<Option<ObjectId>, GitError> {
            let Some(oid) = oid else { return Ok(None); };
            let bytes = crate::bounded_read::read(&self.common_dir, &oid, ObjectKind::Commit,
                &scratch, &mut bytes_read, cancel)?;
            let line = bytes.data.split(|b| *b == b'\n').next().unwrap_or_default();
            let tree = line.strip_prefix(b"tree ").and_then(|s| std::str::from_utf8(s).ok())
                .ok_or_else(|| GitError::InvalidObject("commit: missing tree header".into()))?;
            ObjectId::from_hex(tree).map(Some)
        };
        let trees = tree(old).and_then(|old| tree(new).map(|new| (old, new)));
        self.blob_reads = self.blob_reads.saturating_add(scratch.blob_reads.get());
        let mut result = match trees {
            Ok((old, new)) => self.diff_trees_bounded(old, new, limits, account, cancel)?,
            Err(GitError::TreeDiffLimit(reason)) => {
                let mut result = diff::BoundedTreeDiff::empty(account);
                result.exhaust(reason, true);
                result
            }
            Err(error) => return Err(error),
        };
        if cancel() { return Err(GitError::Cancelled); }
        result.counters.bytes_read = result.counters.bytes_read.saturating_add(bytes_read);
        result.counters.scratch_peak_bytes = result.counters.scratch_peak_bytes.max(scratch.peak());
        Ok(result)
    }

    /// Diff two trees, returning the list of changes.
    pub fn diff_trees(
        &mut self,
        old_tree_oid: &ObjectId,
        new_tree_oid: &ObjectId,
    ) -> Result<Vec<TreeChange>, GitError> {
        self.ensure_object_sources()?;
        let old_tree = self.read_tree(old_tree_oid)?;
        let new_tree = self.read_tree(new_tree_oid)?;
        // We need a closure that can call self.read_tree, but self is already borrowed.
        // Workaround: use the lower-level read_object to avoid borrowing self in closure.
        let git_dir = self.common_dir.clone();
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

    /// Compute status with explicit traversal options.
    pub fn status_with_options(
        &mut self,
        options: worktree::StatusOptions,
    ) -> Result<worktree::Status, GitError> {
        self.ensure_object_sources()?;
        let index = self.read_index()?;

        // Try to build HEAD tree file map. If HEAD objects are missing locally
        // (e.g. partial clones), fall back to index/worktree-only status.
        let head_files = match (|| -> Result<HashMap<String, ObjectId>, GitError> {
            match self.head_oid() {
                Ok(head_oid) => {
                    let commit = self.read_commit(&head_oid)?;
                    let tree = self.read_tree(&commit.tree)?;
                    let git_dir = self.common_dir.clone();
                    let packs = &self.packs;
                    let alts = &self.alternates;
                    worktree::flatten_tree(&tree, "", &mut |oid| {
                        read_tree_standalone(&git_dir, oid, packs, alts)
                    })
                }
                Err(GitError::RefNotFound(_)) => Ok(HashMap::new()),
                Err(e) => Err(e),
            }
        })() {
            Ok(head_files) => head_files,
            Err(GitError::ObjectNotFound(_)) => {
                return worktree::compute_status_worktree_only_with_options(
                    &index,
                    &self.workdir,
                    options,
                );
            }
            Err(e) => return Err(e),
        };

        match worktree::compute_status_with_options(&head_files, &index, &self.workdir, options) {
            Ok(status) => Ok(status),
            // Some repos (e.g. partial clones) may miss HEAD objects locally.
            // Fall back to index/worktree comparison so modified/untracked files
            // still show up.
            Err(GitError::ObjectNotFound(_)) => {
                worktree::compute_status_worktree_only_with_options(&index, &self.workdir, options)
            }
            Err(e) => Err(e),
        }
    }

    /// Compute the full status of the working tree.
    pub fn status(&mut self) -> Result<worktree::Status, GitError> {
        self.status_with_options(worktree::StatusOptions::default())
    }

    /// Compute status for file-tree UIs: skips hidden entries and `target/`
    /// while scanning for untracked files to keep traversal bounded.
    pub fn status_for_file_tree(&mut self) -> Result<worktree::Status, GitError> {
        self.status_with_options(worktree::StatusOptions {
            skip_hidden: true,
            skip_target_dirs: true,
            skip_worktree_content_compare: false,
        })
    }

    /// Compute status for a single path with file-tree semantics.
    ///
    /// This avoids full worktree traversal and is suitable for fs-delta updates.
    pub fn status_for_path_for_file_tree(
        &mut self,
        path: &str,
    ) -> Result<Option<worktree::FileStatus>, GitError> {
        let path = normalize_rel_path(path);
        if path.is_empty() {
            return Ok(None);
        }
        let index = self.read_index()?;
        let options = worktree::StatusOptions {
            skip_hidden: true,
            skip_target_dirs: true,
            skip_worktree_content_compare: false,
        };

        let head_oid = match self.head_blob_oid_for_path(&path) {
            Ok(oid) => oid,
            Err(GitError::ObjectNotFound(_)) => {
                return worktree::compute_status_for_path_worktree_only_with_options(
                    &index,
                    &self.workdir,
                    &path,
                    options,
                );
            }
            Err(err) => return Err(err),
        };

        match worktree::compute_status_for_path_with_options(
            head_oid,
            &index,
            &self.workdir,
            &path,
            options,
        ) {
            Ok(status) => Ok(status),
            Err(GitError::ObjectNotFound(_)) => {
                worktree::compute_status_for_path_worktree_only_with_options(
                    &index,
                    &self.workdir,
                    &path,
                    options,
                )
            }
            Err(err) => Err(err),
        }
    }

    fn head_blob_oid_for_path(&mut self, path: &str) -> Result<Option<ObjectId>, GitError> {
        let head_oid = match self.head_oid() {
            Ok(head_oid) => head_oid,
            Err(GitError::RefNotFound(_)) => return Ok(None),
            Err(err) => return Err(err),
        };
        let commit = self.read_commit(&head_oid)?;
        let mut tree = self.read_tree(&commit.tree)?;
        let mut components = path
            .split('/')
            .filter(|component| !component.is_empty())
            .peekable();
        while let Some(component) = components.next() {
            let Some(entry) = tree.entries.iter().find(|entry| entry.name == component) else {
                return Ok(None);
            };
            if components.peek().is_none() {
                if entry.is_tree() {
                    return Ok(None);
                }
                return Ok(Some(entry.oid));
            }
            if !entry.is_tree() {
                return Ok(None);
            }
            tree = self.read_tree(&entry.oid)?;
        }
        Ok(None)
    }

    /// Stage a file (add to index).
    pub fn stage_file(&mut self, path: &str) -> Result<(), GitError> {
        let mut index = self.read_index()?;
        worktree::stage_file(&self.common_dir, &self.workdir, &mut index, path)?;
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
        self.ensure_object_sources()?;
        let refname = format!("refs/heads/{}", branch);
        let target_oid = refs::resolve_ref_in(&self.git_dir, &self.common_dir, &refname)?;
        let target_commit = self.read_commit(&target_oid)?;
        let target_tree = self.read_tree(&target_commit.tree)?;

        // Get current HEAD tree files for cleanup
        let old_files = match self.head_oid() {
            Ok(head_oid) => {
                let commit = self.read_commit(&head_oid)?;
                let tree = self.read_tree(&commit.tree)?;
                let git_dir = self.common_dir.clone();
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
        let git_dir = self.common_dir.clone();
        let packs = &self.packs;
        let alts = &self.alternates;
        let new_files = worktree::flatten_tree(&target_tree, "", &mut |oid| {
            read_tree_standalone(&git_dir, oid, packs, alts)
        })?;

        // Remove files that are in old but not in new
        worktree::remove_worktree_files(&self.workdir, &old_files, &new_files)?;

        // Checkout new tree
        let mut index_entries = Vec::new();
        let git_dir = self.common_dir.clone();
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

    /// Move a clean checkout from `old_tree` to `new_tree` the way
    /// `git read-tree -u -m` does: only paths whose blob or mode differ are
    /// rewritten or removed, so unchanged files keep their timestamps, and a
    /// file that no longer matches `old_tree` is never overwritten. The index
    /// is updated for exactly those paths. Nothing is refused for untracked
    /// files; callers assert cleanliness first.
    pub fn update_worktree(
        &mut self,
        old_tree: &ObjectId,
        new_tree: &ObjectId,
    ) -> Result<(), GitError> {
        self.ensure_object_sources()?;
        let old_files = self.flatten_tree_with_mode(old_tree)?;
        let new_files = self.flatten_tree_with_mode(new_tree)?;
        let mut removed: Vec<&String> = Vec::new();
        let mut written: Vec<(&String, ObjectId, u32)> = Vec::new();
        for (path, (oid, mode)) in &old_files {
            if !new_files.contains_key(path) {
                removed.push(path);
                let _ = (oid, mode);
            }
        }
        for (path, (oid, mode)) in &new_files {
            if old_files.get(path) != Some(&(*oid, *mode)) {
                written.push((path, *oid, *mode));
            }
        }
        // Refuse before touching anything: every path we would replace or
        // delete must still hold its old content.
        for path in removed.iter().copied().chain(written.iter().map(|(p, _, _)| *p)) {
            if let Some((old_oid, _)) = old_files.get(path) {
                let file = self.workdir.join(path);
                if file.is_file() && worktree::hash_file_blob(&file)? != *old_oid {
                    return Err(GitError::InvalidObject(format!(
                        "{path} was modified locally; the update would overwrite it"
                    )));
                }
            }
        }
        removed.sort();
        written.sort_by(|a, b| a.0.cmp(b.0));
        let mut index = self.read_index()?;
        for path in &removed {
            let file = self.workdir.join(path);
            if file.is_file() {
                fs::remove_file(&file)?;
            }
            worktree::unstage_file(&mut index, path);
            if let Some(parent) = file.parent() {
                worktree::remove_empty_dirs(parent, &self.workdir);
            }
        }
        for (path, oid, mode) in &written {
            let data = self.read_blob(oid)?;
            let entry = worktree::write_worktree_file(&self.workdir, path, *oid, *mode, &data)?;
            match index.entries.binary_search_by(|e| e.path.cmp(&entry.path)) {
                Ok(at) => index.entries[at] = entry,
                Err(at) => index.entries.insert(at, entry),
            }
        }
        self.write_index(&index)
    }

    /// Flatten a tree into `path -> (oid, mode)` using the shared object store.
    pub fn flatten_tree_with_mode(
        &mut self,
        tree: &ObjectId,
    ) -> Result<HashMap<String, (ObjectId, u32)>, GitError> {
        self.ensure_object_sources()?;
        let root = self.read_tree(tree)?;
        let git_dir = self.common_dir.clone();
        let packs = &self.packs;
        let alts = &self.alternates;
        merge::flatten_tree_with_mode(&root, "", &mut |oid| {
            read_tree_standalone(&git_dir, oid, packs, alts)
        })
    }

    // --- Merge Operations ---

    /// Find the merge base of two commits.
    pub fn merge_base(
        &mut self,
        oid_a: &ObjectId,
        oid_b: &ObjectId,
    ) -> Result<Option<ObjectId>, GitError> {
        self.ensure_object_sources()?;
        let git_dir = self.common_dir.clone();
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
        self.ensure_object_sources()?;
        let ours_oid = self.head_oid()?;
        let refname = format!("refs/heads/{}", branch);
        let theirs_oid = refs::resolve_ref_in(&self.git_dir, &self.common_dir, &refname)?;

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
                    refs::write_ref_in(&self.git_dir, &self.common_dir, &refname, &theirs_oid)?;
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
                let git_dir = self.common_dir.clone();
                let packs = &self.packs;
                let alts = &self.alternates;
                worktree::flatten_tree(&ours_tree, "", &mut |oid| {
                    read_tree_standalone(&git_dir, oid, packs, alts)
                })?
            };

            let git_dir = self.common_dir.clone();
            let packs = &self.packs;
            let alts = &self.alternates;
            let new_files = worktree::flatten_tree(&theirs_tree, "", &mut |oid| {
                read_tree_standalone(&git_dir, oid, packs, alts)
            })?;

            worktree::remove_worktree_files(&self.workdir, &old_files, &new_files)?;

            let mut index_entries = Vec::new();
            let git_dir = self.common_dir.clone();
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

        let git_dir = self.common_dir.clone();
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
                    refs::write_ref_in(&self.git_dir, &self.common_dir, &refname, &commit_oid)?;
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

    /// Load packs and alternates (loose directories and their packs) once,
    /// before any code path that reads objects through the standalone
    /// closures.
    pub(crate) fn ensure_object_sources(&mut self) -> Result<(), GitError> {
        if self.packs.is_none() || self.alternates.is_none() {
            let sources = object_sources(&self.common_dir)?;
            let mut packs = sources.packs;
            if let Some(cache) = &self.read_cache { for pack in &mut packs { pack.set_read_budget(cache.account.clone(), cache.budget)?; } }
            self.packs = Some(packs);
            self.alternates = Some(sources.alternate_dirs);
            self.alternate_packs = Some(Vec::new());
        }
        Ok(())
    }

    fn ensure_packs(&mut self) -> Result<(), GitError> {
        self.ensure_object_sources()
    }

    fn ensure_alternates(&mut self) -> Result<(), GitError> {
        self.ensure_object_sources()
    }
}

fn normalize_rel_path(path: &str) -> String {
    let mut path = path.replace('\\', "/");
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped.to_string();
    }
    path.trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

/// Read a loose object directly from an objects/ directory (for alternates).
pub(crate) fn read_loose_object_from_objects_dir(
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

    let raw = makepad_fast_inflate::zlib_decompress_vec(&compressed)
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
pub(crate) fn read_object_standalone(
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
pub(crate) fn read_tree_standalone(
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
