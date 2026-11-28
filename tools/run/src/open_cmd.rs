//! Follow the per-app status written by the Runner's focus mode (runner/src/focus.rs).
use makepad_remote::procs::is_alive;
use relax_runner::HelpLine::{Blank, Text, Usage};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const COMMAND: relax_runner::HostCommand = relax_runner::HostCommand {
    name: "open",
    row: ("open <app id>", "Build and start an app with the Runner"),
    title: "open an app with the Runner",
    page: &[
        Blank,
        Usage("makepad open <app id>"),
        Blank,
        Text("The Runner builds and starts the app. This command prints progress and then"),
        Text("the shell commands for talking to the app. The Runner keeps going if you leave."),
        Text("Run the command again to attach to a run in progress."),
    ],
    main,
};

#[derive(Default)]
struct Status<'a> {
    runner: u32,
    title: &'a str,
    started_ms: u64,
    stage: &'a str,
    done: u64,
    total: u64,
    ready: Option<(u32, &'a str)>,
    failed: Option<(&'a str, &'a str)>,
}

fn parse<'a>(text: &'a str, id: &'a str) -> Status<'a> {
    let mut status = Status { title: id, ..Status::default() };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('\t') else { continue };
        match key {
            "runner" => status.runner = value.parse().unwrap_or(0),
            "title" if !value.is_empty() => status.title = value,
            "started" => status.started_ms = value.parse().unwrap_or(0),
            "stage" => status.stage = value,
            "done" => status.done = value.parse().unwrap_or(0),
            "total" => status.total = value.parse().unwrap_or(0),
            "ready" => {
                status.ready = value.split_once('\t').and_then(|(pid, socket)| Some((pid.parse().ok()?, socket)));
            }
            "failed" => status.failed = value.split_once('\t'),
            _ => {}
        }
    }
    status
}

#[derive(Debug, PartialEq)]
enum Next {
    Attach(u32),
    Replace(u32),
    Start,
}

fn next_step(status: &Status<'_>, alive: impl Fn(u32) -> bool) -> Next {
    if status.runner == 0 || !alive(status.runner) {
        return Next::Start;
    }
    match status.stage {
        "failed" => Next::Replace(status.runner),
        "ready" if !status.ready.is_some_and(|(pid, _)| alive(pid)) => Next::Replace(status.runner),
        _ => Next::Attach(status.runner),
    }
}

fn progress_line(status: &Status<'_>, elapsed: Duration) -> String {
    let title = status.title;
    let seconds = elapsed.as_secs();
    let time = format!("{}:{:02}", seconds / 60, seconds % 60);
    match status.stage {
        "export" => format!("  {title}: getting its code."),
        "compile" => {
            let total = if status.total == 0 { String::new() } else { format!(" / {}", status.total) };
            format!("  {title}: compiling, {}{total} crates, {time} so far.", status.done)
        }
        "bundle" => format!("  {title}: bundling."),
        "install" => format!("  {title}: installing."),
        "start" => format!("  {title}: starting."),
        _ => format!("  Starting the Runner ({time})."),
    }
}

/// What the how-to lines name the app by: its pid when this shell's registry lists it, else its
/// socket (a coding agent's shell may have a temp folder of its own, where the app's discovery
/// file is not), quoted when it holds a space.
fn talk_target(pid: u32, socket: &str, listed: bool) -> String {
    if listed || socket.is_empty() {
        pid.to_string()
    } else if socket.contains(char::is_whitespace) {
        format!("\"{socket}\"")
    } else {
        socket.to_string()
    }
}

fn ready_block(id: &str, title: &str, pid: u32, socket: &str, target: &str, entry: &Path) -> String {
    let socket = if socket.is_empty() { String::new() } else { format!(" socket={socket}") };
    let entry = entry.display();
    format!(
        "  Ready: {id} pid={pid}{socket} makepad={entry}
  {title} is running. Talk to it from your shell:
    {entry} inspect {target} tools                       (its AI tools)
    {entry} inspect {target} call <service>.<tool> {{..}}  (one tool call; pictures are saved as files)
    {entry} inspect {target} grab                        (a picture of its window)
    {entry} inspect {target} help                        (every remote command)"
    )
}

fn read_status(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io::Error::new(e.kind(), format!("{}: {e}", path.display()))),
    }
}

fn start_runner(id: &str, home: &Path, log_path: &Path) -> io::Result<u32> {
    if let Some(dir) = log_path.parent() {
        fs::create_dir_all(dir)?;
    }
    let log = OpenOptions::new().create(true).append(true).open(log_path)?;
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["app", "runner", "--", "--focus", id])
        .current_dir(home)
        .stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x8 | 0x200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    Ok(command.spawn()?.id())
}

fn say(text: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{text}")?;
    out.flush()
}

/// Why `makepad open` does not start the app: on a computer whose apps open no remote endpoint
/// (the default; makepad_remote::ask has the one rule) a coding agent could not talk to it, and
/// the Runner never opens one on its own.
fn endpoint_off(setting: Option<&Path>) -> String {
    let file = setting.map_or("the per-user setting".to_string(), |p| p.display().to_string());
    format!("  The apps on this computer open no remote endpoint, which a coding agent talks to them through.
  Turn it on for your user with the one word `local` in {file}
  (or MAKEPAD_REMOTE=local in this shell), then run this again.")
}

fn open(id: &str) -> io::Result<i32> {
    if makepad_remote::ask::request() == makepad_remote::ask::Request::Off {
        say(&endpoint_off(makepad_remote::ask::setting_path().as_deref()))?;
        return Ok(1);
    }
    let home = relax_runner::fetch::home();
    let path = home.join("runner/focus").join(format!("{id}.status"));
    let log = home.join("cache/logs/runner-focus.log");
    let previous = read_status(&path)?;
    let status = parse(previous.as_deref().unwrap_or(""), id);
    let next = next_step(&status, is_alive);
    if let Next::Replace(pid) = next {
        say(&format!("  The Runner from the last run of {} is still open. Closing it.", status.title))?;
        // A link lost after the request went out is the Runner quitting, not a refusal: wait for it.
        let problem = match makepad_remote::Client::connect(&pid.to_string()) {
            Err(_) => Some("could not be reached"),
            Ok(client) => match makepad_remote::app::run_line(&client, "quit", &mut |_| true) {
                Ok(true) => Some("refused to quit"),
                Ok(false) | Err(_) => None,
            },
        };
        let mut last_line = Instant::now();
        while is_alive(pid) {
            if let Some(problem) = problem {
                say(&format!("  The Runner from the last run (pid {pid}) {problem}. Close its window, then run this again."))?;
                say(&format!("  The Runner's log: {}", log.display()))?;
                return Ok(1);
            }
            if last_line.elapsed() >= Duration::from_secs(5) {
                say(&format!("  Waiting for the open Runner (pid {pid}) to close..."))?;
                last_line = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let mut runner = match next {
        Next::Attach(pid) => pid,
        Next::Replace(_) | Next::Start => {
            if previous.is_some() {
                fs::remove_file(&path)?;
            }
            start_runner(id, &home, &log)?
        }
    };

    let waiting = Instant::now();
    let mut last_line: Option<(String, Instant)> = None;
    loop {
        // Read after the pid check: an exiting Runner may have published a final status.
        let alive = is_alive(runner);
        let text = read_status(&path)?;
        let status = parse(text.as_deref().unwrap_or(""), id);
        let title = status.title;
        match status.stage {
            "ready" => {
                let (pid, socket) = status.ready.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ready status is missing the app pid or socket field"))?;
                let entry = home.join(if cfg!(windows) { "makepad.cmd" } else { "makepad" });
                let entry = if entry.is_file() { entry } else { std::env::current_exe()? };
                let listed = makepad_remote::discovery::apps().iter().any(|a| a.pid == pid);
                say(&ready_block(id, title, pid, socket, &talk_target(pid, socket, listed), &entry))?;
                return Ok(0);
            }
            "failed" => {
                let (reason, app_log) = status.failed.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "failed status is missing the reason or log field"))?;
                say(&format!("  {title} did not start: {reason}"))?;
                if !app_log.is_empty() {
                    say(&format!("  Its log: {app_log}"))?;
                }
                break;
            }
            _ => {}
        }
        if status.runner != 0 && status.runner != runner {
            runner = status.runner;
        } else if !alive {
            say(&format!("  The Runner stopped before {title} was ready."))?;
            break;
        }
        if last_line.as_ref().is_none_or(|(stage, at)| stage != status.stage || at.elapsed() >= Duration::from_secs(10)) {
            let elapsed = if text.is_some() {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
                now.saturating_sub(Duration::from_millis(status.started_ms))
            } else {
                waiting.elapsed()
            };
            say(&progress_line(&status, elapsed))?;
            last_line = Some((status.stage.to_owned(), Instant::now()));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    say(&format!("  The Runner's log: {}", log.display()))?;
    Ok(1)
}

fn main(args: &[String]) -> i32 {
    match args {
        [id] if relax_runner::launch::valid_id(id) => open(id).unwrap_or_else(|e| {
            eprintln!("makepad open: {e}");
            1
        }),
        _ => {
            eprintln!("usage: makepad open <app id>");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_status_and_shell_output() {
        let mut compile = parse("runner\t7\ntitle\tFab\nstarted\t1000\nstage\tcompile\ndone\t214\ntotal\t611\n", "fab");
        assert_eq!((compile.runner, compile.title, compile.started_ms, compile.stage), (7, "Fab", 1000, "compile"), "compile identifies the runner, app, start time and stage");
        assert_eq!((compile.done, compile.total), (214, 611), "compile reads both crate counts");
        let elapsed = Duration::from_millis(125_999);
        assert_eq!(progress_line(&compile, elapsed), "  Fab: compiling, 214 / 611 crates, 2:05 so far.", "compile shows both counts and whole seconds as m:ss");
        compile.total = 0;
        assert_eq!(progress_line(&compile, elapsed), "  Fab: compiling, 214 crates, 2:05 so far.", "compile omits an unknown total");

        let ready = parse("runner\t7\ntitle\tFab\nstarted\t1000\nstage\tready\nready\t48213\t/tmp/a b.sock\n", "fab");
        assert_eq!((ready.stage, ready.ready), ("ready", Some((48213, "/tmp/a b.sock"))), "ready preserves the pid and the space in the socket path");
        let (pid, socket) = ready.ready.unwrap();
        let entry = Path::new("makepad");
        let expected = [
            "  Ready: fab pid=48213 socket=/tmp/a b.sock makepad=makepad",
            "  Fab is running. Talk to it from your shell:",
            "    makepad inspect 48213 tools                       (its AI tools)",
            "    makepad inspect 48213 call <service>.<tool> {..}  (one tool call; pictures are saved as files)",
            "    makepad inspect 48213 grab                        (a picture of its window)",
            "    makepad inspect 48213 help                        (every remote command)",
        ].join("\n");
        assert_eq!(ready_block("fab", ready.title, pid, socket, &talk_target(pid, socket, true), entry), expected, "ready prints the connection details and all four shell commands");
        assert_eq!(ready_block("fab", ready.title, pid, "", &talk_target(pid, "", false), entry), expected.replace(" socket=/tmp/a b.sock", ""), "ready omits the socket field when empty");
        assert_eq!(talk_target(pid, socket, false), "\"/tmp/a b.sock\"", "an app this shell's registry does not list is named by its socket, quoted");
        assert_eq!(talk_target(pid, "/run/x.sock", false), "/run/x.sock", "a socket without spaces is not quoted");

        let failed = parse("runner\t7\ntitle\tFab\nstarted\t1000\nstage\tfailed\nfailed\tdid not compile at all\t/l/install.log\n", "fab");
        assert_eq!((failed.stage, failed.failed), ("failed", Some(("did not compile at all", "/l/install.log"))), "failed separates the reason and its log at the tab");
        assert_eq!(parse("title\t\nextra\tvalue\n", "fab").title, "fab", "an unnamed app uses its id and ignores unknown fields");

        let both_alive = |pid| [7, 48213].contains(&pid);
        let only_runner_alive = |pid| pid == 7;
        let only_app_alive = |pid| pid == 48213;
        assert_eq!(next_step(&compile, only_runner_alive), Next::Attach(7), "attach while the live Runner is compiling");
        assert_eq!(next_step(&ready, both_alive), Next::Attach(7), "attach to the ready Runner while its app is alive");
        assert_eq!(next_step(&ready, only_runner_alive), Next::Replace(7), "replace the ready Runner after its app exits");
        assert_eq!(next_step(&failed, only_runner_alive), Next::Replace(7), "replace the live Runner after a failed build");
        assert_eq!(next_step(&failed, only_app_alive), Next::Start, "start after the failed Runner exits");
        assert_eq!(next_step(&ready, only_app_alive), Next::Start, "start after the ready Runner exits even if its app is alive");
        assert_eq!(next_step(&parse("", "fab"), both_alive), Next::Start, "start when the status is empty");
    }
}
