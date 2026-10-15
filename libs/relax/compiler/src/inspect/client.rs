//! `makepad inspect <pid|app|all|list> <command> [args]`: the client side of the inspect protocol.
//! Finds running apps by their discovery files (makepad-remote-<pid>.json in the temp dir), sends
//! one request line and prints the answer's JSON lines (the `end` line left out). Exit status 1 when
//! the answer had an error, 2 for usage / no such app.
//!
//! `all`: every running app, including ones started later: `watch` returns the first event of any
//! of them; `events follow=1` streams all of them; other commands run on each in turn.

use std::io::Write;

pub struct App {
    pub pid: u32,
    pub app: String,
    pub socket: String,
}

/// The running apps (discovery files `makepad-remote-<pid>.json`; stale ones of ended processes are removed).
pub fn apps() -> Vec<App> {
    makepad_remote::discovery::apps().into_iter().map(|a| App { pid: a.pid, app: a.app, socket: a.socket }).collect()
}

fn usage() -> i32 {
    eprintln!("usage: makepad inspect <pid|app|all|list> <command> [args]   (makepad inspect <pid> help: the commands)");
    eprintln!("       makepad inspect <host>:<app|pid> <command> | <host>:list | <host>:session <launch|kill> .. | hosts | all:list");
    2
}

pub fn main(args: &[String]) -> i32 {
    let Some(target) = args.first() else { return usage() };
    // `file=<x.mkgp> gpu prof <query>`: a saved GPU recording, no app
    if let Some(path) = target.strip_prefix("file=") {
        return super::gpuprof::file_main(path, &args[1..]);
    }
    if target == "list" {
        for a in apps() {
            println!("{{\"pid\":{},\"app\":{},\"socket\":{}}}", a.pid, super::json_str(&a.app), super::json_str(&a.socket));
        }
        return 0;
    }
    if target == "hosts" {
        return remote::hosts();
    }
    if let Some((host, app)) = target.split_once(':') {
        let line = if args.len() > 1 { args[1..].join(" ") } else { "help".to_string() };
        return remote::main(host, app, &line);
    }
    if target == "schema" {
        println!("{}", super::schema());
        return 0;
    }
    let line = if args.len() > 1 { args[1..].join(" ") } else { "help".to_string() };
    let cmd = line.split_whitespace().next().unwrap_or("").to_string();
    if target == "all" {
        return all(&line, &cmd);
    }
    let list = apps();
    let chosen: Vec<&App> = match target.parse::<u32>() {
        Ok(pid) => list.iter().filter(|a| a.pid == pid).collect(),
        Err(_) => list.iter().filter(|a| &a.app == target).collect(),
    };
    let app = match chosen.as_slice() {
        [] => {
            eprintln!("makepad inspect: no running app `{}` (running: {})", target, list.iter().map(|a| format!("{} {}", a.pid, a.app)).collect::<Vec<_>>().join(", "));
            return 2;
        }
        // the most recent of several with that name
        many => many[many.len() - 1],
    };
    match ask(app, &line, &mut |l| {
        println!("{}", l);
        true
    }) {
        Ok(had_err) => {
            if had_err {
                1
            } else {
                0
            }
        }
        Err(e) => {
            eprintln!("makepad inspect: {} (pid {}): {}", app.app, app.pid, e);
            2
        }
    }
}

/// Sends one request; `each` gets every answer line but the end line (false: stop reading).
/// Ok(true) when the answer had an error line.
fn ask(app: &App, line: &str, each: &mut dyn FnMut(&str) -> bool) -> std::io::Result<bool> {
    let to_io = |e: makepad_remote::link::Error| std::io::Error::other(e.to_string());
    let client = makepad_remote::Client::connect(&app.socket).map_err(to_io)?;
    // the commands that are images (`grab`, `gseq`, `gq`) are answered client side from the
    // `frames` service, like `makepad-remote app` does: the same library function
    makepad_remote::app::run_line(&client, line, each).map_err(to_io)
}

/// `all`: every app now and every app that starts while this runs.
fn all(line: &str, cmd: &str) -> i32 {
    let streaming = cmd == "watch" || (cmd == "events" && line.contains("follow=1"));
    if !streaming {
        let mut code = 0;
        for a in apps() {
            println!("{{\"app\":{},\"pid\":{}}}", super::json_str(&a.app), a.pid);
            match ask(&a, line, &mut |l| {
                println!("{}", l);
                true
            }) {
                Ok(true) => code = 1,
                Ok(false) => {}
                Err(e) => println!("{{\"err\":{}}}", super::json_str(&e.to_string())),
            }
        }
        return code;
    }
    // one reader thread per app; `watch` ends at the first event line of any
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut seen: Vec<u32> = Vec::new();
    let once = cmd == "watch";
    loop {
        for a in apps() {
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
                println!("{}", l);
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
                eprintln!("makepad inspect: {e}");
                2
            }
        }
    }

    pub fn main(host: &str, app: &str, line: &str) -> i32 {
        if host == "all" {
            return all(app);
        }
        let (service, target, args, request) = match app {
            "" => {
                eprintln!("makepad inspect: `{host}:` needs an app, a pid, `list` or `session`");
                return 2;
            }
            "list" => ("list", host.to_string(), "", "list".to_string()),
            "session" => ("session", host.to_string(), "", line.to_string()),
            _ => ("cmd", format!("{host}:{app}"), "", line.to_string()),
        };
        let client = match hub_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("makepad inspect: {e}");
                return 2;
            }
        };
        match hub::ask(&client, &target, service, args, &request, &mut |l| {
            println!("{}", if service == "list" { tag(host, l) } else { l.to_string() });
            true
        }) {
            Ok(had_err) => i32::from(had_err),
            Err(e) => {
                eprintln!("makepad inspect: {host}:{app}: {e}");
                2
            }
        }
    }

    /// `all:list`: the apps here, then those of every host in the registry.
    fn all(what: &str) -> i32 {
        if what != "list" {
            eprintln!("makepad inspect: all:{what}: only `all:list` spans hosts");
            return 2;
        }
        for a in super::apps() {
            println!("{{\"host\":\"local\",\"pid\":{},\"app\":{},\"socket\":{}}}", a.pid, super::super::json_str(&a.app), super::super::json_str(&a.socket));
        }
        let reg = match Hosts::load(&registry_path()) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("makepad inspect: {e}");
                return 2;
            }
        };
        if reg.hosts.is_empty() {
            return 0;
        }
        let client = match hub_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("makepad inspect: {e}");
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
        eprintln!("makepad inspect: remote hosts are not available on Windows yet");
        2
    }
    pub fn main(_: &str, _: &str, _: &str) -> i32 {
        hosts()
    }
}
