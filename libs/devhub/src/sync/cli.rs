//! The master's sync words (`makepad devhub …`). They talk to the local master hub's control
//! socket and reach node hubs through its `route`; orders are signed here, with this machine's
//! own fleet key (the master's admin credential), and git runs read-only in the user's repo
//! (rev-parse, bundle create): nothing builds on the master.

use super::git::{valid_branch, valid_sha};
use super::sign;
use crate::proto;
use makepad_remote::hub;
use makepad_remote::link::{Client, Target};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

pub const HELP: &str = "  sync words (the master; nodes by hub name, `all` = every node):
  sync status [node...]                         each node's synced checkout, store refs, last apply
  autosync status   what the master's auto-sync last did per node (~/.makepad/devhub/autosync.state)
  sync push <node,...|all> [--makepad <branch>] [--commercial <branch>|--no-commercial]
            [--repo <dir>] [--build on|off] [--restart none|auto|wm|apps]
            [--session <node>/<pid>[:<arg>],... --tick '<inspect cmd, {} = arg>' [--lead <ticks>]]
                                                push branches (default work + commercial main) as signed
                                                bundles, fast-forward the node's checkout, prebuild,
                                                restart; --session: every listed JIT patches at one tick
  rebuild <node,...|all> [wm|apps]              push work + commercial main, prebuild, restart the WM (wm), our
                                                launches (apps) or what the build replaced (default)
  wm-restart <node>                             restart the node's WM (and AI Hub) through its service
  launch <node> --lease <id> [--visible|--hidden] [--lan] [K=V...] <app> [args...]
                                                start <app> (registry name or package) under makepad run on the
                                                node; prints its JIT id (--lease defaults to $DEVHUB_LEASE)
                                                (a lane's TEST instance: badged, not the dock's app, LAN off
                                                unless --lan)
  show <node> <app>                             open the node's WM dock app <app> (launch-or-focus: the user's
                                                one instance, the latest synced tree under the JIT); how a lane
                                                shows the user what it landed
  open <app>                                    on a WM machine's shell: open this machine's dock app <app>
                                                (what clicking it in the dock does)
  restart <node>/<pid>                          stop a launch and start it again (same lease, env, args)
  stop <node>/<pid>                             stop a launch by its JIT id (also <node>/<app>#<pid>)
  launched <node>                               the node's running launches
  launch-log <node>/<pid> [n]                   the tail of a launch's log";

fn control() -> Result<Client, String> {
    let c = Client::new(Target::Unix(proto::control_socket()), "devhub-sync");
    c.conn().map_err(|_| format!("no devhub runs here ({}): start makepad-app-devhub", proto::control_socket().display()))?;
    Ok(c)
}

fn ask(c: &Client, node: &str, service: &str, line: &str) -> Result<Vec<String>, String> {
    let mut got = Vec::new();
    hub::ask(c, node, service, "", line, &mut |l| {
        got.push(l.to_string());
        true
    })?;
    Ok(got)
}

fn is_err(l: &str) -> bool {
    l.starts_with("{\"err\":")
}

fn print_lines(node: &str, lines: &[String]) -> i32 {
    let mut code = 0;
    for l in lines {
        code |= is_err(l) as i32;
        println!("{node} {l}");
    }
    code
}

/// `all`, or a comma list.
fn nodes(c: &Client, spec: &str) -> Result<Vec<String>, String> {
    if spec != "all" {
        return Ok(spec.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect());
    }
    fleet_nodes(c)
}

pub(super) fn fleet_nodes(c: &Client) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for h in ask_local(c, "hosts", "hosts")? {
        let Ok(v) = makepad_strict_json::parse(h.as_bytes()) else { continue };
        if v.get("via").and_then(|x| x.as_str()) == Some("fleet") {
            if let Some(n) = v.get("host").and_then(|x| x.as_str()) {
                out.push(n.to_string());
            }
        }
    }
    Ok(out)
}

pub(super) fn ask_local(c: &Client, service: &str, line: &str) -> Result<Vec<String>, String> {
    let ch = c.open(service, "").map_err(|e| e.to_string())?;
    ch.send(line.as_bytes().to_vec()).map_err(|e| e.to_string())?;
    let mut got = Vec::new();
    loop {
        match ch.recv() {
            Some(makepad_remote::Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    if l.starts_with("{\"end\":") {
                        return Ok(got);
                    }
                    got.push(l.to_string());
                }
            }
            Some(makepad_remote::Event::Opened(_)) => {}
            Some(makepad_remote::Event::Closed(_, why)) => return Err(why),
            None => return Err("the hub connection ended".into()),
        }
    }
}

// ---------------------------------------------------------------- git, read-only, in the user's repo

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = makepad_remote::group::command("git").arg("-C").arg(repo).args(args).stdin(Stdio::null()).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!("git {}: {}", args.first().unwrap_or(&""), proto::clean(String::from_utf8_lossy(&out.stderr).trim(), 300)))
    }
}

pub(super) fn branch_sha(repo: &Path, branch: &str) -> Result<String, String> {
    if !valid_branch(branch) {
        return Err(format!("`{branch}` is not a branch name"));
    }
    git(repo, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}^{{commit}}")]).map_err(|_| format!("{}: no branch {branch}", repo.display()))
}

/// One repo to push: where it is, the branch, its commit.
#[derive(Clone)]
struct Push {
    repo: &'static str,
    dir: PathBuf,
    branch: String,
    sha: String,
}

/// What a node's store holds of `repo` (its refs and its checkout).
fn node_has(status: &str, repo: &str) -> Vec<String> {
    let Ok(v) = makepad_strict_json::parse(status.as_bytes()) else { return Vec::new() };
    let Some(r) = v.get("repos").and_then(|r| r.get(repo)) else { return Vec::new() };
    let mut out: Vec<String> = r.get("refs").and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|x| x.get("sha")?.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if let Some(c) = r.get("checkout").and_then(|c| c.as_str()).filter(|_| r.get("held").and_then(|h| h.as_bool()) != Some(false)) {
        out.push(c.to_string());
    }
    out.retain(|s| valid_sha(s));
    out.sort();
    out.dedup();
    out
}

/// Bundles made for this run, by repo and base set (nodes with the same history share one).
#[derive(Default)]
struct Bundles(Mutex<HashMap<String, Arc<Mutex<Option<Result<(PathBuf, String, u64), String>>>>>>);

impl Bundles {
    fn get(&self, p: &Push, bases: &[String]) -> Result<(PathBuf, String, u64), String> {
        let key = format!("{} {} {}", p.repo, p.sha, bases.join(","));
        let slot = self.0.lock().unwrap().entry(key).or_default().clone();
        let mut slot = slot.lock().unwrap();
        if slot.is_none() {
            *slot = Some(make_bundle(p, bases));
        }
        slot.clone().unwrap()
    }

    fn remove_all(&self) {
        for slot in self.0.lock().unwrap().values() {
            if let Some(Ok((path, _, _))) = slot.lock().unwrap().as_ref() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn make_bundle(p: &Push, bases: &[String]) -> Result<(PathBuf, String, u64), String> {
    // unique per call: parallel node pushes with different bases make bundles of one commit at once
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("devhub-push-{}-{n}-{}-{}.bundle", std::process::id(), p.repo, &p.sha[..12]));
    // the bundle names `refs/heads/<branch>` at exactly `p.sha`, even when the branch has moved
    // since (another landing): it is made in a scratch bare repo that borrows the user repo's
    // objects (alternates), so nothing is written to the user's repo and no ref is read twice
    let scratch = path.with_extension("git");
    let made = (|| {
        let common = git(&p.dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        git(&p.dir, &["init", "-q", "--bare", &scratch.to_string_lossy()])?;
        std::fs::write(scratch.join("objects/info/alternates"), format!("{common}/objects\n")).map_err(|e| format!("{}: {e}", scratch.display()))?;
        git(&scratch, &["update-ref", &format!("refs/heads/{}", p.branch), &p.sha])?;
        let mut args: Vec<String> = vec!["bundle".into(), "create".into(), "-q".into(), path.to_string_lossy().into_owned(), format!("refs/heads/{}", p.branch)];
        args.extend(bases.iter().map(|b| format!("^{b}")));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        git(&scratch, &refs).map(|_| ())
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    made?;
    let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut h = makepad_network::digest::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        size += n as u64;
    }
    Ok((path, sign::hex(&h.finalise()), size))
}

/// Sends one bundle down `node`'s `sync-push`.
fn send_bundle(c: &Client, node: &str, p: &Push, path: &Path, hash: &str, size: u64) -> Result<String, String> {
    let order = sign::sign(&format!("push {} {} {} {hash} {size}", p.repo, p.branch, p.sha))?;
    let ch = c.open("route", &format!("{node} sync-push")).map_err(|e| e.to_string())?;
    ch.send(order.into_bytes()).map_err(|e| e.to_string())?;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 << 10];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        ch.send(buf[..n].to_vec()).map_err(|e| e.to_string())?;
    }
    let mut answer = String::new();
    loop {
        match ch.recv() {
            Some(makepad_remote::Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    if l.starts_with("{\"end\":") {
                        return if is_err(&answer) { Err(answer) } else { Ok(answer) };
                    }
                    answer = l.to_string();
                }
            }
            Some(makepad_remote::Event::Opened(_)) => {}
            Some(makepad_remote::Event::Closed(_, why)) => return Err(if answer.is_empty() { why } else { answer }),
            None => return Err("the hub connection ended".into()),
        }
    }
}

/// Makes sure `node` holds every push's commit; reports what moved. `Ok(true)`: a device's store
/// took the files and its JIT patched already (no apply step follows).
fn push_to(c: &Client, node: &str, pushes: &[Push], bundles: &Bundles, out: &mut dyn FnMut(&str, &str)) -> Result<bool, String> {
    let status = ask(c, node, "sync", "status")?.into_iter().next().ok_or("no status")?;
    if is_err(&status) {
        return Err(status);
    }
    if status.contains("\"apply\":\"store\"") {
        for p in pushes {
            push_files(c, node, &status, p, out)?;
        }
        return Ok(true);
    }
    for p in pushes {
        let has = node_has(&status, p.repo);
        // bases this repo knows; a node that holds the commit (or a descendant) needs no bundle
        let bases: Vec<String> = has.iter().filter(|b| git(&p.dir, &["cat-file", "-e", &format!("{b}^{{commit}}")]).is_ok()).cloned().collect();
        if bases.iter().any(|b| *b == p.sha || git(&p.dir, &["merge-base", "--is-ancestor", &p.sha, b]).is_ok()) {
            out(node, &format!("{} {} {}: already there", p.repo, p.branch, &p.sha[..10]));
            continue;
        }
        let (path, hash, size) = bundles.get(p, &bases)?;
        let t = std::time::Instant::now();
        let r = send_bundle(c, node, p, &path, &hash, size)?;
        out(node, &format!("{} {} {}: pushed {:.1} MB in {:.1} s {r}", p.repo, p.branch, &p.sha[..10], size as f64 / 1048576.0, t.elapsed().as_secs_f64()));
    }
    Ok(false)
}

/// A device's store: the files that differ from its rev (all files under its prefixes on a first
/// push), read from git here, sent as a file tree over `sync-files`.
fn push_files(c: &Client, node: &str, status: &str, p: &Push, out: &mut dyn FnMut(&str, &str)) -> Result<(), String> {
    let v = makepad_strict_json::parse(status.as_bytes()).map_err(|_| "a bad status")?;
    let r = v.get("repos").and_then(|r| r.get(p.repo));
    let from = r.and_then(|r| r.get("checkout")).and_then(|c| c.as_str()).map(str::to_string);
    let prefixes: Vec<String> = r.and_then(|r| r.get("prefixes")).and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if from.as_deref() == Some(p.sha.as_str()) {
        out(node, &format!("{} {} {}: already there", p.repo, p.branch, &p.sha[..10]));
        return Ok(());
    }
    let changes = tree_changes(&p.dir, from.as_deref(), &p.sha, &prefixes)?;
    let payload = super::files::encode(&changes)?;
    let mut h = makepad_network::digest::Sha256::new();
    h.update(&payload);
    let order = sign::sign(&format!("files {} {} {} {} {}", p.repo, from.as_deref().unwrap_or("-"), p.sha, sign::hex(&h.finalise()), payload.len()))?;
    let ch = c.open("route", &format!("{node} sync-files")).map_err(|e| e.to_string())?;
    ch.send(order.into_bytes()).map_err(|e| e.to_string())?;
    for chunk in payload.chunks(256 << 10) {
        ch.send(chunk.to_vec()).map_err(|e| e.to_string())?;
    }
    loop {
        match ch.recv() {
            Some(makepad_remote::Event::Data(d)) => {
                let text = String::from_utf8_lossy(&d).into_owned();
                if let Some(l) = text.lines().find(|l| !l.starts_with("{\"end\":") && !l.is_empty()) {
                    out(node, &format!("{} {} {}: {} files -> store {l}", p.repo, p.branch, &p.sha[..10], changes.len()));
                    return if is_err(l) { Err(l.to_string()) } else { Ok(()) };
                }
            }
            Some(makepad_remote::Event::Opened(_)) => {}
            Some(makepad_remote::Event::Closed(_, why)) => return Err(why),
            None => return Err("the hub connection ended".into()),
        }
    }
}

/// The changes from `from` (None: nothing) to `to` under `prefixes` (empty: everything).
pub(super) fn tree_changes(repo: &Path, from: Option<&str>, to: &str, prefixes: &[String]) -> Result<Vec<super::files::Change>, String> {
    let mut args: Vec<String> = match from {
        Some(f) => {
            if git(repo, &["merge-base", "--is-ancestor", f, to]).is_err() {
                return Err(format!("the device holds {}, which is not an ancestor of {}", &f[..f.len().min(10)], &to[..10]));
            }
            vec!["diff".into(), "--name-status".into(), "--no-renames".into(), "-z".into(), f.into(), to.into()]
        }
        None => vec!["ls-tree".into(), "-r".into(), "-z".into(), "--name-only".into(), to.into()],
    };
    args.push("--".into());
    args.extend(prefixes.iter().cloned());
    let out = makepad_remote::group::command("git").arg("-C").arg(repo).args(&args).stdin(Stdio::null()).output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args[0], proto::clean(String::from_utf8_lossy(&out.stderr).trim(), 300)));
    }
    let fields: Vec<String> = out.stdout.split(|b| *b == 0).filter(|f| !f.is_empty()).map(|f| String::from_utf8_lossy(f).into_owned()).collect();
    let mut wanted: Vec<(String, bool)> = Vec::new();
    if from.is_some() {
        for pair in fields.chunks(2) {
            if let [st, path] = pair {
                wanted.push((path.clone(), st.starts_with('D')));
            }
        }
    } else {
        wanted.extend(fields.into_iter().map(|p| (p, false)));
    }
    // the contents of every written file through one `cat-file --batch`
    let mut child = makepad_remote::group::command("git").arg("-C").arg(repo).args(["cat-file", "--batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("git: {e}"))?;
    let mut input = String::new();
    for (path, deleted) in &wanted {
        if !deleted {
            input.push_str(&format!("{to}:{path}\n"));
        }
    }
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut stdout).map_err(|e| e.to_string())?;
    let _ = writer.join();
    let _ = child.wait();
    let mut rest = &stdout[..];
    let mut changes = Vec::new();
    for (path, deleted) in wanted {
        if deleted {
            changes.push((path, None));
            continue;
        }
        let nl = rest.iter().position(|b| *b == b'\n').ok_or("cat-file: short output")?;
        let head = String::from_utf8_lossy(&rest[..nl]).into_owned();
        rest = &rest[nl + 1..];
        let mut w = head.split(' ');
        let (kind, size) = (w.nth(1).unwrap_or(""), w.next().and_then(|s| s.parse::<usize>().ok()));
        let Some(size) = size.filter(|_| kind == "blob") else {
            // a submodule or a symlink-free tree entry that is not a blob: skipped
            continue;
        };
        changes.push((path, Some(rest[..size].to_vec())));
        rest = &rest[size + 1..];
    }
    Ok(changes)
}

/// For leases (DESIGN §11): makes sure `node`'s store holds commit `sha` (pinned from `branch`
/// when the lease was ordered) of the repo at `dir` (`repo` = makepad | commercial), pushing a
/// signed bundle when it does not; moves no checkout. Returns the commit. The lease then makes its worktree from the node's store
/// ([`super::store_dir`]).
pub fn ensure_rev(control: &Client, node: &str, repo: &str, dir: &Path, branch: &str, sha: &str) -> Result<String, String> {
    let repo: &'static str = match repo {
        "makepad" => "makepad",
        "commercial" => "commercial",
        _ => return Err(format!("no repo `{repo}`")),
    };
    if !valid_branch(branch) || !valid_sha(sha) {
        return Err(format!("`{branch}@{sha}` is not branch@sha"));
    }
    git(dir, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).map_err(|_| format!("{}: no commit {sha}", dir.display()))?;
    let p = Push { repo, dir: dir.to_path_buf(), branch: branch.to_string(), sha: sha.to_string() };
    let bundles = Bundles::default();
    let r = push_to(control, node, std::slice::from_ref(&p), &bundles, &mut |n, l| println!("{n} {l}"));
    bundles.remove_all();
    r.map(|_| p.sha)
}

fn flag<'a>(args: &'a [&'a str], name: &str) -> Option<&'a str> {
    args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1).copied())
}

/// Pushes and applies without a session. An empty node names a run-wide progress line.
/// Worker output reaches `out` on the calling thread, so the CLI still shows live progress.
pub(super) fn push_and_apply(c: &Client, repo: &Path, makepad_branch: &str, commercial: Option<&str>, nodes: &[String], build: &str, restart: &str, out: &mut dyn FnMut(&str, &str)) -> i32 {
    let mut pushes = Vec::new();
    for (name, dir, branch) in [("makepad", repo.to_path_buf(), Some(makepad_branch)), ("commercial", repo.join("apps/commercial"), commercial)] {
        let Some(branch) = branch else { continue };
        match branch_sha(&dir, branch) {
            Ok(sha) => pushes.push(Push { repo: name, dir, branch: branch.to_string(), sha }),
            Err(e) => {
                for n in nodes {
                    out(n, &format!("{{\"err\":{}}}", makepad_remote::json_str(&e)));
                }
                return 2;
            }
        }
    }
    let revs: String = pushes.iter().map(|p| format!(" {}={}", p.repo, p.sha)).collect();
    for p in &pushes {
        out("", &format!("pushing {} {} {}", p.repo, p.branch, &p.sha[..10]));
    }
    let bundles = Bundles::default();
    // 1. bundles to every node, in parallel (the watcher passes just one node)
    let results = std::thread::scope(|s| {
        let (tx, rx) = std::sync::mpsc::channel();
        let hs: Vec<_> = nodes.iter().map(|n| {
            let (pushes, bundles) = (&pushes, &bundles);
            let tx = tx.clone();
            (n, s.spawn(move || push_to(c, n, pushes, bundles, &mut |n, l| { let _ = tx.send((n.to_string(), l.to_string())); })))
        }).collect();
        drop(tx);
        for (n, l) in rx {
            out(&n, &l);
        }
        hs.into_iter().map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("panicked".into())))).collect::<Vec<_>>()
    });
    bundles.remove_all();
    let mut code = 0;
    let mut ready = Vec::new();
    for (n, r) in results {
        match r {
            Ok(false) => ready.push(n),
            Ok(true) => {}
            Err(e) => {
                out(n, &format!("{{\"err\":{}}}", makepad_remote::json_str(&e)));
                code = 1;
            }
        }
    }
    // 2. apply, keeping the node's fast-forward check and restart policy
    let words = format!("apply{revs} build={build} restart={restart}");
    let answers = std::thread::scope(|s| {
        let hs: Vec<_> = ready.into_iter().map(|n| {
            let words = &words;
            (n, s.spawn(move || {
                let order = sign::sign(words)?;
                ask(c, n, "sync", &format!("apply {order}"))
            }))
        }).collect();
        hs.into_iter().map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("panicked".into())))).collect::<Vec<_>>()
    });
    for (n, r) in answers {
        match r {
            Ok(lines) => for l in lines {
                code |= is_err(&l) as i32;
                out(n, &l);
            },
            Err(e) => {
                out(n, &format!("{{\"err\":{}}}", makepad_remote::json_str(&e)));
                code = 1;
            }
        }
    }
    code
}

fn sync_push(c: &Client, args: &[&str]) -> i32 {
    let Some(spec) = args.first() else {
        eprintln!("{HELP}");
        return 2;
    };
    let repo = match flag(args, "--repo") {
        Some(r) => PathBuf::from(r),
        None => match git(&std::env::current_dir().unwrap_or_default(), &["rev-parse", "--show-toplevel"]) {
            Ok(t) => PathBuf::from(t),
            Err(e) => {
                eprintln!("makepad devhub sync: {e} (run it in the makepad checkout, or --repo <dir>)");
                return 2;
            }
        },
    };
    let mut pushes = Vec::new();
    let mut add = |repo_name: &'static str, dir: PathBuf, branch: &str| match branch_sha(&dir, branch) {
        Ok(sha) => {
            pushes.push(Push { repo: repo_name, dir, branch: branch.to_string(), sha });
            Ok(())
        }
        Err(e) => Err(e),
    };
    if let Err(e) = add("makepad", repo.clone(), flag(args, "--makepad").unwrap_or("work")) {
        eprintln!("makepad devhub sync: {e}");
        return 2;
    }
    if !args.contains(&"--no-commercial") {
        if let Err(e) = add("commercial", repo.join("apps").join("commercial"), flag(args, "--commercial").unwrap_or("main")) {
            eprintln!("makepad devhub sync: {e} (or --no-commercial)");
            return 2;
        }
    }
    let nodes = match nodes(c, spec) {
        Ok(n) if !n.is_empty() => n,
        Ok(_) => {
            eprintln!("makepad devhub sync: no nodes");
            return 2;
        }
        Err(e) => {
            eprintln!("makepad devhub sync: {e}");
            return 2;
        }
    };
    let build = flag(args, "--build").unwrap_or("on");
    let restart = flag(args, "--restart").unwrap_or("none");
    if !matches!(restart, "none" | "auto" | "wm" | "apps") || !matches!(build, "on" | "off") {
        eprintln!("{HELP}");
        return 2;
    }
    let mut out = |n: &str, l: &str| {
        if n.is_empty() { println!("{l}"); } else { println!("{n} {l}"); }
    };
    let Some(session) = flag(args, "--session") else {
        let commercial = (!args.contains(&"--no-commercial")).then(|| flag(args, "--commercial").unwrap_or("main"));
        return push_and_apply(c, &repo, flag(args, "--makepad").unwrap_or("work"), commercial, &nodes, build, restart, &mut out);
    };
    let revs: String = pushes.iter().map(|p| format!(" {}={}", p.repo, p.sha)).collect();
    for p in &pushes {
        out("", &format!("pushing {} {} {}", p.repo, p.branch, &p.sha[..10]));
    }
    let bundles = Arc::new(Bundles::default());
    let pushes = Arc::new(pushes);
    // 1. bundles to every node, in parallel
    let results: Vec<(String, Result<bool, String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = nodes.iter().map(|n| {
            let (pushes, bundles, n2) = (pushes.clone(), bundles.clone(), n.clone());
            let c = c.clone();
            (n.clone(), s.spawn(move || push_to(&c, &n2, &pushes, &bundles, &mut |n, l| out(n, l))))
        }).collect();
        hs.into_iter().map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("panicked".into())))).collect()
    });
    bundles.remove_all();
    let mut code = 0;
    let ready: Vec<String> = results
        .into_iter()
        .filter_map(|(n, r)| match r {
            Ok(false) => Some(n),
            Ok(true) => None,
            Err(e) => {
                out(&n, &format!("{{\"err\":{}}}", makepad_remote::json_str(&e)));
                code = 1;
                None
            }
        })
        .collect();
    // 2. apply at one session tick
    // `<node>/<pid>[:<arg>]`: `{}` in the tick command is the JIT's own arg (its probe id)
    let jits: Vec<_> = session.split(',').filter_map(|j| {
        let (j, arg) = j.split_once(':').map_or((j, ""), |(a, b)| (a, b));
        let (n, p) = j.split_once('/')?;
        Some((n.to_string(), p.split('#').last()?.parse::<u32>().ok()?, arg.to_string()))
    }).collect();
    let Some(tick_cmd) = flag(args, "--tick") else {
        eprintln!("makepad devhub sync: --session needs --tick '<inspect command that answers the tick>'");
        return 2;
    };
    let lead: u64 = flag(args, "--lead").and_then(|l| l.parse().ok()).unwrap_or(180);
    let hexcmd = |arg: &str| sign::hex(tick_cmd.replace("{}", arg).as_bytes());
    let mut ticks = Vec::new();
    for (n, pid, arg) in &jits {
        match ask(c, n, "sync", &format!("tick {pid} {}", hexcmd(arg))).map(|l| l.into_iter().next().unwrap_or_default()) {
            Ok(l) if !is_err(&l) => match super::jits::tick_of(&l) {
                Some(t) => ticks.push(t),
                None => {
                    out(n, &l);
                    return 1;
                }
            },
            Ok(l) => {
                out(n, &l);
                return 1;
            }
            Err(e) => {
                out(n, &format!("{{\"err\":{}}}", makepad_remote::json_str(&e)));
                return 1;
            }
        }
    }
    let at = ticks.iter().max().copied().unwrap_or(0) + lead;
    let id = format!("s{}", proto::wall_ms() % 1_000_000_000);
    out("", &format!("session {id}: ticks {ticks:?}: every JIT patches at tick {at}"));
    let orders: Vec<(String, String)> = jits.iter().filter(|(n, _, _)| ready.contains(n)).map(|(n, pid, arg)| (n.clone(), format!("apply{revs} build={build} restart={restart} session={id} at={at} jit={pid} tickcmd={}", hexcmd(arg)))).collect();
    let answers: Vec<(String, Result<Vec<String>, String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = orders.iter().map(|(n, words)| {
            let c = c.clone();
            (n.clone(), s.spawn(move || -> Result<Vec<String>, String> {
                let order = sign::sign(words)?;
                ask(&c, n, "sync", &format!("apply {order}"))
            }))
        }).collect();
        hs.into_iter().map(|(n, h)| (n, h.join().unwrap_or_else(|_| Err("panicked".into())))).collect()
    });
    for (n, r) in answers {
        match r {
            Ok(lines) => for l in lines {
                code |= is_err(&l) as i32;
                out(&n, &l);
            },
            Err(e) => {
                out(&n, &format!("{{\"err\":{}}}", makepad_remote::json_str(&e)));
                code = 1;
            }
        }
    }
    code
}

/// `launch` arguments as the node's `launch` line: (node, words). Flags go anywhere before the
/// app: `--lease L3`, `--lease=L3` and `lease=L3` are one flag; without one, `DEVHUB_LEASE`
/// (what `devhub lease` prints to export) is used.
fn launch_words(args: &[&str]) -> Result<(String, String), String> {
    let mut node = None;
    let mut lease = None;
    let mut words = vec!["launch".to_string()];
    let mut i = 0;
    let mut app = false;
    while i < args.len() {
        let w = args[i];
        if app {
            words.push(w.to_string());
        } else {
            match w {
                "--visible" => words.push("visible".into()),
                "--hidden" => words.push("hidden".into()),
                "--lan" => words.push("lan".into()),
                "--lease" => {
                    i += 1;
                    lease = Some(args.get(i).ok_or("--lease needs a lease id")?.to_string());
                }
                "--" => {}
                w if w.starts_with("--lease=") => lease = Some(w["--lease=".len()..].to_string()),
                w if w.starts_with("lease=") => lease = Some(w["lease=".len()..].to_string()),
                w if w.starts_with("--") => return Err(format!("unknown flag `{w}`")),
                w if node.is_none() => node = Some(w.to_string()),
                // K=V env words, then the app and its args
                w => {
                    app = !(w.contains('=') && w.split('=').next().is_some_and(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')));
                    words.push(w.to_string());
                }
            }
        }
        i += 1;
    }
    let node = node.ok_or("which node?")?;
    if !app {
        return Err("which app?".into());
    }
    let lease = lease.or_else(|| std::env::var("DEVHUB_LEASE").ok().filter(|l| !l.is_empty())).ok_or("needs a write lease: --lease <id> (take one with `makepad devhub lease screen --node <node>`)")?;
    words.insert(1, format!("lease={lease}"));
    Ok((node, words.join(" ")))
}

/// `<node>/<pid>` or `<node>/<app>#<pid>`.
fn jit_id(s: &str) -> Option<(String, u32)> {
    let (n, rest) = s.split_once('/')?;
    Some((n.to_string(), rest.rsplit('#').next()?.parse().ok()?))
}

pub fn main(args: &[&str]) -> Option<i32> {
    let verb = *args.first()?;
    if !matches!(verb, "sync" | "autosync" | "launch" | "show" | "open" | "stop" | "launched" | "launch-log" | "wm-restart" | "rebuild" | "restart") {
        return None;
    }
    if args.len() < 2 || matches!(args[1], "-h" | "--help" | "help") {
        println!("{HELP}");
        return Some(if args.len() < 2 { 2 } else { 0 });
    }
    // Last successful applies remain readable when the master is stopped.
    if let ("autosync", ["status"]) = (verb, &args[1..]) {
        return Some(match std::fs::read_to_string(proto::home().join("autosync.state")) {
            Ok(text) => { print!("{text}"); 0 }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => { println!("no auto-sync state yet"); 0 }
            Err(e) => { eprintln!("makepad devhub autosync: {e}"); 1 }
        });
    }
    // the dock's app on this machine, from its own shell: no hub, no master
    if let ("open", [app]) = (verb, &args[1..]) {
        return Some(match super::launch::show(app) {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("makepad devhub open: {e}");
                1
            }
        });
    }
    let c = match control() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("makepad devhub: {e}");
            return Some(2);
        }
    };
    let out = |node: &str, r: Result<Vec<String>, String>| match r {
        Ok(l) => print_lines(node, &l),
        Err(e) => {
            eprintln!("makepad devhub: {node}: {e}");
            2
        }
    };
    Some(match (verb, &args[1..]) {
        ("sync", ["status", rest @ ..]) => {
            let ns = if rest.is_empty() { nodes(&c, "all") } else { Ok(rest.iter().map(|s| s.to_string()).collect()) };
            match ns {
                Ok(ns) => ns.iter().map(|n| out(n, ask(&c, n, "sync", "status"))).fold(0, |a, b| a | b),
                Err(e) => {
                    eprintln!("makepad devhub: {e}");
                    2
                }
            }
        }
        ("sync", ["push", rest @ ..]) => sync_push(&c, rest),
        ("wm-restart", [node]) => match sign::sign("wm-restart") {
            Ok(order) => out(node, ask(&c, node, "sync", &format!("wm-restart {order}"))),
            Err(e) => {
                eprintln!("makepad devhub: {e}");
                2
            }
        },
        ("launch", rest) => match launch_words(rest) {
            Ok((node, words)) => out(&node, ask(&c, &node, "launch", &words)),
            Err(e) => {
                eprintln!("makepad devhub launch: {e}\n{HELP}");
                2
            }
        },
        ("restart", [id]) => match jit_id(id) {
            Some((n, pid)) => out(&n, ask(&c, &n, "launch", &format!("relaunch {pid}"))),
            None => {
                eprintln!("makepad devhub restart <node>/<pid>");
                2
            }
        },
        ("rebuild", [node, what @ ..]) => {
            let restart = match what {
                [] => "auto",
                ["wm"] => "wm",
                ["apps"] => "apps",
                _ => {
                    eprintln!("makepad devhub rebuild <node,...|all> [wm|apps]");
                    return Some(2);
                }
            };
            sync_push(&c, &[node, "--build", "on", "--restart", restart])
        }
        ("stop", [id]) => match jit_id(id) {
            Some((n, pid)) => out(&n, ask(&c, &n, "launch", &format!("stop {pid}"))),
            None => {
                eprintln!("makepad devhub stop <node>/<pid>");
                2
            }
        },
        ("launched", [node]) => out(node, ask(&c, node, "launch", "list")),
        ("show", [node, app]) => out(node, ask(&c, node, "launch", &format!("show {app}"))),
        ("launch-log", [id, rest @ ..]) => match jit_id(id) {
            Some((n, pid)) => out(&n, ask(&c, &n, "launch", &format!("log {pid} {}", rest.first().unwrap_or(&"40")))),
            None => {
                eprintln!("makepad devhub launch-log <node>/<pid> [n]");
                2
            }
        },
        _ => {
            eprintln!("{HELP}");
            2
        }
    })
}

#[cfg(test)]
mod tests {
    /// The master's side of a device push, from a real git repo: a first push is the tree under
    /// the prefixes, the next one the diff (writes and deletes).
    #[test]
    fn tree_changes_from_git() {
        let d = std::env::temp_dir().join(format!("devhub-sync-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("apps/game")).unwrap();
        std::fs::create_dir_all(d.join("other")).unwrap();
        let g = |args: &[&str]| super::git(&d, &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "work"]);
        std::fs::write(d.join("apps/game/a.rs"), "a1").unwrap();
        std::fs::write(d.join("apps/game/b.rs"), "b1").unwrap();
        std::fs::write(d.join("other/x.rs"), "x").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "1"]);
        let one = g(&["rev-parse", "HEAD"]);
        std::fs::write(d.join("apps/game/a.rs"), "a2").unwrap();
        std::fs::remove_file(d.join("apps/game/b.rs")).unwrap();
        std::fs::write(d.join("other/x.rs"), "y").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-q", "-m", "2"]);
        let two = g(&["rev-parse", "HEAD"]);
        let pre = vec!["apps/game".to_string()];
        let first = super::tree_changes(&d, None, &one, &pre).unwrap();
        assert_eq!(first, vec![("apps/game/a.rs".to_string(), Some(b"a1".to_vec())), ("apps/game/b.rs".to_string(), Some(b"b1".to_vec()))]);
        let next = super::tree_changes(&d, Some(&one), &two, &pre).unwrap();
        assert_eq!(next, vec![("apps/game/a.rs".to_string(), Some(b"a2".to_vec())), ("apps/game/b.rs".to_string(), None)]);
        assert!(super::tree_changes(&d, Some(&two), &one, &pre).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The gate race: a lease pins `rc-x@<sha>` at order time, then another landing moves `rc-x`
    /// before the master bundles it. The bundle still holds exactly the pinned commit (the node's
    /// check "the bundle's rc-x is .., the order names .." passes), on the user repo nothing is
    /// written, and the pin itself comes from the CLI's rev parsing.
    #[test]
    fn a_moved_branch_ships_the_pinned_commit() {
        let d = std::env::temp_dir().join(format!("devhub-sync-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("apps/commercial")).unwrap();
        let g = |dir: &std::path::Path, args: &[&str]| super::git(dir, &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        let c = d.join("apps/commercial");
        for r in [&d, &c] {
            g(r, &["init", "-q", "-b", "main"]);
            std::fs::write(r.join("f"), "1").unwrap();
            g(r, &["add", "f"]);
            g(r, &["commit", "-q", "-m", "1"]);
        }
        g(&d, &["branch", "rc-x"]);
        let spec = crate::lease::pin_revs(&d, "rc-x,main").unwrap();
        let (mk, com) = crate::lease::parse_pinned(&spec).unwrap();
        assert_eq!((mk.branch.as_str(), mk.sha.as_str()), ("rc-x", g(&d, &["rev-parse", "rc-x"]).as_str()));
        assert_eq!(com.unwrap().sha, g(&c, &["rev-parse", "main"]));
        // an explicit short pin expands; a bare name is not accepted past the CLI
        assert_eq!(crate::lease::pin_revs(&d, &format!("rc-x@{}", &mk.sha[..10])).unwrap(), format!("rc-x@{}", mk.sha));
        assert!(crate::lease::parse_pinned("rc-x,main").is_err());
        assert!(crate::lease::pin_revs(&d, "no-such").is_err());
        // another landing moves rc-x
        g(&d, &["checkout", "-q", "rc-x"]);
        std::fs::write(d.join("f"), "2").unwrap();
        g(&d, &["commit", "-q", "-am", "2"]);
        let moved = g(&d, &["rev-parse", "rc-x"]);
        assert_ne!(moved, mk.sha);
        let refs_before = g(&d, &["for-each-ref"]);
        let p = super::Push { repo: "makepad", dir: d.clone(), branch: "rc-x".into(), sha: mk.sha.clone() };
        let (bundle, _, _) = super::make_bundle(&p, &[]).unwrap();
        let heads = g(&d, &["bundle", "list-heads", &bundle.to_string_lossy()]);
        assert_eq!(heads, format!("{} refs/heads/rc-x", mk.sha));
        assert_eq!(g(&d, &["for-each-ref"]), refs_before);
        // an incremental bundle against a base the node has
        let p2 = super::Push { sha: moved.clone(), ..p };
        let (inc, _, _) = super::make_bundle(&p2, std::slice::from_ref(&mk.sha)).unwrap();
        assert_eq!(g(&d, &["bundle", "list-heads", &inc.to_string_lossy()]), format!("{moved} refs/heads/rc-x"));
        // two gate leases ordered at once for this commit bundle it at the same moment (FABIO: they
        // collided on git's `<bundle>.lock`, one never started): every call gets its own path
        let made: Vec<_> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..8).map(|_| s.spawn(|| super::make_bundle(&super::Push { repo: "makepad", dir: d.clone(), branch: "rc-x".into(), sha: mk.sha.clone() }, &[]))).collect();
            hs.into_iter().map(|h| h.join().unwrap().unwrap()).collect()
        });
        let mut paths: Vec<_> = made.iter().map(|(p, _, _)| p.clone()).collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), 8);
        for p in &paths {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::remove_file(&bundle);
        let _ = std::fs::remove_file(&inc);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn node_store_listing() {
        let st = r#"{"machine":"217","repos":{"makepad":{"checkout":"d8cb46d6c1a69041cd12c5822fc2f89a46fedc81","refs":[{"ref":"refs/devhub/pushed/work","sha":"374efe9d7f0000000000000000000000000000aa"}]},"commercial":{"checkout":null,"refs":[]}}}"#;
        assert_eq!(super::node_has(st, "makepad").len(), 2);
        assert!(super::node_has(st, "commercial").is_empty());
        let shipped = r#"{"repos":{"makepad":{"checkout":"d8cb46d6c1a69041cd12c5822fc2f89a46fedc81","held":false,"refs":[]}}}"#;
        assert!(super::node_has(shipped, "makepad").is_empty());
        assert_eq!(super::jit_id("217/sandbox#4412"), Some(("217".to_string(), 4412)));
        assert_eq!(super::jit_id("100/981"), Some(("100".to_string(), 981)));
    }
}
