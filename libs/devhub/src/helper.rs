//! A helper process its parent keeps over its exec in place ([`crate::node::exec`]): a child that
//! never execs itself, so it holds what the system ties to a process's first image for the life
//! of its pid. The Mac's menu bar is one: once a process exec'd, it refuses that process's status
//! items for good (MenuBarAgent keeps the first image "pending exit" by pid), so the master's
//! menu-bar item lives in a helper, and each image of the master takes the same helper on (11-17:
//! after the master's first self-update its item was an empty pill).
//!
//! The parent writes the helper whole states, one line each, and only the newest waits: a state
//! not yet written gives way to the next, and the parent never waits on the pipe (a thread of its
//! own writes). The helper answers with lines of its own. At the end of its input (its parent
//! ended) the helper ends. Should the helper end first, the parent says so once, starts another
//! after a pause and gives it the newest state. An image that runs other code than the one that
//! started the helper ([`Spec::version`]) replaces it: a self-update brings the helper its fixes
//! too (11-17: a fixed menu-bar item waited for the master's next full restart). With
//! [`Spec::ready`] the new helper starts beside the old one, which ends once the new one says it
//! is ready: what the old one shows stays on screen until the new one shows it (the devhub panel's
//! window, 11-24).

use std::io::{BufRead, Write};
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How a parent starts and hears its helper.
pub struct Spec {
    /// The environment variable that carries the helper over an exec: `<parent pid>:<helper
    /// pid>:<to fd>:<from fd>:<version>`.
    pub env: &'static str,
    /// The code a new helper runs, in a word without `:` (the linker's id of this image): a
    /// helper an earlier image started with other code is replaced.
    pub version: String,
    /// A new helper's command: its stdin and stdout are the pipes, set here.
    pub command: Box<dyn Fn() -> Result<Command, String> + Send + Sync>,
    /// Each line the helper writes, on a thread of the helper's own.
    pub answer: Box<dyn Fn(String) + Send + Sync>,
    /// What the parent says of its helper: an end, a start that failed.
    pub say: Box<dyn Fn(String) + Send + Sync>,
    /// The pause before another helper starts after one ended. It doubles while helpers end
    /// within a minute of their start, up to a minute.
    pub pause: Duration,
    /// Whether a line of a new helper says it is ready to take over. With it, a helper of other
    /// code is replaced by one started beside it, and the old one ends once the new one says so
    /// (or ends); meanwhile the old one's answers still count. Without it the old one ends first.
    pub ready: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
}

/// The parent's helper.
pub struct Helper(Arc<Shared>);

struct Shared {
    spec: Spec,
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    link: Option<Link>,
    /// the newest state not written yet
    pending: Option<String>,
    /// the newest state written: a new helper's first
    written: Option<String>,
    /// a helper this image took on: a line the last image left half-written ends first
    fresh: bool,
    /// the helper a write failed to (gone; the next is waited for)
    failed: Option<u32>,
    /// the helper that said it is ready ([`Spec::ready`]): the one it replaces ends
    ready: Option<u32>,
}

#[derive(Clone)]
struct Link {
    pid: u32,
    to: Arc<OwnedFd>,
    from: Arc<OwnedFd>,
    at: Instant,
}

const MINUTE: Duration = Duration::from_secs(60);

impl Helper {
    /// The helper this pid's last image left it ([`Spec::env`]), else a new one.
    pub fn start(spec: Spec) -> Helper {
        let shared = Arc::new(Shared { spec, state: Mutex::new(State::default()), wake: Condvar::new() });
        let mut old = None;
        let adopted = match adopt(shared.spec.env) {
            Some((link, version)) if version != shared.spec.version => {
                let was = if version.is_empty() { "unnamed" } else { &version };
                let how = if shared.spec.ready.is_some() { "another replaces it once that one is ready" } else { "another replaces it" };
                (shared.spec.say)(format!("its helper (pid {}) runs other code than this image ({was}, now {}): {how}", link.pid, shared.spec.version));
                set_link(&shared, None);
                match shared.spec.ready {
                    Some(_) => old = Some(link),
                    None => {
                        let pid = link.pid;
                        let ended = end(link);
                        let s = shared.clone();
                        let _ = std::thread::Builder::new().name("devhub-helper-end".into()).spawn(move || say_end(&s, pid, ended, "its replaced helper", ""));
                    }
                }
                None
            }
            adopted => adopted.map(|(link, _)| link),
        };
        match adopted {
            Some(link) => {
                shared.state.lock().unwrap_or_else(|e| e.into_inner()).fresh = true;
                set_link(&shared, Some(link));
            }
            None => match spawn(&shared.spec) {
                Ok(link) => {
                    let new = link.pid;
                    set_link(&shared, Some(link));
                    if let Some(old) = old.take() {
                        let s = shared.clone();
                        let _ = std::thread::Builder::new().name("devhub-helper-retire".into()).spawn(move || retire(&s, old, new));
                    }
                }
                Err(e) => (shared.spec.say)(format!("its helper did not start: {e}; it tries again in {} s", shared.spec.pause.as_secs_f64())),
            },
        }
        // (no new one: the old one ends now, off the caller's thread)
        if let Some(old) = old {
            let s = shared.clone();
            let _ = std::thread::Builder::new().name("devhub-helper-retire".into()).spawn(move || {
                let pid = old.pid;
                let ended = end(old);
                say_end(&s, pid, ended, "its replaced helper", "");
            });
        }
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("devhub-helper-write".into()).spawn(move || write_loop(&s));
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("devhub-helper-read".into()).spawn(move || supervise(&s));
        Helper(shared)
    }

    /// Hands the helper `line`, a whole state: it replaces one not written yet. Never waits on
    /// the helper.
    pub fn send(&self, line: String) {
        let mut st = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending = Some(line);
        self.0.wake.notify_all();
    }

    /// The helper's pid, while there is one.
    pub fn pid(&self) -> Option<u32> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).link.as_ref().map(|l| l.pid)
    }
}

/// The replaced helper `old` beside its successor `new`: its answers still count until it ends,
/// which it does once `new` said it is ready or is gone.
fn retire(shared: &Arc<Shared>, old: Link, new: u32) {
    let (s, from) = (shared.clone(), old.from.clone());
    let _ = std::thread::Builder::new().name("devhub-helper-read-old".into()).spawn(move || {
        // SAFETY: `from` keeps the descriptor open while this borrows it; ManuallyDrop never closes it
        let input = ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(from.as_raw_fd()) });
        for line in std::io::BufReader::new(&*input).lines().map_while(Result::ok) {
            if !line.is_empty() {
                (s.spec.answer)(line);
            }
        }
    });
    {
        let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        while st.ready != Some(new) && st.link.as_ref().is_some_and(|l| l.pid == new) {
            st = shared.wake.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }
    let pid = old.pid;
    let ended = end(old);
    say_end(shared, pid, ended, "its replaced helper", &format!(": pid {new} took over"));
}

/// Says how a helper's end went, as it went: one that still runs after its KILL is looked at
/// again, less often each time, until it is gone, and that is said too.
fn say_end(shared: &Shared, pid: u32, ended: Ended, which: &str, then: &str) {
    match ended {
        Ended::Gone => (shared.spec.say)(format!("{which} (pid {pid}) ended{then}")),
        Ended::Killed => (shared.spec.say)(format!("{which} (pid {pid}) did not end within {} s of its TERM and was killed{then}", GRACE.as_secs_f64())),
        Ended::Runs => {
            (shared.spec.say)(format!("{which} (pid {pid}) still runs after its TERM and KILL{then}; it is looked at until it is gone"));
            let t = Instant::now();
            let mut wait = Duration::from_secs(1);
            while !gone(pid) {
                std::thread::sleep(wait);
                wait = (wait * 2).min(MINUTE);
            }
            (shared.spec.say)(format!("{which} (pid {pid}) is gone, {} s after its KILL", t.elapsed().as_secs()));
        }
    }
}

/// The helper `env` names, when this pid's last image left it one that still runs, and the code it
/// runs.
fn adopt(env: &str) -> Option<(Link, String)> {
    let v = std::env::var(env).ok()?;
    let mut w = v.split(':');
    let (parent, pid, to, from) = (w.next()?.parse::<u32>().ok()?, w.next()?.parse::<u32>().ok()?, w.next()?.parse::<i32>().ok()?, w.next()?.parse::<i32>().ok()?);
    let version = w.next().unwrap_or_default().to_string();
    if parent != std::process::id() || to < 3 || from < 3 || to == from {
        return None;
    }
    // both ends open here (close-on-exec again: no child of this image gets them)
    if crate::node::close_on_exec(to, true).is_err() || crate::node::close_on_exec(from, true).is_err() {
        return None;
    }
    // SAFETY: the last image handed this image both descriptors, and nothing else here owns them
    let (to, from) = unsafe { (OwnedFd::from_raw_fd(to), OwnedFd::from_raw_fd(from)) };
    // (one that ended: its ends close as they drop here)
    alive(pid).then(|| (Link { pid, to: Arc::new(to), from: Arc::new(from), at: Instant::now() }, version))
}

fn alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // SAFETY: signal 0 only asks
    pid > 1 && unsafe { kill(pid as i32, 0) } == 0
}

fn spawn(spec: &Spec) -> Result<Link, String> {
    let mut cmd = (spec.command)()?;
    let (to_r, to_w) = std::io::pipe().map_err(|e| format!("its pipe: {e}"))?;
    let (from_r, from_w) = std::io::pipe().map_err(|e| format!("its pipe: {e}"))?;
    cmd.stdin(to_r).stdout(from_w).env_remove(spec.env);
    // (its Child is never waited on: [`end`] reaps it by pid, in this image or a later one)
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    Ok(Link { pid: child.id(), to: Arc::new(OwnedFd::from(to_w)), from: Arc::new(OwnedFd::from(from_r)), at: Instant::now() })
}

/// The helper this process has now, kept over its exec ([`crate::node::keep_fds`]); a new one gets
/// the newest state.
fn set_link(shared: &Shared, link: Option<Link>) {
    let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    let kept = link.as_ref().map(|l| {
        let (to, from) = (l.to.as_raw_fd(), l.from.as_raw_fd());
        (format!("{}:{}:{to}:{from}:{}", std::process::id(), l.pid, shared.spec.version), vec![to, from])
    });
    crate::node::keep_fds(shared.spec.env, kept);
    if link.is_some() {
        st.failed = None;
        if st.pending.is_none() { st.pending = st.written.clone(); }
    }
    st.link = link;
    shared.wake.notify_all();
}

/// Writes the newest state to the helper whenever there is one, on its own thread.
fn write_loop(shared: &Shared) {
    loop {
        let (line, link, fresh) = {
            let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while st.pending.is_none() || st.link.as_ref().is_none_or(|l| Some(l.pid) == st.failed) {
                st = shared.wake.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            let fresh = std::mem::take(&mut st.fresh);
            (st.pending.take().unwrap_or_default(), st.link.clone().expect("a link"), fresh)
        };
        let text = format!("{}{line}\n", if fresh { "\n" } else { "" });
        // SAFETY: `link.to` keeps the descriptor open while this borrows it; ManuallyDrop never closes it
        let mut out = ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(link.to.as_raw_fd()) });
        let wrote = out.write_all(text.as_bytes());
        let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        match wrote {
            Ok(()) => st.written = Some(line),
            Err(_) => {
                // gone: the next helper gets it ([`supervise`])
                st.failed = Some(link.pid);
                if st.pending.is_none() { st.pending = Some(line); }
            }
        }
    }
}

/// Reads the helper's answers until it ends; then another starts after a pause.
fn supervise(shared: &Arc<Shared>) {
    let spec = &shared.spec;
    let mut pause = spec.pause;
    loop {
        let link = shared.state.lock().unwrap_or_else(|e| e.into_inner()).link.clone();
        let Some(link) = link else {
            std::thread::sleep(pause);
            match spawn(spec) {
                Ok(link) => set_link(shared, Some(link)),
                Err(e) => (spec.say)(format!("its helper did not start: {e}; it tries again in {} s", (pause * 2).min(MINUTE).as_secs_f64())),
            }
            pause = (pause * 2).min(MINUTE);
            continue;
        };
        // SAFETY: `link.from` keeps the descriptor open while this borrows it; ManuallyDrop never closes it
        let input = ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(link.from.as_raw_fd()) });
        for line in std::io::BufReader::new(&*input).lines() {
            match line {
                Ok(line) if line.is_empty() => {}
                Ok(line) => {
                    if spec.ready.as_ref().is_some_and(|r| r(&line)) {
                        shared.state.lock().unwrap_or_else(|e| e.into_inner()).ready = Some(link.pid);
                        shared.wake.notify_all();
                    }
                    (spec.answer)(line)
                }
                Err(_) => break,
            }
        }
        let (pid, lived) = (link.pid, link.at.elapsed());
        set_link(shared, None);
        let ended = end(link);
        // (one that ran a minute starts the pauses over)
        if lived >= MINUTE {
            pause = spec.pause;
        }
        let how = if ended == Ended::Gone { "ended" } else { "closed its output" };
        (spec.say)(format!("its helper (pid {pid}) {how} after {} s; another starts in {} s", lived.as_secs(), pause.as_secs_f64()));
        if ended != Ended::Gone {
            // (the next one starts meanwhile: this one is followed on a thread of its own)
            let s = shared.clone();
            let _ = std::thread::Builder::new().name("devhub-helper-end".into()).spawn(move || say_end(&s, pid, ended, "its helper", ""));
        }
    }
}

/// How a helper's end went, checked by its pid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    /// it ended within [`GRACE`] of its TERM
    Gone,
    /// it did not, and ended at its KILL
    Killed,
    /// it still ran [`GRACE`] after its KILL
    Runs,
}

/// How long a helper gets to end after its TERM, and again after its KILL.
const GRACE: Duration = Duration::from_secs(2);

/// A helper that is to end: its input closes as the last copy of its end drops (the writer's, at
/// the latest when its write fails), it is asked to end (TERM), and killed (KILL) when it has not
/// within [`GRACE`]. Each step looks by its pid whether it is gone ([`gone`]).
fn end(link: Link) -> Ended {
    let pid = link.pid;
    drop(link);
    end_pid(pid, GRACE)
}

fn end_pid(pid: u32, grace: Duration) -> Ended {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;
    let gone_within = |grace: Duration| {
        let t = Instant::now();
        loop {
            if gone(pid) {
                return true;
            }
            if t.elapsed() >= grace {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    // SAFETY: signals one pid, the helper this process started
    let _ = unsafe { kill(pid as i32, SIGTERM) };
    if gone_within(grace) {
        return Ended::Gone;
    }
    // SAFETY: as above
    let _ = unsafe { kill(pid as i32, SIGKILL) };
    if gone_within(grace) { Ended::Killed } else { Ended::Runs }
}

/// Whether `pid` is gone: reaped here when it is this process's child, else (no child here, or
/// one the reaper of an earlier image's children took: ECHILD) no process has the pid. ECHILD
/// alone was taken for an end, and a helper that ran on was said to have ended.
fn gone(pid: u32) -> bool {
    extern "C" {
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }
    const WNOHANG: i32 = 1;
    let mut status = 0i32;
    // SAFETY: waits for one pid without blocking
    match unsafe { waitpid(pid as i32, &mut status, WNOHANG) } {
        r if r == pid as i32 => true,
        0 => false,
        _ => !alive(pid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const ENV: &str = "DEVHUB_TEST_HELPER";
    const OUT: &str = "DEVHUB_TEST_HELPER_OUT";
    const STEP: &str = "DEVHUB_TEST_HELPER_STEP";

    fn sandbox() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var_os(OUT).expect("the test's sandbox"))
    }

    /// The helper role: notes whether the helper before it still ran at its start, says it is
    /// ready, answers each state it gets with the state and its pid, and ends with its input.
    #[test]
    fn helper_role_item() {
        if std::env::var_os(OUT).is_none() { return; }
        let list = sandbox().join("helpers");
        let before = std::fs::read_to_string(&list).unwrap_or_default();
        let beside = before.lines().last().and_then(|l| l.split(' ').next()?.parse::<u32>().ok()).is_some_and(alive);
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&list).unwrap();
        writeln!(f, "{} {}", std::process::id(), if beside { "beside" } else { "alone" }).unwrap();
        let out = std::io::stdout();
        let _ = writeln!(out.lock(), "ready");
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.is_empty() { continue; }
            let mut out = out.lock();
            let _ = writeln!(out, "got {line} {}", std::process::id());
            let _ = out.flush();
        }
        std::process::exit(0);
    }

    /// The parent role: hands its helper one state, then execs itself into its next step; the
    /// last step runs other code (another version), and it also ends its helper and waits for the
    /// next one.
    #[test]
    fn helper_role_parent() {
        if std::env::var_os(OUT).is_none() || std::env::var_os(STEP).is_none() { return; }
        let dir = sandbox();
        let step: u32 = std::env::var(STEP).unwrap().parse().unwrap();
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let d = dir.clone();
        let helper = Helper::start(Spec {
            env: ENV,
            version: if step < 2 { "a".into() } else { "b".into() },
            command: Box::new(move || Ok(crate::testenv::role("helper::tests::helper_role_item", &d))),
            // (the helper role's test harness writes its own lines there too)
            answer: Box::new(move |line| if line.starts_with("got ") { let _ = tx.lock().unwrap().send(line); }),
            say: Box::new(|line| eprintln!("helper test: {line}")),
            pause: Duration::from_millis(100),
            ready: Some(Box::new(|line| line == "ready")),
        });
        let log = |line: String| {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("seen")).unwrap();
            writeln!(f, "{line}").unwrap();
        };
        let heard = |what: &str| -> String {
            let line = rx.recv().unwrap_or_else(|e| panic!("no answer to {what}: {e}"));
            assert!(line.starts_with(&format!("got {what} ")), "{line}");
            line.rsplit(' ').next().unwrap().to_string()
        };
        helper.send(format!("glyph {step}"));
        log(format!("{step} {}", heard(&format!("glyph {step}"))));
        if step < 2 {
            let mut next = crate::testenv::role("helper::tests::helper_role_parent", &dir);
            next.env(OUT, &dir).env(STEP, (step + 1).to_string());
            panic!("exec failed: {}", crate::node::exec(&mut next));
        }
        // the helper ends: another starts, and gets the newest state
        let pid: i32 = helper.pid().expect("a helper") as i32;
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // SAFETY: this test's own helper, by its recorded pid
        assert_eq!(unsafe { kill(pid, 9) }, 0);
        log(format!("respawn {}", heard(&format!("glyph {step}"))));
        helper.send("glyph 3".into());
        log(format!("3 {}", heard("glyph 3")));
    }

    /// A helper outlives an exec of its parent in place, and each image hands it states; an image
    /// that runs other code replaces it by one started beside it, and one that ends gets a
    /// successor with the newest state. The master's menu-bar item lives in one (11-17: an exec'd
    /// process's status items are refused on macOS 27; a self-update brings it its fixes), with
    /// the panel's window (11-24: it stays on screen through a self-update). A helper's end is
    /// looked at by its pid: one that ignores its TERM is killed, and one that is no child here is
    /// not taken for gone.
    #[test]
    fn a_helper_is_kept_over_its_parents_execs_and_replaced_when_it_ends() {
        let dir = std::env::temp_dir().join(format!("devhub-helper-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let status = crate::testenv::role("helper::tests::helper_role_parent", &dir).env(OUT, &dir).env(STEP, "0")
            .stdout(std::process::Stdio::null()).status().unwrap();
        assert!(status.success(), "{status}");
        let seen = std::fs::read_to_string(dir.join("seen")).unwrap();
        let lines: Vec<(&str, &str)> = seen.lines().map(|l| l.split_once(' ').unwrap()).collect();
        assert_eq!(lines.iter().map(|(what, _)| *what).collect::<Vec<_>>(), ["0", "1", "2", "respawn", "3"], "{seen}");
        assert!(lines[1].1 == lines[0].1, "one helper through an exec: {seen}");
        assert!(lines[2].1 != lines[0].1, "a new helper for an image with other code: {seen}");
        let first: u32 = lines[0].1.parse().unwrap();
        assert!(!alive(first), "the replaced helper ended: {seen}");
        assert!(lines[3].1 != lines[2].1 && lines[4].1 == lines[3].1, "a new helper after the end of the last: {seen}");
        let helpers = std::fs::read_to_string(dir.join("helpers")).unwrap();
        let starts: Vec<(&str, &str)> = helpers.lines().map(|l| l.split_once(' ').unwrap()).collect();
        assert_eq!(starts.iter().map(|(pid, _)| *pid).collect::<Vec<_>>(), [lines[0].1, lines[2].1, lines[3].1], "{helpers}");
        assert_eq!(starts.iter().map(|(_, how)| *how).collect::<Vec<_>>(), ["alone", "beside", "alone"],
            "the replacement started beside the helper it replaced, which ended once it was ready; a successor of one that ended, alone: {helpers}");
        // the parent ended: so does its helper
        let last: u32 = lines[4].1.parse().unwrap();
        let t = Instant::now();
        while alive(last) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(last), "the helper outlived its parent");

        // a helper's end, by its pid ([`end_pid`]), with a short grace: `sh` holds its input open
        use std::io::Read;
        let grace = Duration::from_millis(200);
        let sh = |script: &str| {
            std::process::Command::new("/bin/sh").args(["-c", script]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap()
        };
        let mut ends = sh("read x");
        assert_eq!(end_pid(ends.id(), grace), Ended::Gone, "one that ends at its TERM is gone");
        // (once its trap is set: a TERM before it ends the shell)
        let mut stays = sh("trap '' TERM; echo up; read x");
        let mut up = String::new();
        std::io::BufReader::new(stays.stdout.take().unwrap()).read_line(&mut up).unwrap();
        assert_eq!(up.trim(), "up");
        let pid = stays.id();
        assert_eq!(end_pid(pid, grace), Ended::Killed, "one that ignores its TERM is killed after its grace");
        assert!(!alive(pid), "the killed one is gone");
        // (a TERM-proof `sleep` whose `sh` ended: no child of this process, waitpid says ECHILD)
        let mut parent = sh("trap '' TERM; sleep 30 > /dev/null 2>&1 & echo $!");
        let mut said = String::new();
        parent.stdout.take().unwrap().read_to_string(&mut said).ok();
        let _ = parent.wait();
        let orphan: u32 = said.trim().parse().unwrap_or_else(|e| panic!("the orphan's pid {said:?}: {e}"));
        assert!(alive(orphan), "the orphan runs");
        assert_eq!(end_pid(orphan, grace), Ended::Killed, "one that is no child here is looked at by its pid, not taken for gone at ECHILD");
        assert!(!alive(orphan), "the orphan is gone");
        drop((ends.stdin.take(), stays.stdin.take()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
