//! `makepad observe …`: the agent's and the user's door to the local devhub (its control
//! socket). Output is token-lean text; `--json` gives the records as they travel.

use crate::proto::{self, Record, Sev};
use makepad_remote::hub;
use makepad_remote::link::{Client, Target};
use makepad_strict_json::Value;
use std::io::Write;
use std::time::Duration;

const HELP: &str = "makepad devhub (or observe): the local devhub (start it with `cargo run --release -p makepad-app-devhub`)
call $DH; a landing's CLI goes live when its rebuild finishes
  help [<command..>] | <command..> --help  that command's entry (`job result --help`); no words: all of them
  stream [--json] [--all]                 one line per event, live (--all: info lines too)
  ls [machine]                            machines and their JITs, each with what it is up to
  hot [--json]                            hot failures / hot loads, causes and apps (24h and all time)
  inspect <machine>/<pid|app> <cmd...>    any inspect command on that JIT (master: every node)
  top <node>                              CPU, GPU, RAM, disk and the busiest processes of a node
  log <node>/<pid|app> [n]                the tail of a JIT's log (its session log, or its launch log)
  reply <request-id> <status> <message>   answer an engine request (received working deployed declined note)
  inbox [--all] [--since <n>s|m|h]        what needs a look (master), newest first; --since: only entries
                                          seen in that window (by full time: a line shows its time of day only)
  show <id>                               an event with its detail (master inbox)
  clear <id|all>                          remove from the inbox (seen <id|all> marks it read)
  hubs [--json]                           the node hubs this one links to (nodes.conf + the nodes they heard)
  cache [ls|clear] [--node <n>] [--app <package>] [--dir <run dir>] [--force]
                                          the JIT cache (makepad run's compiled code): what a clear frees, or
                                          clear it (the next start is cold; --dir: that one cache); refused
                                          while JITs run on it unless --force (they keep running). `ls` with
                                          no --node adds a line per fleet machine, its dev store named
  serve [--node <conf>]                   run a hub here without its app (master, or a node with that conf)
  master start                            a master in a session of its own, at this shell's priority; its log
                                          is master.log in devhub's home. A niced shell is refused (zsh runs
                                          `&` jobs at +5; restarts keep a level and shown apps inherit it)
  land [<branch|-> [<commercial worktree> <commercial branch>]]
                                          hand in for landing (no args: this worktree's branch and its nested
                                          commercial branch); merges onto the current tips in queue order;
                                          timer buckets check the landed tips afterwards
  land [...] --show <app> [--on <device>] [-- <devhub words>]
                                          the chain: when it lands, show the app there, then run the words
  land status [all|<branch>] [--json]     per hand-in: queued / failed + error lines /
                                          conflict + files / waiting / landed <shas>
  land wait <branch>                      until it lands, fails or conflicts; prints its status (exit 0 landed)
  land hold <branch> [reason] | unhold <branch> | retry <branch> | drop <branch>
  bucket [status]                        the last 10 post-land bucket runs and attributed lanes
  bucket run <fast|tests|hot|ci|full>    that bucket's run now, of the tip then (full: every package's tests)
  land pause [reason] | resume | paused  the whole train: no new round while paused (the menu bar's Pause landing)";

pub(crate) fn control() -> Result<Client, String> {
    let c = Client::new(Target::Unix(proto::control_socket()), "observe");
    c.conn().map_err(|e| format!("no devhub master at {} (connect: {e}); `makepad devhub master start` starts one", proto::control_socket().display()))?;
    agent_seen(&c);
    Ok(c)
}

/// Once a process: tells the master this agent (`DEVHUB_AGENT`) made a devhub call, the mark
/// `lane quiet` holds an overdue lease's agent to ([`crate::lease::master::LeaseMaster::silent_overdue`]).
/// A call that stays open (`job wait`, a test's wait for its lease) is the one call it began as.
fn agent_seen(c: &Client) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Some(agent) = std::env::var("DEVHUB_AGENT").ok().filter(|a| makepad_devhub_conf::valid_name(a)) else { return };
    if has(c, "agent-seen") && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let _ = ask(c, None, "agent-seen", &format!("agent-seen {agent}"));
    }
}

/// Submission waits out master restarts; application errors still stand.
pub(crate) fn retry_master<T>(call: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    retry_master_wait(call, || {
        std::thread::sleep(Duration::from_secs(2));
        control().is_ok()
    })
}

/// `beat` waits a while and says whether the master answers now. Each change is said once, because
/// a call that stays open (`job wait`, `test --live`) can run for long after the master is back:
/// "waiting" as its last line read as a client stuck on a master that had been up for 18 min (11-05).
/// A cut call whose master answers at once (its restart in place: the dial waited on the socket it
/// kept open) is no "not reachable".
pub(crate) fn retry_master_wait<T>(mut call: impl FnMut() -> Result<T, String>, mut beat: impl FnMut() -> bool) -> Result<T, String> {
    let mut told = None;
    loop {
        match call() {
            Ok(v) => return Ok(v),
            Err(e) if e.starts_with("no devhub master at ") || e.contains("link lost") || e.contains("hub connection ended") => {
                let up = beat();
                if told != Some(up) {
                    eprintln!("{}", match (told, up) {
                        (_, false) => "master not reachable: waiting",
                        (None, true) => "the master's link was cut: asking again",
                        (Some(_), true) => "the master is back: asking again",
                    });
                    told = Some(up);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// Sends one line to a lines service and collects the answer.
pub fn ask(c: &Client, target: Option<&str>, service: &str, line: &str) -> Result<Vec<String>, String> {
    ask_lines(c, target, service, line).map_err(|e| older(target, service, e))
}

/// Asks of one node's service, one at a time, on one routed channel ([`ask`] routes each one:
/// [`makepad_remote::hub::Asking`]), opened at the first ask and again after an error.
pub struct NodeAsks {
    node: String,
    service: &'static str,
    ch: Option<hub::Asking>,
}

impl NodeAsks {
    pub fn new(node: &str, service: &'static str) -> NodeAsks {
        NodeAsks { node: node.to_string(), service, ch: None }
    }

    /// [`ask`] of the node's service through `c`, on this channel.
    pub fn ask(&mut self, c: &Client, line: &str) -> Result<Vec<String>, String> {
        let older = |e: String| older(Some(&self.node), self.service, e);
        let ch = match self.ch.take() {
            Some(ch) => ch,
            None => hub::Asking::open(c, &self.node, self.service).map_err(older)?,
        };
        let mut got = Vec::new();
        ch.ask(line, &mut |l| got.push(l.to_string())).map_err(older)?;
        self.ch = Some(ch);
        Ok(got)
    }
}

/// A refusal of a service this client knows means the other side runs older code: say so, and
/// what brings it up to date (a bare "unsupported service native" failed whole buckets, 11-02).
fn older(target: Option<&str>, service: &str, e: String) -> String {
    if !e.contains("unsupported service") {
        return e;
    }
    match target {
        None => format!("the master is older than this client (it has no `{service}` service): run `devhub master restart`"),
        Some(t) => format!("{t}'s hub is older than this client (it has no `{service}` service): it updates itself once its deployment has the landing (`devhub nodes` says when)"),
    }
}

fn ask_lines(c: &Client, target: Option<&str>, service: &str, line: &str) -> Result<Vec<String>, String> {
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
            ch.send(line.as_bytes().to_vec()).map_err(|e| format!("link lost: {e}"))?;
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
                    Some(makepad_remote::Event::Closed(makepad_remote::conn::CLOSE_LINK_LOST, why)) => return Err(format!("link lost: {why}")),
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
    render_machine(&m, out);
}

fn render_machine(m: &crate::explorer::MachineRow, out: &mut String) {
    if let Some(e) = &m.err {
        out.push_str(&format!("{}  error: {}\n", m.name, proto::clean(e, 200)));
        return;
    }
    let battery = m.battery.map_or(String::new(), |(level, charging)| format!("  battery {level}%{}", if charging { " charging" } else { "" }));
    out.push_str(&format!("{}  {}  load {:.1}/{}  mem {}/{} GB free{battery}  {} JITs\n", m.name, m.os, m.load1, m.cpus, m.mem_avail_mb / 1024, m.mem_total_mb / 1024, m.jits.iter().filter(|j| j.jit).count()));
    if let Some(hub) = &m.ai_hub {
        out.push_str(&format!("  {}\n", hub.line()));
    }
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

/// Configured hubs that have stopped also count; unconfigured and unknown nodes do not
/// become false outage reports. Keep the denominator to hubs with a known serving status.
pub fn ai_hub_summary(machines: &[crate::explorer::MachineRow]) -> Option<String> {
    let hubs: Vec<_> = machines.iter().filter(|m| m.err.is_none() && m.ai_hub.as_ref().is_some_and(|h| h.serving.is_some())).collect();
    let down: Vec<_> = hubs.iter().filter(|m| m.ai_hub.as_ref().is_some_and(|h| h.serving == Some(false))).map(|m| proto::clean(&m.name, 64)).collect();
    if down.is_empty() {
        None
    } else {
        Some(format!("AI hub: {} of {} nodes not serving the fleet: {}", down.len(), hubs.len(), down.join(", ")))
    }
}

fn ls(c: &Client, only: Option<&str>) -> i32 {
    let mut out = String::new();
    let mut machines = Vec::new();
    if only.is_none_or(|o| o == local_name(c)) {
        match ask(c, None, "ps", "ps") {
            Ok(lines) => {
                let m = crate::explorer::parse_ps("", &lines);
                render_machine(&m, &mut out);
                machines.push(m);
            }
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
                Ok(lines) => {
                    let mut m = crate::explorer::parse_ps(&name, &lines);
                    m.name = name;
                    render_machine(&m, &mut out);
                    machines.push(m);
                }
                Err(e) => out.push_str(&format!("{name}  down: {}\n", proto::clean(&e, 160))),
            }
        }
        if has(c, "prof") {
            match ask(c, None, "prof", "prof --summary") {
                Ok(lines) => for line in lines {
                    if let Ok(v) = makepad_strict_json::parse(line.as_bytes()) {
                        if let Some(text) = v.get("text").and_then(|v| v.as_str()) { out.push_str(text); }
                        if let Some(e) = v.get("err").and_then(|v| v.as_str()) { out.push_str(&format!("last hour: unavailable ({e})\n")); }
                    }
                },
                Err(e) => out.push_str(&format!("last hour: unavailable ({e})\n")),
            }
        }
    }
    if let Some(summary) = ai_hub_summary(&machines) {
        println!("{summary}");
    }
    if let Ok(lines) = ask(c, None, "hot", "summary") {
        for line in lines { out.push_str(&crate::hot::render(&line, true)); }
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
    // the app's AI tools (`tools`, `call <service>.<tool> ..`) are its `app` service, not a command
    let request = makepad_remote::app::app_request_of(&line);
    // a grab and the other commands read from the app's frames are answered here, on a client of
    // the app (through the hub when it runs elsewhere): the app has none of them
    let here = request.is_some() || makepad_remote::app::client_side(words);
    let r = if let Some(Err(usage)) = &request {
        Err(usage.clone())
    } else if machine == local_name(c) {
        // this machine: the JIT's own socket, exactly `makepad inspect`
        match Client::connect(app) {
            Ok(j) if here => makepad_remote::app::run(&j, words, &mut print).map(|_| ()).map_err(|e| e.to_string()),
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
    } else if let Some(Ok(request)) = &request {
        hub::ask_app(c, &format!("{machine}:{app}"), request, &mut print).map(|_| ())
    } else if here {
        makepad_remote::app::run(&Client::routed(c, &format!("{machine}:{app}")), words, &mut print).map(|_| ()).map_err(|e| e.to_string())
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

fn help_text() -> String {
    #[cfg(feature = "checkout")]
    let orch = format!("{}\n{}", crate::orch::cli::HELP, crate::orch::ci::HELP);
    #[cfg(not(feature = "checkout"))]
    let orch = String::new();
    format!("{HELP}\n{}\n{}\n{orch}", crate::lease::client::HELP, crate::sync::help())
}

/// The words a help request asks about: `help <words>`, `<words> --help`, `<word> help`; they end
/// at the first flag. None: no help asked. What follows `--` is the command's, and so is what
/// follows the name of a command that hands its words on (an app's own `help`).
fn help_words<'a>(a: &[&'a str]) -> Option<Vec<&'a str>> {
    match a {
        ["help", words @ ..] => return Some(words.to_vec()),
        [word, "help"] => return Some(vec![*word]),
        _ => {}
    }
    let own = &a[..a.iter().position(|w| *w == "--").unwrap_or(a.len())];
    let at = own.iter().position(|w| matches!(*w, "--help" | "-h"))?;
    let hands_on: [&[&str]; 5] = [&["inspect"], &["job", "inspect"], &["live", "act"], &["launch"], &["reply"]];
    if hands_on.iter().any(|name| own.starts_with(name) && at > name.len()) {
        return None;
    }
    Some(own[..at].iter().take_while(|w| !w.starts_with('-')).copied().collect())
}

/// One command's entry in the help: its lines, and the names it answers to.
struct HelpEntry<'a> {
    lines: Vec<&'a str>,
    names: Vec<Vec<&'a str>>,
}

/// An entry starts two spaces in; deeper lines continue it. `makepad ci:` starts a section whose
/// entries leave out that word.
fn help_entries(text: &str) -> Vec<HelpEntry<'_>> {
    let mut entries: Vec<HelpEntry> = Vec::new();
    let (mut section, mut open) = (None, false);
    for line in text.lines() {
        match line.strip_prefix("  ") {
            Some(rest) if rest.starts_with(' ') => {
                if open { entries.last_mut().unwrap().lines.push(line); }
            }
            Some(rest) if !rest.is_empty() => {
                entries.push(HelpEntry { lines: vec![line], names: help_names(rest, section) });
                open = true;
            }
            _ => {
                open = false;
                section = line.strip_prefix("makepad ").and_then(|l| l.strip_suffix(':'));
            }
        }
    }
    entries
}

/// The names in an entry's head (up to the gap before its description): each ` | ` form's words
/// up to its first argument; a later form may leave out the first word (`land hold <b> | unhold <b>`).
fn help_names<'a>(head: &'a str, section: Option<&'a str>) -> Vec<Vec<&'a str>> {
    let mut names: Vec<Vec<&str>> = Vec::new();
    for form in head.split("  ").next().unwrap_or("").split(" | ") {
        let name: Vec<&str> = form.split_whitespace().take_while(|w| !w.starts_with(['<', '[', '-'])).collect();
        if let Some(first) = names.first().and_then(|n| n.first()).copied().filter(|f| name.first() != Some(f)) {
            names.push(std::iter::once(first).chain(name.iter().copied()).collect());
        }
        names.push(name);
    }
    if let Some(section) = section {
        for name in &mut names { name.insert(0, section); }
    }
    names
}

/// The lines of every entry that answers to the words; arguments after a command's name
/// (`test <lane> <pkg> --help`) fall away: the longest run of the words that names something.
fn help_for<'a>(text: &'a str, words: &[&str]) -> Vec<&'a str> {
    let entries = help_entries(text);
    for n in (1..=words.len()).rev() {
        let words = &words[..n];
        let lines: Vec<&str> = entries.iter()
            .filter(|e| e.names.iter().any(|name| words.len() <= name.len() && words.iter().zip(name).all(|(w, part)| part.split('|').any(|p| p == *w))))
            .flat_map(|e| e.lines.iter().copied()).collect();
        if !lines.is_empty() { return lines; }
    }
    Vec::new()
}

fn help(words: &[&str]) -> i32 {
    let text = help_text();
    if words.is_empty() {
        println!("{text}");
        return 0;
    }
    let lines = help_for(&text, words);
    if lines.is_empty() {
        eprintln!("makepad devhub: no command `{}`; `makepad devhub help` lists them all", words.join(" "));
        return 2;
    }
    println!("{}", lines.join("\n"));
    0
}

/// `makepad observe <args>`; returns the exit code.
pub fn main(args: &[String]) -> i32 {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    if a.is_empty() {
        println!("{}", help_text());
        return 2;
    }
    if let Some(words) = help_words(&a) {
        return help(&words);
    }
    if a.first() == Some(&"serve") {
        return serve(&a[1..]);
    }
    #[cfg(all(unix, feature = "checkout"))]
    if a == ["master", "start"] {
        return master_start();
    }
    // the master's jobs first: `lanes` (the lane view) is theirs, `lanes now` the merge bot's
    #[cfg(feature = "checkout")]
    if let Some(code) = crate::orch::cli::main(&a) {
        return code;
    }
    #[cfg(feature = "checkout")]
    if let Some(code) = crate::orch::ci::cli(&a) {
        return code;
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
        ["land", rest @ ..] => land(&c, rest),
        #[cfg(feature = "checkout")]
        ["bucket", rest @ ..] => bucket(&c, rest),
        ["ls", rest @ ..] => ls(&c, rest.first().copied()),
        ["hot", rest @ ..] if rest.is_empty() || *rest == ["--json"] => match ask(&c, None, "hot", "summary") {
            Ok(lines) => {
                for line in lines { if rest.contains(&"--json") { println!("{line}"); } else { print!("{}", crate::hot::render(&line, false)); } }
                0
            }
            Err(e) => { eprintln!("makepad devhub hot: {e}"); 2 }
        },
        ["stream", rest @ ..] => stream(&c, rest.contains(&"--json"), rest.contains(&"--all")),
        ["inspect", target, cmd @ ..] if !cmd.is_empty() => inspect(&c, target, &args[2..]),
        ["reply", id, status, msg @ ..] => reply(&c, id, status, &msg.join(" ")),
        ["inbox", rest @ ..] => match inbox_window(rest) {
            Ok(window) => lines_out(ask(&c, None, "inbox", if rest.contains(&"--all") { "list all" } else { "list" }).map(|l| seen_within(l, window, proto::wall_ms())), true),
            Err(e) => {
                eprintln!("makepad devhub inbox: {e}");
                2
            }
        },
        ["show", id] => lines_out(ask(&c, None, "inbox", &format!("show {}", id.trim_start_matches("id="))), false),
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
/// `inbox`'s `--since <n>s|m|h`: the window in ms (None: every entry).
fn inbox_window(args: &[&str]) -> Result<Option<u64>, String> {
    let Some(at) = args.iter().position(|a| *a == "--since") else { return Ok(None) };
    let arg = args.get(at + 1).ok_or("--since needs a window: 20m, 2h, 90s")?;
    let (n, unit) = arg.split_at(arg.find(|c: char| !c.is_ascii_digit()).unwrap_or(arg.len()));
    let n: u64 = n.parse().map_err(|_| format!("--since {arg}: not <n>s|m|h"))?;
    let ms = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return Err(format!("--since {arg}: not <n>s|m|h")),
    };
    Ok(Some(n * ms))
}

/// The inbox lines last seen within `window` ms of `now_ms` (an entry's `last_ms`, a full time:
/// its line's time of day cannot tell yesterday from today).
fn seen_within(lines: Vec<String>, window: Option<u64>, now_ms: u64) -> Vec<String> {
    let Some(window) = window else { return lines };
    lines.into_iter().filter(|l| makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| v.get("last_ms").and_then(|t| t.as_u64())).is_some_and(|t| t + window >= now_ms)).collect()
}

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
    // (no process the hub starts shows an error window on the box's desktop)
    crate::os::quiet_errors();
    crate::procinfo::reap_inherited();
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
    // the conf is read before the hub settles in its home (a relative path names the caller's dir)
    let conf = match args {
        ["--node", conf] => Some(makepad_devhub_conf::NodeConf::load(std::path::Path::new(conf))),
        _ => None,
    };
    if let Err(e) = proto::settle_cwd() {
        eprintln!("devhub: {e}");
        return 1;
    }
    match args {
        ["--node", _] => {
            let conf = conf.expect("read above");
            let t = trust(&conf.name);
            match crate::node::Node::start(conf, t, log) {
                Ok(n) => {
                    eprintln!("devhub: node {} up; {}", n.conf.name, crate::node::lan_said(&n.lan));
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
            let name = std::env::var("MAKEPAD_DEVHUB_NAME").ok().filter(|n| makepad_devhub_conf::valid_name(n)).unwrap_or_else(|| "mac".into());
            match crate::master::Master::start(&name, makepad_devhub_conf::HITCH_MS, trust(&name), std::sync::Arc::new(|_r| {})) {
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
            crate::jitcache::service(&makepad_devhub_conf::default_machine_name(), &line, &mut out, &|_| {});
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
    #[cfg(feature = "checkout")]
    if verb == "plan" && node.is_none() && app.is_none() && dir.is_none() {
        if let Ok(c) = control() {
            cache_fleet(&c);
        }
    }
    code
}

/// The other machines' JIT caches, one line each, with the dev store of their overlay leases:
/// what grows on the boxes, without asking them one by one.
#[cfg(feature = "checkout")]
fn cache_fleet(c: &Client) {
    let Ok(nodes) = crate::sync::fleet_nodes(c) else { return };
    let local = local_name(c);
    let answers: Vec<(String, Result<Vec<String>, String>)> = std::thread::scope(|s| {
        let asks: Vec<_> = nodes.into_iter().filter(|n| *n != local).map(|n| s.spawn(move || {
            let a = ask(c, Some(&n), "jitcache", "jitcache plan");
            (n, a)
        })).collect();
        asks.into_iter().filter_map(|a| a.join().ok()).collect()
    });
    let human = crate::jitcache::human;
    for (n, a) in answers {
        let sum = a.as_ref().ok().and_then(|l| l.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).find(|v| v.get("total_bytes").is_some()));
        match (sum, a) {
            (Some(v), _) => {
                let bytes = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                println!("{n}: {} in {} caches, of it the dev store {} (cap {} GB)", human(bytes("total_bytes")), bytes("caches"), human(bytes("dev_bytes")), crate::lease::relaxgit::DEV_STORE_MB / 1024);
            }
            (None, Err(e)) => println!("{n}: {e}"),
            (None, Ok(_)) => println!("{n}: no answer"),
        }
    }
}

#[cfg(feature = "checkout")]
fn bucket(c: &Client, args: &[&str]) -> i32 {
    if let ["run", name] = args {
        let lines = match ask(c, None, "bucket", &format!("run {name}")) {
            Ok(lines) => lines,
            Err(e) => { eprintln!("makepad devhub bucket: {e}"); return 2; }
        };
        let v = lines.first().and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok());
        return match v.as_ref().and_then(|v| v.get("said")).and_then(Value::as_str) {
            Some(said) => { println!("{said}"); 0 }
            None => { eprintln!("makepad devhub bucket: {}", v.as_ref().and_then(|v| v.get("error")).and_then(Value::as_str).unwrap_or("no answer")); 2 }
        };
    }
    if !matches!(args, [] | ["status"]) {
        eprintln!("makepad devhub bucket [status] | bucket run <bucket>");
        return 2;
    }
    let lines = match ask(c, None, "bucket", "status") {
        Ok(lines) => lines,
        Err(e) => { eprintln!("makepad devhub bucket: {e}"); return 2; }
    };
    for line in lines {
        let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let since = |units: Option<&Value>| match units {
            Some(Value::Obj(units)) => units.iter().map(|(unit, who)| format!("{unit} ({})", crate::bucket::still_since(who.as_str().unwrap_or("?")))).collect::<Vec<_>>().join(", "),
            _ => String::new(),
        };
        if let Some(name) = v.get("still_failing").and_then(Value::as_str) {
            // each with the log of its last look, the failure's own lines in it
            let units = match v.get("units") {
                Some(Value::Obj(units)) => units.iter().map(|(unit, who)| {
                    let log = v.get("logs").and_then(|l| l.get(unit)).and_then(Value::as_str).filter(|l| !l.is_empty()).map_or(String::new(), |l| format!("; log {l}"));
                    format!("{unit} ({}{log})", crate::bucket::still_since(who.as_str().unwrap_or("?")))
                }).collect::<Vec<_>>().join(", "),
                _ => String::new(),
            };
            println!("{name} STILL FAILING: {units}");
            continue;
        }
        let tip = v.get("tip").map(|p| crate::state::Pair::new(&text(p, "work"), &text(p, "main")).short()).unwrap_or_default();
        let ok = v.get("ok").and_then(Value::as_bool) == Some(true);
        let failing = v.get("failing").and_then(Value::as_arr).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let labels = v.get("landings").and_then(Value::as_arr).map(|a| a.iter()
            .filter(|l| l.get("units").and_then(Value::as_arr).is_some_and(|u| !u.is_empty()))
            .map(|l| text(l, "label")).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let still = since(v.get("still"));
        let still = if still.is_empty() { still } else { format!("; not rerun: {still}") };
        // new warnings (a WARN, the row green) and who brought them
        let warns = match v.get("warns") {
            Some(Value::Obj(w)) if !w.is_empty() => format!("; warns: {}", w.iter().map(|(u, who)| format!("{u} ({})", who.as_str().unwrap_or("?"))).collect::<Vec<_>>().join(", ")),
            _ => String::new(),
        };
        println!("{} {tip} {} {failing}{}{still}{warns}", text(&v, "bucket"), if ok { "ok" } else { "FAILED" }, if labels.is_empty() { String::new() } else { format!(": {labels}") });
        // the warnings themselves, file:line, so the owning lane is plain without a rerun
        // and the command that shows a unit's own lines alone, for its lane to run
        let repro = |unit: &str| v.get("repro").and_then(|r| r.get(unit)).and_then(Value::as_str);
        let warned: &[(String, Value)] = match v.get("warn_lines") { Some(Value::Obj(units)) => units, _ => &[] };
        for (unit, lines) in warned {
            for l in lines.as_arr().unwrap_or_default().iter().filter_map(Value::as_str).take(3) {
                println!("    {unit}: {l}");
            }
            if let Some(cmd) = repro(unit) { println!("    {}", crate::bucket::repro_said(unit, cmd)); }
        }
        // a failing unit's own lines (its errors, the tests a hung one still ran), then its command
        let errors: &[(String, Value)] = match v.get("errors") { Some(Value::Obj(units)) => units, _ => &[] };
        if let Some(Value::Obj(units)) = v.get("repro") {
            for (unit, cmd) in units.iter().filter(|(u, _)| !warned.iter().any(|(w, _)| w == u)) {
                for l in errors.iter().filter(|(e, _)| crate::bucket::unit_key(e) == *unit).flat_map(|(_, l)| l.as_arr().unwrap_or_default()).filter_map(Value::as_str).take(3) {
                    println!("    {unit}: {l}");
                }
                if let Some(cmd) = cmd.as_str() { println!("    {}", crate::bucket::repro_said(unit, cmd)); }
            }
        }
    }
    0
}

/// `land …`: the master's landing train ([`crate::land`]).
fn land(c: &Client, a: &[&str]) -> i32 {
    let show = |lines: &[String], full: bool, json: bool| {
        let now = proto::wall_ms();
        for l in lines {
            if json {
                println!("{l}");
                continue;
            }
            let Some(e) = crate::land::Entry::parse(l) else {
                println!("{l}");
                continue;
            };
            let age = now.saturating_sub(e.changed_ms) / 1000;
            let age = if age >= 3600 { format!("{}h", age / 3600) } else if age >= 60 { format!("{}m", age / 60) } else { format!("{age}s") };
            let mut line = format!("#{} {} {}", e.id, e.label(), e.state.as_str());
            if !e.landed.is_empty() {
                line += &format!(" {}", e.landed);
            }
            if !e.held.is_empty() {
                line += &format!(" HELD: {}", e.held);
            }
            println!("{line} {age}");
            let n = if full { usize::MAX } else { 12 };
            for d in e.detail.iter().take(n) {
                println!("  | {d}");
            }
            if e.detail.len() > n {
                println!("  | … {} more (land status {})", e.detail.len() - n, e.label());
            }
        }
    };
    let ask_land = |line: &str| -> Result<Vec<String>, i32> {
        match ask(c, None, "land", line) {
            Ok(l) => match l.iter().find_map(|x| makepad_strict_json::parse(x.as_bytes()).ok().and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string))) {
                Some(e) => {
                    eprintln!("makepad devhub land: {e}");
                    Err(1)
                }
                None => Ok(l),
            },
            Err(e) => {
                eprintln!("makepad devhub land: {e} (an older master without the train? restart devhub)");
                Err(2)
            }
        }
    };
    match a {
        ["status", rest @ ..] => {
            let json = rest.contains(&"--json");
            let which: Vec<&str> = rest.iter().copied().filter(|x| *x != "--json").collect();
            match ask_land(&format!("status {}", which.join(" "))) {
                Ok(l) => {
                    if l.is_empty() {
                        println!("nothing queued");
                    }
                    show(&l, !which.is_empty() && which[0] != "all", json);
                    0
                }
                Err(code) => code,
            }
        }
        ["wait", b] => loop {
            match ask_land(&format!("status {b}")) {
                Ok(l) => {
                    let Some(e) = l.last().and_then(|x| crate::land::Entry::parse(x)) else {
                        eprintln!("makepad devhub land: no entry {b}");
                        return 1;
                    };
                    if matches!(e.state, crate::land::St::Landed | crate::land::St::Failed | crate::land::St::Conflict) {
                        show(&l[l.len() - 1..], true, false);
                        return if e.state == crate::land::St::Landed { 0 } else { 1 };
                    }
                }
                Err(code) => return code,
            }
            std::thread::sleep(Duration::from_secs(5));
        },
        [verb @ ("pause" | "resume" | "paused"), rest @ ..] => match ask_land(&format!("{verb} {}", rest.join(" "))) {
            Ok(l) => {
                let why = l.first().and_then(|x| makepad_strict_json::parse(x.as_bytes()).ok()).and_then(|v| v.get("paused").and_then(|p| p.as_str()).map(str::to_string)).unwrap_or_default();
                println!("{}", if why.is_empty() { "landing runs".to_string() } else { format!("landing paused: {why}") });
                0
            }
            Err(code) => code,
        },
        [verb @ ("hold" | "unhold" | "retry" | "drop" | "import"), rest @ ..] => match ask_land(&format!("{verb} {}", rest.join(" "))) {
            Ok(_) => {
                println!("{verb} {}: ok", rest.join(" "));
                0
            }
            Err(code) => code,
        },
        _ => {
            // the chain: `land [...] --show <app> [--on <device>] [-- <devhub words>]`: when this
            // lands, show the app on the device, then run the words; one command, nobody polls
            let (a, then) = match a.iter().position(|x| *x == "--") {
                Some(i) => (&a[..i], &a[i + 1..]),
                None => (a, &a[a.len()..]),
            };
            let flag = |w: &str| a.iter().position(|x| *x == w).and_then(|i| a.get(i + 1).copied());
            let (chain_app, chain_on) = (flag("--show"), flag("--on").unwrap_or("mac"));
            let mut plain: Vec<&str> = Vec::new();
            let mut k = 0;
            while k < a.len() {
                if a[k] == "--show" || a[k] == "--on" {
                    k += 2;
                    continue;
                }
                plain.push(a[k]);
                k += 1;
            }
            if chain_app.is_none() && !then.is_empty() {
                eprintln!("makepad devhub land: `-- <words>` runs after a `--show <app>`");
                return 2;
            }
            let a = plain.as_slice();
            // a hand-in: given, or this worktree's branch (+ its nested commercial branch when it is on one)
            let args: Vec<String> = if a.is_empty() {
                let g = |dir: &str, args: &[&str]| crate::git_exec::command().arg("-C").arg(dir).args(args).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
                let Some(top) = g(".", &["rev-parse", "--show-toplevel"]) else {
                    eprintln!("makepad devhub land: run it in a lane worktree, or name the branch");
                    return 2;
                };
                let b = g(&top, &["branch", "--show-current"]).unwrap_or_default();
                let cdir = format!("{top}/apps/commercial");
                let cb = g(&cdir, &["branch", "--show-current"]).unwrap_or_default();
                let mut v = vec![if b.is_empty() || b == "work" { "-".to_string() } else { b }];
                if !cb.is_empty() && cb != "main" {
                    v.extend([cdir, cb]);
                }
                if v.len() == 1 && v[0] == "-" {
                    eprintln!("makepad devhub land: this worktree is on no lane branch");
                    return 2;
                }
                v
            } else {
                a.iter().map(|x| x.to_string()).collect()
            };
            let agent = std::env::var("DEVHUB_AGENT").ok().filter(|x| makepad_devhub_conf::valid_name(x)).map(|x| format!(" agent={x}")).unwrap_or_default();
            match ask_land(&format!("add {}{agent}", args.join(" "))) {
                Ok(l) => {
                    let v = l.first().and_then(|x| makepad_strict_json::parse(x.as_bytes()).ok());
                    let n = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_u64()).unwrap_or(0);
                    let s = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_str());
                    let key = if args[0] == "-" { args.last().unwrap() } else { &args[0] };
                    let label = s("label").map(str::to_string).unwrap_or_else(|| args.join(" "));
                    let wait_key = s("label").unwrap_or(key);
                    let included = s("included").map(|b| format!(" (+ {b} from the lane)")).unwrap_or_default();
                    println!("queued #{} {label}{included} ({} open; `makepad devhub land wait {wait_key}`)", n("id"), n("pos"));
                    let Some(app) = chain_app else { return 0 };
                    let by = std::env::var("DEVHUB_AGENT").ok().filter(|x| !x.is_empty()).unwrap_or_else(|| key.to_string());
                    let j = makepad_remote::json_str;
                    let list = |w: &[&str]| w.iter().map(|x| j(x)).collect::<Vec<_>>().join(",");
                    let shown = ask(c, None, "show", &format!("show add {{\"app\":{},\"on\":{},\"after\":[\"landed\",{}],\"replace\":false,\"by\":{}}}", j(app), j(chain_on), j(key), j(&by)));
                    match &shown {
                        Ok(l) if !l.iter().any(|x| x.starts_with("{\"err\"")) => println!("then: show {app} on {chain_on} when it lands"),
                        Ok(l) => {
                            eprintln!("makepad devhub land: the show was not queued: {}", l.join(" "));
                            return 1;
                        }
                        Err(e) => {
                            eprintln!("makepad devhub land: the show was not queued: {e}");
                            return 1;
                        }
                    }
                    if then.is_empty() {
                        return 0;
                    }
                    match ask(c, None, "after", &format!("after add {{\"by\":{},\"cond\":[\"shown\",{},{}],\"cmd\":[{}]}}", j(&by), j(app), j(chain_on), list(then))) {
                        Ok(l) if !l.iter().any(|x| x.starts_with("{\"err\"")) => {
                            println!("then: `devhub {}` once it runs", then.join(" "));
                            0
                        }
                        Ok(l) => {
                            eprintln!("makepad devhub land: the action was not queued: {}", l.join(" "));
                            1
                        }
                        Err(e) => {
                            eprintln!("makepad devhub land: the action was not queued: {e}");
                            1
                        }
                    }
                }
                Err(code) => code,
            }
        }
    }
}

/// `devhub master start`: `serve` in a session of its own, so it outlives this shell, at this shell's
/// priority. It waits until the master answers or its start has failed. A `serve &` by hand in zsh
/// ran at nice 5 from then on ([`crate::master::start_level`]).
#[cfg(all(unix, feature = "checkout"))]
fn master_start() -> i32 {
    let fail = |why: String| {
        eprintln!("devhub: {why}");
        1
    };
    let socket = proto::control_socket();
    let answers = || Client::new(Target::Unix(socket.clone()), "probe").conn().is_ok();
    if answers() {
        return fail(format!("a devhub already serves {}", socket.display()));
    }
    if let Err(why) = crate::master::start_level() {
        return fail(why);
    }
    let home = proto::home();
    let log = home.join("master.log");
    let out = std::fs::create_dir_all(&home).and_then(|()| {
        // (the last master's log says why a new one had to be started)
        let _ = std::fs::rename(&log, home.join("master.log.old"));
        std::fs::File::create(&log)
    });
    let Ok((out, err)) = out.and_then(|o| o.try_clone().map(|e| (o, e))).map_err(|e| fail(format!("{}: {e}", log.display()))) else { return 1 };
    let own = own_argv(&crate::lease::wt::default_repo());
    let mut cmd = std::process::Command::new(&own[0]);
    cmd.args(&own[1..]).arg("serve").env_remove("DEVHUB_AGENT").env_remove("DEVHUB_LANE").stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    let mut child = match crate::os::lease_spawn(&mut cmd, None) {
        Ok((child, _hold)) => child,
        Err(e) => return fail(format!("{}: {e}", own[0])),
    };
    loop {
        if answers() {
            println!("devhub: master up, pid {}; its log is {}", child.id(), log.display());
            return 0;
        }
        match child.try_wait() {
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Ok(Some(status)) => return fail(format!("the master ended at its start ({status}); see {}", log.display())),
            Err(e) => return fail(format!("the master's start: {e}")),
        }
    }
}

/// How devhub runs its own words as a child: this very binary when it is `makepad-devhub` (the
/// master, the CLI), else (devhub linked into an app) the built one in `repo` (the master swaps a
/// landing's in when its rebuild finishes); cargo only builds it when there is none yet.
pub fn own_argv(repo: &std::path::Path) -> Vec<String> {
    if let Some(exe) = std::env::current_exe().ok().filter(|e| e.file_stem().is_some_and(|s| s == "makepad-devhub")) {
        return vec![exe.display().to_string()];
    }
    let built = repo.join("target/host").join(format!("makepad-devhub{}", std::env::consts::EXE_SUFFIX));
    if built.is_file() {
        return vec![built.display().to_string()];
    }
    let mut v: Vec<String> = ["cargo", "run", "--profile", "host", "-q", "--manifest-path"].iter().map(|s| s.to_string()).collect();
    v.push(repo.join("Cargo.toml").display().to_string());
    v.extend(["-p", "makepad-devhub", "--bin", "makepad-devhub", "--"].iter().map(|s| s.to_string()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lanes piped the whole help through grep 777 times in 36 h; `job result --help` answered
    /// "no job --help" (11-04).
    #[cfg(feature = "checkout")]
    #[test]
    fn a_commands_help_is_its_own_entry() {
        let text = help_text();
        let lines = |words: &[&str]| help_for(&text, words);
        for (words, head) in [
            (&["job", "result"][..], "  job wait <id..>"),
            (&["job", "submit"], "  job submit [--kind"),
            (&["job", "wait"], "  job wait <id..>"),
            (&["job", "cancel"], "  job cancel <id..>"),
            (&["test"], "  test <lane> <pkg>"),
            (&["run"], "  run <lane> <kind>"),
            (&["lane", "new"], "  lane new <name>"),
            (&["lane", "keep"], "  lane keep|unkeep"),
            (&["lane", "unkeep"], "  lane keep|unkeep"),
            (&["live", "start"], "  live start <app>"),
            (&["live", "look"], "  live look <id>"),
            (&["live", "release"], "  live sync <id>"),
            (&["land", "wait"], "  land wait <branch>"),
            (&["land", "retry"], "  land hold <branch>"),
            (&["queue"], "  queue | leases"),
            (&["leases"], "  queue | leases"),
            (&["nodes"], "  nodes "),
            (&["why-queued"], "  why-queued <lease>"),
            (&["ci", "status"], "  status [--json]"),
            // arguments after the name fall away
            (&["test", "my-lane", "my-pkg"], "  test <lane> <pkg>"),
            (&["job", "result", "J1"], "  job wait <id..>"),
        ] {
            let got = lines(words);
            assert!(got.first().is_some_and(|l| l.starts_with(head)), "{words:?}: {got:?}");
            assert!(got.len() < 40, "{words:?}: {} lines", got.len());
        }
        // an entry's continuation lines come with it, a command's every form too, and a group
        // word names its commands
        assert!(lines(&["job", "submit"]).iter().any(|l| l.contains("[--whole]")));
        assert!(lines(&["live", "start"]).iter().any(|l| l.starts_with("  live start <pkg> --test")));
        assert!(lines(&["job"]).iter().any(|l| l.starts_with("  job submit")) && lines(&["job"]).iter().any(|l| l.starts_with("  job cancel")));
        assert!(lines(&["no-such-command"]).is_empty());
    }

    #[test]
    fn help_words_leave_what_goes_to_the_command() {
        fn w(line: &str) -> Option<Vec<&str>> {
            help_words(&line.split(' ').collect::<Vec<_>>())
        }
        assert_eq!(w("--help"), Some(vec![]));
        assert_eq!(w("help"), Some(vec![]));
        assert_eq!(w("help job result"), Some(vec!["job", "result"]));
        assert_eq!(w("job result --help"), Some(vec!["job", "result"]));
        assert_eq!(w("job result J1 -h"), Some(vec!["job", "result", "J1"]));
        assert_eq!(w("job help"), Some(vec!["job"]));
        assert_eq!(w("test a-lane a-pkg --lib --help"), Some(vec!["test", "a-lane", "a-pkg"]));
        assert_eq!(w("live act --help"), Some(vec!["live", "act"]));
        assert_eq!(w("job list"), None);
        assert_eq!(w("test a-lane a-pkg -- --help"), None);
        assert_eq!(w("job submit -- cargo --help"), None);
        assert_eq!(w("inspect n1/12 help"), None);
        assert_eq!(w("inspect n1/12 --help"), None);
        assert_eq!(w("live act W1 help"), None);
        assert_eq!(w("live act W1 --help"), None);
    }

    /// A check-in's "last 20 min" compared time-of-day strings: an entry 23 h old (23:06:31Z
    /// against a window from 21:50Z) showed as recent.
    #[test]
    fn inbox_since_is_by_full_time() {
        assert_eq!(inbox_window(&["--all", "--since", "20m"]), Ok(Some(20 * 60_000)));
        assert_eq!(inbox_window(&["--since", "2h"]), Ok(Some(2 * 3_600_000)));
        assert_eq!(inbox_window(&["--all"]), Ok(None));
        assert!(inbox_window(&["--since"]).is_err() && inbox_window(&["--since", "20"]).is_err() && inbox_window(&["--since", "m"]).is_err());
        let now = 1_793_744_000_000u64;
        let entry = |id: &str, last_ms: u64| format!("{{\"id\":\"{id}\",\"count\":1,\"seen\":false,\"last_ms\":{last_ms},\"line\":\"23:06:31Z WARN x\"}}");
        let lines = vec![entry("old", now - 23 * 3_600_000), entry("new", now - 5 * 60_000), entry("edge", now - 20 * 60_000)];
        let kept: Vec<String> = seen_within(lines.clone(), Some(20 * 60_000), now).iter().map(|l| l.split('"').nth(3).unwrap().to_string()).collect();
        assert_eq!(kept, ["new", "edge"]);
        assert_eq!(seen_within(lines.clone(), None, now), lines);
    }
}
