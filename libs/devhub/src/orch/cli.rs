//! The CLI words of the master's jobs: `job`, `lane`, `lanes`, `verify`, `after`, `allapps`, `push`, `state`, `watch`.
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

pub const HELP: &str = "  job submit [--kind test] [--needs k=v,..] [--node N] [--prio P] [--lane L] [--after J1,J2] [--rev R] -- cmd...
                                          an async job (prefer it to holding a lease): returns its id at once; it runs
                                          at this worktree's snapshot (HEAD + its uncommitted files, as they are now;
                                          --rev: those commits alone) on the first free box that fits, and the box is
                                          released when the command ends
  job submit [common flags] --batch <file|->   one job per line (`[flags] -- cmd`, --after @1 = line 1's job):
                                          a whole plan in one call
  job wait <id..> [--any] | job result <id> [--tail] | job list [--all]
                                          block until they end (or pause on a panic); the result: exit, duration,
                                          latency, error lines, log tail
  job cancel <id..> | job rerun <id>     cancel; run again exactly (same snapshot and command)
  job inspect <id> <cmd..> | job resume <id> <P1> retry|restart|unwind | job release <id>
                                          a job paused on a parked panic keeps its box: look, resume or let go
                                          (10 min with no action release it)
  lane new <name> [--from work] [--no-commercial]
                                          a lane worktree: <lanes root>/<name> on rc-<name> from work, with
                                          apps/commercial on rc-<name> from main; prints its path. Lanes never run
                                          git worktree themselves; the master removes a landed lane (30 min idle)
  lane drop <name>                        abandon a lane: unlanded commits (uncommitted work committed first) are
                                          bundled into ~/.makepad/devhub/dropped, then worktrees, branches, node dirs go
  lane keep|unkeep <name>                 exempt a lane from the cleanup after landing (or not)
  lane adopt                              register the existing live worktrees; list the landed ones (removes nothing)
  lane remove <name>                      remove a landed lane now (adopt's list, once agreed)
  lanes [--json]                          each lane: worktree, tip, dirty files, merge state, land state, last job
  verify [--json]                         the rolling verifier: newest verified pair, the run and its legs, the
                                          last failure (first bad landed commit)
  verify kick [--full] | on | off         verify the newest landed pair now (--full: diff every program); the
                                          rolling runs on or off
  verify pair <work> <main> [--dry] [--full]   verify that pair (--dry: no ledger, no bisect)
  after <condition> -- <devhub words>     run the words once: landed <branch> | free <node> | verified [<commit>];
                                          words: launch mac <package> (the Mac: the master's own launch) or any
                                          devhub command (launch <node> .., verify kick, ...)
  after [--all] [--json] | after cancel <id>   the waiters
  allapps [kick|on|off] [--json]          makepad check of every app/example/WM package, hourly when idle
  push [--sha <work sha>] [-m <message>] [--parent <sha>]... [--dry]
                                          the collapsed push of the newest (or that) verified tip to origin/work;
                                          refuses an unverified tip and credential-like text in the delta
  state [verify|allapps|after|pushes]     the master's workflow state as JSON
  watch [kind,..] [--once] [--match <text>] [--sev err|warn] [--json]
                                          block on events of those kinds (verify after allapps push land lanes
                                          lease ...); --once: exit 0 after the first that matches";

const WORDS: [&str; 9] = ["job", "lane", "lanes", "verify", "after", "allapps", "push", "state", "watch"];

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
        "job" => job(&c, &words, json),
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
    let prio = match get("prio") {
        Some(p) => p.parse::<i32>().map_err(|_| "--prio takes a number")?.to_string(),
        None => crate::lease::sched::DEFAULT_PRIO.to_string(),
    };
    Ok(format!(
        "{{\"kind\":{},\"needs\":{},\"node\":{},\"prio\":{prio},\"lane\":{},\"agent\":{},\"cmd\":[{}],\"after\":[{}]}}",
        json_str(&get("kind").unwrap_or_else(|| "test".into())),
        json_str(&get("needs").unwrap_or_default()),
        json_str(&get("node").unwrap_or_default()),
        json_str(&lane),
        json_str(&std::env::var("DEVHUB_AGENT").unwrap_or_default()),
        cmd.iter().map(|x| json_str(x)).collect::<Vec<_>>().join(","),
        after.join(",")
    ))
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
            let (mut common, mut rev, mut batch, mut i) = (Vec::new(), String::new(), None::<String>, 0);
            while i < rest.len() && rest[i] != "--" {
                match (rest[i].as_str(), rest.get(i + 1)) {
                    ("--rev", Some(v)) => rev = v.clone(),
                    ("--batch", Some(v)) => batch = Some(v.clone()),
                    (k @ ("--kind" | "--needs" | "--node" | "--prio" | "--lane" | "--after"), Some(v)) => common.push((k[2..].to_string(), v.clone())),
                    _ => break,
                }
                i += 2;
            }
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
            let line = format!("job submit {{\"tree\":{},\"rev\":{},\"batch\":{},\"jobs\":[{}]}}", json_str(&tree.to_string_lossy()), json_str(&rev), batch.is_some(), reqs.join(","));
            done(one(c, "job", &line), json, &|v| {
                let ids: Vec<&str> = v.get("jobs").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str()).collect();
                let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let snap = if s("snap").is_empty() { "commits only".to_string() } else { format!("{} files={}", s("snap"), v.get("files").and_then(|x| x.as_u64()).unwrap_or(0)) };
                format!("{} queued{} rev={} {snap}\n", ids.join(" "), if s("batch").is_empty() { String::new() } else { format!(" ({})", s("batch")) }, s("rev").split(',').map(|p| { let (b, h) = p.split_once('@').unwrap_or((p, "")); format!("{b}@{}", super::short(h)) }).collect::<Vec<_>>().join(","))
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
            eprintln!("makepad devhub lane new <name> [--from work] [--no-commercial] | drop <name> | keep|unkeep <name> | adopt | remove <name>");
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
                "{:<20} {}@{} dirty={} merge={} land={} job={}{}{}\n      {}\n",
                s("name"),
                s("branch"),
                super::short(&tip),
                s("dirty"),
                s("merge"),
                s("land"),
                s("job"),
                if l.get("keep").and_then(|k| k.as_bool()) == Some(true) { " keep" } else { "" },
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
    let (mut kinds, mut once, mut pat, mut sev) = (Vec::<String>::new(), false, None::<String>, None::<Sev>);
    let mut i = 0;
    while i < words.len() {
        match (words[i], words.get(i + 1)) {
            ("--once", _) => once = true,
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
                if !kinds.is_empty() && !kinds.iter().any(|k| *k == r.kind) {
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
}
