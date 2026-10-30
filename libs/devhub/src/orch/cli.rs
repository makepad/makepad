//! The CLI words of the master's jobs: `lane`, `lanes`, `verify`, `after`, `state`, `watch`.
//! Answer first, terse and stable; `--json` gives the state as the master keeps it.
//!
//! Exit codes: 0 ok, 1 the command failed, 2 usage or no master.

use crate::cli::{ask, control, has};
use crate::proto::{Record, Sev};
use makepad_remote::json_str;
use makepad_remote::link::Client;
use makepad_strict_json::Value;
use std::io::Write;
use std::time::Duration;

pub const HELP: &str = "  lane new <name> [--from work] [--no-commercial]
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
  state [verify|allapps|after|pushes]     the master's workflow state as JSON
  watch [kind,..] [--once] [--match <text>] [--sev err|warn] [--json]
                                          block on events of those kinds (verify after allapps push land lanes
                                          lease ...); --once: exit 0 after the first that matches";

const WORDS: [&str; 6] = ["lane", "lanes", "verify", "after", "state", "watch"];

fn master() -> Result<Client, String> {
    let c = control()?;
    if !has(&c, "verify") {
        return Err("the devhub here is not the master, or an older one without verify/after".into());
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
        "lane" => lane(&c, &words, json),
        "lanes" => lanes(&c, json),
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
