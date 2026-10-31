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
pub use relax_compiler::git::fetch::Repo;
use relax_compiler::git::fetch::fetch_repo;
use std::path::{Path, PathBuf};

/// Relax's own folder.
pub fn home() -> PathBuf {
    relax_compiler::gitstore::relax_home()
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
    relax_compiler::registry_net::install();
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
        // the newest fetch of this ref; any ref of the repository when that one was never fetched
        parts.push((dir, path, relax_compiler::git::fetch::newest(&s, &repo.ref_key())?));
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

/// `makepad export`: writes the snapshot's files into `out` as one working tree (the mounted
/// repositories at their folders), then the edits folder's files over them (MAKEPAD_RUN_EDITS: what
/// the app's runs read instead of the store's), so a native build (cargo) sees what the app runs.
/// Files whose bytes are already there are left alone (their times too: cargo rebuilds only what
/// changed); files an earlier export wrote that this one does not are removed
/// (`<out>/.makepad-export` lists what it wrote). Returns (files in the tree, files written).
pub fn export(snap: &Snapshot, edits: Option<&Path>, out: &Path) -> Result<(usize, usize), String> {
    use std::collections::BTreeSet;
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut wrote = 0usize;
    let mut put = |rel: &str, data: &[u8], exec: bool, files: &mut BTreeSet<String>| -> Result<(), String> {
        if rel.is_empty() || rel.split('/').any(|c| c.is_empty() || c == "." || c == ".." || c.contains('\\')) || rel == ".makepad-export" {
            return Ok(());
        }
        let path = out.join(rel);
        if std::fs::read(&path).ok().as_deref() != Some(data) {
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
            #[cfg(unix)]
            if exec {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
            }
            wrote += 1;
        }
        let _ = exec;
        files.insert(rel.to_string());
        Ok(())
    };
    // the edits folder's files: written instead of the store's
    let mut edited: Vec<(String, PathBuf)> = Vec::new();
    if let Some(edits) = edits {
        fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if rel.is_empty() && name.starts_with(".makepad-run-") {
                    continue;
                }
                let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                let p = e.path();
                if p.is_dir() {
                    walk(&p, &r, out);
                } else if p.is_file() {
                    out.push((r, p));
                }
            }
        }
        walk(edits, "", &mut edited);
    }
    let edited_set: BTreeSet<String> = edited.iter().map(|(r, _)| r.clone()).collect();
    // the root first, then each mount (a mounted repository's folder is its, not the root's)
    for (dir, store_path, commit) in &snap.parts {
        let store = relax_compiler::gitstore::Store::open(store_path).map_err(|e| format!("{}: {e}", store_path.display()))?.ok_or_else(|| format!("{}: no store", store_path.display()))?;
        let table = store.index.table(commit).ok_or_else(|| format!("{}: commit {} is not in it", store_path.display(), relax_compiler::gitstore::hex(commit)))?;
        for e in table {
            if e.mode != 0o100644 && e.mode != 0o100755 {
                continue;
            }
            let rel = if dir.is_empty() { e.path.clone() } else { format!("{dir}/{}", e.path) };
            if edited_set.contains(&rel) {
                continue;
            }
            if snap.parts.iter().any(|(d, _, _)| d.len() > dir.len() && !d.is_empty() && rel.starts_with(&format!("{d}/"))) {
                continue;
            }
            let (_, data) = store.object(&e.oid).map_err(|x| format!("{}: {x}", store_path.display()))?.ok_or_else(|| format!("{}: {} is missing", store_path.display(), e.path))?;
            put(&rel, &data, e.mode == 0o100755, &mut files)?;
        }
    }
    for (rel, p) in edited {
        let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        put(&rel, &data, false, &mut files)?;
    }
    let manifest = out.join(".makepad-export");
    for old in std::fs::read_to_string(&manifest).unwrap_or_default().lines() {
        if !files.contains(old) && !old.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
            let _ = std::fs::remove_file(out.join(old));
        }
    }
    let text: String = files.iter().map(|f| format!("{f}\n")).collect();
    std::fs::write(&manifest, text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    Ok((files.len(), wrote))
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
            "--units" | "--report-to" | "--entry" | "--boundary" | "--run-dir" | "--workspace-dir" => 2,
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
        serve_switch(repo).0
    }

    /// `serve` with a switch: while it is on, every request gets HTTP 503 (the remote is down).
    fn serve_switch(repo: PathBuf) -> (u16, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let down = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let is_down = down.clone();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { continue };
                if is_down.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = c.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    continue;
                }
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
        (port, down)
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

        // export: the tree with the edits folder's files over it; again, nothing written; an edit
        // gone, the release's file back
        let edits = dir.join("edits");
        write(&edits, &[("src/deep/mod.rs", "pub fn g() {}\n"), (".makepad-run-edits", "src/deep/mod.rs\tnew\n")]);
        let out = dir.join("export");
        assert_eq!(export(&snap, Some(&edits), &out).unwrap(), (4, 4));
        assert_eq!(std::fs::read_to_string(out.join("src/deep/mod.rs")).unwrap(), "pub fn g() {}\n");
        assert_eq!(std::fs::read_to_string(out.join("README")).unwrap(), "new\n");
        assert!(!out.join(".makepad-run-edits").exists());
        assert_eq!(export(&snap, Some(&edits), &out).unwrap(), (4, 0));
        std::fs::remove_file(edits.join("src/deep/mod.rs")).unwrap();
        assert_eq!(export(&snap, Some(&edits), &out).unwrap(), (4, 1));
        assert_eq!(std::fs::read_to_string(out.join("src/deep/mod.rs")).unwrap(), "pub fn f() {}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write(dir: &Path, files: &[(&str, &str)]) {
        for (p, text) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }
    }

    fn rev(dir: &Path, r: &str) -> String {
        String::from_utf8(git(dir, &["rev-parse", r])).unwrap().trim().to_string()
    }

    /// A bare repository made from the work tree `work` (committed by the caller).
    fn bare(work: &Path, bare: &Path) {
        let o = std::process::Command::new("git").args(["clone", "-q", "--bare"]).arg(work).arg(bare).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    /// Cargo git dependencies through Relax's resolver (resolve.rs): served over loopback smart
    /// HTTP into the stores, mounted on the VFS, the package found in the repository as cargo
    /// finds it, pinned by Cargo.lock or by relax's own git.lock.
    #[test]
    fn git_dependencies_resolve_from_the_stores() {
        use relax_compiler::resolve::{unit_graph_with, GitPlaces};
        if std::process::Command::new("git").arg("--version").output().is_err() {
            eprintln!("no git: skipped");
            return;
        }
        relax_compiler::registry_net::install();
        let sess = &std::sync::Arc::new(relax_compiler::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-gitdeps-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // the inner repository: one package at its root
        let iw = dir.join("innerw");
        write(&iw, &[("Cargo.toml", "[package]\nname = \"ginner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"), ("src/lib.rs", "pub fn ginner() {}\n")]);
        git(&iw, &["init", "-q", "-b", "main"]);
        git(&iw, &["add", "-A"]);
        git(&iw, &["commit", "-q", "-m", "inner"]);
        bare(&iw, &dir.join("inner.git"));
        let inner = format!("http://127.0.0.1:{}/inner.git", serve(dir.join("inner.git")));
        // the dependency repository: a workspace whose member inherits from it, a path dependency
        // inside it, a git dependency of its own, decoys in a hidden folder and a target folder
        let dw = dir.join("depw");
        write(&dw, &[
            ("Cargo.toml", &format!("[workspace]\nmembers = [\"crates/*\"]\n[workspace.package]\nversion = \"0.3.0\"\nedition = \"2021\"\n[workspace.dependencies]\nginner = {{ git = \"{inner}\" }}\n")),
            ("crates/gdep/Cargo.toml", "[package]\nname = \"gdep\"\nversion.workspace = true\nedition.workspace = true\n[dependencies]\nghelper = { path = \"../ghelper\" }\nginner = { workspace = true }\n"),
            ("crates/gdep/src/lib.rs", "pub fn gdep() -> u32 { 1 }\n"),
            ("crates/ghelper/Cargo.toml", "[package]\nname = \"ghelper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
            ("crates/ghelper/src/lib.rs", "pub fn ghelper() {}\n"),
            (".hidden/gdep/Cargo.toml", "[package]\nname = \"gdep\"\nversion = \"9.9.9\"\n"),
            (".hidden/gdep/src/lib.rs", "decoy\n"),
            ("crates/gdep/target/gdep/Cargo.toml", "[package]\nname = \"gdep\"\nversion = \"8.8.8\"\n"),
            ("crates/gdep/target/gdep/src/lib.rs", "decoy\n"),
        ]);
        git(&dw, &["init", "-q", "-b", "main"]);
        git(&dw, &["add", "-A"]);
        git(&dw, &["commit", "-q", "-m", "c1"]);
        git(&dw, &["tag", "-a", "v1", "-m", "v1"]);
        let c1 = rev(&dw, "HEAD");
        git(&dw, &["checkout", "-q", "-b", "dev"]);
        write(&dw, &[("crates/gdep/src/lib.rs", "pub fn gdep() -> u32 { 2 }\n")]);
        git(&dw, &["commit", "-q", "-am", "c2"]);
        let c2 = rev(&dw, "HEAD");
        // the served repository's HEAD (its default branch) is main
        git(&dw, &["checkout", "-q", "main"]);
        let dep_bare = dir.join("dep.git");
        bare(&dw, &dep_bare);
        let (port, down) = serve_switch(dep_bare.clone());
        let dep = format!("http://127.0.0.1:{port}/dep.git");

        let app = dir.join("app");
        let home = dir.join("home");
        let lock = dir.join("relax").join("git.lock");
        let manifest = |deps: &str| format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{deps}");
        let set_app = |deps: &str| write(&app, &[("Cargo.toml", &manifest(deps)), ("src/lib.rs", "")]);
        let places = || GitPlaces { home: home.clone(), lock: Some(lock.clone()) };
        let graph = |flags: &[&str]| {
            let flags: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
            unit_graph_with(sess, &app.display().to_string(), "app", &flags, places())
        };
        let unit = |g: &relax_compiler::resolve::Units, name: &str| -> (String, String) {
            let us: Vec<_> = g.units.iter().filter(|u| u.name == name).collect();
            assert_eq!(us.len(), 1, "one unit {name}: {:?}", g.units.iter().map(|u| &u.pkg_id).collect::<Vec<_>>());
            (us[0].pkg_id.clone(), us[0].src_path.clone())
        };
        let body = |src: &str| String::from_utf8(relax_compiler::vfs::read(src).unwrap()).unwrap();

        // a branch, a crates-io dependency patched to a git source, a dev-dependency on a tag
        let deps = format!("[dependencies]\ngdep = {{ git = \"{dep}\", branch = \"dev\" }}\nginner = \"0.1\"\n[dev-dependencies]\nghelper = {{ git = \"{dep}\", tag = \"v1\" }}\n[patch.crates-io]\nginner = {{ git = \"{inner}\" }}\n");
        set_app(&deps);
        let g = graph(&[]).unwrap();
        let (id, src) = unit(&g, "gdep");
        assert_eq!(id, format!("git+{dep}?branch=dev#gdep@0.3.0"));
        assert!(src.contains("/vfs/git/dep-") && src.ends_with(&format!("{}/crates/gdep/src/lib.rs", &c2[..12])), "{src}");
        assert_eq!(body(&src), "pub fn gdep() -> u32 { 2 }\n");
        // the path dependency inside the repository shares its source; the patched and the
        // inherited git dependency are one package
        assert_eq!(unit(&g, "ghelper").0, format!("git+{dep}?branch=dev#ghelper@0.1.0"));
        assert_eq!(unit(&g, "ginner").0, format!("git+{inner}#ginner@0.1.0"));
        let pins = std::fs::read_to_string(&lock).unwrap();
        assert!(pins.contains(&format!("git+{dep}?branch=dev\t{c2}\n")), "{pins}");
        assert!(!app.join("Cargo.lock").exists());
        // the lib's tests: the dev-dependency at the tag (another source of the same repository)
        let gt = graph(&["--lib"]).unwrap();
        let tagged: Vec<_> = gt.units.iter().filter(|u| u.name == "ghelper").map(|u| u.pkg_id.clone()).collect();
        assert!(tagged.contains(&format!("git+{dep}?tag=v1#ghelper@0.1.0")), "{tagged:?}");
        assert!(std::fs::read_to_string(&lock).unwrap().contains(&format!("git+{dep}?tag=v1\t{c1}\n")));

        // the branch moves: relax's pin holds (no Cargo.lock written, cargo's behaviour with one)
        git(&dw, &["checkout", "-q", "dev"]);
        write(&dw, &[("crates/gdep/src/lib.rs", "pub fn gdep() -> u32 { 3 }\n")]);
        git(&dw, &["commit", "-q", "-am", "c3"]);
        let c3 = rev(&dw, "HEAD");
        git(&dw, &["push", "-q", dep_bare.to_str().unwrap(), "dev"]);
        assert_eq!(body(&unit(&graph(&[]).unwrap(), "gdep").1), "pub fn gdep() -> u32 { 2 }\n");
        // Cargo.lock names the source: the lockfile wins (fetched incrementally), relax's pin stays
        let cargo_lock = |commit: &str| format!("version = 4\n\n[[package]]\nname = \"gdep\"\nversion = \"0.3.0\"\nsource = \"git+{dep}?branch=dev#{commit}\"\n");
        write(&app, &[("Cargo.lock", &cargo_lock(&c3))]);
        assert_eq!(body(&unit(&graph(&[]).unwrap(), "gdep").1), "pub fn gdep() -> u32 { 3 }\n");
        assert!(std::fs::read_to_string(&lock).unwrap().contains(&format!("git+{dep}?branch=dev\t{c2}\n")));

        // the remote down: locked and pinned sources come from the store
        down.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(body(&unit(&graph(&[]).unwrap(), "gdep").1), "pub fn gdep() -> u32 { 3 }\n");
        // ... and an unpinned ref is the store's newest fetch of it
        std::fs::remove_file(app.join("Cargo.lock")).unwrap();
        std::fs::remove_file(&lock).unwrap();
        assert_eq!(body(&unit(&graph(&[]).unwrap(), "gdep").1), "pub fn gdep() -> u32 { 3 }\n");
        // ... an abbreviated rev the store holds too; a commit it does not hold cannot come from a
        // remote that is down
        set_app(&format!("[dependencies]\ngdep = {{ git = \"{dep}\", rev = \"{}\" }}\n", &c2[..10]));
        assert_eq!(body(&unit(&graph(&[]).unwrap(), "gdep").1), "pub fn gdep() -> u32 { 2 }\n");
        set_app(&format!("[dependencies]\ngdep = {{ git = \"{dep}\", rev = \"0123456789\" }}\n"));
        let e = graph(&[]).unwrap_err();
        assert!(e.contains("503"), "{e}");
        down.store(false, std::sync::atomic::Ordering::Relaxed);

        // a fresh store and a locked commit that is no ref's tip (wanted by its id)
        let home2 = dir.join("home2");
        let places2 = || GitPlaces { home: home2.clone(), lock: Some(dir.join("relax2").join("git.lock")) };
        set_app(&format!("[dependencies]\ngdep = {{ git = \"{dep}\", branch = \"dev\" }}\n"));
        write(&app, &[("Cargo.lock", &cargo_lock(&c2))]);
        let g = unit_graph_with(sess, &app.display().to_string(), "app", &[], places2()).unwrap();
        assert_eq!(body(&unit(&g, "gdep").1), "pub fn gdep() -> u32 { 2 }\n");
        std::fs::remove_file(app.join("Cargo.lock")).unwrap();
        // an abbreviated rev that is no tip, in a fresh store: found in the fetched history (cargo
        // resolves it so too)
        set_app(&format!("[dependencies]\ngdep = {{ git = \"{dep}\", rev = \"{}\" }}\n", &c2[..10]));
        let places3 = GitPlaces { home: dir.join("home3"), lock: Some(dir.join("relax3").join("git.lock")) };
        let g = unit_graph_with(sess, &app.display().to_string(), "app", &[], places3).unwrap();
        assert_eq!(unit(&g, "gdep").0, format!("git+{dep}?rev={}#gdep@0.3.0", &c2[..10]));
        assert_eq!(body(&unit(&g, "gdep").1), "pub fn gdep() -> u32 { 2 }\n");
        // the default branch (main: c1) and a full commit id
        set_app(&format!("[dependencies]\ngdep = {{ git = \"{dep}\" }}\n"));
        assert_eq!(body(&unit(&graph(&[]).unwrap(), "gdep").1), "pub fn gdep() -> u32 { 1 }\n");
        set_app(&format!("[dependencies]\ngdep = {{ git = \"{dep}\", rev = \"{c3}\" }}\n"));
        assert_eq!(body(&unit(&graph(&[]).unwrap(), "gdep").1), "pub fn gdep() -> u32 { 3 }\n");

        // [patch."<url>"] with a path: the repository is not fetched for it
        write(&dir, &[("local/gdep/Cargo.toml", "[package]\nname = \"gdep\"\nversion = \"0.3.0\"\nedition = \"2021\"\n"), ("local/gdep/src/lib.rs", "pub fn gdep() -> u32 { 4 }\n")]);
        set_app(&format!("[dependencies]\ngdep = {{ git = \"{dep}\", branch = \"dev\" }}\n[patch.\"{dep}\"]\ngdep = {{ path = \"../local/gdep\" }}\n"));
        let (id, src) = unit(&graph(&[]).unwrap(), "gdep");
        assert!(id.starts_with("path+file://"), "{id}");
        assert_eq!(body(&src), "pub fn gdep() -> u32 { 4 }\n");
        // a package the repository lacks, and a version it does not meet
        set_app(&format!("[dependencies]\nnope = {{ git = \"{dep}\" }}\n"));
        assert!(graph(&[]).unwrap_err().contains("no package named `nope`"));
        set_app(&format!("[dependencies]\ngdep = {{ git = \"{dep}\", version = \"1\" }}\n"));
        assert!(graph(&[]).unwrap_err().contains("does not satisfy"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
