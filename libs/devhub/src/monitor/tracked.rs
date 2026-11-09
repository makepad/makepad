//! Tracked-only cleanliness for orphan readiness, with monitor invalidation.
use super::*;
use makepad_git::{IndexEntry, ObjectId};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp { len: u64, modified: Option<std::time::SystemTime>, identity: u64, changed: (i64, i64), mode: u32 }
fn stamp(path: &Path) -> Result<Option<Stamp>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => {
            #[cfg(unix)] let (identity, changed, mode) = { use std::os::unix::fs::MetadataExt; (m.ino(), (m.ctime(), m.ctime_nsec()), m.mode()) };
            #[cfg(not(unix))] let (identity, changed, mode) = (0, (0, 0), 0);
            Ok(Some(Stamp { len: m.len(), modified: m.modified().ok(), identity, changed, mode }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
pub(super) struct Tracked {
    head: ObjectId,
    index: Option<Stamp>,
    config: Option<Stamp>,
    worktree_config: Option<Stamp>,
    conversion_config: String,
    attributes: Option<Stamp>,
    files: HashMap<String, IndexEntry>,
    dirty: BTreeSet<String>,
    stamps: HashMap<String, Option<Stamp>>,
    staged: bool,
    filemode: bool,
}
impl Monitor {
    /// None of the tracked files differs from HEAD/index. Untracked files do
    /// not hold an otherwise finished lane. Initial/index/HEAD changes rebuild
    /// the tracked baseline, trusting only complete non-racy index stat records; steady ticks stat tracked paths and hash only changed
    /// metadata or monitor touches (including excluded or delayed events). An index
    /// change is checked directly because linked-worktree indexes live outside
    /// the watched root. Any event racing the read defers release to a new pass.
    pub fn tracked_clean(&self, tree: &Path) -> Result<(String, bool), String> {
        let root = tree.canonicalize().map_err(|e| e.to_string())?;
        let p = crate::gitread::paths(&root).ok_or("not a worktree")?;
        let head = makepad_git::refs::resolve_head_in(&p.git_dir, &p.common_dir).map_err(|e| e.to_string())?;
        let index = stamp(&p.git_dir.join("index"))?;
        let config = stamp(&p.common_dir.join("config"))?;
        let worktree_config = stamp(&p.git_dir.join("config.worktree"))?;
        let attributes = stamp(&p.common_dir.join("info/attributes"))?;
        let (old, touched, seq, stale) = {
            let mut g = self.inner.lock().unwrap();
            let t = g.trees.entry(root.clone()).or_default();
            (t.tracked.take(), t.tracked_touched.clone(), t.seq, t.tracked_stale)
        };
        let watched = self.watcher.is_some() && self.roots.iter().any(|r| root.starts_with(r));
        let fresh = old.as_ref().is_none_or(|s| s.head != head || s.index != index || s.config != config || s.worktree_config != worktree_config || s.attributes != attributes) || !watched || stale;
        let mut state = if fresh {
            let index_data = crate::gitread::worktree(p.clone()).and_then(|r| r.read_index()).map_err(|e| e.to_string())?;
            let entries = crate::gitread::with_common(&p.common_dir, |repo| {
                let tree = repo.read_commit(&head)?.tree;
                repo.flatten_tree_with_mode(&tree)
            }).ok_or("cannot read shared object store")?.map_err(|e| e.to_string())?;
            let config_text = makepad_git::text_clean::conversion_config(&p)?;
            let filemode = makepad_git::config::get(&config_text, "core.filemode").map_err(|e| e.to_string())?.as_deref().and_then(makepad_git::config::parse_bool).unwrap_or(cfg!(unix));
            let mut staged = index_data.entries.iter().any(|e| e.stage() != 0);
            let files: HashMap<_, _> = index_data.entries.into_iter().filter(|e| e.stage() == 0).map(|e| (e.path.clone(), e)).collect();
            staged |= files.len() != entries.len() || files.iter().any(|(path, e)| entries.get(path) != Some(&(e.oid, e.mode)));
            Tracked { head, index: index.clone(), config: config.clone(), worktree_config: worktree_config.clone(), conversion_config: config_text, attributes: attributes.clone(), files, dirty: BTreeSet::new(), stamps: HashMap::new(), staged, filemode }
        } else { old.unwrap() };
        let mut candidates = touched.clone();
        let mut observed = HashMap::new();
        for path in state.files.keys() {
            let now = stamp(&root.join(path))?;
            if fresh || state.stamps.get(path) != Some(&now) { candidates.insert(path.clone()); }
            observed.insert(path.clone(), now);
        }
        for path in candidates {
            let clean = match state.files.get(&path) {
                Some(e) => file_clean(&root, &p.common_dir, e, state.filemode, &state.conversion_config, index.as_ref().and_then(|s| s.modified), touched.contains(&path))?,
                None => true,
            };
            if let Some(before) = observed.get(&path) {
                if stamp(&root.join(&path))?.as_ref() != before.as_ref() { return Err("file changed during tracked status".into()); }
            }
            if clean { state.dirty.remove(&path); } else { state.dirty.insert(path); }
        }
        // Gitlinks can change HEAD without a file event in the parent tree.
        for e in state.files.values().filter(|e| e.mode == 0o160000) {
            let path = root.join(&e.path);
            let clean = if let Some(sub) = crate::gitread::paths(&path).filter(|p| p.workdir == path && path.symlink_metadata().is_ok_and(|m| m.is_dir())) {
                let subhead = makepad_git::refs::resolve_head_in(&sub.git_dir, &sub.common_dir).map_err(|e| e.to_string())?;
                subhead == e.oid && self.tracked_clean(&path)?.1
            } else { path.symlink_metadata().is_ok_and(|m| m.is_dir()) };
            if clean { state.dirty.remove(&e.path); } else { state.dirty.insert(e.path.clone()); }
        }
        state.stamps = observed;
        let clean = !state.staged && state.dirty.is_empty();
        if makepad_git::refs::resolve_head_in(&p.git_dir, &p.common_dir).ok() != Some(head) || stamp(&p.git_dir.join("index"))? != index || stamp(&p.common_dir.join("config"))? != config || stamp(&p.git_dir.join("config.worktree"))? != worktree_config || stamp(&p.common_dir.join("info/attributes"))? != attributes {
            return Err("worktree changed during tracked status".into());
        }
        let mut g = self.inner.lock().unwrap();
        let t = g.trees.get_mut(&root).ok_or("worktree was forgotten")?;
        if t.seq != seq { return Err("files changed during tracked status".into()); }
        t.tracked = Some(state);
        t.tracked_touched.clear();
        t.tracked_stale = false;
        Ok((head.to_hex(), clean))
    }
}
fn file_clean(root: &Path, common: &Path, e: &IndexEntry, filemode: bool, config: &str, index_time: Option<std::time::SystemTime>, touched: bool) -> Result<bool, String> {
    // Git's assume-valid and extended skip-worktree entries leave the
    // worktree out of the comparison; staged state is checked separately.
    if e.flags & 0xc000 != 0 { return Ok(true); }
    let mut parent = root.to_path_buf();
    let parts: Vec<_> = Path::new(&e.path).components().collect();
    for (i, part) in parts.iter().enumerate() {
        let std::path::Component::Normal(name) = part else { return Err("invalid tracked path".into()); };
        parent.push(name);
        if i + 1 < parts.len() && !parent.symlink_metadata().is_ok_and(|m| m.is_dir()) { return Ok(false); }
    }
    let path = root.join(&e.path);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(err) if matches!(err.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => return Ok(false),
        Err(err) => return Err(err.to_string()),
    };
    if e.mode == 0o160000 { return Ok(meta.is_dir()); }
    if e.mode == 0o120000 {
        if !meta.file_type().is_symlink() { return Ok(false); }
        let target = std::fs::read_link(&path).map_err(|e| e.to_string())?;
        #[cfg(unix)] let bytes = { use std::os::unix::ffi::OsStrExt; target.as_os_str().as_bytes().to_vec() };
        #[cfg(not(unix))] let bytes = target.to_string_lossy().as_bytes().to_vec();
        return Ok(makepad_git::oid::hash_object("blob", &bytes) == e.oid);
    }
    if !meta.is_file() || meta.file_type().is_symlink() { return Ok(false); }
    #[cfg(unix)] if filemode {
        use std::os::unix::fs::PermissionsExt;
        if (meta.permissions().mode() & 0o100 != 0) != (e.mode & 0o100 != 0) { return Ok(false); }
    }
    #[cfg(not(unix))] let _ = filemode;
    // A fresh baseline can trust Git's complete stat record when the file
    // predates the index. Same-size edits change nanosecond ctime/mtime; an
    // event or a racy index timestamp forces a content check regardless.
    #[cfg(unix)] if !touched {
        use std::os::unix::fs::MetadataExt;
        let predates_index = meta.modified().ok().zip(index_time).is_some_and(|(file, index)| file < index);
        if predates_index && meta.len() == e.file_size as u64 && meta.dev() as u32 == e.dev && meta.ino() as u32 == e.ino
            && meta.uid() == e.uid && meta.gid() == e.gid && meta.ctime() as u32 == e.ctime_sec && meta.ctime_nsec() as u32 == e.ctime_nsec
            && meta.mtime() as u32 == e.mtime_sec && meta.mtime_nsec() as u32 == e.mtime_nsec { return Ok(true); }
    }
    #[cfg(not(unix))] let _ = (index_time, touched);
    // A touched file is hashed even when its size and mtime are unchanged.
    // The existing second-resolution status shortcut is not a release proof.
    if makepad_git::hash_file_blob(&path).map_err(|e| e.to_string())? == e.oid { return Ok(true); }
    makepad_git::text_clean::cleans_to(root, common, &e.path, e.oid, config)
}
