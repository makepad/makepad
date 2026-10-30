//! `devhub after <condition> -- <devhub words>`: the master runs the words once the condition
//! holds (replacing the manager's waiter scripts). Waiters are state: they survive a master
//! restart, and each one fires once.
//!
//! Conditions: `landed <branch>` (in makepad `work`, or commercial `main` for a commercial
//! branch; the branch may be deleted after landing: its last seen tip counts), `free <node>` (up,
//! on for dev, no lease held or waiting), `verified [<commit|branch>]` (a verify passes after the
//! waiter was added; with a commit: a verified pair that contains it).
//!
//! Actions are devhub words: `launch mac <package> [args]` (the user's Mac: the master's own
//! launch, `cargo run --profile host -p makepad-run --bin makepad -- run <package>` in the
//! checkout), or any other `makepad devhub` command (`launch <node> ...`, `verify kick`, ...).

use super::{git, is_ancestor, Ctx};
use crate::proto::{wall_ms, Sev};
use crate::state::{Cond, Waiter};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const TICK: Duration = Duration::from_secs(5);

pub fn start(ctx: Arc<Ctx>) {
    // actions a previous master was running: their outcome is unknown
    let lost: Vec<String> = ctx.store.update(|d| {
        let mut ids = Vec::new();
        for w in d.after.iter_mut().filter(|w| w.state == "running") {
            w.state = "failed".into();
            w.note = "the master restarted while it ran".into();
            ids.push(w.id.clone());
        }
        ids
    });
    for id in lost {
        ctx.store.event("after", Sev::Warn, format!("after {id}: its action was running when the master restarted (outcome unknown)"), None);
    }
    let _ = std::thread::Builder::new().name("devhub-after".into()).spawn(move || loop {
        tick(&ctx);
        ctx.store.wait(TICK);
    });
}

/// Fires every waiter whose condition holds now.
fn tick(ctx: &Arc<Ctx>) {
    let waiting: Vec<Waiter> = ctx.store.read(|d| d.after.iter().filter(|w| w.state == "waiting").cloned().collect());
    for w in waiting {
        let (met, cond) = check(ctx, &w);
        if cond != w.cond {
            ctx.store.update(|d| {
                if let Some(x) = d.after.iter_mut().find(|x| x.id == w.id) {
                    x.cond = cond.clone();
                }
            });
        }
        let Some(note) = met else { continue };
        let fired = ctx.store.update(|d| match d.after.iter_mut().find(|x| x.id == w.id && x.state == "waiting") {
            Some(x) => {
                x.state = "running".into();
                x.fired_ms = wall_ms();
                x.note = note.clone();
                true
            }
            None => false,
        });
        if !fired {
            continue;
        }
        ctx.store.event("after", Sev::Info, format!("after {}: {} ({note}) -> {}", w.id, cond.describe(), w.cmd.join(" ")), None);
        let (ctx, w) = (ctx.clone(), w.clone());
        let _ = std::thread::Builder::new().name("devhub-after-run".into()).spawn(move || act(&ctx, &w));
    }
}

/// Whether the condition holds (`Some(what met it)`), and the condition with what it learned (a
/// branch's newest tip).
fn check(ctx: &Ctx, w: &Waiter) -> (Option<String>, Cond) {
    match &w.cond {
        Cond::Landed { branch, repo, sha } => {
            let (dir, target) = if repo == "commercial" { (ctx.commercial(), "refs/heads/main") } else { (ctx.repo.clone(), "refs/heads/work") };
            let tip = git(&dir, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}^{{commit}}")]).unwrap_or_else(|_| sha.clone());
            let cond = Cond::Landed { branch: branch.clone(), repo: repo.clone(), sha: tip.clone() };
            let met = (!tip.is_empty() && is_ancestor(&dir, &tip, target)).then(|| format!("{branch}@{} is in {}", super::short(&tip), if repo == "commercial" { "main" } else { "work" }));
            (met, cond)
        }
        Cond::Free { node } => {
            let met = ctx.leases.nodes().into_iter().find(|n| &n.name == node).filter(|n| n.up && n.dev && n.leases == 0 && n.waiting == 0 && n.hold.is_none()).map(|_| format!("{node} is free"));
            (met, w.cond.clone())
        }
        Cond::Verified { contains } => {
            let tips: Vec<crate::state::Tip> = ctx.store.read(|d| d.verify.verified.iter().filter(|t| t.at_ms >= w.added_ms).cloned().collect());
            let hit = tips.iter().rev().find(|t| match contains {
                None => true,
                Some(c) => is_ancestor(&ctx.repo, c, &t.pair.work) || is_ancestor(&ctx.commercial(), c, &t.pair.main),
            });
            (hit.map(|t| format!("verified {}", t.pair.short())), w.cond.clone())
        }
        Cond::None => (None, Cond::None),
    }
}

/// Runs the waiter's words; its exit and log are state.
fn act(ctx: &Ctx, w: &Waiter) {
    let dir = ctx.logs("after");
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join(format!("{}.log", w.id));
    let r = match w.cmd.as_slice() {
        [launch, node, rest @ ..] if launch == "launch" && is_mac(ctx, node) => mac_launch(ctx, rest, &log),
        words => devhub(ctx, words, &w.id, &log),
    };
    let (state, exit, note, sev) = match r {
        Ok((0, note)) => ("done", Some(0), note, Sev::Info),
        Ok((code, note)) => ("failed", Some(code), note, Sev::Warn),
        Err(e) => ("failed", None, e, Sev::Warn),
    };
    ctx.store.update(|d| {
        if let Some(x) = d.after.iter_mut().find(|x| x.id == w.id) {
            x.state = state.into();
            x.exit = exit;
            x.note = note.clone();
            x.log = log.to_string_lossy().into_owned();
        }
    });
    ctx.store.event("after", sev, format!("after {} {state}: {} (exit {}){}", w.id, w.cmd.join(" "), exit.map_or("-".into(), |c| c.to_string()), if note.is_empty() { String::new() } else { format!(": {note}") }), None);
}

/// The user's Mac: `mac`, `local` or the master's own name.
fn is_mac(ctx: &Ctx, node: &str) -> bool {
    matches!(node, "mac" | "local") || node == ctx.machine
}

fn cargo() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let c = home.join(".cargo/bin/cargo");
    if c.exists() {
        c
    } else {
        PathBuf::from("cargo")
    }
}

/// The master's own launch: the package under the JIT from the checkout (as the Dock launchers
/// do), detached, its output in `~/Library/Logs/makepad/<package>.log`; the launch's pid.
fn mac_launch(ctx: &Ctx, words: &[String], log: &std::path::Path) -> Result<(i32, String), String> {
    let (pkg, args) = words.split_first().ok_or("launch mac <package> [args...]")?;
    if pkg.is_empty() || !pkg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(format!("launch mac: `{pkg}` is not a package name"));
    }
    let logs = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library/Logs/makepad");
    let _ = std::fs::create_dir_all(&logs);
    let app_log = logs.join(format!("{pkg}.log"));
    let out = std::fs::OpenOptions::new().create(true).append(true).open(&app_log).map_err(|e| format!("{}: {e}", app_log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = makepad_remote::group::command(cargo());
    cmd.args(["run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "run", pkg]).args(args).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| format!("cargo: {e}"))?;
    let pid = child.id();
    let _ = std::fs::write(log, format!("launch mac {pkg}: pid {pid}, log {}\n", app_log.display()));
    // reaped when it ends (said on the stream)
    let (store, pkg2) = (ctx.store.clone(), pkg.clone());
    let _ = std::thread::Builder::new().name("devhub-after-launch".into()).spawn(move || {
        let st = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        store.event("after", if st == 0 { Sev::Info } else { Sev::Warn }, format!("launch mac {pkg2}#{pid} ended (exit {st})"), None);
    });
    Ok((0, format!("{pkg} launched on the Mac as pid {pid} (log {})", app_log.display())))
}

/// `makepad devhub <words>` with the master's own binary, in the checkout.
fn devhub(ctx: &Ctx, words: &[String], id: &str, log: &std::path::Path) -> Result<(i32, String), String> {
    let exe = std::env::current_exe().map_err(|e| format!("this binary: {e}"))?;
    let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let st = makepad_remote::group::command(exe)
        .arg("devhub")
        .args(words)
        .current_dir(&ctx.repo)
        .env("DEVHUB_AGENT", format!("after-{}", id.to_lowercase()))
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        .status()
        .map_err(|e| format!("devhub: {e}"))?;
    let tail = std::fs::read_to_string(log).unwrap_or_default();
    let last = tail.lines().rev().find(|l| !l.trim().is_empty()).map(|l| crate::proto::clean(l, 200)).unwrap_or_default();
    Ok((st.code().unwrap_or(-1), last))
}

/// A condition from its words: `landed <branch>`, `free <node>`, `verified [<rev>]`.
pub fn parse_cond(ctx: &Ctx, words: &[&str]) -> Result<Cond, String> {
    match words {
        ["landed", branch] => {
            let valid = !branch.is_empty() && !branch.starts_with('-') && branch.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b));
            if !valid {
                return Err(format!("`{branch}` is not a branch name"));
            }
            let has = |dir: &std::path::Path| git(dir, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}^{{commit}}")]).ok();
            if let Some(sha) = has(&ctx.repo) {
                return Ok(Cond::Landed { branch: branch.to_string(), repo: "makepad".into(), sha });
            }
            if let Some(sha) = has(&ctx.commercial()) {
                return Ok(Cond::Landed { branch: branch.to_string(), repo: "commercial".into(), sha });
            }
            Err(format!("no branch {branch} in {} or its apps/commercial", ctx.repo.display()))
        }
        ["free", node] => {
            if !ctx.leases.nodes().iter().any(|n| n.name == *node) {
                return Err(format!("no node {node} (devhub nodes)"));
            }
            Ok(Cond::Free { node: node.to_string() })
        }
        ["verified"] => Ok(Cond::Verified { contains: None }),
        ["verified", rev] => {
            let sha = git(&ctx.repo, &["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")]).or_else(|_| git(&ctx.commercial(), &["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")])).map_err(|_| format!("no commit {rev}"))?;
            Ok(Cond::Verified { contains: Some(sha) })
        }
        _ => Err("conditions: landed <branch> | free <node> | verified [<commit|branch>]".into()),
    }
}

/// `after list [all]`, `after add {"by","cond":[words],"cmd":[words]}`, `after cancel <id>`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let w: Vec<&str> = line.split_whitespace().skip(1).collect();
    match w.as_slice() {
        [] | ["list"] | ["list", "all"] => {
            let all = w.get(1) == Some(&"all");
            let v = ctx.store.read(|d| d.to_value());
            let list: Vec<String> = v.get("after").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter(|x| all || matches!(x.get("state").and_then(|s| s.as_str()), Some("waiting" | "running"))).map(|x| x.to_json()).collect();
            Ok(format!("[{}]", list.join(",")))
        }
        ["cancel", id] => {
            let ok = ctx.store.update(|d| match d.after.iter_mut().find(|x| x.id == *id && x.state == "waiting") {
                Some(x) => {
                    x.state = "cancelled".into();
                    true
                }
                None => false,
            });
            if !ok {
                return Err(format!("no waiting {id}"));
            }
            ctx.store.event("after", Sev::Info, format!("after {id} cancelled"), None);
            Ok(format!("{{\"id\":{},\"state\":\"cancelled\"}}", makepad_remote::json_str(id)))
        }
        ["add", ..] => {
            // `add {"by":..,"cond":[..],"cmd":[..]}`: words keep their spaces
            let json = line.splitn(3, ' ').nth(2).unwrap_or("");
            let v = makepad_strict_json::parse(json.as_bytes()).map_err(|_| "after add: bad request")?;
            let list = |k: &str| -> Vec<String> { v.get(k).and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).collect() };
            let (by, cond, cmd) = (v.get("by").and_then(|b| b.as_str()).unwrap_or("").to_string(), list("cond"), list("cmd"));
            if cmd.is_empty() {
                return Err("after <condition> -- <devhub words>: no words".into());
            }
            if cmd[0] == "after" {
                return Err("an after's action cannot be another after".into());
            }
            let cond = parse_cond(ctx, &cond.iter().map(String::as_str).collect::<Vec<_>>())?;
            let w = ctx.store.update(|d| {
                d.after_next += 1;
                let w = Waiter { id: format!("A{}", d.after_next), cond, cmd, by, added_ms: wall_ms(), state: "waiting".into(), ..Default::default() };
                d.after.push(w.clone());
                w
            });
            ctx.store.event("after", Sev::Info, format!("after {} waiting: {} -> {} (by {})", w.id, w.cond.describe(), w.cmd.join(" "), w.by), None);
            Ok(format!("{{\"id\":{},\"state\":\"waiting\",\"cond\":{}}}", makepad_remote::json_str(&w.id), makepad_remote::json_str(&w.cond.describe())))
        }
        _ => Err("after list [all] | add <request> | cancel <id>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::master::LeaseMaster;
    use crate::state::{Pair, Store, Tip};

    fn ctx(name: &str) -> Ctx {
        let home = std::env::temp_dir().join(format!("devhub-after-test-{name}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        let leases = Arc::new(LeaseMaster::new("mac", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        Ctx { store: Store::open(&home, "mac", Arc::new(|_| {})), leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: home.clone(), home }
    }

    #[test]
    fn waiters_are_added_met_and_cancelled() {
        let c = ctx("a");
        let add = |cond: &str, cmd: &str| serve(&c, &format!("after add {{\"by\":\"lane x\",\"cond\":[{cond}],\"cmd\":[{cmd}]}}"));
        let a = add("\"verified\"", "\"launch\",\"mac\",\"makepad-fab\"").unwrap();
        assert!(a.contains("\"id\":\"A1\"") && a.contains("\"waiting\""), "{a}");
        assert!(add("\"free\",\"nope\"", "\"verify\"").unwrap_err().contains("no node nope"));
        assert!(add("\"verified\"", "").unwrap_err().contains("no words"));
        assert!(add("\"verified\"", "\"after\",\"x\"").is_err());
        assert!(add("\"soon\"", "\"verify\"").is_err());
        let w = c.store.read(|d| d.after[0].clone());
        assert_eq!((w.by.as_str(), w.cmd.len()), ("lane x", 3));
        // not met until a verify passes after it was added
        assert_eq!(check(&c, &w).0, None);
        c.store.update(|d| d.verify.verified.push(Tip { pair: Pair::new(&"a".repeat(40), &"b".repeat(40)), at_ms: w.added_ms + 1, diff: "affected".into() }));
        assert_eq!(check(&c, &w).0.as_deref(), Some("verified aaaaaaaaaa,bbbbbbbbbb"));
        assert!(serve(&c, "after list").unwrap().contains("A1"));
        assert!(serve(&c, "after cancel A1").is_ok());
        assert!(serve(&c, "after cancel A1").is_err());
        assert_eq!(serve(&c, "after list").unwrap(), "[]");
        assert!(serve(&c, "after list all").unwrap().contains("cancelled"));
        let _ = std::fs::remove_file(c.repo.join("state.json"));
    }
}
