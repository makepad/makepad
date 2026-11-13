//! `makepad inspect <pid|app|all|list> <command> [args]`: the client side of the inspect protocol.
//! Finds running apps by their discovery files (makepad-remote-<pid>.json in the registry dir, `makepad_remote::discovery::registry_dir`), sends
//! one request line and prints the answer's JSON lines (the `end` line left out). Exit status 1 when
//! the answer had an error, 2 for usage / no such app.
//!
//! `all`: every running app, including ones started later: `watch` returns the first event of any
//! of them; `events follow=1` streams all of them; other commands run on each in turn.
//!
//! An app name and `all` reach only what this caller started, or anything for someone at a
//! terminal; other apps are named by pid ([`makepad_remote::discovery::Caller`]).

use std::io::Write;

pub use makepad_remote::discovery::App;
use makepad_remote::discovery::{named, Caller};

/// The running apps (discovery files `makepad-remote-<pid>.json`; stale ones of ended processes are removed).
pub fn apps() -> Vec<App> {
    makepad_remote::discovery::apps()
}

fn usage() -> i32 {
    eprintln!("usage: {tool} inspect <pid|list> <command> [args]   ({tool} inspect <pid> help: the commands)", tool = crate::tool_name());
    eprintln!("       {tool} inspect <app|all> <command> [args]   (at a terminal, or the apps of your own lease; anything else is named by pid)", tool = crate::tool_name());
    eprintln!("       {tool} inspect <host>:<pid> <command> | <host>:list | <host>:session <launch|kill> .. | hosts | all:list", tool = crate::tool_name());
    eprintln!("       {tool} inspect <pid|app> pf <query> [s=3] [save=x.mkpf] | file=<x.mkpf> pf <query> [base=y.mkpf]   (pf help: the profiler queries)", tool = crate::tool_name());
    2
}

pub fn main(args: &[String]) -> i32 {
    // `as=json` is the client's own option: it never goes into the request (an `eval` expression
    // or a probe's code would take it as text)
    let text_out = !json_wanted(args);
    // (`as=md` is the default text: the same renderer)
    let args: Vec<String> = args.iter().filter(|w| *w != "as=json" && *w != "--json" && *w != "as=md").cloned().collect();
    let args = &args[..];
    let Some(target) = args.first() else { return usage() };
    // `file=<x.mkgp> gpu prof <query>`: a saved GPU recording, no app
    if let Some(path) = target.strip_prefix("file=") {
        // a flight recorder recording replayed into a hidden instance
        if args.get(1).map(String::as_str) == Some("rec") && args.get(2).map(String::as_str) == Some("replay") {
            return super::replay::file_main(path, &args[3..]);
        }
        // a profile recording (`file=<x.mkpf> pf ...`)
        if path.ends_with(".mkpf") || args.get(1).map(String::as_str) == Some("pf") {
            return super::pf::file_main(path, &args[1..]);
        }
        // a file a host provider reads (`file=<x.mkcap> cap ...`), else a GPU recording
        if let Some(code) = super::host_file_query(path, &args[1..]) {
            return code;
        }
        return super::gpuprof::file_main(path, &args[1..]);
    }
    if target == "list" {
        for a in apps() {
            let error = a.text("socket_error").map_or(String::new(), |why| format!(",\"socket_error\":{}", super::json_str(&why)));
            println!("{{\"pid\":{},\"app\":{},\"socket\":{}{}}}", a.pid, super::json_str(&a.app), super::json_str(&a.socket), error);
        }
        return 0;
    }
    if target == "hosts" {
        return remote::hosts();
    }
    if let Some((host, app)) = target.split_once(':') {
        let line = makepad_remote::app::typed_line(&args[1..]);
        return remote::main(host, app, &line, text_out);
    }
    if target == "schema" {
        println!("{}", super::schema());
        return 0;
    }
    let line = makepad_remote::app::typed_line(&args[1..]);
    let cmd = line.split_whitespace().next().unwrap_or("").to_string();
    let text = text_out;
    let caller = Caller::command_line();
    let list = apps();
    if target == "all" {
        if let Some(why) = caller.all_refused(&list) {
            eprintln!("relax inspect: {why}");
            return 2;
        }
        return all(&line, &cmd, text, &caller);
    }
    let chosen = match target.parse::<u32>() {
        Ok(pid) => list.iter().rfind(|a| a.answers_to(pid)).cloned(),
        Err(_) if list.iter().any(|a| &a.app == target) => match named(&list, target, &caller) {
            Ok(a) => Some(a),
            Err(why) => {
                eprintln!("relax inspect: {why}");
                return 2;
            }
        },
        Err(_) => None,
    };
    let Some(app) = &chosen else {
        eprintln!("relax inspect: no running app `{}` (running: {})", target, list.iter().map(|a| format!("{} {}", a.pid, a.app)).collect::<Vec<_>>().join(", "));
        return 2;
    };
    if let Some(why) = app.unreachable() {
        eprintln!("relax inspect: {why}");
        return 2;
    }
    if args.get(1).map(String::as_str) == Some("pf") {
        return super::pf::live_main(app, &args[2..]);
    }
    let streaming = cmd == "watch" || (cmd == "events" && line.contains("follow=1"));
    let mut got: Vec<String> = Vec::new();
    let r = ask(app, &line, &mut |l| {
        if !text {
            println!("{}", l);
        } else if streaming {
            println!("{}", super::answer::render_streamed(&line, l));
            let _ = std::io::stdout().flush();
        } else {
            got.push(l.to_string());
        }
        true
    });
    if text && !streaming {
        print!("{}", super::answer::render(&line, &got));
    }
    match r {
        Ok(had_err) => {
            if had_err {
                1
            } else {
                0
            }
        }
        Err(e) => {
            eprintln!("relax inspect: {} (pid {}): {}", app.app, app.pid, e);
            2
        }
    }
}

/// `as=json` among the words: the answer's JSON lines, unrendered (for programs).
fn json_wanted(words: &[String]) -> bool {
    words.iter().any(|w| w == "as=json" || w == "--json")
}

/// Sends one request; `each` gets every answer line but the end line (false: stop reading).
/// Ok(true) when the answer had an error line.
fn ask(app: &App, line: &str, each: &mut dyn FnMut(&str) -> bool) -> std::io::Result<bool> {
    let to_io = |e: makepad_remote::link::Error| std::io::Error::other(e.to_string());
    if let Some(why) = app.unreachable() {
        return Err(std::io::Error::other(why));
    }
    let client = makepad_remote::Client::connect(&app.socket).map_err(to_io)?;
    // the commands that are images (`grab`, `gseq`, `gq`) are answered client side from the
    // `frames` service, like `makepad-remote app` does: the same library function
    makepad_remote::app::run_line(&client, line, each).map_err(to_io)
}

/// `all`: every app of `caller` now and every one that starts while this runs.
fn all(line: &str, cmd: &str, text: bool, caller: &Caller) -> i32 {
    let streaming = cmd == "watch" || (cmd == "events" && line.contains("follow=1"));
    if !streaming {
        let mut code = 0;
        for a in apps().into_iter().filter(|a| caller.reaches(a)) {
            if text {
                println!("## {} (pid {})", a.app, a.pid);
            } else {
                println!("{{\"app\":{},\"pid\":{}}}", super::json_str(&a.app), a.pid);
            }
            let mut got = Vec::new();
            match ask(&a, line, &mut |l| {
                if text {
                    got.push(l.to_string());
                } else {
                    println!("{}", l);
                }
                true
            }) {
                Ok(true) => code = 1,
                Ok(false) => {}
                Err(e) if text => println!("error: {}", e),
                Err(e) => println!("{{\"err\":{}}}", super::json_str(&e.to_string())),
            }
            if text {
                print!("{}", super::answer::render(line, &got));
            }
        }
        return code;
    }
    // one reader thread per app; `watch` ends at the first event line of any
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut seen: Vec<u32> = Vec::new();
    let once = cmd == "watch";
    loop {
        for a in apps().into_iter().filter(|a| caller.reaches(a)) {
            if seen.contains(&a.pid) {
                continue;
            }
            seen.push(a.pid);
            let tx = tx.clone();
            let line = line.to_string();
            { let _k = crate::zone_alloc::kept();
            std::thread::spawn(move || {
                let _ = ask(&a, &line, &mut |l| {
                    // only events (watch answers are events; follow streams start with a header)
                    if l.starts_with("{\"event\":") {
                        return tx.send(l.to_string()).is_ok();
                    }
                    true
                });
            });
            }
        }
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(l) => {
                if once && l.starts_with("{\"event\":\"none\"") {
                    continue;
                }
                if text {
                    println!("{}", super::answer::render_streamed(line, &l));
                } else {
                    println!("{}", l);
                }
                let _ = std::io::stdout().flush();
                if once {
                    return 0;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return 0,
        }
    }
}

/// Addresses with a host part (`149:stage`), `hosts` and `all:list`: through the hub.
#[cfg(unix)]
mod remote {
    use makepad_remote::hosts::{registry_path, Hosts};
    use makepad_remote::hub;

    fn hub_client() -> Result<makepad_remote::Client, String> {
        hub::connect(std::env::current_exe().ok().as_deref())
    }

    /// `{"host":"<h>",` in front of a JSON object line.
    fn tag(host: &str, line: &str) -> String {
        match line.strip_prefix('{') {
            Some(rest) if rest != "}" => format!("{{\"host\":{},{rest}", super::super::json_str(host)),
            _ => line.to_string(),
        }
    }

    pub fn hosts() -> i32 {
        match hub_client().and_then(|c| {
            hub::hosts(&c, &mut |l| {
                println!("{l}");
                true
            })
        }) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("relax inspect: {e}");
                2
            }
        }
    }

    pub fn main(host: &str, app: &str, line: &str, text: bool) -> i32 {
        if host == "all" {
            return all(app);
        }
        let (service, target, args, request) = match app {
            "" => {
                eprintln!("relax inspect: `{host}:` needs an app, a pid, `list` or `session`");
                return 2;
            }
            "list" => ("list", host.to_string(), "", "list".to_string()),
            "session" => ("session", host.to_string(), "", line.to_string()),
            _ => ("cmd", format!("{host}:{app}"), "", line.to_string()),
        };
        let client = match hub_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("relax inspect: {e}");
                return 2;
            }
        };
        let render = text && service == "cmd";
        let mut got = Vec::new();
        let r = hub::ask(&client, &target, service, args, &request, &mut |l| {
            if render {
                got.push(l.to_string());
            } else {
                println!("{}", if service == "list" { tag(host, l) } else { l.to_string() });
            }
            true
        });
        if render {
            print!("{}", super::super::answer::render(&request, &got));
        }
        match r {
            Ok(had_err) => i32::from(had_err),
            Err(e) => {
                eprintln!("relax inspect: {host}:{app}: {e}");
                2
            }
        }
    }

    /// `all:list`: the apps here, then those of every host in the registry.
    fn all(what: &str) -> i32 {
        if what != "list" {
            eprintln!("relax inspect: all:{what}: only `all:list` spans hosts");
            return 2;
        }
        for a in super::apps() {
            println!("{{\"host\":\"local\",\"pid\":{},\"app\":{},\"socket\":{}}}", a.pid, super::super::json_str(&a.app), super::super::json_str(&a.socket));
        }
        let reg = match Hosts::load(&registry_path()) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("relax inspect: {e}");
                return 2;
            }
        };
        if reg.hosts.is_empty() {
            return 0;
        }
        let client = match hub_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("relax inspect: {e}");
                return 2;
            }
        };
        let mut code = 0;
        for h in &reg.hosts {
            if let Err(e) = hub::ask(&client, &h.name, "list", "", "list", &mut |l| {
                println!("{}", tag(&h.name, l));
                true
            }) {
                println!("{{\"host\":{},\"err\":{}}}", super::super::json_str(&h.name), super::super::json_str(&e));
                code = 1;
            }
        }
        code
    }
}

#[cfg(not(unix))]
mod remote {
    pub fn hosts() -> i32 {
        eprintln!("relax inspect: remote hosts are not available on Windows yet");
        2
    }
    pub fn main(_: &str, _: &str, _: &str, _: bool) -> i32 {
        hosts()
    }
}
