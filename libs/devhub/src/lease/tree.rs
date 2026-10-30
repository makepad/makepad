//! The node's ONE tree: every lease with a revision runs in the node's synced checkout (the tree
//! its WM and dock apps run from, `sync.conf checkout`, default `~/makepad` + `apps/commercial`),
//! moved in place to the lease's pinned commits. One tree, one cargo target (`<tree>/target`),
//! one JIT (the host profile the WM's apps run, always started as `cargo run --profile host -p
//! makepad-run --bin makepad -- <args>`: cargo rebuilds it only when its own crates changed) and one JIT cache (`<tree>/target/makepad-run`, keyed by the loader build id)
//! per node.
//!
//! - A lease moves the tree with `read-tree -m -u` ([`Store::move_checkout`]): only files that
//!   differ between the two commits get new mtimes, so cargo and the JIT stay warm.
//! - Leases of the same commits share the tree and run at once; a lease for other commits is
//!   refused here while one holds it (the scheduler does not place it: it waits, or goes to a
//!   node already at its commits or free).
//! - When the last holder ends, in the same release step, the tree goes back to the shared commits
//!   (the landed work + main pair the last sync distributed, [`shared`]): the WM and its apps sit
//!   on landed code between leases, and the next lease moves only its own diff. Edits a lease
//!   left in tracked files are committed and bundled to `<work>/recovered/` first. Untracked
//!   files are never removed (the WM's apps write there too); one in the way of a move makes it
//!   refuse. Ignored files (the target, the JIT cache) are never touched.
//! - A hub that starts with no lease holding the tree puts a stray tree (its previous process
//!   died mid-lease) back on the shared commits before it takes a lease ([`restore_if_free`]).
//! - A sync moves the tree only while no lease holds it ([`free`]).
//! - The WM's JIT apps run from the same tree: they live-reload onto whatever it holds.
//!
//! A node whose checkout is a git clone of its own (a `.git` dir: someone's working repo, never
//! devhub's) runs leases in `<work>/tree` instead, the same way.

use super::Pin;
use crate::sync::git::{fleet_revision, Store, CHECKOUT_REF, SYNCED_REF};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// Who holds the tree now: lease id -> its pin.
#[derive(Default)]
pub struct Holders(BTreeMap<String, Pin>);

static TREE: Mutex<Holders> = Mutex::new(Holders(BTreeMap::new()));

fn lock() -> MutexGuard<'static, Holders> {
    TREE.lock().unwrap_or_else(|e| e.into_inner())
}

impl Holders {
    /// Why `pin` cannot take the tree now (another pin holds it), if it cannot.
    pub fn conflict(&self, pin: &Pin) -> Option<String> {
        let other: Vec<&String> = self.0.iter().filter(|(_, p)| !p.fits(pin)).map(|(id, _)| id).collect();
        let first = other.first()?;
        Some(format!("the tree is at {} for {}", self.0[*first].short(), other.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")))
    }
}

/// The node's tree: the sync checkout when it is devhub's (synced: a FLEET-REVISION.txt or our
/// `.git` link; or not there yet), else `<work>/tree` (a Mac's `~/makepad` that is someone's own
/// clone or build mirror is never touched).
pub fn root(work: &Path) -> PathBuf {
    let sync = crate::sync::SyncConf::load().checkout;
    if ours(&sync) {
        sync
    } else {
        work.join("tree")
    }
}

/// Whether `dir` is (or can become) devhub's tree.
pub fn ours(dir: &Path) -> bool {
    let empty = dir.read_dir().map_or(true, |mut d| d.next().is_none());
    let stores = std::fs::canonicalize(crate::sync::git::stores_dir()).ok();
    let linked = std::fs::read_to_string(dir.join(".git")).ok().and_then(|t| t.trim().strip_prefix("gitdir:").map(|g| PathBuf::from(g.trim()))).and_then(|g| std::fs::canonicalize(g).ok()).is_some_and(|g| stores.is_some_and(|s| g.starts_with(s)));
    empty || linked || (!dir.join(".git").exists() && dir.join("FLEET-REVISION.txt").is_file())
}

/// The commits the tree holds now (`caps`'s `tree`): None until something moved it or a deploy
/// recorded it.
pub fn current(root: &Path) -> Option<Pin> {
    let at = |repo: &str| Store::open(repo).ok().and_then(|s| s.rev(CHECKOUT_REF)).or_else(|| fleet_revision(&root, repo));
    Some(Pin { makepad: at("makepad")?, commercial: at("commercial") })
}

/// The shared commits: what the last sync moved the tree to (the landed work + main pair it
/// distributes), else what the deploy recorded. None: never synced.
pub fn shared(root: &Path) -> Option<Pin> {
    let at = |repo: &str| Store::open(repo).ok().and_then(|s| s.rev(SYNCED_REF)).or_else(|| fleet_revision(root, repo));
    Some(Pin { makepad: at("makepad")?, commercial: at("commercial") })
}

/// Moves the tree (no holder) to the shared commits: what it moved to, None when it was there.
fn to_shared(root: &Path) -> Result<Option<String>, String> {
    let Some(to) = shared(root) else { return Ok(None) };
    let mut moved = false;
    for (repo, sha) in [("makepad", Some(&to.makepad)), ("commercial", to.commercial.as_ref())] {
        let Some(sha) = sha else { continue };
        let store = Store::open(repo)?;
        let dir = if repo == "makepad" { root.to_path_buf() } else { root.join("apps").join("commercial") };
        let from = store.rev(CHECKOUT_REF).or_else(|| fleet_revision(root, repo)).filter(|f| store.has(f));
        if from.as_deref() == Some(sha.as_str()) || !store.has(sha) {
            continue;
        }
        store.move_checkout(&dir, from.as_deref(), sha)?;
        moved = true;
    }
    Ok(moved.then(|| to.short()))
}

/// A hub starting with no lease on the tree: a tree left off the shared commits goes back.
pub fn restore_if_free(root: &Path) -> Result<Option<String>, String> {
    let h = lock();
    if !h.0.is_empty() {
        return Ok(None);
    }
    to_shared(root)
}

/// A sync may move the tree: no lease holds it. The guard keeps leases off while the sync moves it.
pub fn free() -> Result<MutexGuard<'static, Holders>, String> {
    let h = lock();
    if h.0.is_empty() {
        return Ok(h);
    }
    let ids: Vec<&str> = h.0.keys().map(|s| s.as_str()).collect();
    Err(format!("leases hold the tree ({}): it moves when they end", ids.join(" ")))
}

/// What a lease start did to the tree.
#[derive(Debug, PartialEq)]
pub struct Taken {
    pub root: PathBuf,
    /// files written per repo (0: it was there already)
    pub moved: Vec<(String, usize)>,
}

/// Lease `id` takes the tree at `pin`: moved there in place unless it is there already; refused
/// while another pin holds it.
pub fn take(id: &str, pin: &Pin, root: &Path) -> Result<Taken, String> {
    let mut h = lock();
    if let Some(why) = h.conflict(pin) {
        return Err(why);
    }
    let root = root.to_path_buf();
    let mut moved = Vec::new();
    for (repo, to) in [("makepad", Some(&pin.makepad)), ("commercial", pin.commercial.as_ref())] {
        let Some(to) = to else { continue };
        let store = Store::open(repo)?;
        if !store.has(to) {
            return Err(format!("{repo} {} is not in this node's store", &to[..10]));
        }
        let dir = if repo == "makepad" { root.clone() } else { root.join("apps").join("commercial") };
        let from = store.rev(CHECKOUT_REF).or_else(|| fleet_revision(&root, repo)).filter(|f| store.has(f));
        if from.is_none() && dir.read_dir().is_ok_and(|mut d| d.next().is_some()) && !(repo == "makepad" && crate::sync::serve::only_commercial(&dir)) {
            return Err(format!("{} holds files but no known commit: sync it first", dir.display()));
        }
        let m = store.move_checkout(&dir, from.as_deref(), to)?;
        moved.push((repo.to_string(), m.files.len()));
    }
    h.0.insert(id.to_string(), pin.clone());
    Ok(Taken { root, moved })
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
pub fn hold(id: &str, pin: &Pin) {
    lock().0.insert(id.to_string(), pin.clone());
}

/// What a release did: the bundles of a lease's leftovers, and the shared commits the tree went
/// back to (None: it was there, or another lease still holds it).
#[derive(Debug, Default)]
pub struct Released {
    pub bundles: Vec<PathBuf>,
    pub restored: Option<String>,
}

/// Lease `id` lets go of the tree. When it was the last holder: edits it left in tracked files
/// are bundled to `out` (`<tag>.bundle`, `<tag>-commercial.bundle`), then the tree goes back to
/// the shared commits, all before this returns (before the node counts as free).
pub fn release(id: &str, root: &Path, out: &Path, tag: &str) -> Result<Released, String> {
    let mut h = lock();
    let Some(pin) = h.0.remove(id) else { return Ok(Released::default()) };
    if !h.0.is_empty() {
        return Ok(Released::default());
    }
    let mut bundles = Vec::new();
    // nested (commercial) first: it lives inside the makepad tree
    for (repo, at) in [("commercial", pin.commercial.as_ref()), ("makepad", Some(&pin.makepad))] {
        let Some(at) = at else { continue };
        let store = Store::open(repo)?;
        let dir = if repo == "makepad" { root.to_path_buf() } else { root.join("apps").join("commercial") };
        if store.tree_dirty(&dir)? {
            let t = if repo == "makepad" { tag.to_string() } else { format!("{tag}-{repo}") };
            bundles.push(store.recover_tree(&dir, at, id, out, &t)?);
        }
    }
    let restored = to_shared(root)?;
    Ok(Released { bundles, restored })
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

    /// Two leases at the same commits share the tree, one at other commits is refused until they
    /// end; a sync is held off while leases hold it; the last one's leftovers are bundled; and no
    /// tree stays on a lane's commits after its lease ends (released, or a hub that died
    /// mid-lease): it is back on the shared commits, its target untouched.
    #[test]
    fn leases_share_the_one_tree_and_leave_it_warm() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, root, stores) = (base.join("src"), base.join("makepad"), base.join("stores"));
        std::fs::create_dir_all(&src).unwrap();
        let g = |args: &[&str]| crate::sync::git::run(Some(&src), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
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
        // the node's synced tree at `two` (a fresh one: what a sync distributed), with a warm target
        store.move_checkout(&root, None, &two).unwrap();
        store.set_ref(SYNCED_REF, &two).unwrap();
        std::fs::create_dir_all(root.join("target/release")).unwrap();
        std::fs::write(root.join("target/release/makepad"), "jit").unwrap();
        let at = |c: &str| Pin { makepad: c.to_string(), commercial: None };
        assert_eq!(current(&root), Some(at(&two)));

        // two leases at `two`: no file moves; a lease at `one` waits for them
        let t = take("L1", &at(&two), &root).unwrap();
        assert_eq!((t.root.clone(), t.moved.clone()), (root.clone(), vec![("makepad".to_string(), 0)]));
        take("L2", &at(&two), &root).unwrap();
        let why = take("L3", &at(&one), &root).unwrap_err();
        assert!(why.contains("L1 L2"), "{why}");
        assert!(free().is_err(), "a sync waits while leases hold the tree");
        // the first one leaves: nothing changes for the second; the second leaves the tree warm
        std::fs::write(root.join("b.rs"), "edited by L2").unwrap();
        std::fs::write(root.join("scratch.txt"), "left by L2").unwrap();
        assert!(release("L1", &root, &base.join("rec"), "x-L1").unwrap().bundles.is_empty());
        assert_eq!(std::fs::read_to_string(root.join("b.rs")).unwrap(), "edited by L2", "a holder is still on the tree");
        let b = release("L2", &root, &base.join("rec"), "x-L2").unwrap();
        assert_eq!((b.bundles.len(), b.restored), (1, None), "at the shared commits already");
        assert_eq!(std::fs::read_to_string(root.join("b.rs")).unwrap(), "1");
        assert!(root.join("scratch.txt").exists() && root.join("target/release/makepad").exists());
        drop(free().unwrap());
        // a lane lease at `one` takes it: only a.rs is written; the target stays
        let b_time = std::fs::metadata(root.join("b.rs")).unwrap().modified().unwrap();
        let t = take("L3", &at(&one), &root).unwrap();
        assert_eq!(t.moved, vec![("makepad".to_string(), 1)]);
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "1");
        assert_eq!(std::fs::metadata(root.join("b.rs")).unwrap().modified().unwrap(), b_time);
        // its end puts the tree back on the shared commits in the same step: no box stays on a lane rev
        let r = release("L3", &root, &base.join("rec"), "x-L3").unwrap();
        assert!(r.bundles.is_empty() && r.restored.is_some());
        assert_eq!(current(&root), Some(at(&two)));
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "2");
        assert_eq!(std::fs::metadata(root.join("b.rs")).unwrap().modified().unwrap(), b_time, "only the lane's diff moves back");
        assert_eq!(std::fs::read_to_string(root.join("target/release/makepad")).unwrap(), "jit", "the warm target stays");
        // a restarted hub takes its holds back from the ledger; their end (the watchdog's) restores too
        take("L4", &at(&one), &root).unwrap();
        hold("L4", &at(&one));
        assert!(take("L5", &at(&two), &root).is_err());
        assert_eq!(restore_if_free(&root).unwrap(), None, "a held tree is the lease's");
        assert!(release("L4", &root, &base.join("rec"), "x-L4").unwrap().restored.is_some());
        assert_eq!(current(&root), Some(at(&two)));
        // a hub that died mid-lease with no ledger left the tree on a lane rev: the new hub puts it back
        store.move_checkout(&root, Some(&two), &one).unwrap();
        assert!(restore_if_free(&root).unwrap().is_some());
        assert_eq!(current(&root), Some(at(&two)));
        assert_eq!(restore_if_free(&root).unwrap(), None);
        // `devhub run` on top of a pin: an edited file, a new file in a new dir, a deleted file;
        // all undone at the end, then the tree is back on the shared commits
        take("L6", &at(&one), &root).unwrap();
        std::fs::write(root.join("a.rs"), "lane edit").unwrap();
        std::fs::create_dir_all(root.join("newcrate/src")).unwrap();
        std::fs::write(root.join("newcrate/src/lib.rs"), "new").unwrap();
        std::fs::remove_file(root.join("b.rs")).unwrap();
        let shipped = vec!["a.rs".to_string(), "newcrate/src/lib.rs".to_string(), "b.rs".to_string()];
        assert_eq!(undo_overlay(&root, &at(&one), &shipped).unwrap(), 3);
        assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "1");
        assert!(root.join("b.rs").exists() && !root.join("newcrate").exists(), "the new dir goes with its file");
        let r = release("L6", &root, &base.join("rec"), "x-L6").unwrap();
        assert!(r.bundles.is_empty(), "nothing of the overlay is left to bundle");
        assert_eq!(current(&root), Some(at(&two)));
        assert!(root.join("target/release/makepad").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
