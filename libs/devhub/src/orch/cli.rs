//! The CLI words of the master's jobs: `job`, `live`, `show`, `shows`, `lane`, `lanes`, `verify`, `after`, `allapps`, `push`, `state`, `watch`.
//! Answer first, terse and stable; `--json` gives the state as the master keeps it.
//!
//! Exit codes: 0 ok, 1 the command failed (or a push was refused), 2 usage or no master.

use crate::cli::{ask, control, has};
use crate::proto::{Record, Sev};
use makepad_remote::json_str;
use makepad_remote::link::Client;
use makepad_strict_json::Value;
use std::io::Write;
use std::time::Duration;

pub const HELP: &str = "  job submit [--kind test] [--needs k=v,..] [--node N|--only N] [--prio P] [--lane L] [--after J1,J2] [--rev R] [--with-uncommitted] -- cmd...
                                          an async job (prefer it to holding a lease): returns its id at once; it runs
                                          at this worktree's snapshot (HEAD + its uncommitted files, as they are now;
                                          --rev: those commits alone) on the first free box that fits, and the box is
                                          released when the command ends. Submit lists the files it ships: never .claude
                                          or local/, and a main checkout's only with --with-uncommitted. The box
                                          ends exactly at the shared commits: whatever a job leaves outside ignored
                                          paths goes; results that outlive it belong under target/
  job submit [common flags] --batch <file|->   one job per line (`[flags] -- cmd`, --after @1 = line 1's job):
                                          a whole plan in one call
  job wait <id..> [--any] | job result <id> [--tail] | job list [--all]
                                          block until they end (or pause on a panic); the result: exit, duration,
                                          latency, error lines, log tail
  job cancel <id..> | job rerun <id>     cancel; run again exactly (same snapshot and command)
  job inspect <id> <cmd..> | job resume <id> <P1> retry|restart|unwind | job release <id>
                                          a job paused on a parked panic keeps its box: look, resume or let go
                                          (10 min with no action release it)
  live start <app> [--needs k=v,..] [--visible] [--lane L]
                                          the app under the JIT on a box (headless, or visible on a screen box), held
                                          for this lane: every file saved in this worktree ships to it as it comes and
                                          hot-patches in; returns its id at once
  live act <id> <inspect cmd..>           any inspect command in the app (ui, key, click, grab, test hooks, stacks)
  live wait <id> patched|settled|frame [timeout_s]   the newest edit's verdict (patched, restart, refused, with
                                          save → verdict → frame times); the JIT idle; a frame drawn
  live sync <id> | live release <id> | live list [--all] | live <id>
                                          ship the whole dirty set now; end it (10 min with no edit or act end it)
  show <app> [--on mac|<node>|quest] [--after landed <branch>] [--replace] [--pin]
  show pin <app> [--on <device>] on|off   an operation runs on the app: no edit or landing reaches it and it does not
                                          restart, while landings go on for everyone else; off: it catches up
                                          the latest landed build of an app on the user's device (the Mac: the master's
                                          own launch; a node: its dock app; quest: built on the Mac, adb install), one
                                          per app per device, tracked; a running one is replaced only with --replace
                                          (ask the user first: it may hold unsaved work); closing it ends the show
  show stop <app> [--on <device>] | shows [--all]   stop one; what runs on each device
  lane new <name> [--from work] [--no-commercial]
                                          a lane worktree: <lanes root>/<name> on rc-<name> from work, with
                                          apps/commercial on rc-<name> from main; prints its path. Lanes never run
                                          git worktree themselves; the master removes a landed lane (30 min idle)
  lane drop <name>                        abandon a lane: unlanded commits (uncommitted work committed first) are
                                          bundled into ~/.makepad/devhub/dropped, then worktrees, branches, node dirs go
  timeline <lane>                         the lane's time: thinking, queueing, gating, booting, running
  lane keep|unkeep <name>                 exempt a lane from the cleanup after landing (or not)
  lane live <name> on|off                 the manager's mark: the user waits for this lane. Its landings take the
                                          fast path (clean merge + check of the changed app, ahead of the queue; the
                                          boot at `devhub show`, the tests after landing)
  lane adopt                              register the existing live worktrees; list the landed ones (removes nothing)
  lane remove <name>                      remove a landed lane now (adopt's list, once agreed)
  lane sweep [--dry] | lane sweep status  everything under the lanes root that is no makepad lane (other
                                          repos' worktrees, clones, no-git folders) goes, after ONE archive
                                          (~/makepad-archive/wt-noindex-<date>.tar.xz) of what is not in git;
                                          live lanes and folders a process runs in stay
  lanes [--json]                          each lane: worktree, tip, dirty files, merge state, land state, last job
  verify [--json]                         the rolling verifier: newest verified pair, the run and its legs, the
                                          last failure (first bad landed commit)
  verify kick [--full] | on | off         verify the newest landed pair now (--full: diff every program); the
                                          rolling runs on or off
  verify pair <work> <main> [--dry] [--full]   verify that pair (--dry: no ledger, no bisect)
  after <condition> -- <devhub words>     run the words once: landed <branch> | free <node> | verified [<commit>];
                                          words: any devhub command (verify kick, job submit ...); an app on the
                                          user's display: show <app> --after landed <branch>
  after [--all] [--json] | after cancel <id>   the waiters
  allapps [kick|on|off] [--json]          makepad check of every app/example/WM package, hourly when idle
  push [--sha <work sha>] [-m <message>] [--parent <sha>]... [--dry]
                                          the collapsed push of the newest (or that) verified tip to origin/work;
                                          refuses an unverified tip and credential-like text in the delta
  state [verify|allapps|after|pushes]     the master's workflow state as JSON
  watch [kind,..] [--once] [--until fail] [--match <text>] [--sev err|warn] [--json]
                                          block on events of those kinds (verify after allapps push land lanes
                                          lease ...); --once: exit 0 after the first that matches; --until fail: exit 1
                                          on a failure (show_failed: a shown app's panic, crash, compile error), 0 when
                                          a show ends";

const WORDS: [&str; 13] = ["job", "live", "show", "shows", "lane", "lanes", "verify", "after", "allapps", "push", "state", "watch", "timeline"];

fn master() -> Result<Client, String> {
    let c = control()?;
    if !has(&c, "verify") {
        return Err("the devhub here is not the master, or an older one without verify/after/allapps/push".into());
    }
    Ok(c)
}

/// One request; the answer's first line (an `err` line is the error).
fn one(c: &Client, service: &str, line: &str) -> Result<Value, String> {
    let lines = ask(c, None, service, line)?;
    let first = lines.first().ok_or_else(|| format!("{service}: no answer"))?;
    let v = makepad_strict_json::parse_depth(first.as_bytes(), 16).map_err(|_| format!("{service}: bad answer"))?;
    match v.get("err").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

fn done(r: Result<Value, String>, json: bool, render: &dyn Fn(&Value) -> String) -> i32 {
    match r {
        Ok(v) => {
            if json {
                println!("{}", v.to_json());
            } else {
                print!("{}", render(&v));
            }
            0
        }
        Err(e) => {
            eprintln!("makepad devhub: {e}");
            1
        }
    }
}

/// `key=value ...` of a flat answer.
fn flat(v: &Value) -> String {
    match v {
        Value::Obj(pairs) => {
            let words: Vec<String> = pairs
                .iter()
                .map(|(k, x)| match x {
                    Value::Str(s) => format!("{k}={}", if s.contains(' ') { json_str(s) } else { s.clone() }),
                    other => format!("{k}={}", other.to_json()),
                })
                .collect();
            format!("{}\n", words.join(" "))
        }
        other => format!("{}\n", other.to_json()),
    }
}

pub fn main(args: &[&str]) -> Option<i32> {
    let verb = *args.first()?;
    if !WORDS.contains(&verb) {
        return None;
    }
    let rest = &args[1..];
    // `lanes now` is the merge bot's
    if verb == "lanes" && rest.iter().any(|a| *a != "--json") {
        return None;
    }
    if rest.first().is_some_and(|a| matches!(*a, "-h" | "--help" | "help")) {
        println!("{HELP}");
        return Some(0);
    }
    let json = rest.contains(&"--json");
    let c = match master() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("makepad devhub {verb}: {e}");
            return Some(2);
        }
    };
    let words: Vec<&str> = rest.iter().copied().filter(|a| *a != "--json").collect();
    Some(match verb {
        "verify" => {
            let line = format!("verify {}", words.join(" "));
            match words.first() {
                None | Some(&"status") => done(one(&c, "verify", &line), json, &super::verify::render),
                _ => done(one(&c, "verify", &line), json, &flat),
            }
        }
        "after" => after(&c, &words, json),
        "allapps" => {
            let line = format!("allapps {}", words.join(" "));
            match words.first() {
                None | Some(&"status") => done(one(&c, "allapps", &line), json, &super::allapps::render),
                _ => done(one(&c, "allapps", &line), json, &flat),
            }
        }
        "lane" => lane(&c, &words, json),
        "show" => show(&c, &words, json),
        "timeline" => match words.as_slice() {
            [lane] => done(one(&c, "lane", &format!("lane timeline {lane}")), json, &|v| v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string()),
            _ => {
                eprintln!("makepad devhub timeline <lane>");
                2
            }
        },
        "shows" => shows(&c, words.contains(&"--all"), json),
        "job" => job(&c, &words, json),
        "live" => live(&c, &words, json),
        "lanes" => lanes(&c, json),
        "push" => push(&c, &words, json),
        "state" => match one(&c, "state", &format!("state {}", words.first().unwrap_or(&""))) {
            Ok(v) => {
                println!("{}", v.to_json());
                0
            }
            Err(e) => {
                eprintln!("makepad devhub state: {e}");
                1
            }
        },
        "watch" => watch(&c, &words, json),
        _ => 2,
    })
}

/// The worktree around the current dir (a nested `apps/commercial` counts as its parent's).
fn tree() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let top = cwd.ancestors().find(|a| a.join(".git").exists())?.to_path_buf();
    if top.ends_with("apps/commercial") {
        if let Some(up) = top.parent().and_then(|p| p.parent()).filter(|p| p.join(".git").exists()) {
            return Some(up.to_path_buf());
        }
    }
    Some(top)
}

/// Words of a batch line (`"..."` and `'...'` keep their spaces).
pub fn words_of(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None::<char>, false);
    for ch in line.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(ch);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// One job's request from its flags and command; `base` holds the common flags.
/// `job submit`'s word on the uncommitted files: every path it ships (the JSON answer has them all
/// too), a warning when they are a main checkout's and not a lane's, or that they stayed out.
pub(super) fn shipped_note(v: &Value) -> String {
    let paths = |k: &str| -> Vec<&str> { v.get(k).and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str()).collect() };
    let (changed, deleted) = (paths("changed"), paths("deleted"));
    let left_out = v.get("left_out").and_then(|n| n.as_u64()).unwrap_or(0);
    if left_out > 0 {
        return format!("main checkout: commits only, its {left_out} uncommitted file(s) stay out (--with-uncommitted ships them)\n");
    }
    if changed.is_empty() && deleted.is_empty() {
        return String::new();
    }
    let tree = v.get("tree").and_then(|t| t.as_str()).unwrap_or("");
    let mut out = String::new();
    if v.get("main_checkout").and_then(|b| b.as_bool()).unwrap_or(false) {
        out += "WARNING: this is a main checkout, not a lane's worktree; these are its uncommitted files:\n";
    }
    out += &format!("ships {} uncommitted file(s) of {tree} on top of those commits, {} deletion(s):\n", changed.len(), deleted.len());
    out.extend(changed.iter().map(|p| format!("  {p}\n")));
    out.extend(deleted.iter().map(|p| format!("  deleted {p}\n")));
    out
}

fn job_req(words: &[String], base: &[(String, String)]) -> Result<String, String> {
    let mut f: Vec<(String, String)> = base.to_vec();
    let mut i = 0;
    let mut cmd: Vec<String> = Vec::new();
    while i < words.len() {
        let w = words[i].as_str();
        if w == "--" {
            cmd = words[i + 1..].to_vec();
            break;
        }
        match (w, words.get(i + 1)) {
            ("--only", Some(v)) => {
                f.retain(|(k, _)| k != "node" && k != "only");
                f.push(("node".to_string(), v.clone()));
                f.push(("only".to_string(), "1".to_string()));
                i += 2;
            }
            ("--kind" | "--needs" | "--node" | "--prio" | "--lane" | "--after", Some(v)) => {
                f.retain(|(k, _)| k != &w[2..]);
                f.push((w[2..].to_string(), v.clone()));
                i += 2;
            }
            _ if !w.starts_with("--") => {
                cmd = words[i..].to_vec();
                break;
            }
            _ => return Err(format!("unknown flag `{w}`")),
        }
    }
    if cmd.is_empty() {
        return Err("no command (`-- cmd...`)".into());
    }
    let get = |k: &str| f.iter().rev().find(|(x, _)| x == k).map(|(_, v)| v.clone());
    let lane = get("lane").or_else(|| std::env::var("DEVHUB_LANE").ok().filter(|l| !l.is_empty())).or_else(|| tree().and_then(|t| t.file_name().map(|n| n.to_string_lossy().to_lowercase()))).unwrap_or_default();
    let after: Vec<String> = get("after").map(|a| a.split(',').filter(|x| !x.is_empty()).map(|x| json_str(x)).collect()).unwrap_or_default();
    let only = get("only").is_some();
    let prio = match get("prio") {
        Some(p) => p.parse::<i32>().map_err(|_| "--prio takes a number")?.to_string(),
        None => crate::lease::sched::DEFAULT_PRIO.to_string(),
    };
    Ok(format!(
        "{{\"kind\":{},\"needs\":{},\"node\":{},\"only\":{only},\"prio\":{prio},\"lane\":{},\"agent\":{},\"cmd\":[{}],\"after\":[{}]}}",
        json_str(&get("kind").unwrap_or_else(|| "test".into())),
        json_str(&get("needs").unwrap_or_default()),
        json_str(&get("node").unwrap_or_default()),
        json_str(&lane),
        json_str(&std::env::var("DEVHUB_AGENT").unwrap_or_default()),
        cmd.iter().map(|x| json_str(x)).collect::<Vec<_>>().join(","),
        after.join(",")
    ))
}

/// The flags before the command of `job submit`: the common ones, --rev, --batch, and where they end.
fn submit_flags(rest: &[String]) -> (Vec<(String, String)>, String, Option<String>, usize, bool) {
    let (mut common, mut rev, mut batch, mut i, mut uncommitted) = (Vec::new(), String::new(), None, 0, false);
    while i < rest.len() && rest[i] != "--" {
        if rest[i] == "--with-uncommitted" {
            uncommitted = true;
            i += 1;
            continue;
        }
        match (rest[i].as_str(), rest.get(i + 1)) {
            ("--rev", Some(v)) => rev = v.clone(),
            ("--batch", Some(v)) => batch = Some(v.clone()),
            ("--only", Some(v)) => {
                common.retain(|(k, _): &(String, String)| k != "node" && k != "only");
                common.push(("node".to_string(), v.clone()));
                common.push(("only".to_string(), "1".to_string()));
            }
            (k @ ("--kind" | "--needs" | "--node" | "--prio" | "--lane" | "--after"), Some(v)) => common.push((k[2..].to_string(), v.clone())),
            _ => break,
        }
        i += 2;
    }
    (common, rev, batch, i, uncommitted)
}

fn jobs_of(v: &Value) -> Vec<crate::state::Job> {
    v.as_arr().unwrap_or(&[]).iter().map(crate::state::job_from).collect()
}

fn job(c: &Client, words: &[&str], json: bool) -> i32 {
    let usage = || {
        eprintln!("makepad devhub job submit [flags] -- cmd | submit [flags] --batch <file|-> | wait <id..> [--any] | result <id> [--tail] | list [--all] | cancel <id..> | rerun <id> | inspect <id> <cmd..> | resume <id> <P> <how> | release <id>");
        2
    };
    match words {
        ["submit", rest @ ..] => {
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            let Some(tree) = tree() else {
                eprintln!("makepad devhub job submit: run it inside a worktree");
                return 2;
            };
            // common flags (and --rev, --batch) before the command
            let (common, rev, batch, i, uncommitted) = submit_flags(&rest);
            let reqs: Result<Vec<String>, String> = match &batch {
                Some(src) => {
                    let text = if src == "-" {
                        let mut s = String::new();
                        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s);
                        s
                    } else {
                        match std::fs::read_to_string(src) {
                            Ok(s) => s,
                            Err(e) => {
                                eprintln!("makepad devhub job submit: {src}: {e}");
                                return 2;
                            }
                        }
                    };
                    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).enumerate().map(|(n, l)| job_req(&words_of(l), &common).map_err(|e| format!("line {}: {e}", n + 1))).collect()
                }
                None => job_req(&rest[i..], &common).map(|r| vec![r]),
            };
            let reqs = match reqs {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("makepad devhub job submit: {e}");
                    return 2;
                }
            };
            let line = format!("job submit {{\"tree\":{},\"rev\":{},\"batch\":{},\"with_uncommitted\":{uncommitted},\"jobs\":[{}]}}", json_str(&tree.to_string_lossy()), json_str(&rev), batch.is_some(), reqs.join(","));
            done(one(c, "job", &line), json, &|v| {
                let ids: Vec<&str> = v.get("jobs").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str()).collect();
                let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let snap = if s("snap").is_empty() { "commits only".to_string() } else { format!("{} files={}", s("snap"), v.get("files").and_then(|x| x.as_u64()).unwrap_or(0)) };
                let queued = format!("{} queued{} rev={} {snap}\n", ids.join(" "), if s("batch").is_empty() { String::new() } else { format!(" ({})", s("batch")) }, s("rev").split(',').map(|p| { let (b, h) = p.split_once('@').unwrap_or((p, "")); format!("{b}@{}", super::short(h)) }).collect::<Vec<_>>().join(","));
                queued + &shipped_note(v)
            })
        }
        ["wait", rest @ ..] if !rest.is_empty() => {
            let any = rest.contains(&"--any");
            let ids: Vec<&str> = rest.iter().copied().filter(|a| *a != "--any").collect();
            // follow the stream first, so no change between the ask and the follow is missed
            let mut feed = c.watch();
            loop {
                let list = match one(c, "job", &format!("job get {}", ids.join(" "))) {
                    Ok(v) => jobs_of(&v),
                    Err(e) => {
                        eprintln!("makepad devhub job wait: {e}");
                        return 1;
                    }
                };
                if let Some(u) = list.iter().find(|j| j.state == "unknown") {
                    eprintln!("makepad devhub job wait: no job {}", u.id);
                    return 2;
                }
                let settled = |j: &crate::state::Job| j.done() || j.state == "paused";
                let ready = if any { list.iter().any(settled) } else { list.iter().all(settled) };
                if ready {
                    let shown: Vec<&crate::state::Job> = if any { list.iter().filter(|j| settled(j)).collect() } else { list.iter().collect() };
                    for j in &shown {
                        if json {
                            println!("{}", crate::state::job_value(j).to_json());
                        } else {
                            print!("{}", super::job::render(j, false));
                        }
                    }
                    return if shown.iter().all(|j| j.state == "ok") { 0 } else { 1 };
                }
                // until a job event (or 30 s)
                let t = std::time::Instant::now();
                while t.elapsed() < Duration::from_secs(30) {
                    match feed.next(Duration::from_secs(5), 30) {
                        Ok(Some((_, l))) => {
                            if Record::parse(&String::from_utf8_lossy(&l)).is_some_and(|r| r.kind == "job") {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            eprintln!("makepad devhub job wait: {e}");
                            return 1;
                        }
                    }
                }
            }
        }
        ["result", id, rest @ ..] => match one(c, "job", &format!("job get {id}")) {
            Ok(v) => match jobs_of(&v).into_iter().next().filter(|j| j.state != "unknown") {
                Some(j) => {
                    if json {
                        println!("{}", crate::state::job_value(&j).to_json());
                    } else {
                        print!("{}", super::job::render(&j, rest.contains(&"--tail")));
                    }
                    if j.state == "ok" { 0 } else { 1 }
                }
                None => {
                    eprintln!("makepad devhub job result: no job {id}");
                    2
                }
            },
            Err(e) => {
                eprintln!("makepad devhub job result: {e}");
                1
            }
        },
        ["list", rest @ ..] => done(one(c, "job", if rest.contains(&"--all") { "job list all" } else { "job list" }), json, &|v| {
            let list = jobs_of(v);
            if list.is_empty() {
                return "no open jobs\n".into();
            }
            list.iter().map(|j| format!("{}\n", super::job::render(j, false).lines().next().unwrap_or(""))).collect()
        }),
        [verb @ ("cancel" | "release"), ids @ ..] if !ids.is_empty() => done(one(c, "job", &format!("job {verb} {}", ids.join(" "))), json, &|v| v.as_arr().unwrap_or(&[]).iter().map(flat).collect()),
        ["rerun", id] => done(one(c, "job", &format!("job rerun {id}")), json, &flat),
        ["resume", id, p, how] => done(one(c, "job", &format!("job resume {id} {p} {how}")), json, &flat),
        ["inspect", id, cmd @ ..] if !cmd.is_empty() => done(one(c, "job", &format!("job inspect {id} {}", cmd.join(" "))), json, &|v| v.get("lines").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().map(|l| format!("{}\n", l.as_str().unwrap_or(""))).collect()),
        _ => usage(),
    }
}

fn live(c: &Client, words: &[&str], json: bool) -> i32 {
    let render = |v: &Value| super::live::render(&crate::state::live_from(v));
    match words {
        ["start", app, rest @ ..] => {
            let (mut needs, mut visible, mut lane) = (String::new(), false, None::<String>);
            let mut i = 0;
            while i < rest.len() {
                match (rest[i], rest.get(i + 1)) {
                    ("--needs", Some(n)) => {
                        needs = n.to_string();
                        i += 1;
                    }
                    ("--lane", Some(l)) => {
                        lane = Some(l.to_string());
                        i += 1;
                    }
                    ("--visible", _) => visible = true,
                    (w, _) => {
                        eprintln!("makepad devhub live start <app> [--needs k=v,..] [--visible] [--lane L]: unknown `{w}`");
                        return 2;
                    }
                }
                i += 1;
            }
            let Some(tree) = tree() else {
                eprintln!("makepad devhub live start: run it inside a worktree");
                return 2;
            };
            let lane = lane.or_else(|| std::env::var("DEVHUB_LANE").ok().filter(|l| !l.is_empty())).unwrap_or_else(|| tree.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
            let req = format!("{{\"app\":{},\"tree\":{},\"lane\":{},\"agent\":{},\"needs\":{},\"visible\":{visible}}}", json_str(app), json_str(&tree.to_string_lossy()), json_str(&lane), json_str(&std::env::var("DEVHUB_AGENT").unwrap_or_default()), json_str(&needs));
            done(one(c, "live", &format!("live start {req}")), json, &flat)
        }
        ["act", id, cmd @ ..] if !cmd.is_empty() => done(one(c, "live", &format!("live act {id} {}", cmd.join(" "))), json, &|v| {
            let mut out: String = v.get("lines").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().map(|l| format!("{}\n", l.as_str().unwrap_or(""))).collect();
            out.push_str(&format!("({} ms)\n", v.get("ms").and_then(|m| m.as_u64()).unwrap_or(0)));
            out
        }),
        ["wait", id, what, rest @ ..] => done(one(c, "live", &format!("live wait {id} {what} {}", rest.join(" "))), json, &render),
        [verb @ ("sync" | "release"), id] => done(one(c, "live", &format!("live {verb} {id}")), json, &flat),
        ["list", rest @ ..] | ["ls", rest @ ..] => done(one(c, "live", if rest.contains(&"--all") { "live list all" } else { "live list" }), json, &|v| {
            let list = v.as_arr().unwrap_or(&[]);
            if list.is_empty() {
                return "no live jobs\n".into();
            }
            list.iter().map(|x| render(x)).collect()
        }),
        [id] if id.starts_with('W') => done(one(c, "live", &format!("live get {id}")), json, &render),
        _ => {
            eprintln!("makepad devhub live start <app> [..] | act <id> <cmd..> | wait <id> patched|settled|frame | sync <id> | release <id> | list | <id>");
            2
        }
    }
}

fn show(c: &Client, words: &[&str], json: bool) -> i32 {
    let usage = || {
        eprintln!("makepad devhub show <app> [--on mac|<node>|quest] [--after landed <branch>] [--replace] [--pin] | show stop <app> [--on <device>] | show pin <app> [--on <device>] on|off");
        2
    };
    let (mut on, mut after, mut replace, mut pin, mut pos) = (String::new(), Vec::<String>::new(), false, false, Vec::<&str>::new());
    let mut i = 0;
    while i < words.len() {
        match (words[i], words.get(i + 1)) {
            ("--on", Some(d)) => {
                on = d.to_string();
                i += 1;
            }
            ("--after", Some(k)) => {
                after.push(k.to_string());
                i += 1;
                if let Some(v) = words.get(i + 1).filter(|v| !v.starts_with("--")) {
                    after.push(v.to_string());
                    i += 1;
                }
            }
            ("--rev", Some(r)) => {
                if *r != "landed" {
                    eprintln!("makepad devhub show: --rev landed is the only one (a show runs the latest landed build)");
                    return 2;
                }
                i += 1;
            }
            ("--replace", _) => replace = true,
            ("--pin", _) => pin = true,
            (w, _) if w.starts_with("--") => return usage(),
            (w, _) => pos.push(w),
        }
        i += 1;
    }
    match pos.as_slice() {
        ["stop", app] => done(one(c, "show", &format!("show stop {app} {}", if on.is_empty() { "mac" } else { &on })), json, &flat),
        ["pin", app, state @ ("on" | "off")] => done(one(c, "show", &format!("show pin {app} {} {state}", if on.is_empty() { "mac" } else { &on })), json, &flat),
        // `show <node> <app>`: the old form of a node's dock app
        [node, app] if on.is_empty() => show_add(c, app, node, &after, replace, pin, json),
        [app] => show_add(c, app, &on, &after, replace, pin, json),
        _ => usage(),
    }
}

fn show_add(c: &Client, app: &str, on: &str, after: &[String], replace: bool, pin: bool, json: bool) -> i32 {
    let by = std::env::var("DEVHUB_AGENT").ok().filter(|a| !a.is_empty()).unwrap_or_else(|| "cli".into());
    let req = format!("{{\"app\":{},\"on\":{},\"after\":[{}],\"replace\":{replace},\"pin\":{pin},\"by\":{}}}", json_str(app), json_str(on), after.iter().map(|a| json_str(a)).collect::<Vec<_>>().join(","), json_str(&by));
    done(one(c, "show", &format!("show add {req}")), json, &flat)
}

fn shows(c: &Client, all: bool, json: bool) -> i32 {
    done(one(c, "show", if all { "show list all" } else { "show list" }), json, &|v| {
        let list: Vec<crate::state::Show> = v.as_arr().unwrap_or(&[]).iter().map(crate::state::show_from).collect();
        if list.is_empty() {
            return "nothing shown\n".into();
        }
        list.iter()
            .map(|x| {
                let ran = if x.started_ms == 0 { String::new() } else { format!(" {}s", (if x.ended_ms > 0 { x.ended_ms } else { crate::proto::wall_ms() }).saturating_sub(x.started_ms) / 1000) };
                let panic = x.panic.as_ref().map_or(String::new(), |p| format!(" panic {}: {} at {}", p.id, p.message, p.location));
                let wait = if x.state == "waiting" { format!(" for {}", x.cond.describe()) } else { String::new() };
                let pinned = if x.pinned { " PINNED" } else { "" };
                let stages = if x.stages.is_empty() { String::new() } else { format!("\n           stages: {}", x.stages) };
                format!("{:<10} {:<24} {}{pinned}{wait} pid={} sha={}{ran}{panic}{}{stages}\n", x.device, x.app, x.state, x.pid, super::short(&x.sha), if x.note.is_empty() { String::new() } else { format!(" ({})", x.note) })
            })
            .collect()
    })
}

fn push(c: &Client, words: &[&str], json: bool) -> i32 {
    let (mut sha, mut msg, mut parents, mut dry) = (None, None, Vec::new(), false);
    let mut i = 0;
    while i < words.len() {
        match (words[i], words.get(i + 1)) {
            ("--sha", Some(v)) => sha = Some(v.to_string()),
            ("-m" | "--message", Some(v)) => msg = Some(v.to_string()),
            ("--parent", Some(v)) => parents.push(v.to_string()),
            ("--dry", _) => {
                dry = true;
                i += 1;
                continue;
            }
            _ => {
                eprintln!("makepad devhub push [--sha <work sha>] [-m <message>] [--parent <sha>]... [--dry]");
                return 2;
            }
        }
        i += 2;
    }
    let opt = |o: &Option<String>| o.as_deref().map_or("null".into(), json_str);
    let req = format!("{{\"sha\":{},\"message\":{},\"parents\":[{}],\"dry\":{dry}}}", opt(&sha), opt(&msg), parents.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","));
    done(one(c, "push", &format!("push {req}")), json, &flat)
}

fn lane(c: &Client, words: &[&str], json: bool) -> i32 {
    match words {
        ["new", name, rest @ ..] => {
            let (mut from, mut commercial) = ("work".to_string(), true);
            let mut i = 0;
            while i < rest.len() {
                match (rest[i], rest.get(i + 1)) {
                    ("--from", Some(f)) => {
                        from = f.to_string();
                        i += 1;
                    }
                    ("--no-commercial", _) => commercial = false,
                    (w, _) => {
                        eprintln!("makepad devhub lane new <name> [--from work] [--no-commercial]: unknown `{w}`");
                        return 2;
                    }
                }
                i += 1;
            }
            let req = format!("{{\"name\":{},\"from\":{},\"commercial\":{commercial}}}", json_str(name), json_str(&from));
            done(one(c, "lane", &format!("lane new {req}")), json, &|v| format!("{}\n", v.get("path").and_then(|p| p.as_str()).unwrap_or("")))
        }
        [verb @ ("drop" | "remove" | "keep" | "unkeep"), name] => done(one(c, "lane", &format!("lane {verb} {name}")), json, &flat),
        ["live", name, on @ ("on" | "off")] => done(one(c, "lane", &format!("lane live {name} {on}")), json, &flat),
        ["sweep", "status"] => done(one(c, "lane", "lane sweep status"), json, &|v| v.get("report").and_then(|r| r.as_str()).unwrap_or("").to_string()),
        ["sweep", rest @ ..] if rest.is_empty() || rest == ["--dry"] => done(one(c, "lane", &format!("lane sweep {}", rest.first().unwrap_or(&""))), json, &|v| {
            format!("sweep started{}: `devhub lane sweep status` (report {})\n", if v.get("dry").and_then(|d| d.as_bool()).unwrap_or(false) { " (dry run, nothing deleted)" } else { "" }, v.get("report").and_then(|r| r.as_str()).unwrap_or(""))
        }),
        ["adopt"] => done(one(c, "lane", "lane adopt"), json, &|v| {
            let s = |x: &Value, k: &str| x.get(k).and_then(|y| y.as_str()).unwrap_or("").to_string();
            let reg: Vec<String> = v.get("registered").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let landed = v.get("landed").and_then(|a| a.as_arr()).unwrap_or(&[]);
            let mut out = format!("adopt: registered {} live lane(s); {} landed worktree(s) listed (nothing removed; `devhub lane remove <name>` once agreed)\n", reg.len(), landed.len());
            for l in landed {
                out.push_str(&format!("landed {} {} {}\n", s(l, "name"), s(l, "branch"), s(l, "path")));
            }
            out
        }),
        _ => {
            eprintln!("makepad devhub lane new <name> [--from work] [--no-commercial] | drop <name> | keep|unkeep <name> | live <name> on|off | adopt | remove <name> | sweep [--dry] | sweep status");
            2
        }
    }
}

fn lanes(c: &Client, json: bool) -> i32 {
    done(one(c, "lane", "lane list"), json, &|v| {
        let list = v.as_arr().unwrap_or(&[]);
        if list.is_empty() {
            return "no lanes registered (devhub lane new | devhub lane adopt)\n".into();
        }
        let mut out = String::new();
        for l in list {
            let s = |k: &str| l.get(k).and_then(|y| y.as_str()).unwrap_or("").to_string();
            let tip = s("tip");
            out.push_str(&format!(
                "{:<20} {}@{} dirty={} merge={} land={} job={}{}{}{}\n      {}\n",
                s("name"),
                s("branch"),
                super::short(&tip),
                s("dirty"),
                s("merge"),
                s("land"),
                s("job"),
                if l.get("keep").and_then(|k| k.as_bool()) == Some(true) { " keep" } else { "" },
                if l.get("waited").and_then(|k| k.as_bool()) == Some(true) { " LIVE" } else { "" },
                if s("note").is_empty() { String::new() } else { format!(" ({})", s("note")) },
                s("path")
            ));
        }
        out
    })
}

fn after(c: &Client, words: &[&str], json: bool) -> i32 {
    match words {
        [] | ["--all"] | ["list"] => {
            let all = words.contains(&"--all");
            done(one(c, "after", if all { "after list all" } else { "after list" }), json, &|v| {
                let list = v.as_arr().unwrap_or(&[]);
                if list.is_empty() {
                    return "after: none waiting\n".into();
                }
                list.iter()
                    .map(|w| {
                        let s = |k: &str| w.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let cond = crate::state::Cond::from_value(w.get("cond")).describe();
                        let cmd: Vec<&str> = w.get("cmd").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str()).collect();
                        let exit = w.get("exit").and_then(|e| e.as_i64()).map_or(String::new(), |e| format!(" exit={e}"));
                        let note = s("note");
                        format!("{} {} {} -> {} (by {}, {} ago){exit}{}\n", s("id"), s("state"), cond, cmd.join(" "), s("by"), super::age(w.get("added_ms").and_then(|x| x.as_u64()).unwrap_or(0)), if note.is_empty() { String::new() } else { format!(": {note}") })
                    })
                    .collect()
            })
        }
        ["cancel", id] => done(one(c, "after", &format!("after cancel {id}")), json, &flat),
        _ => {
            let Some(sep) = words.iter().position(|w| *w == "--") else {
                eprintln!("makepad devhub after <condition> -- <devhub words>\n{HELP}");
                return 2;
            };
            let by = std::env::var("DEVHUB_AGENT").ok().filter(|a| !a.is_empty()).or_else(|| std::env::var("DEVHUB_LANE").ok().filter(|a| !a.is_empty())).unwrap_or_else(|| "cli".into());
            let list = |w: &[&str]| w.iter().map(|x| json_str(x)).collect::<Vec<_>>().join(",");
            let req = format!("{{\"by\":{},\"cond\":[{}],\"cmd\":[{}]}}", json_str(&by), list(&words[..sep]), list(&words[sep + 1..]));
            done(one(c, "after", &format!("after add {req}")), json, &flat)
        }
    }
}

/// `watch [kind,..] [--once] [--match S] [--sev S]`: the master's events of those kinds, live.
fn watch(c: &Client, words: &[&str], json: bool) -> i32 {
    let (mut kinds, mut once, mut pat, mut sev, mut until_fail) = (Vec::<String>::new(), false, None::<String>, None::<Sev>, false);
    let mut i = 0;
    while i < words.len() {
        match (words[i], words.get(i + 1)) {
            ("--once", _) => once = true,
            ("--until", Some(&"fail")) => {
                until_fail = true;
                i += 1;
            }
            ("--match", Some(p)) => {
                pat = Some(p.to_string());
                i += 1;
            }
            ("--sev", Some(s)) => {
                sev = Some(Sev::parse(s));
                i += 1;
            }
            (w, _) if !w.starts_with('-') => kinds.extend(w.split(',').filter(|k| !k.is_empty()).map(str::to_string)),
            (w, _) => {
                eprintln!("makepad devhub watch: unknown flag `{w}`");
                return 2;
            }
        }
        i += 1;
    }
    let rank = |s: Sev| match s {
        Sev::Err => 0,
        Sev::Warn => 1,
        Sev::Req => 2,
        Sev::Info => 3,
    };
    let mut feed = c.watch();
    let mut out = std::io::stdout();
    loop {
        match feed.next(Duration::from_secs(5), 30) {
            Ok(Some((_, line))) => {
                let text = String::from_utf8_lossy(&line);
                let Some(r) = Record::parse(&text) else { continue };
                // `show` also hears `show_failed`
                if !kinds.is_empty() && !kinds.iter().any(|k| *k == r.kind || r.kind.strip_prefix(k.as_str()).is_some_and(|x| x.starts_with('_'))) {
                    continue;
                }
                if sev.is_some_and(|s| rank(r.sev) > rank(s)) {
                    continue;
                }
                if pat.as_ref().is_some_and(|p| !r.line.contains(p.as_str())) {
                    continue;
                }
                let l = if json { r.to_json() } else { r.stream_line() };
                if writeln!(out, "{l}").and_then(|_| out.flush()).is_err() {
                    return 0;
                }
                if once {
                    return 0;
                }
                // a failure (1), or a show that ended (0)
                if until_fail {
                    if r.sev == Sev::Err || r.kind.ends_with("_failed") {
                        return 1;
                    }
                    if r.kind == "show" && (r.line.contains(" closed after ") || r.line.ends_with(" stopped")) {
                        return 0;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("makepad devhub watch: {e}");
                return 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_lines_become_job_requests() {
        assert_eq!(words_of(r#"--after @1 -- cargo test -p "a b" 'x y' z"#), vec!["--after", "@1", "--", "cargo", "test", "-p", "a b", "x y", "z"]);
        assert_eq!(words_of("  "), Vec::<String>::new());
        let base = vec![("kind".to_string(), "build".to_string()), ("lane".to_string(), "l1".to_string())];
        let r = job_req(&words_of("--kind test --after @1,J4 -- cargo test -p x"), &base).unwrap();
        let v = makepad_strict_json::parse(r.as_bytes()).unwrap();
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        assert_eq!((s("kind"), s("lane")), ("test".to_string(), "l1".to_string()));
        assert_eq!(v.get("after").unwrap().to_json(), "[\"@1\",\"J4\"]");
        assert_eq!(v.get("cmd").unwrap().to_json(), "[\"cargo\",\"test\",\"-p\",\"x\"]");
        // a line without flags is all command
        let r = job_req(&words_of("bash tools/run/fixture/x.sh"), &base).unwrap();
        assert!(r.contains("\"cmd\":[\"bash\",\"tools/run/fixture/x.sh\"]") && r.contains("\"kind\":\"build\""), "{r}");
        assert!(job_req(&words_of("--bogus 1 -- x"), &base).is_err());
        assert!(job_req(&words_of("--kind test"), &base).is_err());
    }

    #[test]
    fn only_is_a_common_flag_beside_rev() {
        let (common, rev, _, i, _) = submit_flags(&words_of("--only 104 --rev abc -- cargo test"));
        assert_eq!((rev.as_str(), i), ("abc", 4));
        let r = job_req(&words_of("cargo test"), &common).unwrap();
        assert!(r.contains("\"node\":\"104\"") && r.contains("\"only\":true"), "{r}");
    }
}
