//! An app's smoke script (`ci.splash`), as the CI box ran it (ported from tools/ci drive.rs and
//! drive_natives.rs): a deterministic Splash host; natives never use model output to choose input.
//!
//! What changed in the port:
//! - apps launch under the JIT (`makepad run <package> -- --remote`, hidden and muted), on the box's
//!   one warm JIT cache, instead of a cargo release build;
//! - a launch lives through a settle window after its first window (until the JIT is idle, at most
//!   [`SETTLE`]): a panic (parked or not), a JIT compile error or an exit in it fails the launch
//!   (the first frame comes before an app's worker threads do their work);
//! - a parked panic anywhere fails the app at once: it would hold the thread for minutes, and a
//!   request to a parked UI thread never answers ([`watch_parks`] kills the app and its children);
//! - the vision judge is the fleet's ([`crate::judge`]);
//! - the cargo natives (`check_targets`, `build`, `warm`, `cargo`, `test`) are CI's own shards now
//!   (devhub ci: cross checks, JIT checks, test shards): here they say so and pass.

use crate::judge::Judge;
use crate::report::Run;
use makepad_remote::Client;
use makepad_script::*;
use makepad_strict_json::{self as json, Value};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, String>;

/// The settle window after a launch's first window.
pub const SETTLE: Duration = Duration::from_secs(15);
/// A cold JIT start of a big app can take minutes; a warm one seconds.
const STARTUP: Duration = Duration::from_secs(300);

/// The lines of an app's output that fail it: panics (parked or not), JIT compile errors, fatal
/// JIT errors, the app's own `[E]` errors.
pub fn error_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("[E]") || t.contains("panicked") || t.contains("panic parked") || t.contains("stale access parked") || t.contains("compile error") || t.contains("FATAL")
        })
        .take(40)
        .map(|l| l.chars().take(300).collect())
        .collect()
}

/// A line an app prints when it parks a thread on a panic.
fn parks(line: &str) -> bool {
    line.contains("panic parked") || line.contains("stale access parked")
}

/// The first parked panic in what `reader` holds past `offset` and in what comes after: kills the
/// process `pid` and its children by pid, and leaves the line in `parked`. Ends with the process,
/// or when the app is `left` to a human.
fn watch_parks(pid: u32, mut reader: File, parked: Arc<Mutex<Option<String>>>, left: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new().name("ci-leg-parks".into()).spawn(move || {
        let mut partial = Vec::new();
        // (the start time: `procs::is_alive` would reap the child its owner waits for)
        while makepad_remote::group::start_time(pid).is_some() && !left.load(Ordering::Acquire) {
            let _ = reader.read_to_end(&mut partial);
            let line = partial.split(|b| *b == b'\n').map(String::from_utf8_lossy).find(|l| parks(l)).map(|l| l.into_owned());
            if let Some(line) = line {
                *parked.lock().unwrap_or_else(|e| e.into_inner()) = Some(line);
                let _ = makepad_remote::procs::kill_tree(pid);
                return;
            }
            let keep = partial.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
            partial.drain(..keep);
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}

/// A child's output in a file, read as it grows (tools/ci process.rs `ChildLog`).
struct ChildLog {
    child: Child,
    reader: File,
    text: String,
    offset: u64,
    partial: Vec<u8>,
}

impl ChildLog {
    fn spawn(command: &mut Command, path: PathBuf) -> Result<ChildLog> {
        let file = OpenOptions::new().create(true).truncate(true).read(true).write(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        command.stdin(Stdio::null()).stdout(Stdio::from(file.try_clone().map_err(|e| e.to_string())?)).stderr(Stdio::from(file));
        let child = command.spawn().map_err(|e| format!("{command:?}: {e}"))?;
        let reader = File::open(&path).map_err(|e| e.to_string())?;
        Ok(ChildLog { child, reader, text: String::new(), offset: 0, partial: Vec::new() })
    }

    fn drain(&mut self) -> Result<()> {
        self.reader.seek(SeekFrom::Start(self.offset)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        self.reader.by_ref().take(1024 * 1024).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        self.offset += bytes.len() as u64;
        self.partial.extend(bytes);
        while let Some(pos) = self.partial.iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&self.partial[..pos]).trim_end_matches('\r').to_string();
            self.text.push_str(&line);
            self.text.push('\n');
            self.partial.drain(..=pos);
        }
        Ok(())
    }

    fn alive(&mut self) -> Result<bool> {
        self.child.try_wait().map(|s| s.is_none()).map_err(|e| e.to_string())
    }

    /// This pid and its children, by pid.
    fn kill_exact(&mut self) -> Result<()> {
        if self.alive()? {
            makepad_remote::procs::kill_tree(self.child.id()).map_err(|e| e.to_string())?;
        }
        self.child.wait().map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// The app's remote commands, with ownership checked after every reply (tools/ci remote.rs).
struct Remote {
    client: Client,
    user_seq: Option<u64>,
}

enum Failure {
    Message(String),
    Interrupted(String),
}

fn end_seq(end: &str) -> Option<u64> {
    let rest = end.split("\"user_seq\":").nth(1)?;
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

impl Remote {
    fn request(&self, argv: &[String], mutate: bool) -> std::result::Result<String, Failure> {
        let shown = argv.join(" ");
        let mut argv = argv.to_vec();
        if mutate {
            let seq = self.user_seq.ok_or_else(|| Failure::Message("refusing mutation before activity".into()))?;
            argv.push(format!("if_user_seq={seq}"));
        }
        let mut lines = Vec::new();
        let (_, end) = makepad_remote::app::run_end(&self.client, &argv, &mut |l| {
            lines.push(l.to_string());
            true
        })
        .map_err(|e| Failure::Message(format!("{shown}: {e}")))?;
        let text = lines.join("\n");
        if lines.iter().any(|l| l.starts_with("{\"err\":\"user_interacting\"") || l.starts_with("{\"err\":\"user_intervened\"")) {
            return Err(Failure::Interrupted(format!("{shown}: human intervention ({text}); leaving the app running")));
        }
        if let Some(seq) = self.user_seq {
            match end.as_deref().and_then(end_seq) {
                Some(now) if now == seq => {}
                Some(now) => return Err(Failure::Interrupted(format!("{shown}: human intervention (user_seq={now}); leaving the app running"))),
                // the JIT's endpoint ends its answers without a reading; a mutation still carries
                // `if_user_seq`, which the app enforces (`user_intervened` above)
                None => {}
            }
        }
        Ok(text)
    }
}

struct AppHandle {
    child: ChildLog,
    remote: Option<Remote>,
    interrupted: bool,
    closed: bool,
    name: String,
    report: PathBuf,
    /// the line of a parked panic ([`watch_parks`], which killed the app for it)
    parked: Arc<Mutex<Option<String>>>,
    /// left running for a human: nothing touches it any more
    left: Arc<AtomicBool>,
}

impl AppHandle {
    fn leave_to_human(&mut self) {
        self.interrupted = true;
        self.left.store(true, Ordering::Release);
    }
}

/// The app a default script launches: `package:binary:cwd`.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub package: String,
    pub binary: String,
    pub cwd: String,
}

impl Target {
    pub fn parse(s: &str) -> Option<Target> {
        let mut w = s.splitn(3, ':');
        let package = w.next()?.to_string();
        let binary = w.next().filter(|b| !b.is_empty()).unwrap_or(&package).to_string();
        let cwd = w.next().filter(|c| !c.is_empty()).unwrap_or(".").to_string();
        (!package.is_empty()).then_some(Target { package, binary, cwd })
    }
}

struct Runtime {
    run: Run,
    judge: Judge,
    apps: Vec<AppHandle>,
    images: Vec<PathBuf>,
    group: String,
    step: Option<usize>,
    suppressed: bool,
    fatal: bool,
    warning: String,
    held_pids: Vec<u32>,
}

impl Runtime {
    fn fail(&mut self, text: &str) {
        self.suppressed = true;
        match self.step {
            Some(i) => self.run.end(i, "failed", text),
            None => self.run.fail(&format!("{}: runtime", self.group), text),
        }
    }

    fn allowed(&mut self) -> bool {
        !(self.suppressed || self.fatal)
    }

    fn request(&mut self, i: usize, argv: &[String], mutate: bool) -> Result<String> {
        if argv.is_empty() || argv.iter().any(|a| a.bytes().any(|b| b.is_ascii_control()) || a.starts_with("if_user_seq")) {
            return Err("invalid command (or attempt to replace the pinned user sequence)".into());
        }
        let route = argv.join(" ");
        let app = self.apps.get_mut(i).ok_or("invalid app handle")?;
        if app.interrupted {
            return Err("human intervention; app is no longer owned".into());
        }
        if app.closed {
            return Err(format!("{} already closed", app.name));
        }
        let remote = app.remote.as_ref().ok_or("remote unavailable")?;
        // a grab the app could not place ends in "; retry": ask again; an input never (it applied)
        let mut attempt = 0;
        let result = loop {
            let result = remote.request(argv, mutate);
            let again = !applies_input(argv) && matches!(&result, Err(Failure::Message(e)) if e.contains("; retry"));
            if !again || attempt == 5 {
                break result;
            }
            attempt += 1;
            std::thread::sleep(Duration::from_millis(300));
        };
        let text = match result {
            Ok(t) => t,
            Err(Failure::Interrupted(e)) => {
                app.leave_to_human();
                self.fatal = true;
                self.held_pids.push(app.child.child.id());
                return Err(e);
            }
            Err(Failure::Message(e)) => {
                // (the answer that never came: the app was killed for a parked panic)
                self.parked_panic(i)?;
                return Err(e);
            }
        };
        if let Ok(v) = json::parse_depth(text.as_bytes(), 64) {
            if let Some(e) = v.get("err") {
                return Err(format!("{route}: {}", e.to_json()));
            }
        }
        Ok(text)
    }

    fn request_json(&mut self, i: usize, argv: &[String], mutate: bool) -> Result<Value> {
        json::parse_depth(self.request(i, argv, mutate)?.as_bytes(), 64).map_err(str::to_string)
    }

    /// The app was killed for a parked panic: Err says so with its error lines. Ends the script.
    fn parked_panic(&mut self, i: usize) -> Result<()> {
        let parked = self.apps[i].parked.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(line) = parked else { return Ok(()) };
        self.fatal = true;
        self.apps[i].child.drain()?;
        let mut lines = error_lines(&self.apps[i].child.text);
        if lines.is_empty() {
            lines.push(line);
        }
        Err(format!("{} parked a panic (pid {} and its children killed, not waited out):\n{}", self.apps[i].name, self.apps[i].child.child.id(), lines.join("\n")))
    }

    fn drain_app(&mut self, i: usize) -> Result<()> {
        self.apps[i].child.drain()?;
        self.parked_panic(i)?;
        if self.apps[i].child.text.contains("[makepad-remote] user closed") || self.apps[i].child.text.contains("app exit: user closed") {
            self.apps[i].leave_to_human();
            self.fatal = true;
            return Err("user closed window; leaving the instance alone".into());
        }
        Ok(())
    }

    fn alive(&mut self, i: usize) -> Result<bool> {
        self.drain_app(i)?;
        let alive = self.apps[i].child.alive()?;
        if !alive && !self.apps[i].closed {
            self.fatal = true;
        }
        Ok(alive)
    }

    /// Starts `target` under the JIT, hidden and muted, takes its remote endpoint, waits for a
    /// window, pins ownership, then the settle window.
    fn launch(&mut self, target: &Target, app_env: &[(String, String)]) -> Result<usize> {
        let name = target.binary.clone();
        let cwd = inside(&self.run.root, Path::new(&target.cwd))?;
        let index = self.apps.len();
        let report = self.run.path(&format!("{}-{index}-report", safe_name(&name)));
        let _ = std::fs::remove_file(&report);
        let loader = crate::loader();
        let mut command = Command::new(&loader[0]);
        command.args(&loader[1..]).arg("run").arg("--report-to").arg(&report).arg(&target.package);
        if target.binary != target.package {
            command.arg("--bin").arg(&target.binary);
        }
        command
            .arg("--")
            .arg("--remote")
            .current_dir(cwd)
            .env("MAKEPAD_HIDE_WINDOWS", "1")
            .env("SANDBOX_MUTE", "1")
            .env("MAKEPAD_MUTE", "1")
            .env("MUSIC_MUTE", "1")
            .env_remove("MAKEPAD_FOCUS")
            // the judge's LAN switch is this process's, not the app's
            .env_remove("MAKEPAD_LAN")
            .envs(app_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let log = self.run.path(&format!("{}-{index}-stdout.log", safe_name(&name)));
        let child = ChildLog::spawn(&mut command, log.clone())?;
        self.run.log(&format!("owned {name} pid={}", child.child.id()));
        let (parked, left) = (Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
        watch_parks(child.child.id(), File::open(&log).map_err(|e| e.to_string())?, parked.clone(), left.clone());
        self.apps.push(AppHandle { child, remote: None, interrupted: false, closed: false, name: name.clone(), report, parked, left });
        let deadline = Instant::now() + STARTUP;
        loop {
            self.drain_app(index)?;
            let app = &mut self.apps[index];
            let pid = app.child.child.id();
            if let Some(socket) = app.child.text.lines().find_map(|line| parse_startup(line, pid)) {
                let client = Client::connect(&socket).map_err(|e| format!("{} remote endpoint {socket}: {e}", app.name))?;
                app.remote = Some(Remote { client, user_seq: None });
                break;
            }
            if !app.child.alive()? {
                self.fatal = true;
                return Err(format!("{} exited before remote startup\n{}", app.name, tail(&app.child.text, 30)));
            }
            if Instant::now() >= deadline {
                return Err(format!("remote startup timed out after {} s\n{}", STARTUP.as_secs(), tail(&self.apps[index].child.text, 30)));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        loop {
            if !self.alive(index)? {
                return Err(format!("app died before ready\n{}", tail(&self.apps[index].child.text, 30)));
            }
            if let Ok(v) = self.request_json(index, &argv(&["status"]), false) {
                if v.get("w").and_then(Value::as_arr).is_some_and(|w| !w.is_empty()) {
                    break;
                }
            }
            if self.fatal {
                return Err("human intervention during startup".into());
            }
            if Instant::now() >= deadline {
                return Err("status readiness timed out".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let activity = self.request_json(index, &argv(&["activity"]), false)?;
        let seq = activity.get("user_seq").and_then(Value::as_u64).ok_or("missing activity user_seq")?;
        self.apps[index].remote.as_mut().unwrap().user_seq = Some(seq);
        if activity.get("user_active").and_then(Value::as_bool) == Some(true) {
            self.apps[index].leave_to_human();
            self.fatal = true;
            return Err("human active at startup; leaving app running".into());
        }
        self.settle(index)?;
        Ok(index)
    }

    /// The settle window: until the JIT is idle (two quiet looks) or [`SETTLE`]; a panic, a JIT
    /// compile error or an exit in it fails.
    fn settle(&mut self, i: usize) -> Result<()> {
        let t = Instant::now();
        let pid = self.apps[i].child.child.id().to_string();
        let loader = crate::loader_cmd();
        let mut idle = 0;
        let mut first_busy: Option<String> = None;
        while t.elapsed() < SETTLE && idle < 2 {
            if !self.alive(i)? {
                return Err(format!("{} exited {:.1} s into the settle window\n{}", self.apps[i].name, t.elapsed().as_secs_f64(), tail(&self.apps[i].child.text, 30)));
            }
            let out = Command::new(&loader[0]).args(&loader[1..]).args(["inspect", &pid, "jit", "ms=500"]).stdin(Stdio::null()).output();
            let answer = out.map(|o| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))).unwrap_or_default();
            if answer.lines().any(|l| l.trim_start().starts_with("idle")) {
                idle += 1;
            } else {
                if idle == 0 && first_busy.is_none() {
                    first_busy = answer.lines().find(|l| !l.trim().is_empty()).map(|l| l.chars().take(160).collect());
                }
                idle = 0;
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        self.drain_app(i)?;
        let mut bad = error_lines(&self.apps[i].child.text);
        let report = std::fs::read_to_string(&self.apps[i].report).unwrap_or_default();
        bad.extend(report.lines().filter(|l| l.starts_with("panic\t") || l.starts_with("error\t")).map(|l| l.replace('\t', " ")));
        if bad.is_empty() {
            let how = if idle >= 2 { "JIT idle".to_string() } else { format!("window ended; the JIT said: {}", first_busy.unwrap_or_default()) };
            self.run.log(&format!("{} settled in {:.1} s ({how})", self.apps[i].name, t.elapsed().as_secs_f64()));
            Ok(())
        } else {
            Err(format!("{} failed in the settle window after its first window:\n{}", self.apps[i].name, bad.join("\n")))
        }
    }

    fn key(&mut self, i: usize, code: &str, mods: &[&str]) -> Result<()> {
        self.request_json(i, &key_args("down", code, mods), true)?;
        // always release a pressed key
        std::thread::sleep(Duration::from_millis(180));
        self.request_json(i, &key_args("up", code, mods), true)?;
        Ok(())
    }

    fn type_text(&mut self, i: usize, text: &str) -> Result<()> {
        for c in text.chars() {
            let (code, shift) = character_key(c)?;
            self.key(i, &code, if shift { &["shift"] } else { &[] })?;
        }
        Ok(())
    }

    fn log_text(&mut self, i: usize) -> Result<String> {
        self.drain_app(i)?;
        let v = self.request_json(i, &argv(&["log", "n=10000"]), false)?;
        let mut lines = Vec::new();
        collect_log(&v, &mut lines);
        Ok(lines.join("\n"))
    }

    /// No `[E]` line, panic or JIT compile error in the app's log and output, and alive.
    fn no_errors(&mut self, i: usize) -> Result<()> {
        if !self.alive(i)? {
            return Err("app process is dead".into());
        }
        let log = format!("{}\n{}", self.log_text(i)?, self.apps[i].child.text);
        let errors = error_lines(&log);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    fn wait_log(&mut self, i: usize, text: &str, secs: f64) -> Result<()> {
        if !secs.is_finite() || !(0.0..=3600.0).contains(&secs) {
            return Err("invalid log timeout".into());
        }
        let end = Instant::now() + Duration::from_secs_f64(secs);
        loop {
            if !self.alive(i)? {
                return Err("app died while waiting for log".into());
            }
            if log_has(&self.log_text(i)?, text) {
                return Ok(());
            }
            if Instant::now() >= end {
                return Err(format!("{}: log did not contain {text:?} within {secs}s", self.apps[i].name));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn grab(&mut self, i: usize, name: &str) -> Result<usize> {
        let value = self.request_json(i, &argv(&["grab"]), false)?;
        let mut paths = Vec::new();
        png_paths(&value, &mut paths);
        let source = paths.first().ok_or("grab did not return a PNG path")?;
        let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("grab is not PNG".into());
        }
        let path = self.run.path(&format!("{}.png", safe_name(name)));
        if path.exists() && self.images.contains(&path) {
            return Err(format!("duplicate grab name: {name}"));
        }
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        self.images.push(path);
        Ok(self.images.len() - 1)
    }

    fn quit(&mut self, i: usize) -> Result<()> {
        if self.apps[i].interrupted {
            return Err("human intervention; app left running".into());
        }
        if self.apps[i].closed {
            return Ok(());
        }
        if !self.apps[i].child.alive()? {
            self.apps[i].closed = true;
            return Ok(());
        }
        // the graceful quit on a thread of its own: an app that no longer answers (its UI thread
        // stuck) waits the deadline out, and is killed, however long its answer would take
        let asked = self.apps[i].remote.take().map(|mut remote| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let r = (|| {
                    if remote.user_seq.is_none() {
                        let a = remote.request(&argv(&["activity"]), false)?;
                        let a = json::parse_depth(a.as_bytes(), 64).map_err(|e| Failure::Message(e.to_string()))?;
                        let seq = a.get("user_seq").and_then(Value::as_u64).ok_or_else(|| Failure::Message("cleanup missing user_seq".into()))?;
                        remote.user_seq = Some(seq);
                    }
                    remote.request(&argv(&["gq"]), true).map(|_| ())
                })();
                if matches!(r, Err(Failure::Message(_))) && remote.user_seq.is_some() {
                    let _ = remote.request(&argv(&["quit"]), true);
                }
                let _ = tx.send(r);
            });
            rx
        });
        let mut error = None;
        let mut answered = asked.is_none();
        let end = Instant::now() + Duration::from_secs(10);
        while self.apps[i].child.alive()? && Instant::now() < end {
            self.drain_app(i)?;
            if let Some(r) = asked.as_ref().filter(|_| !answered).and_then(|rx| rx.try_recv().ok()) {
                answered = true;
                match r {
                    Ok(()) => {}
                    Err(Failure::Message(e)) => error = Some(e),
                    Err(Failure::Interrupted(e)) => {
                        self.apps[i].leave_to_human();
                        self.fatal = true;
                        return Err(e);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if self.apps[i].child.alive()? {
            let pid = self.apps[i].child.child.id();
            self.apps[i].child.kill_exact()?;
            self.run.log(&format!("killed exact owned pid={pid} after graceful shutdown timeout"));
            error = Some(format!("pid={pid} required forced cleanup"));
        } else {
            let _ = self.apps[i].child.child.wait();
        }
        self.drain_app(i)?;
        self.apps[i].closed = true;
        error.map_or(Ok(()), Err)
    }
}

/// The remote endpoint of the process this run owns: the platform's `[makepad-remote] pid=<pid>
/// app=<app> socket=<path>`, or the JIT's `makepad-remote: pid <pid> app <app> socket <path> (...)`.
fn parse_startup(line: &str, pid: u32) -> Option<String> {
    if let Some(rest) = line.strip_prefix("makepad-remote: pid ") {
        let (actual, rest) = rest.split_once(' ')?;
        if actual.parse::<u32>().ok()? != pid {
            return None;
        }
        let socket = rest.split_once(" socket ")?.1;
        let socket = socket.rsplit_once(" (").map_or(socket, |(s, _)| s).trim();
        return (!socket.is_empty()).then(|| socket.to_string());
    }
    let rest = line.strip_prefix("[makepad-remote] pid=")?;
    let (actual, rest) = rest.split_once(' ')?;
    if actual.parse::<u32>().ok()? != pid {
        return None;
    }
    let socket = rest.split_once(" socket=")?.1.trim();
    (!socket.is_empty()).then(|| socket.to_string())
}

fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn log_has(log: &str, pattern: &str) -> bool {
    // `*` matches any run of characters within a line
    let parts: Vec<&str> = pattern.split('*').collect();
    log.lines().any(|line| {
        let mut at = 0;
        parts.iter().all(|p| match line[at..].find(p) {
            Some(i) => {
                at += i + p.len();
                true
            }
            None => false,
        })
    })
}

pub fn safe_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn key_args(kind: &str, code: &str, mods: &[&str]) -> Vec<String> {
    let mut v = argv(&["key", &format!("k={kind}"), &format!("c={code}")]);
    v.extend(mods.iter().map(|m| format!("{m}=1")));
    v.push("wait=1".into());
    v
}

fn character_key(c: char) -> Result<(String, bool)> {
    match c {
        'a'..='z' | 'A'..='Z' => Ok((format!("Key{}", c.to_ascii_uppercase()), c.is_ascii_uppercase())),
        '0'..='9' => Ok((format!("Key{c}"), false)),
        ' ' => Ok(("Space".into(), false)),
        _ => Err(format!("type_text does not support character {c:?}")),
    }
}

fn applies_input(argv: &[String]) -> bool {
    argv.first().is_some_and(|c| ["key", "mouse", "text", "click", "drop", "tweak", "ai"].contains(&c.as_str()))
}

fn collect_log(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Str(s) => out.push(s.clone()),
        Value::Arr(a) => a.iter().for_each(|x| collect_log(x, out)),
        Value::Obj(a) => {
            for (k, v) in a {
                if matches!(k.as_str(), "l" | "lines" | "log" | "entries" | "text" | "message" | "msg" | "s") {
                    collect_log(v, out);
                }
            }
        }
        _ => {}
    }
}

fn png_paths(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Obj(a) => {
            for (k, v) in a {
                if k == "png" {
                    match v {
                        Value::Str(s) => out.push(s.clone()),
                        Value::Arr(a) => out.extend(a.iter().filter_map(Value::as_str).map(str::to_string)),
                        _ => {}
                    }
                } else {
                    png_paths(v, out)
                }
            }
        }
        Value::Arr(a) => a.iter().for_each(|v| png_paths(v, out)),
        _ => {}
    }
}

fn inside(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root.join(path).canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err("cwd must remain inside checkout".into());
    }
    Ok(path)
}

// ---------------------------------------------------------------- the Splash side

/// The runtime is this host's one capability.
unsafe impl makepad_script::ScriptHostCapability for Runtime {
    const ID: LiveId = LiveId::from_str("makepad.ci_leg.smoke.runtime.v1");
}

fn rt<'a>(vm: &'a mut ScriptVm) -> &'a mut Runtime {
    vm.host.get_mut::<Runtime>().unwrap()
}

fn value(vm: &ScriptVm, args: ScriptObject, key: LiveId) -> ScriptValue {
    vm.value(args, key)
}

fn text(vm: &mut ScriptVm, v: ScriptValue) -> Result<String> {
    vm.string_with(v, |_, s| s.to_string()).ok_or("expected string".into())
}

fn str_arg(vm: &mut ScriptVm, args: ScriptObject, key: LiveId) -> Result<String> {
    let v = value(vm, args, key);
    text(vm, v)
}

fn num_arg(vm: &ScriptVm, args: ScriptObject, key: LiveId) -> Result<f64> {
    value(vm, args, key).as_number().filter(|v| v.is_finite()).ok_or_else(|| format!("expected a finite number: {:?}", value(vm, args, key)))
}

fn object_field(vm: &ScriptVm, v: ScriptValue, key: LiveId) -> ScriptValue {
    v.as_object().map(|o| value(vm, o, key)).filter(|v| !v.is_err()).unwrap_or(NIL)
}

fn field_text(vm: &mut ScriptVm, v: ScriptValue, key: LiveId) -> Result<Option<String>> {
    let v = object_field(vm, v, key);
    if v.is_nil() {
        Ok(None)
    } else {
        text(vm, v).map(Some)
    }
}

fn json_value(vm: &mut ScriptVm, v: Value) -> ScriptValue {
    match v {
        Value::Null => NIL,
        Value::Bool(v) => v.into(),
        Value::Int(v) => (v as f64).into(),
        Value::F64(v) => v.into(),
        Value::Str(v) => vm.new_string_with(|_, out| out.push_str(&v)),
        v => {
            let mut parser = makepad_script::json::JsonParserThread::default();
            parser.read_json(&v.to_json(), vm.heap_mut())
        }
    }
}

fn result(vm: &mut ScriptVm, r: Result<Value>) -> ScriptValue {
    match r {
        Ok(v) => json_value(vm, v),
        Err(e) => {
            rt(vm).fail(&e);
            NIL
        }
    }
}

fn allow(vm: &mut ScriptVm) -> bool {
    if vm.has_captured_errors() {
        let e = vm.take_errors().join("\n");
        vm.capture_errors();
        rt(vm).fail(&e);
        return false;
    }
    rt(vm).allowed()
}

fn env_pairs(vm: &mut ScriptVm, e: ScriptValue, what: &str) -> Result<Vec<(String, String)>> {
    let mut env = Vec::new();
    if !e.is_nil() {
        let obj = e.as_object().ok_or_else(|| format!("{what} must be an object"))?;
        let entries: Vec<_> = vm.heap().map_entries(obj).into_iter().map(|e| (e.key, e.value)).collect();
        for (k, v) in entries {
            let mut key = String::new();
            vm.heap().cast_to_string(k, &mut key);
            env.push((key, text(vm, v)?));
        }
    }
    Ok(env)
}

/// A native CI owns as its own shards now: noted, passes.
fn covered(vm: &mut ScriptVm, what: &str) -> ScriptValue {
    if allow(vm) {
        rt(vm).run.log(&format!("{what}: covered by the CI run's own shards (devhub ci)"));
    }
    NIL
}

fn register(vm: &mut ScriptVm, target: Option<&Target>) {
    let ci = vm.new_module(id!(ci));
    let t = target.map_or(NIL, |t| json_value(vm, json::obj(vec![("package", json::s(&t.package)), ("binary", json::s(&t.binary)), ("cwd", json::s(&t.cwd))])));
    vm.set_value_def(ci, id!(target), t);
    let host = json_value(vm, json::obj(vec![("target", json::s(std::env::consts::OS)), ("targets", Value::Arr(Vec::new()))]));
    vm.set_value_def(ci, id!(host), host);
    vm.set_value_def(ci, id!(deep), ScriptValue::from(false));
    let apps = json_value(vm, Value::Arr(Vec::new()));
    vm.set_value_def(ci, id!(apps), apps);
    vm.add_method(ci, id_lut!(check_targets), script_args!(spec = NIL), |vm, _| covered(vm, "check_targets"));
    vm.add_method(ci, id_lut!(warm), script_args!(spec = NIL), |vm, _| covered(vm, "warm"));
    vm.add_method(ci, id_lut!(cargo), script_args!(args = NIL, options = NIL), |vm, _| covered(vm, "cargo"));
    vm.add_method(ci, id_lut!(test), script_args!(spec = NIL), |vm, _| covered(vm, "test"));
    // apps run under the JIT: a launch compiles what it runs
    vm.add_method(ci, id_lut!(build), script_args!(spec = NIL), |vm, _| covered(vm, "build (apps run under the JIT)"));
    vm.add_method(ci, id_lut!(shared), script_args!(), |_, _| NIL);
    vm.add_method(ci, id_lut!(exclusive), script_args!(), |_, _| NIL);
    vm.add_method(ci, id_lut!(step), script_args!(name = NIL, body = NIL), |vm, args| {
        let name = match str_arg(vm, args, id!(name)) {
            Ok(v) => v,
            Err(e) => {
                rt(vm).fail(&e);
                return NIL;
            }
        };
        let body = value(vm, args, id!(body));
        if rt(vm).step.is_some() {
            rt(vm).fail("nested ci.step is not supported");
            return NIL;
        }
        let before = vm.take_errors();
        vm.capture_errors();
        if !before.is_empty() {
            rt(vm).fail(&before.join("\n"));
        }
        let r = rt(vm);
        let i = r.run.begin(&format!("{} / {name}", r.group));
        if r.fatal {
            r.run.end(i, "skipped", "app dead or human intervention");
            return NIL;
        }
        r.suppressed = false;
        r.warning.clear();
        r.step = Some(i);
        let answer = vm.call(body, &[]);
        let errors = vm.take_errors();
        vm.capture_errors();
        if !errors.is_empty() {
            rt(vm).fail(&errors.join("\n"));
        }
        if answer.is_err() && !rt(vm).suppressed {
            rt(vm).fail("step callback returned an error");
        }
        let r = rt(vm);
        if !r.suppressed {
            let w = r.warning.clone();
            r.run.end(i, if w.is_empty() { "passed" } else { "warning" }, &w);
        }
        r.step = None;
        r.suppressed = r.fatal;
        NIL
    });
    vm.add_method(ci, id_lut!(launch), script_args!(target = NIL), |vm, args| {
        if !allow(vm) {
            return app_object(vm, usize::MAX);
        }
        let v = value(vm, args, id!(target));
        let r = (|| -> Result<usize> {
            let package = field_text(vm, v, id!(package))?.ok_or("launch requires package")?;
            let binary = field_text(vm, v, id!(binary))?.unwrap_or_else(|| package.clone());
            let cwd = field_text(vm, v, id!(cwd))?.unwrap_or_else(|| ".".into());
            let app_env = env_pairs(vm, object_field(vm, v, id!(app_env)), "app_env")?;
            let r = rt(vm);
            let i = r.run.begin(&format!("{} / launch {binary}", r.group));
            let t = Target { package, binary, cwd };
            let result = r.launch(&t, &app_env);
            match &result {
                Ok(_) => r.run.end(i, "passed", ""),
                Err(e) => {
                    r.run.end(i, "failed", e);
                    r.fatal = true;
                }
            }
            result
        })();
        match r {
            Ok(i) => app_object(vm, i),
            Err(e) => {
                rt(vm).fatal = true;
                if rt(vm).step.is_some() {
                    rt(vm).fail(&e);
                }
                app_object(vm, usize::MAX)
            }
        }
    });
    vm.add_method(ci, id_lut!(sleep), script_args!(secs = NIL), |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = num_arg(vm, args, id!(secs)).map(|n| {
            std::thread::sleep(Duration::from_secs_f64(n.clamp(0.0, 600.0)));
            Value::Null
        });
        result(vm, r)
    });
    vm.add_method(ci, id_lut!(check), script_args!(condition = NIL, message = NIL), |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        if value(vm, args, id!(condition)).as_bool() != Some(true) {
            let message = str_arg(vm, args, id!(message)).unwrap_or_else(|e| e);
            rt(vm).fail(&message);
        }
        NIL
    });
    vm.add_method(ci, id_lut!(fail), script_args!(message = NIL), |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let s = str_arg(vm, args, id!(message)).unwrap_or_else(|e| e);
        rt(vm).fail(&s);
        NIL
    });
    vm.add_method(ci, id_lut!(log), script_args!(text = NIL), |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = str_arg(vm, args, id!(text)).map(|s| {
            rt(vm).run.log(&s);
            Value::Null
        });
        result(vm, r)
    });
    for (name, accept) in [(id_lut!(judge), false), (id_lut!(accept), true)] {
        vm.add_method(ci, name, script_args!(image = NIL, text = NIL), move |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = (|| {
                let prompt = str_arg(vm, args, id!(text))?;
                let index = num_arg(vm, args, id!(image))? as usize;
                let r = rt(vm);
                let image = r.images.get(index).ok_or("invalid image handle")?.clone();
                let verdict = r.judge.judge(&image, &prompt)?;
                r.run.log(&format!("judge {} on {} in {:.1} s: {}", image.display(), verdict.hub, verdict.secs, verdict.raw.lines().last().unwrap_or("")));
                let step = r.step.map(|i| r.run.steps[i].name.clone()).unwrap_or_else(|| "judge".into());
                r.run.evidence.push(json::obj(vec![("step", json::s(step)), ("prompt", json::s(&prompt)), ("image", json::s(image.display().to_string())), ("verdict", verdict.json())]));
                if accept && !verdict.yes {
                    return Err(if verdict.parse_error { format!("vision parse failed\n{}", verdict.raw) } else { verdict.raw });
                }
                Ok(verdict.json())
            })();
            result(vm, r)
        });
    }
}

fn app_object(vm: &mut ScriptVm, index: usize) -> ScriptValue {
    let obj = vm.new_object();
    vm.add_method(obj, id_lut!(key), script_args!(code = NIL, options = NIL), move |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = (|| {
            let code = str_arg(vm, args, id!(code))?;
            let options = value(vm, args, id!(options));
            let mut mods: Vec<&str> = Vec::new();
            for (k, s) in [(id!(cmd), "cmd"), (id!(shift), "shift"), (id!(ctrl), "ctrl"), (id!(alt), "alt")] {
                if object_field(vm, options, k).as_bool() == Some(true) {
                    mods.push(s);
                }
            }
            rt(vm).key(index, &code, &mods)?;
            Ok(Value::Null)
        })();
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(type_text), script_args!(text = NIL), move |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = str_arg(vm, args, id!(text)).and_then(|s| rt(vm).type_text(index, &s).map(|_| Value::Null));
        result(vm, r)
    });
    for (method, scroll) in [(id_lut!(click), false), (id_lut!(scroll), true)] {
        vm.add_method(obj, method, script_args!(x = NIL, y = NIL, dx = 0.0, dy = 0.0), move |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = (|| {
                let x = num_arg(vm, args, id!(x))?;
                let y = num_arg(vm, args, id!(y))?;
                let command = if scroll {
                    argv(&["mouse", "k=scroll", &format!("x={x}"), &format!("y={y}"), &format!("dx={}", num_arg(vm, args, id!(dx))?), &format!("dy={}", num_arg(vm, args, id!(dy))?), "wait=1"])
                } else {
                    argv(&["click", &format!("x={x}"), &format!("y={y}"), "wait=1"])
                };
                rt(vm).request_json(index, &command, true)?;
                Ok(Value::Null)
            })();
            result(vm, r)
        });
    }
    vm.add_method(obj, id_lut!(cmd), script_args!(command = NIL), move |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = str_arg(vm, args, id!(command)).and_then(|s| {
            let words: Vec<String> = s.split_whitespace().map(str::to_string).collect();
            rt(vm).request(index, &words, true).map(json::s)
        });
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(ui), script_args!(query = NIL), move |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = str_arg(vm, args, id!(query)).and_then(|s| rt(vm).request_json(index, &argv(&["ui", "find", "json=1", &format!("q={s}")]), false));
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(wait_log), script_args!(substring = NIL, secs = NIL), move |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = (|| {
            let s = str_arg(vm, args, id!(substring))?;
            let secs = num_arg(vm, args, id!(secs))?;
            rt(vm).wait_log(index, &s, secs)?;
            Ok(Value::Null)
        })();
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(log), script_args!(), move |vm, _| {
        if !allow(vm) {
            return NIL;
        }
        let r = rt(vm).log_text(index).map(json::s);
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(no_errors), script_args!(), move |vm, _| {
        if !allow(vm) {
            return NIL;
        }
        let r = rt(vm).no_errors(index).map(|_| Value::Null);
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(alive), script_args!(), move |vm, _| {
        if !allow(vm) {
            return NIL;
        }
        let r = rt(vm).alive(index).map(Value::Bool);
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(grab), script_args!(name = NIL), move |vm, args| {
        if !allow(vm) {
            return NIL;
        }
        let r = str_arg(vm, args, id!(name)).and_then(|s| rt(vm).grab(index, &s).map(|i| Value::Int(i as i64)));
        result(vm, r)
    });
    vm.add_method(obj, id_lut!(quit), script_args!(), move |vm, _| {
        if !allow(vm) {
            return NIL;
        }
        let r = rt(vm).quit(index).map(|_| Value::Null);
        result(vm, r)
    });
    obj.into()
}

/// Runs the script at `path` (relative to `root`) as the tile `dir`; its apps are cleaned up after.
pub fn run(root: PathBuf, path: &str, dir: &str, target: Option<Target>, judge: Judge) -> i32 {
    let out = root.join("local/ci-leg").join(safe_name(dir));
    let _ = std::fs::remove_dir_all(&out);
    let run = Run::new(root.clone(), out);
    let source = std::fs::read_to_string(root.join(path));
    let mut host = ScriptVmHost::new(
        Runtime { run, judge, apps: Vec::new(), images: Vec::new(), group: dir.to_string(), step: None, suppressed: false, fatal: false, warning: String::new(), held_pids: Vec::new() },
        (),
    );
    match source {
        Err(e) => host.host.run.fail(dir, &format!("{path}: {e}")),
        Ok(code) => {
            let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
            register(&mut vm, target.as_ref());
            vm.capture_errors();
            let answer = vm.with_instruction_limit(5_000_000, |vm| vm.eval(ScriptMod::new("", "", path, 0, 0, &code, &[])));
            let errors = vm.take_errors();
            if !errors.is_empty() {
                rt(&mut vm).fail(&errors.join("\n"));
            }
            if answer.is_err() {
                rt(&mut vm).fail("script returned an error");
            }
        }
    }
    let r = &mut host.host;
    for i in 0..r.apps.len() {
        let stage = r.run.begin(&format!("{} / cleanup {}", r.group, r.apps[i].name));
        match r.quit(i) {
            Ok(()) => r.run.end(stage, "passed", ""),
            Err(e) => r.run.end(stage, "failed", &e),
        }
    }
    let what = format!("smoke {dir}");
    host.host.run.finish(&what)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_lines_name_this_pid_and_logs_match_with_stars() {
        assert_eq!(parse_startup("[makepad-remote] pid=7 app=test socket=/tmp/a b/makepad-remote-7.sock", 7).as_deref(), Some("/tmp/a b/makepad-remote-7.sock"));
        assert_eq!(parse_startup("[makepad-remote] pid=8 app=test socket=/tmp/x.sock", 7), None);
        assert_eq!(parse_startup("makepad-remote: pid 7 app calc socket /h/t/L1/makepad-remote-7.sock (makepad inspect 7 help)", 7).as_deref(), Some("/h/t/L1/makepad-remote-7.sock"));
        assert_eq!(parse_startup("makepad-remote: pid 8 app calc socket /x.sock (y)", 7), None);
        assert!(log_has("x\nterminal client 4 first frame\n", "terminal client * first frame"));
        assert!(!log_has("terminal client 4\n", "terminal client * first frame"));
        assert_eq!(end_seq("{\"end\":\"snap\",\"ms\":3,\"user_seq\":12}"), Some(12));
    }

    #[cfg(unix)]
    #[test]
    fn a_parked_panic_kills_the_app_and_its_children_at_once() {
        let dir = std::env::temp_dir().join(format!("ci-leg-parks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log");
        std::fs::write(dir.join("app.sh"), "sleep 600 & echo $! > child; echo 'makepad: panic parked on thread main (boom) for 600 s'; sleep 600").unwrap();
        let mut app = ChildLog::spawn(Command::new("/bin/sh").arg("app.sh").current_dir(&dir), path.clone()).unwrap();
        let parked = Arc::new(Mutex::new(None));
        let t = Instant::now();
        watch_parks(app.child.id(), File::open(&path).unwrap(), parked.clone(), Arc::new(AtomicBool::new(false)));
        app.child.wait().unwrap();
        assert!(t.elapsed() < Duration::from_secs(30), "waited {:?}", t.elapsed());
        assert!(parked.lock().unwrap().as_deref().is_some_and(parks));
        let child: u32 = std::fs::read_to_string(dir.join("child")).unwrap().trim().parse().unwrap();
        for _ in 0..50 {
            if !makepad_remote::procs::is_alive(child) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!makepad_remote::procs::is_alive(child));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn panics_and_jit_compile_errors_fail_an_app() {
        let out = "ok\nmakepad-run: [t=5251 ms] compile error in glue4: x\nmakepad: panic parked on thread model (boom) for 30 s\n[E] bad\nfine";
        assert_eq!(error_lines(out).len(), 3);
        assert!(error_lines("all good\n[I] info").is_empty());
        assert_eq!(Target::parse("makepad-scope:scope:apps/commercial/scope"), Some(Target { package: "makepad-scope".into(), binary: "scope".into(), cwd: "apps/commercial/scope".into() }));
        assert_eq!(Target::parse("p").unwrap().binary, "p");
    }
}
