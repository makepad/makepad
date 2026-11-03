//! Frozen refs for one orchestration pass; immutable answers survive passes.
//! A new pass observes loose ref edits/deletions and packed-ref replacement.
//! Object readers come from gitread's existing common-directory registry.
use super::{handle, paths};
use makepad_git::{memory::{MemoryAccount, ReadLru}, refs, ObjectId, Reach, RefTarget, Repository, RepositoryPaths};
use std::{collections::{BTreeMap, HashMap, HashSet}, path::{Path, PathBuf}, sync::{Arc, Mutex}};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Own(ObjectId, ObjectId),
    Ancestor(ObjectId, ObjectId),
    Range(ObjectId, ObjectId),
    Merge(ObjectId, ObjectId),
}
enum Answer {
    Oid(Option<ObjectId>),
    Ancestor(bool),
    Range(Arc<HashSet<String>>),
}
struct Queries {
    account: MemoryAccount,
    memo: ReadLru<Key, Answer>,
    reaches: HashMap<String, Reach>,
    range_base: Reach,
    shallow: Vec<u8>,
}
impl Queries {
    fn new(shallow: Vec<u8>) -> Self {
        let bytes = 8 << 20;
        Self { account: MemoryAccount::new(bytes), memo: ReadLru::new(bytes), reaches: HashMap::new(), range_base: Reach::new(), shallow }
    }
}
fn queries(common: &Path) -> Option<Arc<Mutex<Queries>>> {
    type Registry = HashMap<PathBuf, Arc<Mutex<Queries>>>;
    static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);
    let shallow = match std::fs::read(common.join("shallow")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return None,
    };
    let query = {
        let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let registry = guard.get_or_insert_with(HashMap::new);
        registry.retain(|path, _| path.is_dir());
        registry.entry(common.to_path_buf()).or_insert_with(|| Arc::new(Mutex::new(Queries::new(shallow.clone())))).clone()
    };
    let mut q = query.lock().unwrap_or_else(|e| e.into_inner());
    if q.shallow != shallow { *q = Queries::new(shallow); }
    drop(q);
    Some(query)
}
struct View {
    repo: Arc<Mutex<Repository>>,
    query: Arc<Mutex<Queries>>,
    refs: BTreeMap<String, RefTarget>,
}
impl View {
    fn capture(common: &Path) -> Option<Self> {
        let read = || refs::list_refs_in(common, common, "refs/").ok().map(|v| v.into_iter().map(|r| (r.name, r.target)).collect::<BTreeMap<_, _>>());
        let first = read()?;
        // A racing ref publication defers this pass. No answer mixes names
        // captured before and after an observed change; the next pass retries.
        if first != read()? { return None; }
        Some(Self { repo: handle(common)?, query: queries(common)?, refs: first })
    }
    fn reference(&self, name: &str) -> Option<ObjectId> {
        let mut at = name;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(at) { return None; }
            match self.refs.get(at)? {
                RefTarget::Direct(oid) => return Some(*oid),
                RefTarget::Symbolic(next) => at = next,
            }
        }
    }
    fn ancestor(&self, a: ObjectId, b: ObjectId, anchor: &str) -> Option<bool> {
        let mut q = self.query.lock().unwrap_or_else(|e| e.into_inner());
        let mut repo = self.repo.lock().unwrap_or_else(|e| e.into_inner());
        repo.read_commit(&a).ok()?;
        repo.read_commit(&b).ok()?;
        if matches!(anchor, "work" | "main" | "refs/heads/work" | "refs/heads/main") {
            let reach = q.reaches.entry(anchor.trim_start_matches("refs/heads/").into()).or_default();
            reach.advance(&mut repo, b).ok()?;
            return Some(reach.contains(&a));
        }
        if let Some(Answer::Ancestor(value)) = q.memo.get(&Key::Ancestor(a, b)) { return Some(*value); }
        let value = makepad_git::is_ancestor(&mut repo, a, b).ok()?;
        let account = q.account.clone();
        q.memo.insert(Key::Ancestor(a, b), 256, &account, || Answer::Ancestor(value));
        Some(value)
    }
}

#[derive(Default)]
pub struct Pass {
    views: HashMap<PathBuf, Option<View>>,
    heads: HashMap<PathBuf, Option<ObjectId>>,
}
impl Pass {
    fn view(&mut self, p: &RepositoryPaths) -> Option<&View> {
        let common = p.common_dir.canonicalize().ok()?;
        self.views.entry(common.clone()).or_insert_with(|| View::capture(&common)).as_ref()
    }
    fn oid(&mut self, p: &RepositoryPaths, name: &str) -> Option<ObjectId> {
        let name = name.strip_suffix("^{commit}").unwrap_or(name);
        if name.len() == 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) { return ObjectId::from_hex(name).ok(); }
        if name == "HEAD" {
            let key = p.git_dir.canonicalize().ok()?;
            if let Some(oid) = self.heads.get(&key) { return *oid; }
            let target = refs::read_head(&p.git_dir).ok()?;
            let oid = match &target {
                RefTarget::Direct(oid) => Some(*oid),
                RefTarget::Symbolic(name) => self.view(p)?.reference(name),
            };
            if refs::read_head(&p.git_dir).ok().as_ref() != Some(&target) { self.heads.insert(key, None); return None; }
            self.heads.insert(key, oid);
            return oid;
        }
        let view = self.view(p)?;
        if name.starts_with("refs/") { view.reference(name) }
        else { ["refs/heads/", "refs/tags/", "refs/remotes/"].into_iter().find_map(|pre| view.reference(&format!("{pre}{name}"))) }
    }
    pub fn rev(&mut self, dir: &Path, name: &str) -> Option<String> {
        let p = paths(dir)?;
        let oid = self.oid(&p, name)?;
        self.view(&p)?.repo.lock().unwrap_or_else(|e| e.into_inner()).read_commit(&oid).ok()?;
        Some(oid.to_hex())
    }
    /// Direct refs from this pass, in Git's refname order.
    pub fn direct_refs(&mut self, dir: &Path, prefix: &str) -> Option<Vec<(String, ObjectId)>> {
        let p = paths(dir)?;
        Some(self.view(&p)?.refs.iter().filter_map(|(name, target)| match target {
            RefTarget::Direct(oid) if name.starts_with(prefix) => Some((name.clone(), *oid)),
            _ => None,
        }).collect())
    }
    pub fn is_ancestor(&mut self, dir: &Path, a: &str, b: &str) -> Option<bool> {
        let p = paths(dir)?;
        let a = self.oid(&p, a)?;
        let b_oid = self.oid(&p, b)?;
        self.view(&p)?.ancestor(a, b_oid, b)
    }
    pub fn own_tip(&mut self, dir: &Path, branch: &str, target: &str) -> Option<String> {
        let p = paths(dir)?;
        let head = self.oid(&p, &format!("refs/heads/{branch}"))?;
        let view = self.view(&p)?;
        let (pre, post) = (format!("Merge {target} ("), format!(") into {branch}"));
        let pattern = makepad_git::oid::hash_object("blob", format!("{pre}\0{post}").as_bytes());
        let key = Key::Own(head, pattern);
        let mut q = view.query.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Answer::Oid(value)) = q.memo.get(&key) { return value.map(|o| o.to_hex()); }
        let mut repo = view.repo.lock().unwrap_or_else(|e| e.into_inner());
        let mut own = None;
        for entry in repo.first_parents(head).take(200) {
            let (oid, commit) = entry.ok()?;
            own = Some(oid);
            let subject = commit.message.split('\n').map(|l| l.trim_end_matches(|c: char| c.is_ascii_whitespace())).skip_while(|l| l.is_empty()).take_while(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
            if !(subject.starts_with(&pre) && subject.ends_with(&post)) { break; }
        }
        let account = q.account.clone();
        q.memo.insert(key, 256, &account, || Answer::Oid(own));
        own.map(|o| o.to_hex())
    }
    pub fn range(&mut self, dir: &Path, base: &str, tip: &str) -> Result<Arc<HashSet<String>>, String> {
        let read = |name: &str| format!("cannot read commit {name}");
        let p = paths(dir).ok_or_else(|| "not a repository".to_string())?;
        let base = self.oid(&p, base).ok_or_else(|| read(base))?;
        let tip = self.oid(&p, tip).ok_or_else(|| read(tip))?;
        let view = self.view(&p).ok_or_else(|| "refs changed while capturing the pass".to_string())?;
        let mut q = view.query.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Answer::Range(range)) = q.memo.get(&Key::Range(base, tip)) { return Ok(range.clone()); }
        let mut repo = view.repo.lock().unwrap_or_else(|e| e.into_inner());
        repo.read_commit(&base).map_err(|e| e.to_string())?;
        repo.read_commit(&tip).map_err(|e| e.to_string())?;
        q.range_base.advance(&mut repo, base).map_err(|e| e.to_string())?;
        let range: Arc<HashSet<String>> = Arc::new(q.range_base.range(&mut repo, tip).map_err(|e| e.to_string())?.commits.into_iter().map(|(oid, _)| oid.to_hex()).collect());
        let bytes = range.len().saturating_mul(128).saturating_add(256);
        let account = q.account.clone();
        q.memo.insert(Key::Range(base, tip), bytes, &account, || Answer::Range(range.clone()));
        Ok(range)
    }
}

impl Pass {
    pub fn retired_base(&mut self, dir: &Path, branch: &str, target: &str) -> Option<String> {
        let p = paths(dir)?;
        let branch = self.oid(&p, branch)?;
        let target = self.oid(&p, target)?;
        let view = self.view(&p)?;
        let retired: Vec<_> = view.refs.keys().filter(|name| name.starts_with("refs/heads/work-old-")).filter_map(|name| view.reference(name)).collect();
        for retired in retired {
            let base = {
                let mut q = view.query.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(Answer::Oid(base)) = q.memo.get(&Key::Merge(branch, retired)) { *base }
                else {
                    let mut repo = view.repo.lock().unwrap_or_else(|e| e.into_inner());
                    let base = match best_base(&mut repo, branch, retired) { Ok(base) => base, Err(_) => continue };
                    let account = q.account.clone();
                    q.memo.insert(Key::Merge(branch, retired), 256, &account, || Answer::Oid(base));
                    base
                }
            };
            if let Some(base) = base {
                if !view.ancestor(base, target, "")? { return Some(base.to_hex()); }
            }
        }
        None
    }
    pub fn retired_holds(&mut self, dir: &Path, from: &str) -> Option<bool> {
        let p = paths(dir)?;
        let from = self.oid(&p, from)?;
        let view = self.view(&p)?;
        for name in view.refs.keys().filter(|name| name.starts_with("refs/heads/work-old-")) {
            let Some(target) = view.reference(name) else { continue };
            if view.ancestor(from, target, "") == Some(true) { return Some(true); }
        }
        Some(false)
    }
}

/// Paint both histories; a common commit makes its parents obsolete. Unlike
/// the old bidirectional BFS helper, a short path to an older common ancestor
/// cannot hide a better common ancestor. Timestamps order work, never prune it.
fn best_base(repo: &mut Repository, a: ObjectId, b: ObjectId) -> Result<Option<ObjectId>, makepad_git::GitError> {
    use std::{cmp::Reverse, collections::BinaryHeap};
    if a == b { repo.read_commit(&a)?; return Ok(Some(a)); }
    const STALE: u8 = 4;
    const RESULT: u8 = 8;
    let shallow = repo.shallow_boundary()?;
    let mut read = HashMap::new();
    let mut flags = HashMap::from([(a, 1u8), (b, 2u8)]);
    let mut pending = HashSet::new();
    let mut queue = BinaryHeap::new();
    let mut serial = 0u64;
    for oid in [a, b] {
        let c = repo.read_commit(&oid)?;
        queue.push((c.committer.timestamp, Reverse(serial), oid)); serial += 1;
        read.insert(oid, (c.committer.timestamp, if shallow.contains(&oid) { Vec::new() } else { c.parents }));
        pending.insert(oid);
    }
    let mut live = 2usize;
    let mut results = Vec::new();
    while live > 0 {
        let Some((time, _, oid)) = queue.pop() else { break };
        pending.remove(&oid);
        let mut f = flags[&oid];
        if f & STALE == 0 { live -= 1; }
        if f & 7 == 3 {
            if f & RESULT == 0 { results.push((time, oid)); flags.insert(oid, f | RESULT); }
            f |= STALE;
        }
        let parents = read[&oid].1.clone();
        for parent in parents {
            let old = flags.get(&parent).copied().unwrap_or(0);
            let next = old | (f & 7);
            if old == next { continue; }
            flags.insert(parent, next);
            if pending.contains(&parent) {
                if old & STALE == 0 && next & STALE != 0 { live -= 1; }
                continue;
            }
            let time = if let Some((time, _)) = read.get(&parent) { *time } else {
                let c = repo.read_commit(&parent)?;
                let time = c.committer.timestamp;
                read.insert(parent, (time, if shallow.contains(&parent) { Vec::new() } else { c.parents }));
                time
            };
            pending.insert(parent);
            if next & STALE == 0 { live += 1; }
            queue.push((time, Reverse(serial), parent)); serial += 1;
        }
    }
    results.retain(|(_, oid)| flags[oid] & STALE == 0);
    results.sort_by(|a, b| b.0.cmp(&a.0));
    // Clock skew can leave an obsolete result below an all-stale queue tail.
    // Remove redundant common ancestors before choosing the newest best one.
    for (_, candidate) in &results {
        let mut redundant = false;
        for (_, other) in &results {
            if candidate != other && makepad_git::is_ancestor(repo, *candidate, *other)? { redundant = true; break; }
        }
        if !redundant { return Ok(Some(*candidate)); }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retired_merge_bases_match_git_on_forks_merges_and_clock_skew() {
        let temp = makepad_git::test_support::tempdir().unwrap();
        let dir = temp.path();
        let init = std::process::Command::new("git").args(["init", "--bare", "-q"]).arg(dir).status().unwrap();
        assert!(init.success());
        let tree = makepad_git::object::write_loose_object(dir, makepad_git::ObjectKind::Tree, &[]).unwrap();
        let commit = |parents: Vec<ObjectId>, time: i64, name: &str| {
            let sig = makepad_git::Signature { name: "test".into(), email: String::new(), timestamp: time, tz_offset: "+0000".into() };
            let c = makepad_git::Commit { tree, parents, author: sig.clone(), committer: sig, message: name.into() };
            makepad_git::object::write_loose_object(dir, makepad_git::ObjectKind::Commit, &makepad_git::commit::serialize_commit(&c)).unwrap()
        };
        let root = commit(vec![], 100, "root");
        let a = commit(vec![root], 101, "a");
        let b = commit(vec![root], 102, "b");
        let c = commit(vec![a, b], 103, "c");
        let d = commit(vec![b, a], 104, "d");
        // Shortcuts to the root must not make it win over a newer common base.
        let e = commit(vec![c, root], 105, "e");
        let f = commit(vec![c, root], 106, "f");
        let g = commit(vec![e, d], 99, "clock skew");
        let h = commit(vec![f, d], 98, "more skew");
        let mut repo = crate::gitread::worktree(makepad_git::RepositoryPaths { git_dir: dir.into(), common_dir: dir.into(), workdir: dir.into() }).unwrap();
        for (a, b) in [(root, h), (a, b), (c, d), (e, f), (g, h), (h, g), (a, a)] {
            let cli = std::process::Command::new("git").arg("-C").arg(dir).args(["merge-base", &a.to_hex(), &b.to_hex()]).output().unwrap();
            assert!(cli.status.success());
            assert_eq!(best_base(&mut repo, a, b).unwrap().map(|o| o.to_hex()).as_deref(), Some(String::from_utf8_lossy(&cli.stdout).trim()), "merge base {a} {b}");
        }
    }
}
