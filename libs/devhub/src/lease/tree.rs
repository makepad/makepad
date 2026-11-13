//! Each box has a deployment tree, a shared work tree and bounded native lane trees:
//! - Deployment: the sync checkout and its target. The WM, dock apps, node hub and AI hub run
//!   from it. Only sync apply moves it, to landed work + the paired nested repository's main
//!   ([`crate::state::PAIRED`]); leases never hold it.
//!   A sync checkout that is someone's own clone ([`ours`] is false) is not a deployment tree.
//! - Work: `<work>/ci-tree` + `<work>/ci-target`. CI, gates and user boxes keep this detached
//!   worktree.
//! - One lease at a time holds a tree, at its pin too; another waits for it to end (two leases of
//!   one lane at one pin shared its native tree, and a script patching a file left the other's
//!   build half-patched, 11-03).
//! - Native: `<work>/native/<lane>` + `<work>/native-target/<lane>`. Other pinned lane leases
//!   hold only their lane's tree. Each nested repository a pin names ([`crate::nested`]) is a
//!   worktree of its store at `apps/<name>`. All trees share the bare stores' objects. Native moves use
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

/// Who holds the tree now: lease id -> its pin; which of them share it ([`take_native`]); and the
/// nested repositories a take left as someone left them ([`prepare`]): the release leaves them too.
#[derive(Default)]
pub struct Holders(BTreeMap<String, Pin>, std::collections::BTreeSet<String>, std::collections::BTreeSet<String>);

static TREE: Mutex<BTreeMap<PathBuf, Holders>> = Mutex::new(BTreeMap::new());

fn lock() -> MutexGuard<'static, BTreeMap<PathBuf, Holders>> {
    TREE.lock().unwrap_or_else(|e| e.into_inner())
}

impl Holders {
    /// Why lease `id` cannot take the tree now (another lease holds it), if it cannot.
    pub fn conflict(&self, id: &str) -> Option<String> {
        let other: Vec<&String> = self.0.keys().filter(|held| *held != id).collect();
        let first = other.first()?;
        Some(format!("the tree is at {} for {}: one lease at a time in a tree", self.0[*first].short(), other.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")))
    }

    /// A sharing lease `id` at `pin` may join the tree as it is: it is held, and only by sharing
    /// leases at that pin.
    fn joins(&self, id: &str, pin: &Pin) -> bool {
        let mut other = self.0.iter().filter(|(held, _)| *held != id).peekable();
        other.peek().is_some() && other.all(|(held, at)| at == pin && self.1.contains(held))
    }

    fn insert(&mut self, id: &str, pin: &Pin, shares: bool) {
        self.0.insert(id.to_string(), pin.clone());
        if shares { self.1.insert(id.to_string()); }
    }

    /// A take asked for `asked` and holds the tree at `at`: the nested repositories `at` names
    /// and `asked` does not stayed as someone left them.
    fn left(&mut self, asked: &Pin, at: &Pin) {
        self.2.extend(at.nested.keys().filter(|n| !asked.nested.contains_key(*n)).cloned());
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

pub(super) fn touch_native(root: &Path) {
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
    synced(root, CHECKOUT_REF)
}

/// The last synced tips, used for a CI lease that names no pin.
pub fn shared(root: &Path) -> Option<Pin> {
    synced(root, SYNCED_REF)
}

/// The commits the sync left at store ref `r`: makepad's, the paired nested repository's when it
/// has one, and each other nested checkout's at `root` that a store of this node holds (the
/// repositories a sync deploys, [`crate::sync::followed`]).
fn synced(root: &Path, r: &str) -> Option<Pin> {
    let at = |repo: &str| Store::open(repo).ok().and_then(|s| s.rev(r)).or_else(|| fleet_revision(root, repo));
    let mut pin = Pin::at(&at("makepad")?);
    let stores = crate::sync::git::store_names();
    let others = crate::nested::names(root).into_iter().filter(|n| n != crate::state::PAIRED && stores.contains(n));
    for name in std::iter::once(crate::state::PAIRED.to_string()).chain(others) {
        if let Some(sha) = at(&name) {
            pin.nested.insert(name, sha);
        }
    }
    Some(pin)
}

/// Repository `repo`'s side of the tree at `root`: makepad's is the root, a nested one's
/// `apps/<name>`.
fn side(root: &Path, repo: &str) -> PathBuf {
    if repo == "makepad" { root.to_path_buf() } else { root.join(crate::nested::dir(repo)) }
}

/// What repository `repo`'s paths start with in the tree: makepad's nothing, a nested one's
/// `apps/<name>/`.
fn prefix(repo: &str) -> String {
    if repo == "makepad" { String::new() } else { crate::nested::prefix(repo) }
}

/// The repositories the tree at `root` holds: its nested ones, then makepad.
fn repos_in(root: &Path) -> Vec<String> {
    crate::nested::names(root).into_iter().chain(["makepad".to_string()]).collect()
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
    let pin = Pin::at(&head(root)?);
    Some(crate::nested::names(root).into_iter().fold(pin, |pin, name| match head(&side(root, &name)) {
        Some(sha) => pin.with(&name, &sha),
        None => pin,
    }))
}

/// Moves the tree at `dir` (at `from`, None: no tree yet) to `to`: the count of files it changed.
fn move_to(store: &Store, dir: &Path, from: Option<&str>, to: &str, native: bool) -> Result<usize, String> {
    match from {
        Some(from) => {
            let files = run(Some(dir), None, &[], &["diff", "--name-only", "--no-renames", from, to])?.lines().count();
            if native {
                let _ = run(Some(dir), None, &[], &["update-index", "-q", "--refresh"]);
                run(Some(dir), None, &[], &["read-tree", "-m", "-u", from, to])?;
                run(Some(dir), None, &[], &["update-ref", "--no-deref", "HEAD", to])?;
            } else {
                run(Some(dir), None, &[], &["checkout", "-q", "--detach", to])?;
            }
            Ok(files)
        }
        None => {
            std::fs::create_dir_all(dir.parent().ok_or("work tree has no parent")?).map_err(|e| e.to_string())?;
            run(None, Some(&store.git_dir), &[], &["worktree", "add", "-q", "--detach", &dir.to_string_lossy(), to])?;
            Ok(run(Some(dir), None, &[], &["ls-files"])?.lines().count())
        }
    }
}

/// A move that failed part way back to `from`: what it wrote goes (the tree was scrubbed before
/// it, so nothing there is anyone's), and a tree it was adding is removed.
fn undo_move(store: &Store, dir: &Path, from: Option<&str>) {
    match from {
        Some(from) => {
            let _ = run(Some(dir), None, &[], &["reset", "-q", "--hard", from]);
            let _ = store.clean_tree(dir);
        }
        None => {
            let _ = run(None, Some(&store.git_dir), &[], &["worktree", "remove", "--force", &dir.to_string_lossy()]);
            let _ = run(None, Some(&store.git_dir), &[], &["worktree", "prune"]);
        }
    }
}

/// The objects of commit `to`'s tree (every tree and blob in it) the store lacks.
fn lacking(store: &Store, to: &str) -> Vec<String> {
    run(None, Some(&store.git_dir), &[], &["rev-list", "--objects", "--no-walk", "--missing=print", to])
        .map(|out| out.lines().filter_map(|l| l.strip_prefix('?')).map(|s| s.trim().to_string()).filter(|s| crate::sync::git::valid_sha(s)).collect())
        .unwrap_or_default()
}

fn prepare(id: &str, pin: &Pin, root: &Path, out: &Path, native: bool) -> Result<Taken, String> {
    let mut taken = Taken { root: root.to_path_buf(), pin: pin.clone(), moved: Vec::new(), cleared: Released::default() };
    // A pin must not inherit a previous pin's checkout of a nested repository it does not name: a
    // clean one (this node's own worktree, at its commit, no other file in it) goes. One holding
    // anything else (edits, untracked or ignored files) is work someone left: it stays as it is,
    // said, and the lease's pin names it at its commit.
    for name in crate::nested::names(root).into_iter().filter(|n| !pin.nested.contains_key(n)) {
        let dir = side(root, &name);
        let store = Store::open(&name)?;
        if store.admin_of(&dir).is_none() { return Err(format!("{} is not this node's worktree", dir.display())); }
        let held = held_paths(&dir)?;
        if !held.is_empty() {
            let shown: Vec<&str> = held.iter().take(8).map(String::as_str).collect();
            taken.cleared.kept.push(format!("{} stays, though the lease's pin does not name it: it holds {} uncommitted or untracked path(s): {}{}", prefix(&name), held.len(), shown.join(" "), if held.len() > shown.len() { " .." } else { "" }));
            if let Some(sha) = head(&dir) { taken.pin.nested.insert(name, sha); }
            continue;
        }
        // (--force for its build output, or a clean submodule's checkout: anything else held it above)
        run(None, Some(&store.git_dir), &[], &["worktree", "remove", "--force", &dir.to_string_lossy()])?;
        taken.cleared.removed.push(prefix(&name));
    }
    for (repo, to, _) in pin.repos() {
        let dir = side(root, repo);
        let store = Store::open(repo)?;
        if dir.join(".git").exists() && store.admin_of(&dir).is_none() {
            return Err(format!("{} is not this node's worktree", dir.display()));
        }
        // a lock a killed writer left in its git is cleared first, and one a live writer holds
        // waited for: a hub the WM replaced mid-start left `config.worktree.lock`, and every later
        // start in that tree was refused on it (11-13)
        taken.cleared.locks.extend(if dir.join(".git").exists() { crate::gitlock::sweep_worktree(&dir) } else { crate::gitlock::sweep(&store.git_dir) });
        let from = head(&dir);
        if from.as_deref() == Some(to) {
            crate::ci_merge::install_merge_driver(&dir, root)?;
            taken.moved.push((repo.to_string(), 0));
            continue;
        }
        if !store.has(to) {
            return Err(format!("{repo} {} is not in this node's store", to));
        }
        scrub(repo, root, from.as_deref(), id, out, &format!("{id}-start"), &mut taken.cleared)?;
        // A store holding the commit can lack objects of its tree: a reset fails reading them
        // (L966518, 11-13), and a checkout moves anyway, without the files it could not read (it
        // exits 0). The node says which before it moves, and the master sends them
        // ([`NO_OBJECTS`]): one listing of the tree, some 40 ms for this repository's.
        let missing = lacking(&store, to);
        if !missing.is_empty() {
            return Err(format!("{} {repo} {}: this node's store holds the commit, not its tree", super::NO_OBJECTS, missing.join(" ")));
        }
        // a move that fails with every object there met the store's own maintenance (a repack):
        // once more, after a look for objects that went missing
        let files = match move_to(&store, &dir, from.as_deref(), to, native) {
            Ok(files) => files,
            Err(e) => {
                undo_move(&store, &dir, from.as_deref());
                let missing = lacking(&store, to);
                if !missing.is_empty() {
                    return Err(format!("{} {repo} {}: {e}", super::NO_OBJECTS, missing.join(" ")));
                }
                move_to(&store, &dir, from.as_deref(), to, native).map_err(|again| {
                    undo_move(&store, &dir, from.as_deref());
                    format!("{again} (and before it: {e})")
                })?
            }
        };
        crate::ci_merge::install_merge_driver(&dir, root)?;
        taken.moved.push((repo.to_string(), files));
    }
    Ok(taken)
}

/// Every path of the checkout at `dir` that is not its commit's as committed: edited (a
/// submodule's edits too), untracked or ignored. What is untracked or ignored in its own `target/`
/// is build output, no one's work; a tracked file there, or a `target` dir further down, may be
/// someone's.
fn held_paths(dir: &Path) -> Result<Vec<String>, String> {
    let out = run(Some(dir), None, &[("LC_ALL", Path::new("C"))], &["-c", "core.quotePath=false", "status", "--porcelain", "--ignored", "--untracked-files=all", "--ignore-submodules=none"])?;
    Ok(out.lines().filter(|l| !(l.starts_with("!! target/") || l.starts_with("?? target/"))).map(|l| l.get(3..).unwrap_or(l).to_string()).collect())
}

/// What a lease start did to the tree.
#[derive(Debug)]
pub struct Taken {
    pub root: PathBuf,
    /// the pin the tree is at for the lease: the one it asked for, and a nested repository it does
    /// not name that stayed ([`Released::kept`])
    pub pin: Pin,
    /// files written per repo (0: it was there already)
    pub moved: Vec<(String, usize)>,
    /// what stood in the way and was cleared first
    pub cleared: Released,
}

/// Takes the work tree at `pin`; another lease holding it refuses the take.
pub fn take(id: &str, pin: &Pin, root: &Path, out: &Path) -> Result<Taken, String> {
    let mut h = lock();
    if let Some(why) = h.get(root).and_then(|h| h.conflict(id)) {
        return Err(why);
    }
    let t = prepare(id, pin, root, out, false)?;
    let holds = h.entry(root.to_path_buf()).or_default();
    holds.0.insert(id.to_string(), t.pin.clone());
    holds.left(pin, &t.pin);
    Ok(t)
}

/// Take a lane's native tree; capacity and disk pressure are retryable waits, never fallback
/// to the shared work tree. The same lock protects creation, holds, scrub and disk reclaim.
/// `shares`: the lease reads the tree at its pin beside others that do, the shards of one tests
/// bucket run ([`super::sched::tests_shard`]): one joins the tree as its sharing holders keep it,
/// untouched, and the last one's release scrubs it.
pub fn take_native(id: &str, pin: &Pin, work: &Path, lane: &str, limit: usize, floor: u64, shares: bool) -> Result<Taken, String> {
    if !super::exec::valid_name(lane) { return Err("invalid native lane".into()); }
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let root = native_root(work, lane);
    let out = work.join("recovered");
    let mut h = lock();
    if let Some(held) = h.get_mut(&root).filter(|held| shares && held.joins(id, pin)) {
        held.insert(id, pin, true);
        touch_native(&root);
        let moved = pin.repos().into_iter().map(|(repo, _, _)| (repo.to_string(), 0)).collect();
        return Ok(Taken { root, pin: pin.clone(), moved, cleared: Released::default() });
    }
    if let Some(why) = h.get(&root).and_then(|h| h.conflict(id)) { return Err(why); }
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
            super::disk::empty_trash_soon(work);
        }
    }
    if super::caps::disk_space(work).map_or(0, |(free, _)| free) < floor { return Err("native trees busy: disk below its floor".into()); }
    let mut t = prepare(id, pin, &root, &out, true)?;
    t.cleared.bundles.extend(cleared.bundles);
    t.cleared.removed.extend(cleared.removed);
    if super::caps::disk_space(work).map_or(0, |(free, _)| free) < floor { return Err("native trees busy: disk below its floor after checkout".into()); }
    let holds = h.entry(root.clone()).or_default();
    holds.insert(id, &t.pin, shares);
    holds.left(pin, &t.pin);
    touch_native(&root);
    Ok(t)
}

fn remove_native_idle(work: &Path, lane: &str) -> Result<Released, String> {
    let root = native_root(work, lane);
    let r = scrub_tree(&root, "reclaim", &work.join("recovered"), &format!("{lane}-reclaim-{}", crate::proto::wall_ms()), &Default::default());
    if !r.unclean.is_empty() { return Err(r.unclean.join("; ")); }
    let mut stores = Vec::new();
    for repo in repos_in(&root) {
        let dir = side(&root, &repo);
        if !dir.join(".git").exists() { continue; }
        let store = Store::open(&repo)?;
        if store.admin_of(&dir).is_none() { return Err(format!("{} is not this node's worktree", dir.display())); }
        stores.push(store);
    }
    // to the trash in one move each, a delete under a lock no more: the stores forget the
    // worktrees that left
    for path in [&root, &native_target(work, lane)] {
        super::disk::to_trash(work, path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    for store in stores {
        run(None, Some(&store.git_dir), &[], &["worktree", "prune"])?;
    }
    let _ = std::fs::remove_file(root.with_file_name(format!(".last-{lane}")));
    Ok(r)
}

/// Who holds a native tree while [`remove_native`] takes it apart.
const REMOVING: &str = "its-removal";

/// Reclaim a native tree and its target (to the trash: the caller empties it), checking its holds
/// again at deletion time. Its scrub and its moves hold the tree alone: the other trees' leases
/// go on.
pub fn remove_native(work: &Path, lane: &str) -> Result<Released, String> {
    if !super::exec::valid_name(lane) { return Err("invalid native lane".into()); }
    let root = native_root(work, lane);
    {
        let mut h = lock();
        let holds = h.entry(root.clone()).or_default();
        if !holds.0.is_empty() { return Err(format!("native tree {lane} is held")); }
        holds.0.insert(REMOVING.to_string(), Pin::default());
    }
    let r = remove_native_idle(work, lane);
    lock().remove(&root);
    r
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
    let (dir, prefix) = (side(root, repo), prefix(repo));
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
    scrub_tree(root, "startup", out, &tag, &Default::default())
}

/// Scrubs every repository of the tree but those `left` names (kept as someone left them).
fn scrub_tree(root: &Path, id: &str, out: &Path, tag: &str, left: &std::collections::BTreeSet<String>) -> Released {
    let mut r = Released::default();
    for repo in repos_in(root).into_iter().filter(|r| !left.contains(r)) {
        if let Err(e) = scrub(&repo, root, head(&side(root, &repo)).as_deref(), id, out, tag, &mut r) {
            r.unclean.push(format!("{repo}: {e}"));
        }
    }
    r
}

/// Undoes what `devhub run` shipped on top of `pin` (`paths` from the tree's root; a nested
/// repository's under `apps/<name>/`): files the pin tracks get its version back (deleted ones
/// too), the others (new files of the lane's worktree) go, with the dirs they leave empty. The count.
pub fn undo_overlay(root: &Path, pin: &Pin, paths: &[String]) -> Result<usize, String> {
    let mut n = 0;
    let names: std::collections::BTreeSet<String> = pin.nested.keys().cloned().chain(crate::nested::names(root)).collect();
    let mut by_repo: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in paths {
        let (repo, rest) = crate::nested::split(p, names.iter().map(String::as_str)).unwrap_or(("makepad", p.as_str()));
        by_repo.entry(repo).or_default().push(rest);
    }
    for (repo, mine) in by_repo {
        let dir = side(root, repo);
        let sha = if repo == "makepad" { Some(pin.makepad.as_str()) } else { pin.nested.get(repo).map(String::as_str) };
        let Some(sha) = sha else {
            // a lease that pinned none of this nested repository shipped its files: they are
            // untracked here
            for p in &mine {
                n += remove_shipped(&dir, p) as usize;
            }
            continue;
        };
        let g = |args: &[&str]| crate::sync::git::run(Some(&dir), None, &[], args);
        let mut tracked = std::collections::HashSet::new();
        for chunk in mine.chunks(500) {
            let mut args = vec!["ls-tree", "-r", "-z", "--name-only", sha, "--"];
            args.extend(chunk.iter().copied());
            tracked.extend(g(&args)?.split('\0').filter(|s| !s.is_empty()).map(str::to_string));
        }
        let (back, gone): (Vec<&str>, Vec<&str>) = mine.iter().partition(|p| tracked.contains(**p));
        for chunk in back.chunks(500) {
            let mut args = vec!["checkout", "-q", sha, "--"];
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
pub fn hold(id: &str, pin: &Pin, root: &Path, shares: bool) {
    lock().entry(root.to_path_buf()).or_default().insert(id, pin, shares);
}

/// What scrubbing recovered, removed, or could not clear.
#[derive(Debug, Default)]
pub struct Released {
    pub bundles: Vec<PathBuf>,
    pub removed: Vec<String>,
    pub unclean: Vec<String>,
    /// what stayed in the tree because it holds someone's files, and why
    pub kept: Vec<String>,
    /// git lock files a writer that is gone left, cleared, or one a live writer held, waited for
    /// ([`crate::gitlock::settle`])
    pub locks: Vec<String>,
}

/// The last holder bundles tracked edits and removes untracked files; the pin and target stay.
pub fn release(id: &str, root: &Path, out: &Path, tag: &str) -> Released {
    let mut h = lock();
    let Some(holders) = h.get_mut(root) else { return Released::default() };
    holders.1.remove(id);
    if holders.0.remove(id).is_none() || !holders.0.is_empty() {
        return Released::default();
    }
    let r = scrub_tree(root, id, out, tag, &holders.2);
    h.remove(root);
    touch_native(root);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(m: char, c: Option<char>) -> Pin {
        let pin = Pin::at(&m.to_string().repeat(40));
        match c {
            Some(c) => pin.with("commercial", &c.to_string().repeat(40)),
            None => pin,
        }
    }

    #[test]
    fn pins_parse_and_one_lease_holds_a_tree() {
        let a = pin('a', Some('c'));
        assert!(a.fits(&pin('a', Some('c'))) && a.fits(&pin('a', None)) && pin('a', None).fits(&a));
        assert!(!a.fits(&pin('b', Some('c'))) && !a.fits(&pin('a', Some('d'))));
        assert_eq!(Pin::parse(&a.key()), Some(a.clone()));
        assert_eq!(Pin::parse(&pin('b', None).key()), Some(pin('b', None)));
        assert_eq!(Pin::parse("xyz"), None);
        assert_eq!(Pin::of_rev(&format!("rc-x@{},commercial=main@{}", "a".repeat(40), "c".repeat(40))), Some(a.clone()));
        assert_eq!(Pin::of_rev(&format!("rc-x@{},main@{}", "a".repeat(40), "c".repeat(40))), None, "a nested side names its repository");
        assert_eq!(Pin::parse(&format!("{},{}", "a".repeat(40), "c".repeat(40))), None, "so does a nested sha in a key");
        // any nested repository by name, in name order
        let (c, e, f) = ("c".repeat(40), "e".repeat(40), "f".repeat(40));
        let two = Pin::at(&"a".repeat(40)).with("source-library", &e).with("commercial", &c);
        assert_eq!(two.key(), format!("{},commercial={c},source-library={e}", "a".repeat(40)));
        assert_eq!(Pin::parse(&two.key()), Some(two.clone()));
        assert_eq!(Pin::parse(&format!("{},commercial={c},commercial={c}", "a".repeat(40))), None, "one side per repository");
        assert!(two.fits(&a) && a.fits(&two) && !two.fits(&Pin::at(&"a".repeat(40)).with("source-library", &f)));
        assert_eq!(two.repo_of("apps/source-library/src/lib.rs"), ("source-library", e.as_str(), "src/lib.rs"));
        assert_eq!(two.repo_of("apps/source-library-x/a.rs"), ("makepad", "a".repeat(40).as_str(), "apps/source-library-x/a.rs"));
        assert_eq!(two.short(), format!("aaaaaaaaaa,commercial=cccccccccc,source-library={}", &e[..10]));
        assert_eq!(two.pair(), crate::state::Pair::new(&"a".repeat(40), &c), "the ledgers' pair: makepad and the paired one");
        let mut h = Holders::default();
        h.0.insert("L1".into(), a.clone());
        assert_eq!(h.conflict("L1"), None, "its own hold");
        let why = h.conflict("L2").unwrap();
        assert!(why.contains("aaaaaaaaaa,commercial=cccccccccc") && why.contains("L1") && why.contains("one lease at a time"), "{why}");
        assert!(!h.joins("L2", &a), "a holder that does not share keeps it alone");
        // the shards of one tests bucket run: each joins the others' tree at their pin
        let mut s = Holders::default();
        assert!(!s.joins("S1", &a), "a tree nobody holds is taken, not joined");
        s.insert("S1", &a, true);
        assert!(s.joins("S2", &a));
        assert!(!s.joins("S2", &pin('b', Some('c'))), "at another pin it waits");
        s.insert("S2", &a, true);
        s.insert("L3", &a, false);
        assert!(!s.joins("S4", &a), "beside one that does not share it waits");
    }

    /// A shard's release leaves its siblings the tree as it is (the last one's scrubs it).
    #[test]
    fn a_sharing_holders_release_leaves_the_tree_to_the_others() {
        let root = std::env::temp_dir().join(format!("devhub-tree-share-{}", std::process::id()));
        let a = pin('a', Some('c'));
        hold("S1", &a, &root, true);
        hold("S2", &a, &root, true);
        assert!(lock().get(&root).is_some_and(|h| h.joins("S3", &a)));
        let r = release("S1", &root, &root.join("recovered"), "t");
        assert!(r.bundles.is_empty() && r.removed.is_empty() && r.unclean.is_empty(), "{r:?}");
        assert!(held(&root) && lock().get(&root).is_some_and(|h| h.joins("S3", &a) && !h.1.contains("S1")), "S2 keeps it");
        lock().remove(&root);
    }

    /// One lease at a time, at its pin too; release and startup scrub without moving the pin or its target.
    #[test]
    fn leases_take_the_one_tree_in_turn_and_leave_it_warm() {
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
        let at = Pin::at;
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

        // a lease at `two`: no file moves; another lease waits for it, at `two` as at `one`
        let t = take("L1", &at(&two), &root, &out).unwrap();
        assert_eq!((t.root.clone(), t.moved.clone()), (root.clone(), vec![("makepad".to_string(), 0)]));
        let why = take("L2", &at(&two), &root, &out).unwrap_err();
        assert!(why.contains("for L1: one lease at a time"), "{why}");
        assert!(take("L3", &at(&one), &root, &out).unwrap_err().contains("L1"));
        assert!(sync(&work, &at(&one)).unwrap().is_none(), "sync leaves a held work tree alone");
        store.move_checkout(&deployment, Some(&one), &two).unwrap();
        store.set_ref(SYNCED_REF, &two).unwrap();
        assert_eq!(deploy(&deployment), Some(at(&two)), "deployment applies while leases hold the work tree");
        std::fs::write(root.join("b.rs"), "edited by L1").unwrap();
        std::fs::write(root.join("scratch.txt"), "left by L1").unwrap();
        let b = release("L1", &root, &out, "x-L1");
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
        hold("L4", &at(&one), &root, false);
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

        // a store that holds a commit but not a blob of its tree: the move says which object it
        // lacks and leaves the tree at its pin, with nothing of the move in it; with the object
        // there (the master's fill), the lease starts (L966518, 11-13)
        // (contents that differ: one blob for both would be copied with a.rs)
        std::fs::write(src.join("c.rs"), "c3").unwrap();
        std::fs::write(src.join("a.rs"), "3").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "three"]);
        let three = g(&["rev-parse", "HEAD"]).trim().to_string();
        let loose = |id: &str| format!("objects/{}/{}", &id[..2], &id[2..]);
        let (tree, blob, a3) = (g(&["rev-parse", "HEAD^{tree}"]).trim().to_string(), g(&["rev-parse", "HEAD:c.rs"]).trim().to_string(), g(&["rev-parse", "HEAD:a.rs"]).trim().to_string());
        for id in [&three, &tree, &a3] {
            std::fs::create_dir_all(store.git_dir.join(loose(id)).parent().unwrap()).unwrap();
            std::fs::copy(src.join(".git").join(loose(id)), store.git_dir.join(loose(id))).unwrap();
        }
        let why = take("L8", &at(&three), &root, &out).unwrap_err();
        assert_eq!(crate::lease::lacking_objects(&why), Some(("makepad".to_string(), vec![blob.clone()])), "{why}");
        assert_eq!(current(&root), Some(at(&two)), "the failed move keeps the tree's pin");
        assert!(!root.join("c.rs").exists() && std::fs::read_to_string(root.join("a.rs")).unwrap() == "2", "nothing of the failed move stays");
        std::fs::create_dir_all(store.git_dir.join(loose(&blob)).parent().unwrap()).unwrap();
        std::fs::copy(src.join(".git").join(loose(&blob)), store.git_dir.join(loose(&blob))).unwrap();
        take("L8", &at(&three), &root, &out).unwrap();
        assert_eq!((current(&root), std::fs::read_to_string(root.join("c.rs")).unwrap()), (Some(at(&three)), "c3".to_string()));
        release("L8", &root, &out, "x-L8");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The shared tree holds a nested repository only at a pin that names it: a lease whose pin
    /// does not gets the tree without it, never at the commit an earlier lease left there. Only
    /// this node's own clean worktree goes: one holding edits or untracked files stays (said, and
    /// the lease's pin names it), and a checkout that is not this node's is never touched.
    #[test]
    fn a_shared_tree_drops_a_nested_repository_its_pin_does_not_name() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-tree-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (work, stores) = (base.join("work"), base.join("stores"));
        let (root, out) = (super::root(&work), work.join("recovered"));
        std::env::set_var("MAKEPAD_DEVHUB_GIT", &stores);
        let git = |dir: &Path, args: &[&str]| run(Some(dir), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        let commit = |dir: &Path, files: &[(&str, &str)]| {
            std::fs::create_dir_all(dir).unwrap();
            git(dir, &["init", "-q", "-b", "main"]);
            for (f, text) in files {
                std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
                std::fs::write(dir.join(f), text).unwrap();
            }
            git(dir, &["add", "."]);
            git(dir, &["commit", "-q", "-m", "one"]);
            git(dir, &["rev-parse", "HEAD"]).trim().to_string()
        };
        let stored = |name: &str, dir: &Path| {
            let bundle = base.join(format!("{name}.bundle"));
            git(dir, &["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/main"]);
            Store::open(name).unwrap().fetch_bundle(&bundle, "main").unwrap();
        };
        let mk = commit(&base.join("src"), &[(".gitignore", "/apps/side-app/\n/apps/user-app/\n"), ("a.rs", "1")]);
        stored("makepad", &base.join("src"));
        // (a tracked file in a `target` dir of its own, and a submodule)
        let sub = commit(&base.join("sub"), &[("s.rs", "s")]);
        commit(&base.join("side"), &[("lib.rs", "side"), ("src/target/x", "x")]);
        git(&base.join("side"), &["update-index", "--add", "--cacheinfo", &format!("160000,{sub},sub")]);
        git(&base.join("side"), &["commit", "-q", "-m", "two"]);
        let side = git(&base.join("side"), &["rev-parse", "HEAD"]).trim().to_string();
        stored("side-app", &base.join("side"));
        let (with, without) = (Pin::at(&mk).with("side-app", &side), Pin::at(&mk));
        let lib = root.join("apps/side-app/lib.rs");

        let t = take("N1", &with, &root, &out).unwrap();
        assert_eq!(t.moved, vec![("makepad".to_string(), 2), ("side-app".to_string(), 3)]);
        assert_eq!((current(&root), std::fs::read_to_string(&lib).unwrap()), (Some(with.clone()), "side".to_string()));
        std::fs::write(&lib, "edited by N1").unwrap();
        release("N1", &root, &out, "x-N1");

        // its clean worktree goes, its build output too: that is no one's work
        std::fs::create_dir_all(root.join("apps/side-app/target/debug")).unwrap();
        std::fs::write(root.join("apps/side-app/target/debug/side"), "built").unwrap();
        let t = take("N2", &without, &root, &out).unwrap();
        assert_eq!((t.cleared.removed.clone(), t.cleared.kept.clone(), t.pin.clone()), (vec!["apps/side-app/".to_string()], Vec::<String>::new(), without.clone()), "{t:?}");
        assert_eq!((current(&root), root.join("apps/side-app").exists()), (Some(without.clone()), false));
        release("N2", &root, &out, "x-N2");
        let t = take("N3", &with, &root, &out).unwrap();
        assert_eq!(t.moved, vec![("makepad".to_string(), 0), ("side-app".to_string(), 3)], "back at its pin");
        assert_eq!(std::fs::read_to_string(&lib).unwrap(), "side");
        release("N3", &root, &out, "x-N3");

        // left with an untracked file, or an edit (in a `target` dir below its root, in its
        // submodule), it stays as it is: said, and in the lease's pin
        git(&base, &["clone", "-q", &base.join("sub").to_string_lossy(), &root.join("apps/side-app/sub").to_string_lossy()]);
        for (file, text, said) in [("notes.txt", "someone's", "notes.txt"), ("lib.rs", "someone's edit", "lib.rs"), ("src/target/x", "someone's edit", "src/target/x"), ("sub/s.rs", "someone's edit", "sub")] {
            let left = root.join("apps/side-app").join(file);
            std::fs::write(&left, text).unwrap();
            let t = take("N4", &without, &root, &out).unwrap();
            assert!(t.cleared.removed.is_empty() && t.cleared.kept.len() == 1 && t.cleared.kept[0].contains("apps/side-app/ stays") && t.cleared.kept[0].ends_with(&format!(": {said}")), "{t:?}");
            assert_eq!((t.pin.clone(), std::fs::read_to_string(&left).unwrap()), (with.clone(), text.to_string()));
            assert!(lock().get(&root).is_some_and(|h| h.0.get("N4") == Some(&with)), "held at the pin it is at");
            release("N4", &root, &out, "x-N4");
            assert_eq!(std::fs::read_to_string(&left).ok().as_deref(), Some(text), "its release leaves it as someone left it too");
            match file { "lib.rs" => std::fs::write(&left, "side").unwrap(), "src/target/x" => std::fs::write(&left, "x").unwrap(), "sub/s.rs" => std::fs::write(&left, "s").unwrap(), _ => std::fs::remove_file(&left).unwrap() }
        }

        // a checkout that is not this node's worktree (someone's clone) is refused, untouched
        let user = root.join("apps/user-app");
        commit(&user, &[("mine.rs", "mine")]);
        std::fs::write(user.join("draft.txt"), "draft").unwrap();
        let why = take("N5", &with, &root, &out).unwrap_err();
        assert!(why.contains("is not this node's worktree"), "{why}");
        assert_eq!((std::fs::read_to_string(user.join("mine.rs")).unwrap(), std::fs::read_to_string(user.join("draft.txt")).unwrap()), ("mine".to_string(), "draft".to_string()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
