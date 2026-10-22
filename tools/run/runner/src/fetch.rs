//! `makepad run <git url>[#ref] [--mount <dir>=<git url>[#ref]]... [--cached] <package> ...`:
//! programs straight from git repositories. Generic: any smart-HTTP git remote (Relax's own
//! git client, the compiler's git module, on the system's HTTP stack, tools/run/light_http); nothing
//! here knows a particular service, repository or app.
//!
//! A run spec is one repository plus optional repositories mounted at subfolders of it (a repo
//! whose packages sit inside another's workspace, the way a git submodule would). Nothing is
//! checked out: each repository is ONE store file, `<home>/cache/git-<name>-<url hash>.store`
//! (store.rs; the two newest fetches per ref kept), and the run reads its sources through makepad run's
//! VFS from a virtual workspace `<home>/vfs/<spec shape>` (vfs.rs; never created on disk).
//! Offline, the newest kept commits run. `--cached` runs the newest kept commits at once and
//! fetches the spec in a detached `makepad fetch` for the next start.
//!
//! `<home>` is $MAKEPAD_RUN_HOME, else `~/.makepad-run` (`%USERPROFILE%\.makepad-run`). Requests carry a plain
//! User-Agent and nothing that identifies the person.
use relax_compiler::git::{
    self, build_info_refs_request, build_upload_pack_request, extract_pack_from_response, parse_info_refs_response,
    Method, ObjectId, ObjectKind, Request, Response,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Relax's own folder.
pub fn home() -> PathBuf {
    relax_compiler::gitstore::relax_home()
}

fn http(req: &Request, timeout: Duration) -> Result<Response, String> {
    let method = match req.method {
        Method::Get => "GET",
        Method::Post => "POST",
    };
    let headers: Vec<(&str, &str)> = req.headers.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let r = relax_light_http::Request {
        method,
        url: &req.url,
        headers: &headers,
        body: &req.body,
        timeout,
        max_body: 4 << 30,
    };
    let mut body = Vec::new();
    // one progress channel (relax_compiler::progress): the runner's bar, else a terminal line
    let label = repo_label(&req.url);
    let resp = relax_light_http::send(&r, &mut body, &mut |n, total| {
        if n >= 1 << 20 {
            relax_compiler::progress::progress(&label, n, total);
        }
    })
    .map_err(|e| format!("{}: {}", host(&req.url), e))?;
    Ok(Response { status_code: resp.status, body })
}

/// "makepad from host" for a git URL `https://host/.../makepad.git/...`.
fn repo_label(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let name = path.split('/').find(|s| s.ends_with(".git")).map(|s| s.trim_end_matches(".git")).unwrap_or("repository");
    format!("{} from {}", name, host(url))
}

fn host(url: &str) -> &str {
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
        if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1") && !url.starts_with("http://localhost") {
            return Err(format!("not an https repository URL: {s}"));
        }
        Ok(Repo { url: url.trim_end_matches('/').to_string(), reference, commit })
    }
    /// The last path segment without `.git` (folder names).
    pub fn name(&self) -> String {
        let last = self.url.rsplit('/').next().unwrap_or("repo");
        let name: String = last.trim_end_matches(".git").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        if name.is_empty() { "repo".into() } else { name }
    }
}

/// What to run: a repository, and repositories mounted at subfolders of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub root: Repo,
    pub mounts: Vec<(String, Repo)>,
}

impl Spec {
    /// `<dir>=<url>[#ref]` (the folder inside the root repository; relative, no `..`).
    pub fn parse_mount(s: &str) -> Result<(String, Repo), String> {
        let (dir, url) = s.split_once('=').ok_or_else(|| format!("--mount wants <dir>=<git url>: {s}"))?;
        let dir = dir.trim_matches('/');
        if dir.is_empty() || dir.split('/').any(|p| p.is_empty() || p == "." || p == ".." || p.contains('\\')) || dir.starts_with('/') {
            return Err(format!("--mount: not a relative folder: {dir}"));
        }
        Ok((dir.to_string(), Repo::parse(url)?))
    }
    /// The words that name this spec on a command line (`makepad fetch` takes them back; the
    /// loader exports them as MAKEPAD_RUN_SPEC for the programs it runs).
    pub fn args(&self) -> Vec<String> {
        let w = |r: &Repo| {
            let base = r.reference.as_ref().map_or(r.url.clone(), |x| format!("{}#{x}", r.url));
            r.commit.as_ref().map_or(base.clone(), |c| format!("{base}@{c}"))
        };
        let mut v = vec![w(&self.root)];
        for (dir, repo) in &self.mounts {
            v.push("--mount".into());
            v.push(format!("{dir}={}", w(repo)));
        }
        v
    }
}

/// The remote's commit for the repo's ref (or HEAD), with the capabilities.
/// How long asking a remote which commit a ref names may take before the stores answer instead
/// (MAKEPAD_RUN_FETCH_TIMEOUT_S overrides it).
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);

fn head_timeout() -> Duration {
    std::env::var("MAKEPAD_RUN_FETCH_TIMEOUT_S").ok().and_then(|v| v.parse().ok()).map(Duration::from_secs).unwrap_or(HEAD_TIMEOUT)
}

fn head(repo: &Repo) -> Result<git::RemoteHead, String> {
    // short: an unreachable or blackholed host must not hold a launch up (the stores answer then)
    let resp = http(&build_info_refs_request(&repo.url), head_timeout())?;
    parse_info_refs_response(&resp, repo.reference.as_deref()).map_err(|e| format!("{}: {}", repo.url, e))
}

/// A spec's repositories at commits: (folder inside the root, "" for the root; store file; commit).
pub struct Snapshot {
    /// stable per spec shape (the repositories and folders, not the commits)
    pub label: String,
    pub parts: Vec<(String, PathBuf, relax_compiler::gitstore::Oid)>,
}

/// Where the git stores live: `<home>/cache` (a run's cache and build files do not: dirs.rs).
pub fn cache_dir(home: &Path) -> PathBuf {
    home.join("cache")
}

fn ref_key(repo: &Repo) -> String {
    repo.reference.clone().unwrap_or_else(|| "HEAD".into())
}

/// Keeps per ref key the two newest fetches (the snapshot policy); everything else goes from the
/// index (its bytes at the next compaction).
fn keep_two(refs: &[relax_compiler::gitstore::Ref]) -> Vec<relax_compiler::gitstore::Ref> {
    let mut by_key: std::collections::BTreeMap<&str, Vec<&relax_compiler::gitstore::Ref>> = Default::default();
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

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// One repository at its ref into its store: fetched when the store does not hold that commit
/// (new objects only where the server negotiates); returns the commit. Offline: the newest kept
/// commit of the ref.
fn fetch_repo(cache: &Path, repo: &Repo) -> Result<relax_compiler::gitstore::Oid, String> {
    use relax_compiler::gitstore as store;
    let path = store::store_path(cache, &repo.url);
    let existing = store::Store::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let key = ref_key(repo);
    // a pinned commit already in the store, or its pack already downloaded: no remote request
    if let Some(pinned) = repo.commit.as_deref().and_then(store::from_hex) {
        if let Some(s) = &existing {
            if s.index.table(&pinned).is_some() {
                if !s.index.refs.iter().filter(|r| r.key == key).max_by_key(|r| r.time).is_some_and(|r| r.commit == pinned) {
                    store::append(&path, store::Append { objects: &[], reference: store::Ref { key, commit: pinned, time: now() } }, &keep_two)?;
                }
                return Ok(pinned);
            }
        }
        let incoming = cache.join("incoming").join(format!("{}.pack", store::hex(&pinned)));
        if let Ok(body) = std::fs::read(&incoming) {
            let _ = std::fs::remove_file(&incoming);
            let pack = extract_pack_from_response(&Response { status_code: 200, body }).map_err(|e| e.to_string())?.ok_or("the downloaded pack is empty")?;
            append_pack(&path, existing.as_ref(), &pack, store::Ref { key, commit: pinned, time: now() }, &repo.url)?;
            return Ok(pinned);
        }
    }
    match fetch_remote(cache, repo, existing.as_ref(), &path, key.clone()) {
        Ok(c) => Ok(c),
        // offline, unreachable or failing: what the stores have runs (never a failed launch while
        // a usable commit is kept), and the run says so
        Err(e) => {
            let s = existing.as_ref().ok_or_else(|| e.clone())?;
            let newest = s.index.refs.iter().filter(|r| r.key == key).max_by_key(|r| r.time).or_else(|| s.index.refs.iter().max_by_key(|r| r.time)).map(|r| r.commit);
            let c = newest.ok_or(e.clone())?;
            let wanted = repo.commit.as_deref().map(|w| format!(" (wanted {})", &w[..w.len().min(12)])).unwrap_or_default();
            relax_compiler::progress::note(&format!("{} unreachable ({e}): running {} from the store{wanted}", host(&repo.url), &store::hex(&c)[..12]));
            Ok(c)
        }
    }
}

/// The remote's answer for `repo`: its commit for the ref (or the pin), fetched into the store
/// unless the store has it already.
fn fetch_remote(cache: &Path, repo: &Repo, existing: Option<&relax_compiler::gitstore::Store>, path: &Path, key: String) -> Result<relax_compiler::gitstore::Oid, String> {
    use relax_compiler::gitstore as store;
    let h = head(repo)?;
    // a pinned commit is what is wanted, whatever the ref names now
    let mut h = h;
    if let Some(pinned) = repo.commit.as_deref().and_then(|c| ObjectId::from_hex(c).ok()) {
        h.oid = pinned;
    }
    let commit = h.oid.0;
    if let Some(s) = existing {
        if s.index.refs.iter().filter(|r| r.key == key).max_by_key(|r| r.time).is_some_and(|r| r.commit == commit) {
            // the ref's newest fetch already is this commit: nothing to write (a --cached start
            // checks every time; rewriting the index each start would only grow the file)
            return Ok(commit);
        }
        if s.index.table(&commit).is_some() {
            // here under another ref (or older): record the ref so the policy keeps it
            let r = store::Ref { key, commit, time: now() };
            store::append(path, store::Append { objects: &[], reference: r }, &keep_two)?;
            return Ok(commit);
        }
    }
    let haves: Vec<ObjectId> = existing
        .map(|s| s.index.refs.iter().map(|r| ObjectId(r.commit)).collect())
        .unwrap_or_default();
    let shallow = h.capabilities.iter().any(|c| c == "shallow");
    let req = build_upload_pack_request(&repo.url, h.oid, &h.capabilities, &haves, shallow.then_some(1)).map_err(|e| e.to_string())?;
    eprintln!("makepad: fetching {} ({})", repo.url, &h.oid.to_hex()[..12]);
    let t0 = std::time::Instant::now();
    // a pack the entry script downloaded while Relax was being built (incoming/<commit>.pack:
    // the upload-pack answer for this commit, renamed into place when complete)
    let incoming = cache.join("incoming").join(format!("{}.pack", h.oid.to_hex()));
    let body = match std::fs::read(&incoming) {
        Ok(b) => {
            let _ = std::fs::remove_file(&incoming);
            b
        }
        Err(_) => http(&req, Duration::from_secs(600))?.body,
    };
    let pack = extract_pack_from_response(&Response { status_code: 200, body }).map_err(|e| e.to_string())?.ok_or("the server sent no pack")?;
    let t_dl = t0.elapsed();
    let (n, t_unpack, t_store) = append_pack(path, existing, &pack, store::Ref { key, commit, time: now() }, &repo.url)?;
    if std::env::var_os("MAKEPAD_RUN_FETCH_TIMES").is_some() {
        eprintln!("makepad: {}: download {:.2} s, unpack {:.2} s ({} objects), store {:.2} s", repo.url, t_dl.as_secs_f64(), t_unpack, n, t_store);
    }
    Ok(commit)
}

/// A pack's objects (deltas resolved against the pack and the store) appended to the store with
/// `reference`: (objects, unpack s, store s).
fn append_pack(path: &Path, existing: Option<&relax_compiler::gitstore::Store>, pack: &[u8], reference: relax_compiler::gitstore::Ref, url: &str) -> Result<(usize, f64, f64), String> {
    use relax_compiler::gitstore as store;
    let mut external = |oid: &ObjectId| -> Option<git::Object> {
        let (kind, data) = existing?.object(&oid.0).ok().flatten()?;
        Some(git::Object { kind: ObjectKind::from_type_num(kind).ok()?, data })
    };
    let t1 = std::time::Instant::now();
    let objects = git::unpack_pack(pack, &mut external).map_err(|e| format!("{url}: {e}"))?;
    let t_unpack = t1.elapsed().as_secs_f64();
    // the store's kind numbers are the pack's (store::COMMIT..TAG = ObjectKind)
    let objects: Vec<(store::Oid, u8, Vec<u8>)> = objects.into_iter().map(|(oid, o)| (oid.0, o.kind as u8, o.data)).collect();
    let t2 = std::time::Instant::now();
    store::append(path, store::Append { objects: &objects, reference }, &keep_two)?;
    Ok((objects.len(), t_unpack, t2.elapsed().as_secs_f64()))
}

fn label(spec: &Spec) -> String {
    let mut s = spec.root.name();
    for (dir, repo) in &spec.mounts {
        s.push_str(&format!("+{}@{}", repo.name(), dir.replace('/', "_")));
    }
    s
}

/// Fetches the spec's repositories at their refs into their stores (offline: the newest kept
/// commits) and returns the snapshot to mount.
pub fn fetch(home: &Path, spec: &Spec) -> Result<Snapshot, String> {
    let cache = cache_dir(home);
    let mut parts = Vec::new();
    for (dir, repo) in std::iter::once((String::new(), &spec.root)).chain(spec.mounts.iter().map(|(d, r)| (d.clone(), r))) {
        let commit = fetch_repo(&cache, repo)?;
        parts.push((dir, relax_compiler::gitstore::store_path(&cache, &repo.url), commit));
    }
    Ok(Snapshot { label: label(spec), parts })
}

/// The newest kept commits of the spec's refs, when every store has one.
fn newest(home: &Path, spec: &Spec) -> Option<Snapshot> {
    let cache = cache_dir(home);
    let mut parts = Vec::new();
    for (dir, repo) in std::iter::once((String::new(), &spec.root)).chain(spec.mounts.iter().map(|(d, r)| (d.clone(), r))) {
        let path = relax_compiler::gitstore::store_path(&cache, &repo.url);
        let s = relax_compiler::gitstore::Store::open(&path).ok()??;
        // a pinned commit: that one or nothing (then a fetch, which may find its pack in incoming/)
        if let Some(pinned) = repo.commit.as_deref().and_then(relax_compiler::gitstore::from_hex) {
            s.index.table(&pinned)?;
            parts.push((dir, path, pinned));
            continue;
        }
        let key = ref_key(repo);
        // the newest fetch of this ref; any ref of the repository when that one was never fetched
        let r = s.index.refs.iter().filter(|r| r.key == key).max_by_key(|r| r.time).or_else(|| s.index.refs.iter().max_by_key(|r| r.time))?;
        parts.push((dir, path, r.commit));
    }
    Some(Snapshot { label: label(spec), parts })
}

/// `--cached`: the newest kept commits at once, with a detached `makepad fetch` refreshing them for
/// the next start; else a fetch now.
pub fn cached(home: &Path, spec: &Spec) -> Result<Snapshot, String> {
    // every commit pinned: nothing newer to look for. `fetch` then answers from the stores (or
    // the packs in incoming/) without a request, and records each pin under its ref, which keeps
    // the commit (the store keeps the newest fetches per ref: a pin is a root)
    let all_pinned = std::iter::once(&spec.root).chain(spec.mounts.iter().map(|(_, r)| r)).all(|r| r.commit.is_some());
    if all_pinned {
        return fetch(home, spec);
    }
    match newest(home, spec) {
        Some(snap) => {
            // the refresh for the next start runs in this process, in the background (no spawn);
            // an append cut short by the program's exit is cut back when the store next opens
            let (home, spec) = (home.to_path_buf(), spec.clone());
            let _ = std::thread::Builder::new().name("run-fetch".into()).spawn(move || {
                let _ = fetch(&home, &spec);
            });
            Ok(snap)
        }
        None => fetch(home, spec),
    }
}

/// Mounts the snapshot under `<home>/vfs/<label>` (a folder that is never created) and returns
/// that virtual workspace.
pub fn mount(home: &Path, snap: &Snapshot) -> Result<String, String> {
    // the mounted stores (index, file tables) are process state, read by the program for as long
    // as it runs: kept heap, never the compiler zone (its destroy at the zone swap freed them under
    // a program reading a file of its workspace after its first present)
    let _k = relax_compiler::zone_alloc::kept();
    let root = home.join("vfs").join(&snap.label).display().to_string();
    for (dir, path, commit) in &snap.parts {
        let s = relax_compiler::gitstore::Store::open(path).map_err(|e| format!("{}: {e}", path.display()))?.ok_or_else(|| format!("{}: no store", path.display()))?;
        let at = if dir.is_empty() { root.clone() } else { format!("{root}/{dir}") };
        relax_compiler::vfs::mount(&at, std::sync::Arc::new(s), commit)?;
    }
    Ok(root)
}

/// Splits `run`/`fetch` words into (options for the run, spec, `--cached`, the rest after the
/// URL). None when no URL is among them.
pub fn split_args(args: &[String]) -> Option<Result<(Vec<String>, Spec, bool, Vec<String>), String>> {
    let mut opts = Vec::new();
    let mut mounts = Vec::new();
    let mut cached = false;
    let mut i = 0;
    while i < args.len() && args[i].starts_with("--") {
        match args[i].as_str() {
            "--mount" => {
                let Some(m) = args.get(i + 1) else { return Some(Err("--mount wants <dir>=<git url>".into())) };
                match Spec::parse_mount(m) {
                    Ok(m) => mounts.push(m),
                    Err(e) => return Some(Err(e)),
                }
                i += 2;
                continue;
            }
            "--cached" => {
                cached = true;
                i += 1;
                continue;
            }
            _ => {}
        }
        let n = match args[i].as_str() {
            "--units" | "--report-to" | "--entry" | "--boundary" => 2,
            _ => 1,
        };
        opts.extend(args[i..(i + n).min(args.len())].iter().cloned());
        i += n;
    }
    let url = args.get(i)?;
    if !url.contains("://") {
        return if mounts.is_empty() && !cached { None } else { Some(Err("--mount / --cached go with a git URL".into())) };
    }
    let root = match Repo::parse(url) {
        Ok(r) => r,
        Err(e) => return Some(Err(e)),
    };
    // --mount / --cached may also follow the URL
    let mut rest = Vec::new();
    let mut j = i + 1;
    while j < args.len() {
        match args[j].as_str() {
            "--mount" if j + 1 < args.len() => {
                match Spec::parse_mount(&args[j + 1]) {
                    Ok(m) => mounts.push(m),
                    Err(e) => return Some(Err(e)),
                }
                j += 2;
            }
            "--cached" => {
                cached = true;
                j += 1;
            }
            "--" => {
                rest.extend(args[j..].iter().cloned());
                break;
            }
            _ => {
                rest.push(args[j].clone());
                j += 1;
            }
        }
    }
    Some(Ok((opts, Spec { root, mounts }, cached, rest)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    #[test]
    fn specs() {
        let (opts, spec, cached, rest) = split_args(&s(&["--quiet", "--cached", "https://h/m.git#main", "--mount", "apps/c=https://h/c.git#r1", "pkg", "--release", "--", "x"])).unwrap().unwrap();
        assert_eq!(opts, s(&["--quiet"]));
        assert!(cached);
        assert_eq!(spec.root, Repo { url: "https://h/m.git".into(), reference: Some("main".into()), commit: None });
        assert_eq!(spec.mounts, vec![("apps/c".to_string(), Repo { url: "https://h/c.git".into(), reference: Some("r1".into()), commit: None })]);
        let pinned = Repo::parse("https://h/m.git#rel@0123456789ABCDEF0123456789abcdef01234567").unwrap();
        assert_eq!(pinned, Repo { url: "https://h/m.git".into(), reference: Some("rel".into()), commit: Some("0123456789abcdef0123456789abcdef01234567".into()) });
        assert_eq!(Spec { root: pinned, mounts: Vec::new() }.args(), vec!["https://h/m.git#rel@0123456789abcdef0123456789abcdef01234567".to_string()]);
        assert_eq!(rest, s(&["pkg", "--release", "--", "x"]));
        assert_eq!(spec.args(), s(&["https://h/m.git#main", "--mount", "apps/c=https://h/c.git#r1"]));
        assert_eq!(label(&spec), "m+c@apps_c");
        assert!(split_args(&s(&["pkg"])).is_none());
        assert!(split_args(&s(&["--mount", "../x=https://h/x", "https://h/m", "p"])).unwrap().is_err());
        assert!(Repo::parse("git@github.com:x/y").is_err());
    }

    /// A loopback smart-HTTP git service: the bare repository `repo` served by
    /// `git upload-pack --stateless-rpc` (what git http-backend runs), one request per connection.
    fn serve(repo: PathBuf) -> u16 {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { continue };
                let mut got = Vec::new();
                let mut b = [0u8; 65536];
                let head_end = loop {
                    if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    let n = c.read(&mut b).unwrap();
                    assert!(n > 0, "request cut short");
                    got.extend_from_slice(&b[..n]);
                };
                let head = String::from_utf8_lossy(&got[..head_end]).to_string();
                let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                while got.len() < head_end + len {
                    let n = c.read(&mut b).unwrap();
                    got.extend_from_slice(&b[..n]);
                }
                let body = &got[head_end..head_end + len];
                let target = head.split(' ').nth(1).unwrap().to_string();
                let (kind, out) = if target.ends_with("/info/refs?service=git-upload-pack") {
                    let o = std::process::Command::new("git").args(["upload-pack", "--stateless-rpc", "--advertise-refs"]).arg(&repo).output().unwrap();
                    let mut out = b"001e# service=git-upload-pack\n0000".to_vec();
                    out.extend_from_slice(&o.stdout);
                    ("advertisement", out)
                } else {
                    assert!(target.ends_with("/git-upload-pack"), "{target}");
                    let mut p = std::process::Command::new("git")
                        .args(["upload-pack", "--stateless-rpc"])
                        .arg(&repo)
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .spawn()
                        .unwrap();
                    p.stdin.take().unwrap().write_all(body).unwrap();
                    ("result", p.wait_with_output().unwrap().stdout)
                };
                let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-{kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
                let _ = c.write_all(&out);
            }
        });
        port
    }

    fn git(dir: &Path, args: &[&str]) -> Vec<u8> {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        o.stdout
    }

    /// The store's file table for `commit` equals `git ls-tree -r`, and every blob's bytes equal git's.
    fn check_store(store: &relax_compiler::gitstore::Store, commit: &relax_compiler::gitstore::Oid, bare: &Path) {
        use relax_compiler::gitstore::hex;
        let table = store.index.table(commit).expect("path table of the fetched commit");
        let listing = String::from_utf8(git(bare, &["ls-tree", "-r", &hex(commit)])).unwrap();
        let want: Vec<(String, u32, String)> = listing
            .lines()
            .map(|l| {
                let (meta, path) = l.split_once('\t').unwrap();
                let m: Vec<&str> = meta.split(' ').collect();
                (path.to_string(), u32::from_str_radix(m[0], 8).unwrap(), m[2].to_string())
            })
            .collect();
        let have: Vec<(String, u32, String)> = table.iter().map(|e| (e.path.clone(), e.mode, hex(&e.oid))).collect();
        assert_eq!(have, want);
        for e in table {
            let (kind, data) = store.object(&e.oid).unwrap().expect("blob in the store");
            assert_eq!(kind, relax_compiler::gitstore::BLOB);
            assert_eq!(data, git(bare, &["cat-file", "blob", &hex(&e.oid)]), "{}", e.path);
        }
    }

    /// `run <url>` as it fetches: ref advertisement, a depth-1 upload-pack, the pack unpacked into
    /// the store; then a second commit fetched incrementally (`have` = the stored commit, so the
    /// service may send a thin pack whose ref-delta bases come from the store).
    #[test]
    fn fetch_from_loopback_service() {
        if std::process::Command::new("git").arg("--version").output().is_err() {
            eprintln!("no git: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("relax-fetch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let work = dir.join("work");
        std::fs::create_dir_all(work.join("src/deep")).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        let big: String = (0..4000).map(|i| format!("line {i} of a file large enough to be deltified\n")).collect();
        std::fs::write(work.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(work.join("src/lib.rs"), &big).unwrap();
        std::fs::write(work.join("src/deep/mod.rs"), "pub fn f() {}\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "one"]);
        let bare = dir.join("repo.git");
        let o = std::process::Command::new("git").args(["clone", "-q", "--bare"]).arg(&work).arg(&bare).output().unwrap();
        assert!(o.status.success());
        let port = serve(bare.clone());
        let home = dir.join("home");
        let spec = Spec { root: Repo::parse(&format!("http://127.0.0.1:{port}/repo.git")).unwrap(), mounts: Vec::new() };

        let snap = fetch(&home, &spec).unwrap();
        let (_, path, commit) = &snap.parts[0];
        assert_eq!(relax_compiler::gitstore::hex(commit), String::from_utf8(git(&bare, &["rev-parse", "main"])).unwrap().trim());
        check_store(&relax_compiler::gitstore::Store::open(path).unwrap().unwrap(), commit, &bare);

        // a second commit: one line of the large file changed, a file added
        std::fs::write(work.join("src/lib.rs"), big.replace("line 2000 of", "line two thousand of")).unwrap();
        std::fs::write(work.join("README"), "new\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "two"]);
        git(&work, &["push", "-q", bare.to_str().unwrap(), "main"]);
        let size_before = std::fs::metadata(path).unwrap().len();
        let snap = fetch(&home, &spec).unwrap();
        let (_, path, commit2) = &snap.parts[0];
        assert_ne!(commit2, commit);
        let store = relax_compiler::gitstore::Store::open(path).unwrap().unwrap();
        check_store(&store, commit2, &bare);
        let grown = std::fs::metadata(path).unwrap().len() - size_before;
        assert!(grown < big.len() as u64 / 2, "the update appended {grown} bytes: not incremental");
        // unchanged remote: nothing written
        let size = std::fs::metadata(path).unwrap().len();
        fetch(&home, &spec).unwrap();
        assert_eq!(std::fs::metadata(path).unwrap().len(), size);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
