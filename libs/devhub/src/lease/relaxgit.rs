//! Pinned source stores and per-lease overlays. Feeds read the node's bare git repos;
//! Dev state stays under `<work>/dev`; the deployment checkout never moves.
//! A per-store feed lock covers planning, append and its possible compaction.
//! The store's own append lock protects its bytes. Only the newest ref per lane and work is kept,
//! and of those only the newest [`LANE_REFS_KEPT`] lanes (and [`CI_REFS_KEPT`] CI pins) besides
//! the lanes with a live lease here: the index holds a path table per kept ref, and a node that
//! kept every lane it ever ran read a 125 MB index (decoding all of it) on every ship.

use super::Pin;
use crate::sync::git;
use relax_base::gitstore::{self as store, Append, Oid, Ref, Store};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub fn path(work: &Path, repo: &str) -> PathBuf { work.join("dev/git").join(format!("{repo}.relaxgit")) }
/// A lease's edits: its source on top of the pin and nobody else's. Two leases of one lane at one
/// pin run side by side, and a folder per lane let the second's ship replace the first's under it.
pub fn overlay(work: &Path, lane: &str, lease: &str) -> PathBuf { lane_overlays(work, lane).join(lease) }
/// The folders of the lane's leases.
pub fn lane_overlays(work: &Path, lane: &str) -> PathBuf { work.join("dev/overlay").join(lane) }
fn bare(repo: &str) -> PathBuf { git::stores_dir().join(format!("{repo}.git")) }
fn run(repo: &str, args: &[&str]) -> Result<String, String> { git::run(None, Some(&bare(repo)), &[], args) }
fn io(e: std::io::Error) -> String { e.to_string() }
fn layer(repo: &str, commit: &str, what: &str, e: impl std::fmt::Display) -> String {
    format!("overlay: {repo} {commit} {what}: {}", e.to_string().replace(['\n', '\r'], " | "))
}

/// The refs of CI overlay lanes (one lane per pin, `ci-<hash>`) and how many of them a store keeps.
const CI_REF: &str = "devhub:ci-";
const CI_REFS_KEPT: usize = 8;
/// The other lanes' refs a store keeps (newest first), besides those with a live lease.
pub const LANE_REFS_KEPT: usize = 12;

/// What the reads of the stores cost this process: opens, index bytes read, path-table entries
/// decoded (the node hub's heap grew to 1.4 GB decoding every lane's table on each open).
static OPENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static INDEX_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `{"relaxgit":{"opens","index_mb","entries"}}`: the reads so far (`devhub heap <node>`).
pub fn counters() -> String {
    let n = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
    format!("{{\"relaxgit\":{{\"opens\":{},\"index_mb\":{},\"entries\":{}}}}}", n(&OPENS), n(&INDEX_BYTES) >> 20, n(&ENTRIES))
}

/// The store with the path tables of `keep` only, counted.
fn open_counted(path: &Path, keep: &dyn Fn(&Oid) -> bool) -> std::io::Result<Option<Store>> {
    let s = Store::open_with(path, keep)?;
    if let Some(s) = &s {
        use std::sync::atomic::Ordering::Relaxed;
        OPENS.fetch_add(1, Relaxed);
        INDEX_BYTES.fetch_add(s.index_len, Relaxed);
        ENTRIES.fetch_add(s.index.tables.iter().map(|(_, t)| t.len() as u64).sum(), Relaxed);
    }
    Ok(s)
}

/// The overlay lanes with a live lease in this process (the node's lease executor says).
static LIVE: std::sync::OnceLock<Box<dyn Fn() -> HashSet<String> + Send + Sync>> = std::sync::OnceLock::new();

/// The node's lease executor names its live overlay lanes: their refs stay in the stores.
pub fn set_live(f: Box<dyn Fn() -> HashSet<String> + Send + Sync>) {
    let _ = LIVE.set(f);
}

/// The refs a store keeps: the newest per lane; of the CI pins the newest few; of the other
/// lanes the newest [`LANE_REFS_KEPT`] and every one in `live` (a lane with a live lease here
/// reads its table at its ship).
fn kept_refs(refs: &[Ref], live: &HashSet<String>) -> Vec<Ref> {
    let mut newest: BTreeMap<&str, Ref> = BTreeMap::new();
    for r in refs.iter().filter(|r| r.key.starts_with("devhub:")) {
        if newest.get(r.key.as_str()).is_none_or(|old| old.time <= r.time) { newest.insert(&r.key, r.clone()); }
    }
    let mut kept: Vec<Ref> = newest.into_values().collect();
    kept.sort_by(|a, b| b.time.cmp(&a.time));
    let (mut ci, mut lanes) = (0, 0);
    kept.retain(|r| {
        let lane = r.key.strip_prefix("devhub:").unwrap_or(&r.key);
        let (n, cap) = if r.key.starts_with(CI_REF) { (&mut ci, CI_REFS_KEPT) } else { (&mut lanes, LANE_REFS_KEPT) };
        *n += 1;
        *n <= cap || live.contains(lane)
    });
    kept
}

/// Objects and uncompressed bytes appended; a repeated pin is a zero-object feed.
pub fn feed(work: &Path, repo: &str, commit: &str, lane: &str) -> Result<(usize, u64), String> {
    let feed = || -> Result<(usize, u64), String> {
        let oid = store::from_hex(commit).ok_or("invalid commit")?;
        let dst = path(work, repo);
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(io)?;
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dst.with_extension("feed-lock")).map_err(io)?;
        lock.lock().map_err(io)?;
        let held = open_counted(&dst, &|c| *c == oid).map_err(io)?;
        let key = format!("devhub:{lane}");
        if held.as_ref().is_some_and(|s| s.index.refs.iter().any(|r| r.key == key && r.commit == oid) && s.index.table(&oid).is_some()) {
            return Ok((0, 0));
        }
        // Retained pins can outgrow argv; rev-list reads the whole revision set from stdin.
        let mut child = crate::git_exec::command()
            .args(["-c", "core.hooksPath=/dev/null", "--git-dir"]).arg(bare(repo)).args(["rev-list", "--objects", "--stdin"]).current_dir(bare(repo))
            .env("GIT_TERMINAL_PROMPT", "0").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(io)?;
        let input = (|| -> Result<(), String> {
            let mut input = child.stdin.take().ok_or("rev-list stdin")?;
            writeln!(input, "{commit}").map_err(io)?;
            if let Some(s) = &held {
                for r in &s.index.refs { writeln!(input, "^{}", store::hex(&r.commit)).map_err(io)?; }
            }
            Ok(())
        })();
        if input.is_err() { let _ = child.kill(); }
        let output = child.wait_with_output().map_err(io)?;
        // (a rev-list that ended early cuts its input short: what it said says why)
        input.map_err(|e| layer(repo, commit, "bare rev-list", format!("writing its input: {e}; {}", git::failure(output.status, &output.stderr))))?;
        if !output.status.success() {
            // the pin itself is not in this box's store: the master's push did not bring it (the
            // master pushes it once more on this word, then asks again)
            if String::from_utf8_lossy(&output.stderr).contains(&format!("bad object {commit}")) {
                return Err(layer(repo, commit, "bare rev-list", format!("{} {commit}", super::NO_COMMIT)));
            }
            return Err(layer(repo, commit, "bare rev-list", git::failure(output.status, &output.stderr)));
        }
        let listing = String::from_utf8_lossy(&output.stdout);
        let mut ids: BTreeSet<Oid> = listing.lines().filter_map(|l| l.split_whitespace().next().and_then(store::from_hex)).collect();
        // Compaction retains kept trees, not their history. A lane can reuse an old blob that
        // rev-list's ancestry exclusion hides; include every missing object in this pin's tree.
        ids.insert(oid);
        let trees = run(repo, &["ls-tree", "-r", "-t", "-z", commit]).map_err(|e| layer(repo, commit, "bare tree", e))?;
        let root = run(repo, &["rev-parse", &format!("{commit}^{{tree}}")])?;
        ids.insert(store::from_hex(root.trim()).ok_or("bad root tree")?);
        for item in trees.split('\0') {
            let Some((meta, _)) = item.split_once('\t') else { continue };
            let mut words = meta.split_whitespace();
            let _mode = words.next();
            if words.next() == Some("commit") { continue; }
            if let Some(oid) = words.next().and_then(store::from_hex) { ids.insert(oid); }
        }
        ids.retain(|o| held.as_ref().is_none_or(|s| !s.contains(o)));
        let mut child = crate::git_exec::command().arg("--git-dir").arg(bare(repo)).args(["cat-file", "--batch"]).current_dir(bare(repo))
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(io)?;
        let result = (|| -> Result<Vec<(Oid, u8, Vec<u8>)>, String> {
            let mut input = child.stdin.take().ok_or("cat-file stdin")?;
            let mut output = std::io::BufReader::new(child.stdout.take().ok_or("cat-file stdout")?);
            let mut objects = Vec::new();
            for oid in ids {
                writeln!(input, "{}", store::hex(&oid)).map_err(io)?;
                input.flush().map_err(io)?;
                let mut head = String::new();
                output.read_line(&mut head).map_err(io)?;
                let words: Vec<_> = head.split_whitespace().collect();
                if words.len() != 3 || store::from_hex(words[0]) != Some(oid) { return Err(format!("cat-file: {}", head.trim())); }
                let kind = match words[1] { "commit" => store::COMMIT, "tree" => store::TREE, "blob" => store::BLOB, "tag" => store::TAG, _ => return Err(format!("cat-file: {}", head.trim())) };
                let size = words[2].parse::<usize>().map_err(|e| e.to_string())?;
                let mut bytes = vec![0; size];
                output.read_exact(&mut bytes).map_err(io)?;
                let mut end = [0];
                output.read_exact(&mut end).map_err(io)?;
                if end != [b'\n'] { return Err("cat-file: missing object terminator".into()); }
                if kind != store::TAG { objects.push((oid, kind, bytes)); }
            }
            Ok(objects)
        })();
        if result.is_err() { let _ = child.kill(); }
        let output = child.wait_with_output().map_err(io)?;
        let objects = result.map_err(|e| layer(repo, commit, "bare objects", format!("{e}; {}", git::failure(output.status, &output.stderr))))?;
        if !output.status.success() { return Err(layer(repo, commit, "bare objects", git::failure(output.status, &output.stderr))); }
        let count = objects.len();
        let bytes = objects.iter().map(|(_, _, b)| b.len() as u64).sum();
        let live = LIVE.get().map(|f| f()).unwrap_or_default();
        store::append(&dst, Append { objects: &objects, reference: Ref { key, commit: oid, time: crate::proto::wall_ms() } }, &|refs| kept_refs(refs, &live))
            .map_err(|e| layer(repo, commit, "RELAXGIT append", e))?;
        Ok((count, bytes))
    };
    feed().map_err(|e| layer(repo, commit, "feed", e))
}

pub fn prepare(work: &Path, lane: &str, lease: &str, pin: &Pin) -> Result<Vec<(String, String)>, String> {
    let root = work.join("dev/src");
    if std::fs::symlink_metadata(&root).is_ok() { return Err(format!("overlay: virtual root {} must not exist on disk", root.display())); }
    let mut mounts = String::new();
    for (repo, sha, prefix) in pin.repos() {
        let at = if prefix.is_empty() { root.clone() } else { root.join(crate::nested::dir(repo)) };
        feed(work, repo, sha, lane)?;
        mounts.push_str(&format!("{}\t{}\t{sha}\n", at.display(), path(work, repo).display()));
    }
    let edits = overlay(work, lane, lease);
    std::fs::create_dir_all(&edits).map_err(|e| layer("makepad", &pin.makepad, "edits directory", e))?;
    let state = work.join("dev/lane").join(lane);
    std::fs::create_dir_all(state.join("run")).map_err(|e| layer("makepad", &pin.makepad, "run directory", e))?;
    std::fs::write(state.join("overlay-pin"), pin.key()).map_err(|e| layer("makepad", &pin.makepad, "pin record", e))?;
    Ok(vec![("MAKEPAD_RUN_HOME".into(), work.join("dev/run-home").to_string_lossy().into_owned()), ("MAKEPAD_RUN_CACHE_MAX".into(), DEV_STORE_MB.to_string()), ("MAKEPAD_RUN_VFS".into(), mounts), ("MAKEPAD_RUN_WORKSPACE".into(), root.to_string_lossy().into_owned()), ("MAKEPAD_RUN_EDITS".into(), edits.to_string_lossy().into_owned())])
}

/// What the dev store may hold (MB). Every overlay lease of a box shares it, across the JIT builds
/// the box keeps and each with its own stores (one build's whole set: up to 6.5 GB), so the single
/// app's default cap evicted what the session before had just stored: the next one started cold
/// (Fab 12 s against 2 s, 11-04). A low disk takes it back first (disk.rs).
pub const DEV_STORE_MB: u64 = super::jitstore::KEEP as u64 * 8 * 1024;

pub fn pin(work: &Path, lane: &str) -> Result<Pin, String> {
    let text = std::fs::read_to_string(work.join("dev/lane").join(lane).join("overlay-pin")).map_err(io)?;
    Pin::parse(text.trim()).ok_or_else(|| format!("overlay {lane}: no pinned commits"))
}

/// Release a stopped lane's retained refs in each repository's dev store; other readers and pins
/// keep their objects.
pub fn forget(work: &Path, lane: &str) -> Result<(), String> {
    let key = format!("devhub:{lane}");
    let stores = std::fs::read_dir(work.join("dev/git")).into_iter().flatten().flatten();
    for repo in stores.filter_map(|e| e.file_name().to_str()?.strip_suffix(".relaxgit").filter(|r| git::valid_repo(r)).map(str::to_string)) {
        let dst = path(work, &repo);
        let _lock = super::jitstore::lock(&dst.with_extension("feed-lock"))?;
        let Some(held) = open_counted(&dst, &|_| false).map_err(io)? else { continue };
        let Some(reference) = held.index.refs.iter().find(|r| r.key == key).cloned() else { continue };
        store::append(&dst, Append { objects: &[], reference }, &|refs| refs.iter().filter(|r| r.key != key).cloned().collect())?;
    }
    Ok(())
}

/// The store with the pin's path table only.
fn open(work: &Path, repo: &str, sha: &str) -> Result<Store, String> {
    let oid = store::from_hex(sha).ok_or("invalid pin")?;
    open_counted(&path(work, repo), &|c| *c == oid).map_err(|e| layer(repo, sha, "RELAXGIT open", e))?.ok_or_else(|| layer(repo, sha, "RELAXGIT open", "missing store"))
}

/// The pin's blob of each tree path `keep` takes (a nested repository's store's under `apps/<name>/`).
/// A ship-done asks about the paths it ships: building every path's entry (~20k) each time was
/// most of a live edit's ship-done on the node (11-04).
pub fn bases(work: &Path, pin: &Pin, keep: &dyn Fn(&str) -> bool) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (repo, sha, prefix) in pin.repos() {
        let s = open(work, repo, sha)?;
        let oid = store::from_hex(sha).ok_or("invalid pin")?;
        let table = s.index.table(&oid).ok_or_else(|| layer(repo, sha, "RELAXGIT table", "commit not kept"))?;
        let mut full = prefix.clone();
        for e in table.iter().filter(|e| e.mode != 0o160000 && !(repo == "makepad" && pin.repo_of(&e.path).0 != "makepad")) {
            full.truncate(prefix.len());
            full.push_str(&e.path);
            if keep(&full) { out.insert(full.clone(), store::hex(&e.oid)); }
        }
    }
    Ok(out)
}

/// Keep each store index open while reading the pin's manifests.
pub struct Blobs {
    pin: Pin,
    /// each repository of the pin's store, by name
    stores: BTreeMap<String, Store>,
}

impl Blobs {
    pub fn open(work: &Path, pin: &Pin) -> Result<Self, String> {
        let stores = pin.repos().into_iter().map(|(repo, sha, _)| -> Result<(String, Store), String> { Ok((repo.to_string(), open(work, repo, sha)?)) }).collect::<Result<BTreeMap<_, _>, String>>()?;
        Ok(Self { pin: pin.clone(), stores })
    }

    pub fn read(&self, path: &str, oid: &str) -> Result<Vec<u8>, String> {
        let (repo, sha, _) = self.pin.repo_of(path);
        let s = self.stores.get(repo).ok_or_else(|| format!("missing {repo} store"))?;
        let oid = store::from_hex(oid).ok_or("invalid blob")?;
        s.object(&oid).map_err(io)?.map(|(_, b)| b).ok_or_else(|| layer(repo, sha, "RELAXGIT blob", path))
    }
}

fn files(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), String> {
    for e in std::fs::read_dir(dir.join(rel)).map_err(io)? {
        let e = e.map_err(io)?;
        let name = e.file_name().to_string_lossy().into_owned();
        let p = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if e.file_type().map_err(io)?.is_dir() { files(dir, &p, out)?; } else { out.push(p); }
    }
    Ok(())
}

fn prune_empty(dir: &Path) -> Result<bool, String> {
    let mut empty = true;
    for e in std::fs::read_dir(dir).map_err(io)? {
        let e = e.map_err(io)?;
        if e.file_type().map_err(io)?.is_dir() && prune_empty(&e.path())? { std::fs::remove_dir(e.path()).map_err(io)?; }
        else { empty = false; }
    }
    Ok(empty)
}

/// A ship is the entire uncommitted set, including whiteouts and each path's pinned base blob.
/// `dir`: the lease's overlay. Returns which source that makes ([`super::ship::source_id`]).
pub fn finish(work: &Path, dir: &Path, pin: &Pin, staged: &Path, shipped: &[String], deleted: &[String], sent_bases: &makepad_strict_json::Value) -> Result<String, String> {
    let dir = dir.to_path_buf();
    let asked: HashSet<&str> = shipped.iter().chain(deleted).map(String::as_str).collect();
    let base = bases(work, pin, &|p| asked.contains(p))?;
    let mut records = String::new();
    for p in shipped.iter().chain(deleted) {
        let want = base.get(p).map_or("new", String::as_str);
        let got = sent_bases.get(p).and_then(|v| v.as_str()).ok_or_else(|| format!("overlay: {p}: missing base blob"))?;
        if got != want { return Err(format!("overlay: {p}: base {got} differs from pin {want}")); }
        records.push_str(&format!("{p}\t{want}\n"));
    }
    let keep: HashSet<_> = shipped.iter().map(String::as_str).collect();
    if let Some(p) = deleted.iter().find(|p| keep.contains(p.as_str())) { return Err(format!("overlay: {p} is both shipped and deleted")); }
    for p in shipped { if !std::fs::symlink_metadata(staged.join(p)).is_ok_and(|m| !m.is_dir()) { return Err(format!("overlay: {p} was not shipped")); } }
    let mut held = Vec::new();
    files(&dir, "", &mut held)?;
    for p in held.iter().filter(|p| !keep.contains(p.as_str()) && !matches!(p.as_str(), ".makepad-run-edits" | ".makepad-run-deleted")) {
        std::fs::remove_file(dir.join(p)).map_err(io)?;
    }
    prune_empty(&dir)?;
    for p in shipped {
        let (src, dst) = (staged.join(p), dir.join(p));
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(io)?;
        let tmp = dst.with_file_name(format!("{}.devhub-part", dst.file_name().unwrap().to_string_lossy()));
        let meta = std::fs::symlink_metadata(&src).map_err(io)?;
        if meta.is_file() && std::fs::symlink_metadata(&dst).is_ok_and(|m| m.is_file() && m.len() == meta.len() && m.modified().ok() == meta.modified().ok() && m.permissions() == meta.permissions()) { continue; }
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&src).map_err(io)?;
            if std::fs::read_link(&dst).is_ok_and(|held| held == target) { continue; }
            let line = format!("{} {}", makepad_network::digest::base64_encode(p.as_bytes()), makepad_network::digest::base64_encode(target.to_string_lossy().as_bytes()));
            super::ship::link(&dir, &line)?;
        } else {
            std::fs::copy(&src, &tmp).map_err(io)?;
            std::fs::OpenOptions::new().write(true).open(&tmp).and_then(|f| f.set_modified(meta.modified()?)).map_err(io)?;
            std::fs::rename(&tmp, &dst).map_err(io)?;
        }
    }
    // the edits ledger last: its rename commits the ship (a live JIT that sees it moved takes
    // every file and the deleted list as whole, without waiting for them to settle)
    for (name, text) in [(".makepad-run-deleted", deleted.iter().map(|p| format!("{p}\n")).collect()), (".makepad-run-edits", records)] {
        let tmp = dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, dir.join(name))).map_err(io)?;
    }
    if staged.is_dir() {
        let mut old = Vec::new();
        files(staged, "", &mut old)?;
        for p in old.iter().filter(|p| !keep.contains(p.as_str())) { std::fs::remove_file(staged.join(p)).map_err(io)?; }
        prune_empty(staged)?;
    }
    Ok(super::ship::source_in(&dir, &pin.key(), shipped, deleted))
}

fn hash(bytes: &[u8]) -> String { makepad_network::digest::sha256_hash(bytes)[..8].iter().map(|b| format!("{b:02x}")).collect() }
fn whiteouts(dir: &Path) -> BTreeSet<String> { std::fs::read_to_string(dir.join(".makepad-run-deleted")).unwrap_or_default().lines().map(str::to_string).collect() }
fn content(path: &Path) -> Result<Vec<u8>, String> {
    if std::fs::symlink_metadata(path).map_err(io)?.file_type().is_symlink() { Ok(std::fs::read_link(path).map_err(io)?.to_string_lossy().as_bytes().to_vec()) } else { std::fs::read(path).map_err(io) }
}

/// The overlays of the lane's leases on this box (lease, folder).
fn lease_overlays(work: &Path, lane: &str) -> Vec<(String, PathBuf)> {
    let mut out: Vec<_> = std::fs::read_dir(lane_overlays(work, lane)).into_iter().flatten().flatten()
        .filter(|e| e.path().join(".makepad-run-edits").is_file())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect();
    out.sort();
    out
}

/// Where `rel` comes from: in each lease's overlay that says something of it, and at the pin.
pub fn why(work: &Path, lane: &str, rel: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (lease, dir) in lease_overlays(work, lane) {
        let p = dir.join(rel);
        if whiteouts(&dir).contains(rel) { out.push(format!("{rel}: {lease}: whiteout; absent")); }
        else if std::fs::symlink_metadata(&p).is_ok_and(|m| !m.is_dir()) { out.push(format!("{rel}: {lease}: overlay file sha256 {}", hash(&content(&p)?))); }
    }
    out.push(why_at_pin(work, lane, rel)?);
    Ok(out)
}

fn why_at_pin(work: &Path, lane: &str, rel: &str) -> Result<String, String> {
    let pin = pin(work, lane)?;
    let (repo, sha, path) = pin.repo_of(rel);
    let s = open(work, repo, sha)?;
    let oid = store::from_hex(sha).ok_or("invalid pin")?;
    let table = s.index.table(&oid).ok_or_else(|| layer(repo, sha, "RELAXGIT table", "commit not kept"))?;
    let Some(e) = table.iter().find(|e| e.path == path && e.mode != 0o160000) else { return Ok(format!("{rel}: absent")) };
    let (_, bytes) = s.object(&e.oid).map_err(io)?.ok_or_else(|| layer(repo, sha, "RELAXGIT blob", store::hex(&e.oid)))?;
    Ok(format!("{rel}: lane commit at {sha} ({repo}) sha256 {}", hash(&bytes)))
}

pub fn status(work: &Path, lane: &str) -> Result<Vec<String>, String> {
    let pin = pin(work, lane)?;
    let mut out = vec![format!("{lane}: {}", pin.repos().iter().map(|(repo, sha, _)| format!("{repo} {sha}")).collect::<Vec<_>>().join(", "))];
    let held = super::jitstore::Store::new(work).snapshot().ids;
    let (selected, _) = super::loaders::overlay_id(work, &pin)?;
    out.push(format!("dev JIT keys: {}", if held.is_empty() { "none".into() } else { held.join(", ") }));
    out.push(format!("pin selects dev JIT {selected} ({})", if held.contains(&selected) { "held" } else { "build on miss" }));
    for (lease, dir) in lease_overlays(work, lane) {
        out.push(format!("lease {lease}"));
        status_of(&pin, &dir, &mut out)?;
    }
    Ok(out)
}

fn status_of(pin: &Pin, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let mut paths = Vec::new();
    files(dir, "", &mut paths)?;
    paths.retain(|p| !p.starts_with(".makepad-run-"));
    paths.sort();
    for p in &paths { out.push(format!("overlay {p} sha256 {}", hash(&content(&dir.join(p))?))); }
    let deleted = whiteouts(dir);
    for p in &deleted { out.push(format!("whiteout {p}")); }
    for (repo, sha, prefix) in pin.repos() {
        let work_tip = run(repo, &["rev-parse", "--verify", git::SYNCED_REF])?;
        out.push(format!("diff vs work {repo} {}", work_tip.trim()));
        let diff = run(repo, &["diff", "--name-status", "--no-renames", "-z", work_tip.trim(), sha])?;
        let mut effective: BTreeMap<String, String> = BTreeMap::new();
        let words: Vec<_> = diff.split('\0').filter(|s| !s.is_empty()).collect();
        for pair in words.chunks_exact(2) {
            let p = format!("{prefix}{}", pair[1]);
            if pin.repo_of(&p).0 == repo { effective.insert(p, pair[0].into()); }
        }
        let work_blobs = run(repo, &["ls-tree", "-r", "-z", work_tip.trim()])?;
        let index: BTreeMap<_, _> = work_blobs.split('\0').filter_map(|s| {
            let (meta, p) = s.split_once('\t')?;
            let mut words = meta.split_whitespace();
            let mode = u32::from_str_radix(words.next()?, 8).ok()?;
            Some((format!("{prefix}{p}"), (mode, words.nth(1)?.to_string())))
        }).collect();
        for p in paths.iter().chain(&deleted).filter(|p| pin.repo_of(p).0 == repo) {
            let old = index.get(p);
            let code = if deleted.contains(p) { old.map(|_| "D") } else {
                let bytes = content(&dir.join(p))?;
                let mut blob = format!("blob {}\0", bytes.len()).into_bytes();
                blob.extend_from_slice(&bytes);
                let meta = std::fs::symlink_metadata(dir.join(p)).map_err(io)?;
                #[cfg(unix)]
                let executable = { use std::os::unix::fs::PermissionsExt; meta.permissions().mode() & 0o111 != 0 };
                #[cfg(not(unix))]
                let executable = old.is_some_and(|(mode, _)| *mode == 0o100755);
                let mode = if meta.file_type().is_symlink() { 0o120000 } else if executable { 0o100755 } else { 0o100644 };
                if old == Some(&(mode, store::hex(&store::sha1(&blob)))) { None }
                else { Some(match old { None => "A", Some((m, _)) if *m & 0o170000 != mode & 0o170000 => "T", _ => "M" }) }
            };
            effective.remove(p);
            if let Some(code) = code { effective.insert(p.clone(), code.into()); }
        }
        for (p, code) in effective { out.push(format!("{code}\t{p}")); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_keeps_the_newest_lanes_and_the_live_ones() {
        let r = |key: &str, time: u64| Ref { key: format!("devhub:{key}"), commit: [time as u8; 20], time };
        let mut refs: Vec<Ref> = (0..30).map(|t| r(&format!("lane{t}"), t)).collect();
        refs.extend((0..12).map(|t| r(&format!("ci-{t}"), 100 + t)));
        refs.push(r("lane29", 5));
        refs.push(Ref { key: "https://x#main".into(), commit: [9; 20], time: 1 });
        let live: HashSet<String> = ["lane0".to_string()].into();
        let kept = kept_refs(&refs, &live);
        let lanes: BTreeSet<&str> = kept.iter().filter(|x| !x.key.starts_with(CI_REF)).map(|x| x.key.as_str()).collect();
        // the 12 newest lanes, the live one however old, the newest ref per lane only
        let want: BTreeSet<String> = (18..30).map(|t| format!("devhub:lane{t}")).chain(["devhub:lane0".to_string()]).collect();
        assert_eq!(lanes, want.iter().map(String::as_str).collect());
        assert_eq!(kept.iter().find(|x| x.key == "devhub:lane29").unwrap().time, 29);
        assert_eq!(kept.iter().filter(|x| x.key.starts_with(CI_REF)).count(), CI_REFS_KEPT);
        assert!(kept.iter().filter(|x| x.key.starts_with(CI_REF)).all(|x| x.time >= 104), "the newest CI pins");
    }
}
