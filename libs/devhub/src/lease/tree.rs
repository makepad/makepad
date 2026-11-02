//! Each box has a deployment tree, a shared work tree and bounded native lane trees:
//! - Deployment: the sync checkout and its target. The WM, dock apps, node hub and AI hub run
//!   from it. Only sync apply moves it, to landed work + commercial main; leases never hold it.
//!   A sync checkout that is someone's own clone ([`ours`] is false) is not a deployment tree.
//! - Work: `<work>/ci-tree` + `<work>/ci-target`. CI, gates and user boxes keep this detached
//!   worktree. Same-pin leases share it; another pin waits for its holders to end.
//! - Native: `<work>/native/<lane>` + `<work>/native-target/<lane>`. Other pinned lane leases
//!   hold only their lane's tree. All trees share the bare stores' objects. Native moves use
//!   `read-tree -m -u`, preserving unchanged files' mtimes. At the configured cap, the oldest
//!   unheld tree and its target go first; if all are held, the request waits.
//!
//! The last release scrubs its tree and leaves it at its pin: tracked edits are bundled to
//! `<work>/recovered/`, untracked files go, ignored files stay. Startup scrubs an unheld tree
//! without moving it. A sync apply also brings an idle work tree to the tips, warm for the next
//! lease. The work tree is devhub's on every box; only the deployment has the user-box
//! never-clear rule.
//! Overlay JIT leases never enter this module: their pinned stores and lane edits stay
//! under `<work>/dev`, separate from these trees; CI overlay leases are keyed per pin.

use super::Pin;
use crate::sync::git::{fleet_revision, run, Store, CHECKOUT_REF, SYNCED_REF};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// Who holds the tree now: lease id -> its pin.
#[derive(Default)]
pub struct Holders(BTreeMap<String, Pin>);

static TREE: Mutex<BTreeMap<PathBuf, Holders>> = Mutex::new(BTreeMap::new());

fn lock() -> MutexGuard<'static, BTreeMap<PathBuf, Holders>> {
    TREE.lock().unwrap_or_else(|e| e.into_inner())
}

impl Holders {
    /// Why `pin` cannot take the tree now (another pin holds it), if it cannot.
    pub fn conflict(&self, pin: &Pin) -> Option<String> {
        self.conflict_with(pin, false)
    }

    fn conflict_with(&self, pin: &Pin, native: bool) -> Option<String> {
        let other: Vec<&String> = self.0.iter().filter(|(_, p)| if native { *p != pin } else { !p.fits(pin) }).map(|(id, _)| id).collect();
        let first = other.first()?;
        Some(format!("the tree is at {} for {}", self.0[*first].short(), other.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")))
    }
}

/// The shared work tree for CI, gates and user boxes.
pub fn root(work: &Path) -> PathBuf {
    work.join("ci-tree")
}

pub fn native_root(work: &Path, lane: &str) -> PathBuf { work.join("native").join(lane) }
pub fn native_target(work: &Path, lane: &str) -> PathBuf { work.join("native-target").join(lane) }

/// Native lanes and their last use (unix ms). The marker stays outside the scrubbed tree.
pub fn natives(work: &Path) -> Vec<(String, u64)> {
    let mut lanes: Vec<_> = std::fs::read_dir(work.join("native")).into_iter().flatten().flatten().filter_map(|e| {
        let lane = e.file_name().to_str()?.to_string();
        if !super::exec::valid_name(&lane) || !e.file_type().ok()?.is_dir() { return None; }
        let used = std::fs::metadata(e.path().with_file_name(format!(".last-{lane}"))).or_else(|_| e.metadata()).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |t| t.as_millis() as u64);
        Some((lane, used))
    }).collect();
    lanes.sort();
    lanes
}

fn touch_native(root: &Path) {
    if root.parent().and_then(Path::file_name).is_some_and(|n| n == "native") {
        if let Some(lane) = root.file_name() {
            if let Err(e) = std::fs::write(root.with_file_name(format!(".last-{}", lane.to_string_lossy())), "") { eprintln!("devhub: native tree last use: {e}"); }
        }
    }
}

pub fn held(root: &Path) -> bool { lock().get(root).is_some_and(|h| !h.0.is_empty()) }

/// Whether the sync checkout is (or can become) devhub's deployment tree.
pub fn ours(dir: &Path) -> bool {
    let empty = dir.read_dir().map_or(true, |mut d| d.next().is_none());
    let stores = std::fs::canonicalize(crate::sync::git::stores_dir()).ok();
    let linked = std::fs::read_to_string(dir.join(".git")).ok().and_then(|t| t.trim().strip_prefix("gitdir:").map(|g| PathBuf::from(g.trim()))).and_then(|g| std::fs::canonicalize(g).ok()).is_some_and(|g| stores.is_some_and(|s| g.starts_with(s)));
    empty || linked || (!dir.join(".git").exists() && dir.join("FLEET-REVISION.txt").is_file())
}

/// The deployment pin (`caps.deploy`); a user's own clone has no deployment pin.
pub fn deploy(root: &Path) -> Option<Pin> {
    if !ours(root) {
        return None;
    }
    let at = |repo: &str| Store::open(repo).ok().and_then(|s| s.rev(CHECKOUT_REF)).or_else(|| fleet_revision(root, repo));
    Some(Pin { makepad: at("makepad")?, commercial: at("commercial") })
}

/// The last synced tips, used for a CI lease that names no pin.
pub fn shared(root: &Path) -> Option<Pin> {
    let at = |repo: &str| Store::open(repo).ok().and_then(|s| s.rev(SYNCED_REF)).or_else(|| fleet_revision(root, repo));
    Some(Pin { makepad: at("makepad")?, commercial: at("commercial") })
}

fn head(dir: &Path) -> Option<String> {
    if !dir.join(".git").exists() {
        return None;
    }
    run(Some(dir), None, &[], &["rev-parse", "--verify", "HEAD^{commit}"]).ok().map(|s| s.trim().to_string()).filter(|s| crate::sync::git::valid_sha(s))
}

/// The work tree's detached HEADs (`caps.tree`).
pub fn current(root: &Path) -> Option<Pin> {
    let _h = lock();
    Some(Pin { makepad: head(root)?, commercial: head(&root.join("apps/commercial")) })
}

fn prepare(id: &str, pin: &Pin, root: &Path, out: &Path, native: bool) -> Result<Taken, String> {
    let mut taken = Taken { root: root.to_path_buf(), moved: Vec::new(), cleared: Released::default() };
    // A makepad-only native pin must not inherit a previous pin's commercial workspace.
    // Recover its edits and remove the nested worktree before scrubbing the containing tree.
    let commercial = root.join("apps/commercial");
    if native && pin.commercial.is_none() && commercial.join(".git").exists() {
        let store = Store::open("commercial")?;
        if store.admin_of(&commercial).is_none() { return Err(format!("{} is not this node's worktree", commercial.display())); }
        scrub("commercial", root, head(&commercial).as_deref(), id, out, &format!("{id}-start"), &mut taken.cleared)?;
        run(None, Some(&store.git_dir), &[], &["worktree", "remove", "--force", &commercial.to_string_lossy()])?;
        taken.cleared.removed.push("apps/commercial/".into());
    }
    for (repo, to) in [("makepad", Some(&pin.makepad)), ("commercial", pin.commercial.as_ref())] {
        let Some(to) = to else { continue };
        let dir = if repo == "makepad" { root.to_path_buf() } else { root.join("apps/commercial") };
        let store = Store::open(repo)?;
        if dir.join(".git").exists() && store.admin_of(&dir).is_none() {
            return Err(format!("{} is not this node's worktree", dir.display()));
        }
        let from = head(&dir);
        if from.as_ref() == Some(to) {
            crate::ci_merge::install_merge_driver(&dir, root)?;
            taken.moved.push((repo.to_string(), 0));
            continue;
        }
        if !store.has(to) {
            return Err(format!("{repo} {} is not in this node's store", to));
        }
        scrub(repo, root, from.as_deref(), id, out, &format!("{id}-start"), &mut taken.cleared)?;
        let files = match from {
            Some(from) => {
                let files = run(Some(&dir), None, &[], &["diff", "--name-only", "--no-renames", &from, to])?.lines().count();
                if native {
                    let _ = run(Some(&dir), None, &[], &["update-index", "-q", "--refresh"]);
                    run(Some(&dir), None, &[], &["read-tree", "-m", "-u", &from, to])?;
                    run(Some(&dir), None, &[], &["update-ref", "--no-deref", "HEAD", to])?;
                } else {
                    run(Some(&dir), None, &[], &["checkout", "-q", "--detach", to])?;
                }
                files
            }
            None => {
                std::fs::create_dir_all(dir.parent().ok_or("work tree has no parent")?).map_err(|e| e.to_string())?;
                run(None, Some(&store.git_dir), &[], &["worktree", "add", "-q", "--detach", &dir.to_string_lossy(), to])?;
                run(Some(&dir), None, &[], &["ls-files"])?.lines().count()
            }
        };
        crate::ci_merge::install_merge_driver(&dir, root)?;
        taken.moved.push((repo.to_string(), files));
    }
    Ok(taken)
}

/// What a lease start did to the tree.
#[derive(Debug)]
pub struct Taken {
    pub root: PathBuf,
    /// files written per repo (0: it was there already)
    pub moved: Vec<(String, usize)>,
    /// what stood in the way and was cleared first
    pub cleared: Released,
}

/// Takes the work tree at `pin`; another pin's holders refuse the move.
pub fn take(id: &str, pin: &Pin, root: &Path, out: &Path) -> Result<Taken, String> {
    let mut h = lock();
    if let Some(why) = h.get(root).and_then(|h| h.conflict(pin)) {
        return Err(why);
    }
    let t = prepare(id, pin, root, out, false)?;
    h.entry(root.to_path_buf()).or_default().0.insert(id.to_string(), pin.clone());
    Ok(t)
}

/// Take a lane's native tree; capacity and disk pressure are retryable waits, never fallback
/// to the shared work tree. The same lock protects creation, holds, scrub and disk reclaim.
pub fn take_native(id: &str, pin: &Pin, work: &Path, lane: &str, limit: usize, floor: u64) -> Result<Taken, String> {
    if !super::exec::valid_name(lane) { return Err("invalid native lane".into()); }
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let root = native_root(work, lane);
    let out = work.join("recovered");
    let mut h = lock();
    if let Some(why) = h.get(&root).and_then(|h| h.conflict_with(pin, true)) { return Err(why); }
    let mut cleared = Released::default();
    if !root.exists() {
        if limit == 0 { return Err("native trees busy: capacity is zero".into()); }
        let mut lanes = natives(work);
        lanes.sort_by_key(|(_, used)| *used);
        while lanes.len() >= limit {
            let at = lanes.iter().position(|(lane, _)| !h.get(&native_root(work, lane)).is_some_and(|h| !h.0.is_empty()))
                .ok_or("native trees busy: waiting for a free tree")?;
            let (old, _) = lanes.remove(at);
            let r = remove_native_idle(work, &old)?;
            cleared.bundles.extend(r.bundles);
            cleared.removed.extend(r.removed);
            h.remove(&native_root(work, &old));
        }
    }
    if super::caps::disk_space(work).map_or(0, |(free, _)| free) < floor { return Err("native trees busy: disk below its floor".into()); }
    let mut t = prepare(id, pin, &root, &out, true)?;
    t.cleared.bundles.extend(cleared.bundles);
    t.cleared.removed.extend(cleared.removed);
    if super::caps::disk_space(work).map_or(0, |(free, _)| free) < floor { return Err("native trees busy: disk below its floor after checkout".into()); }
    h.entry(root.clone()).or_default().0.insert(id.to_string(), pin.clone());
    touch_native(&root);
    Ok(t)
}

fn remove_native_idle(work: &Path, lane: &str) -> Result<Released, String> {
    let root = native_root(work, lane);
    let r = scrub_tree(&root, "reclaim", &work.join("recovered"), &format!("{lane}-reclaim-{}", crate::proto::wall_ms()));
    if !r.unclean.is_empty() { return Err(r.unclean.join("; ")); }
    // The nested commercial worktree must go before its containing makepad worktree.
    for (repo, dir) in [("commercial", root.join("apps/commercial")), ("makepad", root.clone())] {
        if !dir.join(".git").exists() { continue; }
        let store = Store::open(repo)?;
        if store.admin_of(&dir).is_none() { return Err(format!("{} is not this node's worktree", dir.display())); }
        run(None, Some(&store.git_dir), &[], &["worktree", "remove", "--force", &dir.to_string_lossy()])?;
    }
    for path in [&root, &native_target(work, lane)] {
        if path.exists() { super::disk::remove(path).map_err(|e| e.to_string())?; }
    }
    let _ = std::fs::remove_file(root.with_file_name(format!(".last-{lane}")));
    Ok(r)
}

/// Reclaim a native tree and its target, checking its holds again at deletion time.
pub fn remove_native(work: &Path, lane: &str) -> Result<Released, String> {
    if !super::exec::valid_name(lane) { return Err("invalid native lane".into()); }
    let mut h = lock();
    let root = native_root(work, lane);
    if h.get(&root).is_some_and(|h| !h.0.is_empty()) { return Err(format!("native tree {lane} is held")); }
    let r = remove_native_idle(work, lane)?;
    h.remove(&root);
    Ok(r)
}

/// A sync warms the idle work tree without waiting for a lease's tree operation.
pub fn sync(work: &Path, pin: &Pin) -> Result<Option<Taken>, String> {
    let h = match TREE.try_lock() {
        Ok(h) => h,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
    };
    if h.get(&root(work)).is_some_and(|h| !h.0.is_empty()) {
        return Ok(None);
    }
    prepare("sync", pin, &root(work), &work.join("recovered"), false).map(Some)
}

/// Leaves `dir` (`repo`'s side of the tree, no lease on it) with tracked and ignored files only,
/// at `at`: edits in tracked files bundled to `out`, everything untracked removed. Only the node's
/// worktree is touched: a dir that is not this store's worktree is left alone.
fn scrub(repo: &str, root: &Path, at: Option<&str>, id: &str, out: &Path, tag: &str, r: &mut Released) -> Result<(), String> {
    let store = Store::open(repo)?;
    let (dir, prefix) = if repo == "makepad" { (root.to_path_buf(), "") } else { (root.join("apps").join("commercial"), "apps/commercial/") };
    if !dir.join(".git").exists() {
        return Ok(());
    }
    if store.admin_of(&dir).is_none() {
        return Err(format!("{} is not this node's worktree", dir.display()));
    }
    if let Some(at) = at {
        if store.tree_dirty(&dir)? {
            let t = if repo == "makepad" { tag.to_string() } else { format!("{tag}-{repo}") };
            r.bundles.push(store.recover_tree(&dir, at, id, out, &t)?);
        }
    }
    r.removed.extend(store.clean_tree(&dir)?.into_iter().map(|p| format!("{prefix}{p}")));
    Ok(())
}

/// Startup clears an unheld work tree without changing its pin.
pub fn scrub_if_idle(root: &Path, out: &Path) -> Released {
    let h = lock();
    if h.get(root).is_some_and(|h| !h.0.is_empty()) {
        return Released::default();
    }
    let tag = if root.parent().and_then(Path::file_name).is_some_and(|n| n == "native") {
        format!("startup-{}-{}", root.file_name().unwrap().to_string_lossy(), crate::proto::wall_ms())
    } else { "startup".into() };
    scrub_tree(root, "startup", out, &tag)
}

fn scrub_tree(root: &Path, id: &str, out: &Path, tag: &str) -> Released {
    let mut r = Released::default();
    for repo in ["commercial", "makepad"] {
        let dir = if repo == "makepad" { root.to_path_buf() } else { root.join("apps/commercial") };
        if let Err(e) = scrub(repo, root, head(&dir).as_deref(), id, out, tag, &mut r) {
            r.unclean.push(format!("{repo}: {e}"));
        }
    }
    r
}

/// Undoes what `devhub run` shipped on top of `pin` (`paths` from the tree's root; commercial ones
/// under `apps/commercial/`): files the pin tracks get its version back (deleted ones too), the
/// others (new files of the lane's worktree) go, with the dirs they leave empty. The count.
pub fn undo_overlay(root: &Path, pin: &Pin, paths: &[String]) -> Result<usize, String> {
    let mut n = 0;
    for (repo, sha) in [("makepad", Some(&pin.makepad)), ("commercial", pin.commercial.as_ref())] {
        let prefix = if repo == "makepad" { "" } else { "apps/commercial/" };
        let mine: Vec<&str> = paths
            .iter()
            .filter(|p| if repo == "makepad" { !p.starts_with("apps/commercial/") } else { p.starts_with(prefix) })
            .map(|p| &p[prefix.len()..])
            .collect();
        if mine.is_empty() {
            continue;
        }
        let dir = if repo == "makepad" { root.to_path_buf() } else { root.join("apps").join("commercial") };
        let Some(sha) = sha else {
            // a lease that pinned no commercial shipped commercial files: they are untracked here
            for p in &mine {
                n += remove_shipped(&dir, p) as usize;
            }
            continue;
        };
        let g = |args: &[&str]| crate::sync::git::run(Some(&dir), None, &[], args);
        let mut tracked = std::collections::HashSet::new();
        for chunk in mine.chunks(500) {
            let mut args = vec!["ls-tree", "-r", "-z", "--name-only", sha.as_str(), "--"];
            args.extend(chunk.iter().copied());
            tracked.extend(g(&args)?.split('\0').filter(|s| !s.is_empty()).map(str::to_string));
        }
        let (back, gone): (Vec<&str>, Vec<&str>) = mine.iter().partition(|p| tracked.contains(**p));
        for chunk in back.chunks(500) {
            let mut args = vec!["checkout", "-q", sha.as_str(), "--"];
            args.extend(chunk.iter().copied());
            g(&args)?;
            n += chunk.len();
        }
        for p in gone {
            n += remove_shipped(&dir, p) as usize;
        }
    }
    Ok(n)
}

/// Removes one shipped file under `dir` and the dirs it leaves empty (never `dir`; a dir holding
/// anything else stays).
fn remove_shipped(dir: &Path, rel: &str) -> bool {
    let p = dir.join(rel);
    if !std::fs::symlink_metadata(&p).is_ok_and(|m| !m.is_dir()) || std::fs::remove_file(&p).is_err() {
        return false;
    }
    let mut at = Path::new(rel);
    while let Some(parent) = at.parent().filter(|d| !d.as_os_str().is_empty()) {
        if std::fs::remove_dir(dir.join(parent)).is_err() {
            break;
        }
        at = parent;
    }
    true
}

/// A hub that restarted takes its leases' holds back from its ledger (the tree is where they left it).
pub fn hold(id: &str, pin: &Pin, root: &Path) {
    lock().entry(root.to_path_buf()).or_default().0.insert(id.to_string(), pin.clone());
}

/// What scrubbing recovered, removed, or could not clear.
#[derive(Debug, Default)]
pub struct Released {
    pub bundles: Vec<PathBuf>,
    pub removed: Vec<String>,
    pub unclean: Vec<String>,
}

/// The last holder bundles tracked edits and removes untracked files; the pin and target stay.
pub fn release(id: &str, root: &Path, out: &Path, tag: &str) -> Released {
    let mut h = lock();
    let Some(holders) = h.get_mut(root) else { return Released::default() };
    if holders.0.remove(id).is_none() || !holders.0.is_empty() {
        return Released::default();
    }
    let r = scrub_tree(root, id, out, tag);
    h.remove(root);
    touch_native(root);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(m: char, c: Option<char>) -> Pin {
        Pin { makepad: m.to_string().repeat(40), commercial: c.map(|c| c.to_string().repeat(40)) }
    }

    #[test]
    fn pins_share_the_tree_only_at_the_same_commits() {
        let a = pin('a', Some('c'));
        assert!(a.fits(&pin('a', Some('c'))) && a.fits(&pin('a', None)) && pin('a', None).fits(&a));
        assert!(!a.fits(&pin('b', Some('c'))) && !a.fits(&pin('a', Some('d'))));
        assert_eq!(Pin::parse(&a.key()), Some(a.clone()));
        assert_eq!(Pin::parse(&pin('b', None).key()), Some(pin('b', None)));
        assert_eq!(Pin::parse("xyz"), None);
        assert_eq!(Pin::of_rev(&format!("rc-x@{},main@{}", "a".repeat(40), "c".repeat(40))), Some(a.clone()));
        let mut h = Holders::default();
        h.0.insert("L1".into(), a.clone());
        assert_eq!(h.conflict(&pin('a', None)), None);
        let why = h.conflict(&pin('b', None)).unwrap();
        assert!(why.contains("aaaaaaaaaa,cccccccccc") && why.contains("L1"), "{why}");
    }

    /// Same-pin leases share; release and startup scrub without moving the pin or its target.
    #[test]
    fn leases_share_the_one_tree_and_leave_it_warm() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, work, deployment, stores) = (base.join("src"), base.join("work"), base.join("makepad"), base.join("stores"));
        let root = root(&work);
        let out = work.join("recovered");
        let target = work.join("ci-target");
        assert_eq!(root, work.join("ci-tree"));
        std::fs::create_dir_all(&src).unwrap();
        let g = |args: &[&str]| run(Some(&src), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "work"]);
        std::fs::write(src.join(".gitignore"), "/target/\n").unwrap();
        std::fs::write(src.join("a.rs"), "1").unwrap();
        std::fs::write(src.join("b.rs"), "1").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "one"]);
        let one = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(src.join("a.rs"), "2").unwrap();
        g(&["commit", "-q", "-am", "two"]);
        let two = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::env::set_var("MAKEPAD_DEVHUB_GIT", &stores);
        let store = Store::open("makepad").unwrap();
        let bundle = base.join("b.bundle");
        g(&["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work"]);
        store.fetch_bundle(&bundle, "work").unwrap();
        let at = |c: &str| Pin { makepad: c.to_string(), commercial: None };
        store.move_checkout(&deployment, None, &one).unwrap();
        store.set_ref(SYNCED_REF, &one).unwrap();
        assert_eq!(deploy(&deployment), Some(at(&one)));
        assert_eq!(deploy(&src), None, "a user's clone has no deployment pin");
        sync(&work, &at(&two)).unwrap().unwrap();
        assert_eq!(current(&root), Some(at(&two)));
        assert_eq!(deploy(&deployment), Some(at(&one)), "work HEAD never moves the deployment ref");
        std::fs::create_dir_all(target.join("release")).unwrap();
        std::fs::write(target.join("release/makepad"), "jit").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/ignored"), "kept").unwrap();

        // two leases at `two`: no file moves; a lease at `one` waits for both
        let t = take("L1", &at(&two), &root, &out).unwrap();
        assert_eq!((t.root.clone(), t.moved.clone()), (root.clone(), vec![("makepad".to_string(), 0)]));
        take("L2", &at(&two), &root, &out).unwrap();
        let why = take("L3", &at(&one), &root, &out).unwrap_err();
        assert!(why.contains("L1 L2"), "{why}");
        assert!(sync(&work, &at(&one)).unwrap().is_none(), "sync leaves a held work tree alone");
        store.move_checkout(&deployment, Some(&one), &two).unwrap();
        store.set_ref(SYNCED_REF, &two).unwrap();
        assert_eq!(deploy(&deployment), Some(at(&two)), "deployment applies while leases hold the work tree");
        std::fs::write(root.join("b.rs"), "edited by L2").unwrap();
        std::fs::write(root.join("scratch.txt"), "left by L2").unwrap();
        assert!(release("L1", &root, &out, "x-L1").bundles.is_empty());
        assert_eq!(std::fs::read_to_string(root.join("b.rs")).unwrap(), "edited by L2", "a holder is still on the tree");
        let b = release("L2", &root, &out, "x-L2");
        assert_eq!(b.bundles.len(), 1);
        assert!(b.bundles[0].is_file() && b.unclean.is_empty(), "{b:?}");
        assert_eq!(current(&root), Some(at(&two)));
        assert_eq!(std::fs::read_to_string(root.join("b.rs")).unwrap(), "1");
        assert_eq!(b.removed, ["scratch.txt"], "what a lease leaves untracked goes");
        assert!(!root.join("scratch.txt").exists() && root.join("target/ignored").exists());
        assert!(target.join("release/makepad").exists());

        // a lane lease moves only its diff; release keeps that pin and the warm target
        let b_time = std::fs::metadata(root.join("b.rs")).unwrap().modified().unwrap();
        let t = take("L3", &at(&one), &root, &out).unwrap();
        assert_eq!(t.moved, vec![("makepad".to_string(), 1)]);
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "1");
        assert_eq!(std::fs::metadata(root.join("b.rs")).unwrap().modified().unwrap(), b_time);
        let r = release("L3", &root, &out, "x-L3");
        assert!(r.bundles.is_empty() && r.unclean.is_empty());
        assert_eq!(current(&root), Some(at(&one)));
        assert_eq!(deploy(&deployment), Some(at(&two)));
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "1");
        assert_eq!(std::fs::metadata(root.join("b.rs")).unwrap().modified().unwrap(), b_time);
        assert_eq!(std::fs::read_to_string(target.join("release/makepad")).unwrap(), "jit");

        // ledger holds protect running leases from startup scrub too
        hold("L4", &at(&one), &root);
        std::fs::write(root.join("a.rs"), "in flight").unwrap();
        assert!(take("L5", &at(&two), &root, &out).is_err());
        assert!(scrub_if_idle(&root, &out).bundles.is_empty());
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "in flight");
        assert_eq!(release("L4", &root, &out, "x-L4").bundles.len(), 1);
        assert_eq!(current(&root), Some(at(&one)));
        std::fs::write(root.join("a.rs"), "interrupted lease").unwrap();
        std::fs::write(root.join("stray.txt"), "stray").unwrap();
        let r = scrub_if_idle(&root, &out);
        assert_eq!(r.bundles.len(), 1);
        assert_eq!(r.removed, ["stray.txt"]);
        assert!(r.unclean.is_empty() && !root.join("stray.txt").exists());
        assert_eq!(current(&root), Some(at(&one)));
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "1");
        assert!(scrub_if_idle(&root, &out).bundles.is_empty());

        // an overlay's edited, new and deleted files all return to the lease pin
        take("L6", &at(&one), &root, &out).unwrap();
        std::fs::write(root.join("a.rs"), "lane edit").unwrap();
        std::fs::create_dir_all(root.join("newcrate/src")).unwrap();
        std::fs::write(root.join("newcrate/src/lib.rs"), "new").unwrap();
        std::fs::remove_file(root.join("b.rs")).unwrap();
        let shipped = vec!["a.rs".to_string(), "newcrate/src/lib.rs".to_string(), "b.rs".to_string()];
        assert_eq!(undo_overlay(&root, &at(&one), &shipped).unwrap(), 3);
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "1");
        assert!(root.join("b.rs").exists() && !root.join("newcrate").exists(), "the new dir goes with its file");
        let r = release("L6", &root, &out, "x-L6");
        assert!(r.bundles.is_empty(), "nothing of the overlay is left to bundle");
        assert_eq!(current(&root), Some(at(&one)));
        assert!(target.join("release/makepad").exists());

        // a failed scrub is reported; it never changes the pin
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            take("L7", &at(&one), &root, &out).unwrap();
            std::fs::create_dir_all(root.join("locked")).unwrap();
            std::fs::write(root.join("locked/stuck"), "x").unwrap();
            std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
            let r = release("L7", &root, &out, "x-L7");
            std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(r.unclean.iter().any(|e| e.contains("locked")), "{r:?}");
            assert_eq!(current(&root), Some(at(&one)));
            std::fs::remove_file(root.join("locked/stuck")).unwrap();
            std::fs::remove_dir(root.join("locked")).unwrap();
        }
        sync(&work, &at(&two)).unwrap().unwrap();
        assert_eq!(current(&root), Some(at(&two)), "an idle work tree catches up after apply");
        assert!(target.join("release/makepad").exists());
        assert!(!work.join("tree").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
