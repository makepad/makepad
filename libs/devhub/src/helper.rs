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
//! too (11-17: a fixed menu-bar item waited for the master's next full restart).

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
        let adopted = match adopt(shared.spec.env) {
            Some((link, version)) if version != shared.spec.version => {
                let was = if version.is_empty() { "unnamed" } else { &version };
                (shared.spec.say)(format!("its helper (pid {}) runs other code than this image ({was}, now {}): another replaces it", link.pid, shared.spec.version));
                end(link);
                set_link(&shared, None);
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
                Ok(link) => set_link(&shared, Some(link)),
                Err(e) => (shared.spec.say)(format!("its helper did not start: {e}; it tries again in {} s", shared.spec.pause.as_secs_f64())),
            },
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
fn supervise(shared: &Shared) {
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
                Ok(line) => (spec.answer)(line),
                Err(_) => break,
            }
        }
        let (pid, lived) = (link.pid, link.at.elapsed());
        set_link(shared, None);
        end(link);
        // (one that ran a minute starts the pauses over)
        if lived >= MINUTE {
            pause = spec.pause;
        }
        (spec.say)(format!("its helper (pid {pid}) ended after {} s; another starts in {} s", lived.as_secs(), pause.as_secs_f64()));
    }
}

/// A helper whose output ended: its input closes as the last copy of its end drops (the writer's,
/// at the latest when its write fails), it is asked to end, and it is reaped (or the reaper of an
/// earlier image's children has: ECHILD).
fn end(link: Link) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;
    const WNOHANG: i32 = 1;
    let pid = link.pid as i32;
    drop(link);
    let mut status = 0i32;
    // SAFETY: signals and waits for one pid, its own child
    unsafe {
        let _ = kill(pid, SIGTERM);
        for _ in 0..40 {
            if waitpid(pid, &mut status, WNOHANG) != 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = kill(pid, SIGKILL);
        while waitpid(pid, &mut status, 0) < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {}
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

    /// The helper role: answers each state it gets with the state and its pid, and ends with its
    /// input.
    #[test]
    fn helper_role_item() {
        if std::env::var_os(OUT).is_none() { return; }
        let out = std::io::stdout();
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
        });
        let log = |line: String| {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("seen")).unwrap();
            writeln!(f, "{line}").unwrap();
        };
        let heard = |what: &str| -> String {
            let line = rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|e| panic!("no answer to {what}: {e}"));
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
    /// that runs other code replaces it, and one that ends gets a successor with the newest state.
    /// The master's menu-bar item lives in one (11-17: an exec'd process's status items are
    /// refused on macOS 27; a self-update brings it its fixes).
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
        // the parent ended: so does its helper
        let last: u32 = lines[4].1.parse().unwrap();
        let t = Instant::now();
        while alive(last) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(last), "the helper outlived its parent");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
