//! `devhub after <condition> -- <devhub words>`: the master runs the words once the condition
//! holds (replacing the manager's waiter scripts). Waiters are state: they survive a master
//! restart, and each one fires once.
//!
//! Conditions: `landed <branch>` (its own commits in makepad `work`, and the commercial branch of
//! the same name, if any, in `main`; the merge bot's "Merge work (<sha>) into <branch>" commits are
//! not the branch's own and never move what it waits for; the branch may be deleted after
//! landing: its last seen own commit counts; new commits of its own move it, said as an event), `free <node>` (up,
//! on for dev, no lease held or waiting), `verified [<commit|branch>]` (a verify passes after the
//! waiter was added; with a commit: a verified pair that contains it).
//!
//! Actions are devhub words: any `makepad devhub` command (`verify kick`, `job submit ...`);
//! `launch mac <app>` is a show on the Mac (`devhub show`, watched for failures).

use super::{git, Ctx};
use crate::proto::{wall_ms, Sev};
use crate::state::{Cond, Waiter};
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
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
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
    let mut pass = crate::gitread::Pass::default();
    for w in waiting {
        let (met, cond) = cond_met_in(ctx, &w.cond, w.added_ms, &mut pass);
        if let (Cond::Landed { branch, sha: old, .. }, Cond::Landed { sha: new, .. }) = (&w.cond, &cond) {
            if old != new {
                ctx.store.event("after", Sev::Info, format!("after {}: {branch} has new commits of its own: it now waits for {} (was {})", w.id, super::short(new), super::short(old)), None);
            }
        }
        if cond != w.cond {
            ctx.store.update(|d| {
                if let Some(x) = d.after.iter_mut().find(|x| x.id == w.id) {
                    x.cond = cond.clone();
                }
            }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
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
        }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
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
#[cfg(test)]
fn check(ctx: &Ctx, w: &Waiter) -> (Option<String>, Cond) {
    cond_met(ctx, &w.cond, w.added_ms)
}

/// Whether `cond` holds now (`added_ms`: when it was asked for), and the condition with what it
/// learned (a branch's newest tip). Shared with `devhub show --after`.
#[cfg(test)]
pub fn cond_met(ctx: &Ctx, cond: &Cond, added_ms: u64) -> (Option<String>, Cond) {
    cond_met_in(ctx, cond, added_ms, &mut crate::gitread::Pass::default())
}

pub fn cond_met_in(ctx: &Ctx, cond: &Cond, added_ms: u64, pass: &mut crate::gitread::Pass) -> (Option<String>, Cond) {
    match cond {
        Cond::Landed { branch, repo, sha } => {
            let (dir, tip_name) = if repo == "commercial" { (ctx.commercial(), "main") } else { (ctx.repo.clone(), "work") };
            // the newest commit of the branch's own (deleted after landing: the last one seen)
            let own = pass.own_tip(&dir, branch, tip_name).unwrap_or_else(|| sha.clone());
            let cond = Cond::Landed { branch: branch.clone(), repo: repo.clone(), sha: own.clone() };
            let mut met = !own.is_empty() && pass.is_ancestor(&dir, &own, &format!("refs/heads/{tip_name}")) == Some(true);
            // the lane's other half (the commercial branch of a makepad lane, by the same name)
            let other = (repo != "commercial").then(|| ctx.commercial());
            let mut what = format!("{branch}@{} is in {tip_name}", super::short(&own));
            if let (true, Some(cdir)) = (met, other) {
                if let Some(c) = pass.own_tip(&cdir, branch, "main") {
                    met = pass.is_ancestor(&cdir, &c, "refs/heads/main") == Some(true);
                    what = format!("{what}, its commercial half @{} in main", super::short(&c));
                }
            }
            (met.then_some(what), cond)
        }
        Cond::Free { node } => {
            let met = ctx.leases.nodes().into_iter().find(|n| &n.name == node).filter(|n| n.up && n.dev && n.leases == 0 && n.waiting == 0 && n.hold.is_none()).map(|_| format!("{node} is free"));
            (met, cond.clone())
        }
        Cond::Verified { contains } => {
            let tips: Vec<crate::state::Tip> = ctx.store.read(|d| d.verify.verified.iter().filter(|t| t.at_ms >= added_ms).cloned().collect());
            let hit = tips.iter().rev().find(|t| match contains {
                None => true,
                Some(c) => pass.is_ancestor(&ctx.repo, c, &t.pair.work) == Some(true) || pass.is_ancestor(&ctx.commercial(), c, &t.pair.main) == Some(true),
            });
            (hit.map(|t| format!("verified {}", t.pair.short())), cond.clone())
        }
        Cond::Shown { app, device } => {
            let hit = ctx.store.read(|d| d.shows.iter().any(|x| &x.app == app && &x.device == device && x.state == "running" && x.started_ms >= added_ms));
            (hit.then(|| format!("{app} runs on {device}")), cond.clone())
        }
        Cond::None => (None, Cond::None),
    }
}

/// The newest commit on `branch`'s first-parent line that is its own: past the merge bot's
/// "Merge <tip> (<sha>) into <branch>" commits (they never land). None: no such branch.
pub fn own_tip(dir: &std::path::Path, branch: &str, tip_name: &str) -> Option<String> {
    crate::gitread::Pass::default().own_tip(dir, branch, tip_name)
}

/// Runs the waiter's words; its exit and log are state.
fn act(ctx: &Ctx, w: &Waiter) {
    let dir = ctx.logs("after");
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join(format!("{}.log", w.id));
    // `launch mac <app>`: a show, watched like one (its failures are `show_failed` events)
    let r = match w.cmd.as_slice() {
        [launch, dev, app, ..] if launch == "launch" && super::show::device(ctx, dev).is_ok_and(|d| d == "mac") => match super::ctx() {
            Some(c) => super::show::serve(&c, &format!("show add {{\"app\":{},\"on\":\"mac\",\"by\":{}}}", makepad_remote::json_str(app), makepad_remote::json_str(&format!("after {}", w.id)))).map(|a| (0, a)),
            None => Err("no master".into()),
        },
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
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    ctx.store.event("after", sev, format!("after {} {state}: {} (exit {}){}", w.id, w.cmd.join(" "), exit.map_or("-".into(), |c| c.to_string()), if note.is_empty() { String::new() } else { format!(": {note}") }), None);
}

/// devhub's words with the master's own binary, in the checkout.
fn devhub(ctx: &Ctx, words: &[String], id: &str, log: &std::path::Path) -> Result<(i32, String), String> {
    let own = crate::cli::own_argv(&ctx.repo);
    let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let st = makepad_remote::group::command(&own[0])
        .args(&own[1..])
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
                let sha = own_tip(&ctx.repo, branch, "work").unwrap_or(sha);
                return Ok(Cond::Landed { branch: branch.to_string(), repo: "makepad".into(), sha });
            }
            if let Some(sha) = has(&ctx.commercial()) {
                let sha = own_tip(&ctx.commercial(), branch, "main").unwrap_or(sha);
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
        ["shown", app, device] => Ok(Cond::Shown { app: app.to_string(), device: super::show::device(ctx, device)? }),
        ["verified"] => Ok(Cond::Verified { contains: None }),
        ["verified", rev] => {
            let sha = git(&ctx.repo, &["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")]).or_else(|_| git(&ctx.commercial(), &["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")])).map_err(|_| format!("no commit {rev}"))?;
            Ok(Cond::Verified { contains: Some(sha) })
        }
        _ => Err("conditions: landed <branch> | free <node> | verified [<commit|branch>] | shown <app> <device>".into()),
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
            })?;
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
            })?;
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
        Ctx { store: Store::open(&home, "mac", Arc::new(|_| 0)).expect("workflow state must open before services start"), leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: home.clone(), home }
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
        c.store.update(|d| d.verify.verified.push(Tip { pair: Pair::new(&"a".repeat(40), &"b".repeat(40)), at_ms: w.added_ms + 1, diff: "affected".into() })).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
        assert_eq!(check(&c, &w).0.as_deref(), Some("verified aaaaaaaaaa,bbbbbbbbbb"));
        assert!(serve(&c, "after list").unwrap().contains("A1"));
        assert!(serve(&c, "after cancel A1").is_ok());
        assert!(serve(&c, "after cancel A1").is_err());
        assert_eq!(serve(&c, "after list").unwrap(), "[]");
        assert!(serve(&c, "after list all").unwrap().contains("cancelled"));
        let _ = std::fs::remove_file(c.repo.join("state.json"));
    }

    fn g(dir: &std::path::Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// A branch that landed and then got work merged in by the bot is landed: the bot's merge is
    /// not its own (it never lands) and does not move what the waiter waits for.
    #[test]
    fn landed_ignores_the_merge_bots_merges() {
        let base = std::env::temp_dir().join(format!("devhub-after-landed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mut c = ctx("landed");
        c.repo = base.join("makepad");
        std::fs::create_dir_all(&c.repo).unwrap();
        g(&c.repo, &["init", "-q", "-b", "work"]);
        std::fs::write(c.repo.join("a.txt"), "a").unwrap();
        g(&c.repo, &["add", "."]);
        g(&c.repo, &["commit", "-q", "-m", "base"]);
        g(&c.repo, &["checkout", "-q", "-b", "rc-x"]);
        std::fs::write(c.repo.join("x.txt"), "x").unwrap();
        g(&c.repo, &["add", "."]);
        g(&c.repo, &["commit", "-q", "-m", "lane work"]);
        let own = g(&c.repo, &["rev-parse", "HEAD"]);
        let w = Waiter { id: "A1".into(), cond: parse_cond(&c, &["landed", "rc-x"]).unwrap(), ..Default::default() };
        assert_eq!(check(&c, &w).0, None);
        // it lands; work moves on; the bot merges work into the lane
        g(&c.repo, &["checkout", "-q", "work"]);
        g(&c.repo, &["merge", "-q", "--no-ff", "-m", "land x", "rc-x"]);
        std::fs::write(c.repo.join("b.txt"), "b").unwrap();
        g(&c.repo, &["add", "."]);
        g(&c.repo, &["commit", "-q", "-m", "more work"]);
        let work = g(&c.repo, &["rev-parse", "HEAD"]);
        g(&c.repo, &["checkout", "-q", "rc-x"]);
        g(&c.repo, &["merge", "-q", "--no-ff", "-m", &format!("Merge work ({}) into rc-x", &work[..10]), "work"]);
        let (met, cond) = check(&c, &w);
        assert!(met.is_some_and(|m| m.contains(&own[..10])), "{cond:?}");
        assert_eq!(cond, Cond::Landed { branch: "rc-x".into(), repo: "makepad".into(), sha: own });
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_file(c.home.join("state.json"));
    }
    #[test]
    fn readiness_passes_observe_refs_and_spawn_no_git() {
        let temp = makepad_git::test_support::tempdir().unwrap();
        let mut c = ctx("readiness");
        c.repo = temp.path().join("repo");
        std::fs::create_dir_all(&c.repo).unwrap();
        g(&c.repo, &["init", "-q", "-b", "work"]);
        std::fs::write(c.repo.join("file"), "one\n").unwrap();
        std::fs::write(c.repo.join(".gitattributes"), "*.csv text eol=crlf\n*.bin -text\n*.ident ident\n").unwrap();
        std::fs::write(c.repo.join("values.csv"), "one\r\ntwo\r\n").unwrap();
        std::fs::write(c.repo.join("raw.bin"), "one\n").unwrap();
        std::fs::write(c.repo.join("marker.ident"), "$Id: expanded $\n$Id: not closed\n").unwrap();
        g(&c.repo, &["add", "."]);
        g(&c.repo, &["commit", "-q", "-m", "base"]);
        let base = g(&c.repo, &["rev-parse", "HEAD"]);
        g(&c.repo, &["checkout", "-q", "-b", "rc-query"]);
        std::fs::write(c.repo.join("file"), "two\n").unwrap();
        g(&c.repo, &["commit", "-q", "-am", "own"]);
        let own = g(&c.repo, &["rev-parse", "HEAD"]);
        let cond = Cond::Landed { branch: "rc-query".into(), repo: "makepad".into(), sha: own.clone() };
        let mut old = crate::gitread::Pass::default();
        assert_eq!(old.rev(&c.repo, "work"), Some(base.clone()));
        assert_eq!(old.own_tip(&c.repo, "rc-query", "work"), Some(own.clone()));
        assert!(cond_met(&c, &cond, 0).0.is_none());
        g(&c.repo, &["update-ref", "refs/heads/work", &own]);
        assert_eq!(old.rev(&c.repo, "work"), Some(base.clone()), "one pass keeps its ref snapshot");
        assert!(cond_met(&c, &cond, 0).0.is_some(), "next pass sees the moved ref");
        g(&c.repo, &["update-ref", "refs/heads/work-old-test", &own]);
        let live = crate::state::Live { tree: c.repo.to_string_lossy().into(), rev: format!("rc-query@{base}"), ..Default::default() };
        assert!(g(&c.repo, &["status", "--porcelain", "--untracked-files=no"]).is_empty(), "fixture is clean including normalized CRLF text");
        let broken = c.repo.join(".git/refs/heads/work-old-aaa");
        std::fs::write(&broken, format!("{}\n", "ab".repeat(20))).unwrap();
        let before = crate::git_exec::on_this_thread();
        for _ in 0..4 {
            let mut pass = crate::gitread::Pass::default();
            assert_eq!(pass.own_tip(&c.repo, "rc-query", "work"), Some(own.clone()));
            assert!(cond_met_in(&c, &cond, 0, &mut pass).0.is_some());
            assert_eq!(super::super::live::lane_landed(&c, &live, &mut pass), Some(own.clone()));
            let range = crate::bucket::range(&c.repo, &base, &own).unwrap();
            assert_eq!(*range, std::collections::HashSet::from([own.clone()]));
            assert_eq!(crate::land::retired_base(&c.repo, &own, &base), Some(own.clone()));
            assert_eq!(pass.retired_holds(&c.repo, &own), Some(true), "a broken retired ref must not hide a later match");
        }
        assert_eq!(crate::git_exec::on_this_thread(), before, "repeated readiness ticks must launch zero git commands");
        std::fs::remove_file(&broken).unwrap();
        // %s joins the first paragraph, preserving meaningful leading space.
        let mut commit = crate::gitread::with(&c.repo, |_, repo| repo.read_commit(&makepad_git::ObjectId::from_hex(&own).unwrap()).unwrap()).unwrap();
        commit.parents = vec![makepad_git::ObjectId::from_hex(&own).unwrap()];
        for message in ["Merge work (abc) into rc-subject\n", " Merge work (abc) into rc-subject\n", "Merge work (abc)\ninto rc-subject\n", "Merge work (abc) into rc-subject  \n"] {
            commit.message = message.into();
            let oid = makepad_git::object::write_loose_object(&c.repo.join(".git"), makepad_git::ObjectKind::Commit, &makepad_git::commit::serialize_commit(&commit)).unwrap();
            makepad_git::refs::write_ref(&c.repo.join(".git"), "refs/heads/rc-subject", &oid).unwrap();
            let out = std::process::Command::new("git").arg("-C").arg(&c.repo).args(["log", "-1", "--format=%s", &oid.to_hex()]).output().unwrap();
            assert!(out.status.success());
            let text = String::from_utf8(out.stdout).unwrap();
            let subject = text.trim_end_matches('\n');
            let want = if subject.starts_with("Merge work (") && subject.ends_with(") into rc-subject") { own.clone() } else { oid.to_hex() };
            assert_eq!(own_tip(&c.repo, "rc-subject", "work"), Some(want), "subject {subject:?}");
        }
        // Tracked changes block orphan release; untracked files do not.
        std::fs::write(c.repo.join("values.csv"), "one\r\ntwo\r\n").unwrap();
        std::fs::write(c.repo.join("marker.ident"), "$Id: expanded $\n$Id: not closed\n").unwrap();
        assert!(c.monitor.tracked_clean(&c.repo).unwrap().1, "built-in clean conversions agree after stat records change");
        std::fs::write(c.repo.join("raw.bin"), "one\r\n").unwrap();
        assert!(!c.monitor.tracked_clean(&c.repo).unwrap().1, "binary attribute preserves CRLF changes");
        std::fs::write(c.repo.join("raw.bin"), "one\n").unwrap();
        std::fs::write(c.repo.join("untracked"), "scratch").unwrap();
        assert!(c.monitor.tracked_clean(&c.repo).unwrap().1);
        std::fs::write(c.repo.join("file"), "bad\n").unwrap();
        assert!(!c.monitor.tracked_clean(&c.repo).unwrap().1, "same-size edit");
        assert!(super::super::live::lane_landed(&c, &live, &mut crate::gitread::Pass::default()).is_none());
        std::fs::write(c.repo.join("file"), "two\n").unwrap();
        assert!(c.monitor.tracked_clean(&c.repo).unwrap().1, "reverted edit");
        g(&c.repo, &["update-index", "--chmod=+x", "file"]);
        assert!(!c.monitor.tracked_clean(&c.repo).unwrap().1, "staged mode change");
        g(&c.repo, &["update-index", "--chmod=-x", "file"]);
        assert!(c.monitor.tracked_clean(&c.repo).unwrap().1);
        let linked = temp.path().join("linked");
        g(&c.repo, &["worktree", "add", "-q", "--detach", linked.to_str().unwrap(), &base]);
        let mut pass = crate::gitread::Pass::default();
        assert_eq!(pass.rev(&linked, "HEAD"), Some(base.clone()));
        assert_eq!(pass.rev(&c.repo, "HEAD"), Some(own.clone()));
        let p = crate::gitread::paths(&linked).unwrap();
        let q = crate::gitread::paths(&c.repo).unwrap();
        assert!(std::sync::Arc::ptr_eq(&crate::gitread::handle(&p.common_dir).unwrap(), &crate::gitread::handle(&q.common_dir).unwrap()), "linked worktrees share objects");
        // A commercial half must land too.
        let com = c.commercial();
        std::fs::create_dir_all(&com).unwrap();
        g(&com, &["init", "-q", "-b", "main"]);
        g(&com, &["commit", "-q", "--allow-empty", "-m", "commercial base"]);
        g(&com, &["checkout", "-q", "-b", "rc-query"]);
        g(&com, &["commit", "-q", "--allow-empty", "-m", "commercial own"]);
        let com_own = g(&com, &["rev-parse", "HEAD"]);
        assert!(cond_met(&c, &cond, 0).0.is_none(), "commercial half is still pending");
        g(&com, &["update-ref", "refs/heads/main", &com_own]);
        assert!(cond_met(&c, &cond, 0).0.is_some(), "both halves landed");
        // A new own commit moves the wait; deleting its branch keeps the last
        // observed commit. Packed refs must not resurrect a deleted branch.
        std::fs::write(c.repo.join("file"), "new\n").unwrap();
        g(&c.repo, &["commit", "-q", "-am", "new own"]);
        let next = g(&c.repo, &["rev-parse", "HEAD"]);
        let (met, moved) = cond_met(&c, &cond, 0);
        assert!(met.is_none());
        assert_eq!(moved, Cond::Landed { branch: "rc-query".into(), repo: "makepad".into(), sha: next.clone() });
        g(&c.repo, &["pack-refs", "--all"]);
        g(&c.repo, &["update-ref", "-d", "refs/heads/rc-query"]);
        assert_eq!(cond_met(&c, &moved, 0), (None, moved.clone()));
        g(&c.repo, &["update-ref", "refs/heads/work", &next]);
        assert!(cond_met(&c, &moved, 0).0.is_some());
        // Reach follows rewinds too, without retaining now-unreachable tips.
        g(&c.repo, &["update-ref", "refs/heads/work", &base]);
        assert_eq!(crate::gitread::Pass::default().is_ancestor(&c.repo, &next, "work"), Some(false));
    }

}
