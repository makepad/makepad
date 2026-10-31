//! A repository at a ref into its store (gitstore.rs): the one fetch path of `makepad run <git url>`
//! (the runner's fetch.rs: specs, mounts, `--cached`) and of git dependencies (resolve.rs). Smart
//! HTTP (smart_http.rs) on the embedder's transport ([`set_http`]: relax_compiler::registry_net
//! installs relax-light-http), with local reads from Cargo's git cache and Relax's stores. Nothing is
//! checked out: the store's path table of a commit is what its files are read from (vfs.rs).
//!
//! Cargo offline settings and MAKEPAD_RUN_OFFLINE=1 forbid every request.
use super::{build_info_refs_request, build_upload_pack_request, extract_pack_from_response, Advertisement, ObjectId, ObjectKind, Request, Response};
use crate::gitstore::{self as store, Oid};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// One HTTP exchange of the git client (the request's method, URL, headers and body; the
/// response's status and body), within `timeout`.
pub type Http = fn(req: &Request, timeout: Duration) -> Result<Response, String>;

static HTTP: OnceLock<Http> = OnceLock::new();

/// Installs the network access git fetches go through (once per process).
pub fn set_http(f: Http) {
    let _ = HTTP.set(f);
}

fn offline() -> Result<bool, String> {
    super::cargo::offline(&std::env::current_dir().map_err(|e| e.to_string())?)
}

fn http(req: &Request, timeout: Duration) -> Result<Response, String> {
    if offline()? {
        return Err("git network access is disabled (offline)".into());
    }
    let f = HTTP.get().ok_or("this process has no network access for git")?;
    f(req, timeout)
}

/// "host" of a URL.
pub fn host(url: &str) -> &str {
    url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or(url)
}

/// A repository at a ref: `https://host/path[#ref][@commit]` (no ref: the remote's HEAD). A
/// pinned commit (40 hex) is what the ref is taken to name: the store or a pack already in
/// cache/incoming answers it without asking the remote.
#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub url: String,
    pub reference: Option<String>,
    pub commit: Option<String>,
}

/// The URL forms the client reaches: https, and plain http on the loopback (tests, local mirrors).
pub fn check_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1") && !url.starts_with("http://localhost") {
        return Err(format!("not an https repository URL: {url}"));
    }
    Ok(())
}

impl Repo {
    pub fn parse(s: &str) -> Result<Repo, String> {
        let (s, commit) = match s.rsplit_once('@') {
            Some((rest, c)) if c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit()) => (rest, Some(c.to_ascii_lowercase())),
            _ => (s, None),
        };
        let (url, reference) = match s.split_once('#') {
            Some((u, r)) => (u, Some(r.to_string()).filter(|r| !r.is_empty())),
            None => (s, None),
        };
        check_url(url).map_err(|_| format!("not an https repository URL: {s}"))?;
        Ok(Repo { url: url.trim_end_matches('/').to_string(), reference, commit })
    }
    /// The last path segment without `.git` (folder names).
    pub fn name(&self) -> String {
        let last = self.url.rsplit('/').next().unwrap_or("repo");
        let name: String = last.trim_end_matches(".git").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        if name.is_empty() { "repo".into() } else { name }
    }
    /// The store's key of the ref (the snapshot policy keeps the two newest fetches per key).
    pub fn ref_key(&self) -> String {
        self.reference.clone().unwrap_or_else(|| "HEAD".into())
    }
}

/// How long asking a remote which commit a ref names may take before the stores answer instead
/// (MAKEPAD_RUN_FETCH_TIMEOUT_S overrides it).
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);

fn head_timeout() -> Duration {
    std::env::var("MAKEPAD_RUN_FETCH_TIMEOUT_S").ok().and_then(|v| v.parse().ok()).map(Duration::from_secs).unwrap_or(HEAD_TIMEOUT)
}

/// The remote's ref advertisement.
pub fn advertisement(url: &str) -> Result<Advertisement, String> {
    // short: an unreachable or blackholed host must not hold a launch up (the stores answer then)
    let resp = http(&build_info_refs_request(url), head_timeout())?;
    super::smart_http::parse_advertisement(&resp).map_err(|e| format!("{url}: {e}"))
}

/// Keeps per ref key the two newest fetches (the snapshot policy); everything else goes from the
/// index (its bytes at the next compaction).
pub fn keep_two(refs: &[store::Ref]) -> Vec<store::Ref> {
    let mut by_key: std::collections::BTreeMap<&str, Vec<&store::Ref>> = Default::default();
    for r in refs {
        by_key.entry(r.key.as_str()).or_default().push(r);
    }
    let mut out = Vec::new();
    for (_, mut v) in by_key {
        v.sort_by(|a, b| b.time.cmp(&a.time));
        out.extend(v.into_iter().take(2).cloned());
    }
    out
}

pub(super) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The newest kept fetch of `key` (any ref's newest when that key was never fetched).
pub fn newest(s: &store::Store, key: &str) -> Option<Oid> {
    s.index.refs.iter().filter(|r| r.key == key).max_by_key(|r| r.time).or_else(|| s.index.refs.iter().max_by_key(|r| r.time)).map(|r| r.commit)
}

/// Records `commit` (its path table already in the store) under `key`, unless the ref's newest
/// fetch already is that commit.
fn record(path: &Path, existing: &store::Store, key: &str, commit: Oid) -> Result<(), String> {
    if existing.index.refs.iter().filter(|r| r.key == key).max_by_key(|r| r.time).is_some_and(|r| r.commit == commit) {
        // nothing to write (a --cached start checks every time; rewriting the index each start
        // would only grow the file)
        return Ok(());
    }
    store::append(path, store::Append { objects: &[], reference: store::Ref { key: key.to_string(), commit, time: now() } }, &keep_two)?;
    Ok(())
}

/// One repository at its ref into its store: fetched when the store does not hold that commit
/// (new objects only where the server negotiates); returns the commit. Offline: the newest kept
/// commit of the ref.
pub fn fetch_repo(cache: &Path, repo: &Repo) -> Result<Oid, String> {
    let path = store::store_path(cache, &repo.url);
    let existing = store::Store::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let key = repo.ref_key();
    // a pinned commit already in the store, or its pack already downloaded: no remote request
    if let Some(pinned) = repo.commit.as_deref().and_then(store::from_hex) {
        if let Some(s) = &existing {
            if s.index.table(&pinned).is_some() {
                record(&path, s, &key, pinned)?;
                return Ok(pinned);
            }
        }
        let home = super::cargo::home();
        if super::cargo::import(&home, &repo.url, pinned, &path, &key)? { return Ok(pinned); }
        let incoming = cache.join("incoming").join(format!("{}.pack", store::hex(&pinned)));
        if let Ok(body) = std::fs::read(&incoming) {
            let _ = std::fs::remove_file(&incoming);
            let pack = extract_pack_from_response(&Response { status_code: 200, body }).map_err(|e| e.to_string())?.ok_or("the downloaded pack is empty")?;
            append_pack(&path, existing.as_ref(), &pack, store::Ref { key, commit: pinned, time: now() }, &repo.url)?;
            return Ok(pinned);
        }
        if offline()? {
            return Err(super::cargo::missing(&home, &repo.url, &store::hex(&pinned)));
        }
    }
    match fetch_remote(cache, repo, existing.as_ref(), &path, key.clone()) {
        Ok(c) => Ok(c),
        // offline, unreachable or failing: what the stores have runs (never a failed launch while
        // a usable commit is kept), and the run says so
        Err(e) => {
            let s = existing.as_ref().ok_or_else(|| e.clone())?;
            let c = newest(s, &key).ok_or(e.clone())?;
            let wanted = repo.commit.as_deref().map(|w| format!(" (wanted {})", &w[..w.len().min(12)])).unwrap_or_default();
            crate::progress::note(&format!("{} unreachable ({e}): running {} from the store{wanted}", host(&repo.url), &store::hex(&c)[..12]));
            Ok(c)
        }
    }
}

/// The remote's answer for `repo`: its commit for the ref (or the pin), fetched into the store
/// unless the store has it already.
fn fetch_remote(cache: &Path, repo: &Repo, existing: Option<&store::Store>, path: &Path, key: String) -> Result<Oid, String> {
    let adv = advertisement(&repo.url)?;
    let mut h = adv.select(repo.reference.as_deref()).map_err(|e| format!("{}: {}", repo.url, e))?;
    // a pinned commit is what is wanted, whatever the ref names now
    if let Some(pinned) = repo.commit.as_deref().and_then(|c| ObjectId::from_hex(c).ok()) {
        h.oid = pinned;
    }
    let commit = h.oid.0;
    if let Some(s) = existing {
        if s.index.table(&commit).is_some() {
            // here under another ref (or older): record the ref so the policy keeps it
            record(path, s, &key, commit)?;
            return Ok(commit);
        }
    }
    // a pack the entry script downloaded while Relax was being built (incoming/<commit>.pack:
    // the upload-pack answer for this commit, renamed into place when complete)
    let incoming = cache.join("incoming").join(format!("{}.pack", h.oid.to_hex()));
    if let Ok(body) = std::fs::read(&incoming) {
        let _ = std::fs::remove_file(&incoming);
        let pack = extract_pack_from_response(&Response { status_code: 200, body }).map_err(|e| e.to_string())?.ok_or("the server sent no pack")?;
        append_pack(path, existing, &pack, store::Ref { key, commit, time: now() }, &repo.url)?;
        return Ok(commit);
    }
    fetch_commit(path, existing, &repo.url, &adv, commit, &key)?;
    Ok(commit)
}

/// `commit` fetched into the store under `key`: a depth-1 fetch of that commit (with the store's
/// commits as haves, so the service may send a thin pack); a remote that refuses a commit that is
/// no ref's tip (git's default without uploadpack.allowReachableSHA1InWant) is asked for the
/// whole history of its branches and tags instead, which must hold it.
fn fetch_commit(path: &Path, existing: Option<&store::Store>, url: &str, adv: &Advertisement, commit: Oid, key: &str) -> Result<(), String> {
    let caps = &adv.capabilities;
    let can_deepen = caps.iter().any(|c| c == "shallow");
    // the store's kept commits as haves; the ones held without their parents (a depth-1 fetch)
    // are named shallow, else the service would take their history as had and leave out an
    // ancestor that is wanted now (a tag behind a fetched branch)
    let mut haves: Vec<ObjectId> = Vec::new();
    let mut shallow: Vec<ObjectId> = Vec::new();
    if let Some(s) = existing {
        for r in &s.index.refs {
            let o = ObjectId(r.commit);
            if haves.contains(&o) {
                continue;
            }
            let is_shallow = !has_parents(s, &r.commit);
            if is_shallow && !can_deepen {
                continue;
            }
            haves.push(o);
            if is_shallow {
                shallow.push(o);
            }
        }
    }
    let req = build_upload_pack_request(url, &[ObjectId(commit)], caps, &haves, &shallow, can_deepen.then_some(1)).map_err(|e| e.to_string())?;
    eprintln!("makepad: fetching {} ({})", url, &store::hex(&commit)[..12]);
    let t0 = std::time::Instant::now();
    let pack = http(&req, Duration::from_secs(600)).and_then(|r| extract_pack_from_response(&r).map_err(|e| e.to_string())?.ok_or_else(|| "the server sent no pack".to_string()));
    let pack = match pack {
        Ok(p) => p,
        Err(first) => {
            let tips: Vec<ObjectId> = {
                let mut t: Vec<ObjectId> = adv.refs.iter().filter(|(n, _)| (n.starts_with("refs/heads/") || n.starts_with("refs/tags/")) && !n.ends_with("^{}")).map(|(_, o)| *o).collect();
                t.sort();
                t.dedup();
                t
            };
            if tips.is_empty() || tips.iter().any(|t| t.0 == commit) {
                return Err(format!("{url}: {first}"));
            }
            eprintln!("makepad: {url}: {} is no branch or tag tip ({first}): fetching the history", &store::hex(&commit)[..12]);
            // no haves: the store holds shallow commits, whose parents the service would take as had
            let req = build_upload_pack_request(url, &tips, caps, &[], &[], None).map_err(|e| e.to_string())?;
            let r = http(&req, Duration::from_secs(1800))?;
            extract_pack_from_response(&r).map_err(|e| format!("{url}: {e}"))?.ok_or_else(|| format!("{url}: the server sent no pack"))?
        }
    };
    let t_dl = t0.elapsed();
    let (n, t_unpack, t_store) = append_pack(path, existing, &pack, store::Ref { key: key.to_string(), commit, time: now() }, url)?;
    if std::env::var_os("MAKEPAD_RUN_FETCH_TIMES").is_some() {
        eprintln!("makepad: {}: download {:.2} s, unpack {:.2} s ({} objects), store {:.2} s", url, t_dl.as_secs_f64(), t_unpack, n, t_store);
    }
    Ok(())
}

/// Does the store hold every parent of `commit` (a root commit: yes)?
fn has_parents(s: &store::Store, commit: &Oid) -> bool {
    let Ok(Some((_, data))) = s.object(commit) else { return false };
    let text = String::from_utf8_lossy(&data);
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        if let Some(p) = line.strip_prefix("parent ").and_then(store::from_hex) {
            if !s.contains(&p) {
                return false;
            }
        }
    }
    true
}

/// A pack's objects (deltas resolved against the pack and the store) appended to the store with
/// `reference`: (objects, unpack s, store s).
fn append_pack(path: &Path, existing: Option<&store::Store>, pack: &[u8], reference: store::Ref, url: &str) -> Result<(usize, f64, f64), String> {
    let objects = unpack(existing, pack, url)?;
    let t_unpack = objects.1;
    let objects = objects.0;
    if !objects.iter().any(|(o, k, _)| *o == reference.commit && *k == store::COMMIT) && !existing.is_some_and(|s| s.contains(&reference.commit)) {
        return Err(format!("{url}: the pack does not hold commit {}", store::hex(&reference.commit)));
    }
    let t2 = std::time::Instant::now();
    store::append(path, store::Append { objects: &objects, reference }, &keep_two)?;
    Ok((objects.len(), t_unpack, t2.elapsed().as_secs_f64()))
}

/// A pack's objects in the store's form (its kind numbers are the pack's: store::COMMIT..TAG =
/// ObjectKind), and the unpack time.
fn unpack(existing: Option<&store::Store>, pack: &[u8], url: &str) -> Result<(Vec<(Oid, u8, Vec<u8>)>, f64), String> {
    let mut external = |oid: &ObjectId| -> Option<super::Object> {
        let (kind, data) = existing?.object(&oid.0).ok().flatten()?;
        Some(super::Object { kind: ObjectKind::from_type_num(kind).ok()?, data })
    };
    let t1 = std::time::Instant::now();
    let objects = super::unpack_pack(pack, &mut external).map_err(|e| format!("{url}: {e}"))?;
    Ok((objects.into_iter().map(|(oid, o)| (oid.0, o.kind as u8, o.data)).collect(), t1.elapsed().as_secs_f64()))
}

/// What a git dependency names (cargo's GitReference): `branch`, `tag`, `rev`, or none of them
/// (the remote's default branch, HEAD).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitRef {
    DefaultBranch,
    Branch(String),
    Tag(String),
    Rev(String),
}

impl GitRef {
    /// cargo's source-id query (`branch=x`, `tag=x`, `rev=x`; None for the default branch), as
    /// Cargo.lock and package ids spell it.
    pub fn query(&self) -> Option<String> {
        match self {
            GitRef::DefaultBranch => None,
            GitRef::Branch(b) => Some(format!("branch={b}")),
            GitRef::Tag(t) => Some(format!("tag={t}")),
            GitRef::Rev(r) => Some(format!("rev={r}")),
        }
    }

    /// The store's ref key (the runner's spelling for the default branch and for refs).
    fn key(&self) -> String {
        match self {
            GitRef::DefaultBranch => "HEAD".into(),
            GitRef::Branch(b) => format!("refs/heads/{b}"),
            GitRef::Tag(t) => format!("refs/tags/{t}"),
            GitRef::Rev(r) if r.starts_with("refs/") => r.clone(),
            GitRef::Rev(r) => format!("rev={r}"),
        }
    }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A git source (`url` at `r`) in its store; returns the commit. `locked`: the commit a lockfile
/// pins it to -- that commit from local stores/Cargo's db, else fetched, else an error.
/// Unlocked: the commit the remote's ref names now, fetched unless the store holds it; with the
/// remote unreachable or offline the store's newest fetch of the ref, said so.
pub fn fetch_source(cache: &Path, url: &str, r: &GitRef, locked: Option<Oid>, offline: bool) -> Result<Oid, String> {
    check_url(url)?;
    let path = store::store_path(cache, url);
    let existing = store::Store::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let key = r.key();
    let home = super::cargo::home();
    let offline = offline || self::offline()?;
    let locked = locked.or_else(|| match r { GitRef::Rev(rev) => store::from_hex(&rev.to_ascii_lowercase()), _ => None });
    if let Some(c) = locked {
        if let Some(s) = &existing {
            if s.index.table(&c).is_some() {
                if !s.index.refs.iter().any(|x| x.key == key && x.commit == c) {
                    record(&path, s, &key, c)?;
                }
                return Ok(c);
            }
        }
        if super::cargo::import(&home, url, c, &path, &key)? { return Ok(c); }
        if offline { return Err(super::cargo::missing(&home, url, &store::hex(&c))); }
        let adv = advertisement(url).map_err(|e| format!("{url}: the locked commit {} is not in the store and the remote is unreachable: {e}", store::hex(&c)))?;
        fetch_commit(&path, existing.as_ref(), url, &adv, c, &key)?;
        return Ok(c);
    }
    let adv = match if offline { Err(super::cargo::missing(&home, url, &key)) } else { advertisement(url) } {
        Ok(a) => a,
        Err(e) => {
            // a commit id (or an abbreviation of one) the store holds needs no remote; a ref is the
            // store's newest fetch of it
            let kept = existing.as_ref().and_then(|s| match r {
                GitRef::Rev(rev) if is_hex(rev) && rev.len() >= 4 => {
                    let want = rev.to_ascii_lowercase();
                    let found: Vec<Oid> = s.index.tables.iter().map(|(c, _)| *c).filter(|c| store::hex(c).starts_with(&want)).collect();
                    (found.len() == 1).then(|| found[0])
                }
                _ => s.index.refs.iter().filter(|x| x.key == key).max_by_key(|x| x.time).map(|x| x.commit),
            });
            let c = kept.ok_or_else(|| format!("{e} (and the store keeps no fetch of {key})"))?;
            crate::progress::note(&format!("{} unreachable ({e}): {} {} from the store", host(url), url, &store::hex(&c)[..12]));
            return Ok(c);
        }
    };
    let commit = match r {
        GitRef::DefaultBranch => adv.select(None),
        GitRef::Branch(b) => adv.select(Some(&format!("refs/heads/{b}"))),
        GitRef::Tag(t) => adv.select(Some(&format!("refs/tags/{t}"))),
        GitRef::Rev(rev) if rev.starts_with("refs/") => adv.select(Some(rev)),
        GitRef::Rev(rev) if rev.len() == 40 && is_hex(rev) => return fetch_rev(&path, existing.as_ref(), url, &adv, &store::from_hex(&rev.to_ascii_lowercase()).unwrap(), &key),
        GitRef::Rev(rev) => return fetch_short_rev(&path, existing.as_ref(), url, &adv, rev, &key),
    }
    .map_err(|e| format!("{url}: {e}"))?
    .oid
    .0;
    fetch_rev(&path, existing.as_ref(), url, &adv, &commit, &key)
}

/// `commit` into the store under `key` (no request when the store holds it).
fn fetch_rev(path: &Path, existing: Option<&store::Store>, url: &str, adv: &Advertisement, commit: &Oid, key: &str) -> Result<Oid, String> {
    if let Some(s) = existing {
        if s.index.table(commit).is_some() {
            record(path, s, key, *commit)?;
            return Ok(*commit);
        }
    }
    fetch_commit(path, existing, url, adv, *commit, key)?;
    Ok(*commit)
}

/// A `rev` that is neither a full commit id nor a `refs/` name: a branch or tag name, an
/// abbreviated id of a ref's tip, else an abbreviated id searched in the history of every branch
/// and tag (cargo fetches those for such a rev and resolves it there).
fn fetch_short_rev(path: &Path, existing: Option<&store::Store>, url: &str, adv: &Advertisement, rev: &str, key: &str) -> Result<Oid, String> {
    for name in [format!("refs/heads/{rev}"), format!("refs/tags/{rev}")] {
        if let Some(c) = adv.commit(&name) {
            return fetch_rev(path, existing, url, adv, &c.0, key);
        }
    }
    if !is_hex(rev) || rev.len() < 4 {
        return Err(format!("{url}: rev `{rev}` names no branch, tag or commit"));
    }
    let want = rev.to_ascii_lowercase();
    let mut tips: Vec<ObjectId> = adv.refs.values().copied().collect();
    tips.sort();
    tips.dedup();
    let at_tip: Vec<&ObjectId> = tips.iter().filter(|o| o.to_hex().starts_with(&want)).collect();
    if let [one] = at_tip.as_slice() {
        return fetch_rev(path, existing, url, adv, &one.0, key);
    }
    // the store may hold it from an earlier fetch of the history
    if let Some(s) = existing {
        let found: Vec<Oid> = s.index.tables.iter().map(|(c, _)| *c).filter(|c| store::hex(c).starts_with(&want)).collect();
        if let [one] = found.as_slice() {
            record(path, s, key, *one)?;
            return Ok(*one);
        }
    }
    let heads: Vec<ObjectId> = {
        let mut t: Vec<ObjectId> = adv.refs.iter().filter(|(n, _)| (n.starts_with("refs/heads/") || n.starts_with("refs/tags/") || n.as_str() == "HEAD") && !n.ends_with("^{}")).map(|(_, o)| *o).collect();
        t.sort();
        t.dedup();
        t
    };
    if heads.is_empty() {
        return Err(format!("{url}: no branches or tags to look for rev `{rev}` in"));
    }
    eprintln!("makepad: fetching {url} (the history, for rev {rev})");
    let req = build_upload_pack_request(url, &heads, &adv.capabilities, &[], &[], None).map_err(|e| e.to_string())?;
    let r = http(&req, Duration::from_secs(1800))?;
    let pack = extract_pack_from_response(&r).map_err(|e| format!("{url}: {e}"))?.ok_or_else(|| format!("{url}: the server sent no pack"))?;
    let (objects, _) = unpack(existing, &pack, url)?;
    let found: Vec<Oid> = objects.iter().filter(|(o, k, _)| *k == store::COMMIT && store::hex(o).starts_with(&want)).map(|(o, _, _)| *o).collect();
    let commit = match found.as_slice() {
        [one] => *one,
        [] => return Err(format!("{url}: rev `{rev}` is no commit of any branch or tag")),
        _ => return Err(format!("{url}: rev `{rev}` is ambiguous ({} commits)", found.len())),
    };
    store::append(path, store::Append { objects: &objects, reference: store::Ref { key: key.to_string(), commit, time: now() } }, &keep_two)?;
    Ok(commit)
}
