//! `makepad observe …`: the agent's and the user's door to the local devhub (its control
//! socket). Output is token-lean text; `--json` gives the records as they travel.

use crate::proto::{self, Record, Sev};
use makepad_remote::hub;
use makepad_remote::link::{Client, Target};
use makepad_strict_json::Value;
use std::io::Write;
use std::time::Duration;

const HELP: &str = "makepad devhub (or observe): the local devhub (start it with `cargo run --release -p makepad-app-devhub`)
  stream [--json] [--all]                 one line per event, live (--all: info lines too)
  ls [machine]                            machines and their JITs, each with what it is up to
  inspect <machine>/<pid|app> <cmd...>    any inspect command on that JIT (master: every node)
  top <node>                              CPU, GPU, RAM, disk and the busiest processes of a node
  log <node>/<pid|app> [n]                the tail of a JIT's log (its session log, or its launch log)
  reply <request-id> <status> <message>   answer an engine request (received working deployed declined note)
  inbox [--all]                           what needs a look (master), newest first
  show <id>                               an event with its detail (master inbox)
  clear <id|all>                          remove from the inbox (seen <id|all> marks it read)
  hubs [--json]                           the node hubs this one links to (nodes.conf + the nodes they heard)
  cache [ls|clear] [--node <n>] [--app <package>] [--dir <run dir>] [--force]
                                          the JIT cache (makepad run's compiled code): what a clear frees, or
                                          clear it (the next start is cold; --dir: that one cache); refused
                                          while JITs run on it unless --force (they keep running)
  serve [--node <conf>]                   run a hub here without its app (master, or a node with that conf)";

pub(crate) fn control() -> Result<Client, String> {
    let c = Client::new(Target::Unix(proto::control_socket()), "observe");
    c.conn().map_err(|_| format!("no devhub runs here ({}): start makepad-app-devhub", proto::control_socket().display()))?;
    Ok(c)
}

/// Sends one line to a lines service and collects the answer.
pub fn ask(c: &Client, target: Option<&str>, service: &str, line: &str) -> Result<Vec<String>, String> {
    let mut got = Vec::new();
    match target {
        Some(t) => {
            hub::ask(c, t, service, "", line, &mut |l| {
                got.push(l.to_string());
                true
            })?;
        }
        None => {
            let ch = c.open(service, "").map_err(|e| e.to_string())?;
            ch.send(line.as_bytes().to_vec()).map_err(|e| e.to_string())?;
            'recv: loop {
                match ch.recv() {
                    Some(makepad_remote::Event::Data(d)) => {
                        for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                            if l.starts_with("{\"end\":") {
                                break 'recv;
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
    }
    Ok(got)
}

pub(crate) fn has(c: &Client, service: &str) -> bool {
    c.caps().iter().any(|s| s.split('/').next() == Some(service))
}

fn num(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Int(i)) => *i as f64,
        Some(Value::F64(f)) => *f,
        _ => 0.0,
    }
}


/// `ps` lines as the `ls` block of one machine: the machine, then each JIT with what it is up to.
pub fn render_ps(name: Option<&str>, lines: &[String], out: &mut String) {
    let mut m = crate::explorer::parse_ps("", lines);
    if let Some(n) = name {
        m.name = n.to_string();
    }
    if let Some(e) = &m.err {
        out.push_str(&format!("{}  error: {}\n", m.name, proto::clean(e, 200)));
        return;
    }
    let battery = m.battery.map_or(String::new(), |(level, charging)| format!("  battery {level}%{}", if charging { " charging" } else { "" }));
    out.push_str(&format!("{}  {}  load {:.1}/{}  mem {}/{} GB free{battery}  {} JITs\n", m.name, m.os, m.load1, m.cpus, m.mem_avail_mb / 1024, m.mem_total_mb / 1024, m.jits.iter().filter(|j| j.jit).count()));
    for j in &m.jits {
        out.push_str(&format!("  {}\n", j.head()));
        let facts = j.facts();
        if !facts.is_empty() {
            out.push_str(&format!("      {facts}\n"));
        }
        for (kind, line, age) in j.recent.iter().take(2) {
            out.push_str(&format!("      {kind} {}s ago: {}\n", age, proto::clean(line, 160)));
        }
    }
}

fn ls(c: &Client, only: Option<&str>) -> i32 {
    let mut out = String::new();
    if only.is_none_or(|o| o == local_name(c)) {
        match ask(c, None, "ps", "ps") {
            Ok(lines) => render_ps(None, &lines, &mut out),
            Err(e) => out.push_str(&format!("this machine: {e}\n")),
        }
    }
    if has(c, "hosts") {
        for h in ask(c, None, "hosts", "hosts").unwrap_or_default() {
            let Ok(v) = makepad_strict_json::parse(h.as_bytes()) else { continue };
            if v.get("via").and_then(|x| x.as_str()) != Some("fleet") {
                continue;
            }
            let name = v.get("host").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if only.is_some_and(|o| o != name) {
                continue;
            }
            match ask(c, Some(&name), "ps", "ps") {
                Ok(lines) => render_ps(Some(&name), &lines, &mut out),
                Err(e) => out.push_str(&format!("{name}  down: {}\n", proto::clean(&e, 160))),
            }
        }
    }
    print!("{out}");
    0
}

fn stream(c: &Client, json: bool, all: bool) -> i32 {
    let mut feed = c.watch();
    let mut stdout = std::io::stdout();
    loop {
        match feed.next(Duration::from_secs(5), 30) {
            Ok(Some((_, line))) => {
                let text = String::from_utf8_lossy(&line);
                let Some(r) = Record::parse(&text) else { continue };
                if !all && r.sev == Sev::Info {
                    continue;
                }
                let out = if json { r.to_json() } else { r.stream_line() };
                if writeln!(stdout, "{out}").and_then(|_| stdout.flush()).is_err() {
                    return 0;
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("makepad devhub: {e}");
                return 1;
            }
        }
    }
}

pub(crate) fn local_name(c: &Client) -> String {
    ask(c, None, "ps", "ps")
        .ok()
        .and_then(|l| l.into_iter().next())
        .and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .and_then(|v| v.get("machine").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or_default()
}

fn inspect(c: &Client, target: &str, words: &[String]) -> i32 {
    let Some((machine, app)) = target.split_once('/') else {
        eprintln!("makepad devhub inspect: <machine>/<pid|app> <command...>");
        return 2;
    };
    let line = words.join(" ");
    let mut had_err = false;
    let mut print = |l: &str| {
        had_err |= l.starts_with("{\"err\":");
        println!("{l}");
        true
    };
    let r = if machine == local_name(c) {
        // this machine: the JIT's own socket, exactly `makepad inspect`
        match Client::connect(app) {
            Ok(j) => j.cmd(&line).map_err(|e| e.to_string()).map(|lines| {
                for l in lines {
                    match l {
                        Ok(l) => {
                            print(&l);
                        }
                        Err(e) => {
                            eprintln!("makepad devhub: {e}");
                            break;
                        }
                    }
                }
            }),
            Err(e) => Err(e.to_string()),
        }
    } else {
        hub::ask(c, &format!("{machine}:{app}"), "cmd", "", &line, &mut print).map(|_| ())
    };
    match r {
        Ok(()) if had_err => 1,
        Ok(()) => 0,
        Err(e) => {
            eprintln!("makepad devhub: {e}");
            2
        }
    }
}

fn reply(c: &Client, id: &str, status: &str, msg: &str) -> i32 {
    // the hub routes it: the master to the machine the request came from
    lines_out(ask(c, None, "reply", &format!("{id} {status} {msg}")), false)
}

/// `makepad observe <args>`; returns the exit code.
pub fn main(args: &[String]) -> i32 {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    if matches!(a.first().copied(), None | Some("-h") | Some("--help") | Some("help")) {
        println!("{HELP}\n{}\n{}", crate::lease::client::HELP, crate::sync::help());
        return if a.is_empty() { 2 } else { 0 };
    }
    if a.first() == Some(&"serve") {
        return serve(&a[1..]);
    }
    if let Some(code) = crate::sync::cli(&a) {
        return code;
    }
    if let Some(code) = crate::lease::cli(&a) {
        return code;
    }
    if a.first() == Some(&"cache") {
        return cache(&a[1..]);
    }
    let c = match control() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("makepad devhub: {e}");
            return 2;
        }
    };
    match a.as_slice() {
        ["ls", rest @ ..] => ls(&c, rest.first().copied()),
        ["stream", rest @ ..] => stream(&c, rest.contains(&"--json"), rest.contains(&"--all")),
        ["inspect", target, cmd @ ..] if !cmd.is_empty() => inspect(&c, target, &args[2..]),
        ["reply", id, status, msg @ ..] => reply(&c, id, status, &msg.join(" ")),
        ["inbox", rest @ ..] => lines_out(ask(&c, None, "inbox", if rest.contains(&"--all") { "list all" } else { "list" }), true),
        ["show", id] => lines_out(ask(&c, None, "inbox", &format!("show {id}")), false),
        ["clear", id] => lines_out(ask(&c, None, "inbox", &format!("clear {id}")), false),
        ["seen", id] => lines_out(ask(&c, None, "inbox", &format!("seen {id}")), false),
        ["top", node, rest @ ..] => match ask(&c, if *node == local_name(&c) { None } else { Some(node) }, "top", &format!("top {}", rest.first().unwrap_or(&"12"))) {
            Ok(lines) => {
                let mut out = String::new();
                crate::top::render(&lines, &mut out);
                print!("{out}");
                0
            }
            Err(e) => {
                eprintln!("makepad devhub top: {node}: {e}");
                2
            }
        },
        ["log", target, rest @ ..] => {
            let Some((node, who)) = target.split_once('/') else {
                eprintln!("makepad devhub log <node>/<pid|app> [n]");
                return 2;
            };
            let who = who.rsplit('#').next().unwrap_or(who);
            match ask(&c, Some(node), "jitlog", &format!("jitlog {who} {}", rest.first().unwrap_or(&"40"))) {
                Ok(lines) => {
                    let mut code = 0;
                    for l in lines {
                        let Ok(v) = makepad_strict_json::parse(l.as_bytes()) else { continue };
                        if let Some(t) = v.get("line").and_then(|x| x.as_str()) {
                            println!("{t}");
                        } else if let Some(p) = v.get("log").and_then(|x| x.as_str()) {
                            eprintln!("== {node}:{p}");
                        } else if let Some(e) = v.get("err").and_then(|x| x.as_str()) {
                            eprintln!("makepad devhub log: {node}: {e}");
                            code = 1;
                        }
                    }
                    code
                }
                Err(e) => {
                    eprintln!("makepad devhub log: {node}: {e}");
                    2
                }
            }
        }
        ["hubs", rest @ ..] if has(&c, "hosts") => match ask(&c, None, "hosts", "hosts") {
            Ok(l) => {
                for l in &l {
                    match makepad_strict_json::parse(l.as_bytes()).ok().filter(|_| !rest.contains(&"--json")) {
                        Some(v) => {
                            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let tail = match s("state").as_str() {
                                "up" => format!("up  {:.1} ms", num(&v, "rtt_ms")),
                                st => format!("{st}  {}", proto::clean(&s("err"), 120)),
                            };
                            println!("{:<10} {:<22} {tail}", s("host"), s("addr"));
                        }
                        None => println!("{l}"),
                    }
                }
                0
            }
            Err(e) => {
                eprintln!("makepad devhub: {e}");
                2
            }
        },
        ["hubs", ..] => match ask(&c, None, "peers", "peers") {
            Ok(l) => {
                l.iter().for_each(|l| println!("{l}"));
                0
            }
            Err(e) => {
                eprintln!("makepad devhub: {e}");
                2
            }
        },
        _ => {
            eprintln!("{HELP}");
            2
        }
    }
}

/// Prints a lines answer (`text`: inbox rows as their stream line).
fn lines_out(r: Result<Vec<String>, String>, text: bool) -> i32 {
    match r {
        Ok(lines) => {
            let mut code = 0;
            for l in lines {
                code |= l.starts_with("{\"err\":") as i32;
                let shown = text
                    .then(|| makepad_strict_json::parse(l.as_bytes()).ok())
                    .flatten()
                    .and_then(|v| Some(format!("{}{}", v.get("line")?.as_str()?, match v.get("count").and_then(|c| c.as_u64()) { Some(n) if n > 1 => format!("  x{n}"), _ => String::new() })));
                println!("{}", shown.unwrap_or(l));
            }
            code
        }
        Err(e) => {
            eprintln!("makepad devhub: {e}");
            2
        }
    }
}

/// `serve [--node <conf>]`: the hub core without a window or tray (tests, ssh sessions).
fn serve(args: &[&str]) -> i32 {
    let log: crate::trust::Log = std::sync::Arc::new(|l: &str| eprintln!("{l}"));
    let trust = |name: &str| -> Option<std::sync::Arc<dyn crate::trust::Trust>> {
        match crate::trust::FleetTrust::load(name) {
            Ok(t) => Some(std::sync::Arc::new(t)),
            Err(e) => {
                eprintln!("devhub: {e}: this machine's JITs only");
                None
            }
        }
    };
    match args {
        ["--node", conf] => {
            let conf = crate::node::NodeConf::load(std::path::Path::new(conf));
            let t = trust(&conf.name);
            match crate::node::Node::start(conf, t, log) {
                Ok(n) => {
                    eprintln!("devhub: node {} up; {}", n.conf.name, n.addr.map_or_else(|| n.lan_off.clone().unwrap_or_default(), |a| format!("LAN {a}")));
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    eprintln!("devhub: {e}");
                    1
                }
            }
        }
        [] => {
            let name = std::env::var("MAKEPAD_DEVHUB_NAME").ok().filter(|n| crate::trust::valid_name(n)).unwrap_or_else(|| "mac".into());
            match crate::master::Master::start(&name, proto::HITCH_MS, trust(&name), std::sync::Arc::new(|_r| {})) {
                Ok(_m) => {
                    eprintln!("devhub: master {name} up; control {}", proto::control_socket().display());
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    eprintln!("devhub: {e}");
                    1
                }
            }
        }
        _ => {
            eprintln!("devhub serve [--node <conf>]");
            2
        }
    }
}

/// `cache [ls|clear] [--node <n>] [--app <package>] [--force]`: a machine's JIT cache, through
/// its hub's `jitcache` service (this machine's runs in this process when no hub runs here).
fn cache(args: &[&str]) -> i32 {
    let (mut verb, mut node, mut app, mut force, mut dir) = ("plan", None, None, false, None);
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "ls" | "plan" => verb = "plan",
            "clear" => verb = "clear",
            "--force" => force = true,
            "--node" if i + 1 < args.len() => {
                node = Some(args[i + 1]);
                i += 1;
            }
            "--app" if i + 1 < args.len() => {
                app = Some(args[i + 1]);
                i += 1;
            }
            "--dir" if i + 1 < args.len() => {
                dir = Some(args[i + 1]);
                i += 1;
            }
            _ => {
                eprintln!("makepad devhub cache [ls|clear] [--node <n>] [--app <package>] [--dir <run dir>] [--force]");
                return 2;
            }
        }
        i += 1;
    }
    let line = format!("jitcache {verb}{}{}{}", app.map_or(String::new(), |a| format!(" app={a}")), dir.map_or(String::new(), |d| format!(" dir={d}")), if force { " force" } else { "" });
    let answer = match control() {
        Ok(c) => {
            let target = node.filter(|n| *n != local_name(&c));
            ask(&c, target, "jitcache", &line)
        }
        Err(e) if node.is_some() => Err(e),
        Err(_) => {
            let mut out = Vec::new();
            crate::jitcache::service(&proto::default_machine_name(), &line, &mut out, &|_| {});
            Ok(out)
        }
    };
    let who = node.unwrap_or("this machine");
    let lines = match answer {
        Ok(l) => l,
        Err(e) => {
            eprintln!("makepad devhub cache: {who}: {e}");
            return 2;
        }
    };
    let mut code = 0;
    for l in &lines {
        let Ok(v) = makepad_strict_json::parse(l.as_bytes()) else { continue };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        if let Some(e) = v.get("err").and_then(|x| x.as_str()) {
            eprintln!("makepad devhub cache: {who}: {e}");
            code = 1;
        } else if v.get("kept_bytes").is_some() {
            let users: Vec<String> = v
                .get("users")
                .and_then(|u| u.as_arr())
                .map(|u| u.iter().filter_map(|x| Some(format!("{}#{}", x.get("app")?.as_str()?, x.get("pid")?.as_u64()?))).collect())
                .unwrap_or_default();
            let busy = match (users.is_empty(), n("held")) {
                (false, _) => format!("  in use by {}", users.join(", ")),
                (true, h) if h > 0 => "  in use (a makepad run/check/test process)".to_string(),
                _ => String::new(),
            };
            println!("{}/{{{}}}  {} in {} files ({} kept){busy}", s("dir"), crate::jitcache::CLEARED.join(","), crate::jitcache::human(n("bytes")), n("files"), crate::jitcache::human(n("kept_bytes")));
        } else if v.get("total_bytes").is_some() {
            println!("{who}: a clear removes only those folders (never ~/.cargo, target/ builds, downloaded packages or sources)");
            println!("{who}: a clear frees {} in {} cache{}; the next start is cold{}", crate::jitcache::human(n("total_bytes")), n("caches"), if n("caches") == 1 { "" } else { "s" }, if v.get("in_use").and_then(|x| x.as_bool()) == Some(true) { " (in use: clear needs --force)" } else { "" });
        } else if v.get("removed_files").is_some() {
            println!("{}  removed {} files, {}{}", s("dir"), n("removed_files"), crate::jitcache::human(n("freed")), if n("skipped") > 0 { format!(", {} skipped (in use)", n("skipped")) } else { String::new() });
        } else if v.get("freed").is_some() {
            println!("{who}: {}", s("line"));
        }
    }
    code
}
