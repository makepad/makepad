//! The binary in a real pseudo-terminal against the scripted agent
//! (CODE_MOCK=1): type a request, watch it stream, approve the edit with
//! `y`, interrupt a second request with Esc, list and resume the saved
//! session, leave with Ctrl+D. Also one line-mode (pipe) run.

use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod ffi {
    use std::os::raw::{c_char, c_int, c_ulong};
    #[repr(C)]
    pub struct Winsize {
        pub rows: u16,
        pub cols: u16,
        pub x: u16,
        pub y: u16,
    }
    #[cfg(target_os = "linux")]
    pub const TIOCSWINSZ: c_ulong = 0x5414;
    #[cfg(target_os = "macos")]
    pub const TIOCSWINSZ: c_ulong = 0x80087467;
    #[cfg(target_os = "linux")]
    pub const O_NOCTTY: c_int = 0o400;
    #[cfg(target_os = "macos")]
    pub const O_NOCTTY: c_int = 0x20000;
    pub const O_RDWR: c_int = 2;
    extern "C" {
        pub fn posix_openpt(flags: c_int) -> c_int;
        pub fn grantpt(fd: c_int) -> c_int;
        pub fn unlockpt(fd: c_int) -> c_int;
        pub fn ptsname_r(fd: c_int, buf: *mut c_char, len: usize) -> c_int;
        pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    }
    pub const F_SETFD: c_int = 2;
    pub const FD_CLOEXEC: c_int = 1;
}

struct Pty {
    master: std::fs::File,
    slave: PathBuf,
}

fn open_pty() -> Pty {
    unsafe {
        let fd = ffi::posix_openpt(ffi::O_RDWR | ffi::O_NOCTTY);
        assert!(fd >= 0, "posix_openpt");
        assert_eq!(ffi::grantpt(fd), 0);
        assert_eq!(ffi::unlockpt(fd), 0);
        // Children must not inherit the master, or it never closes.
        ffi::fcntl(fd, ffi::F_SETFD, ffi::FD_CLOEXEC);
        // ptsname's buffer is one per process: a test beside this one would change the name under it
        // and both children would sit on one terminal
        let mut name = [0 as std::os::raw::c_char; 128];
        assert_eq!(ffi::ptsname_r(fd, name.as_mut_ptr(), name.len()), 0);
        let name = std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
        Pty { master: std::fs::File::from_raw_fd(fd), slave: name.into() }
    }
}

fn set_size(file: &std::fs::File, cols: u16, rows: u16) {
    use std::os::unix::io::AsRawFd;
    let ws = ffi::Winsize { rows, cols, x: 0, y: 0 };
    unsafe { ffi::ioctl(file.as_raw_fd(), ffi::TIOCSWINSZ, &ws as *const ffi::Winsize) };
}

/// Text without escape sequences, carriage returns kept as line breaks.
fn plain(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for d in chars.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("makepad-code-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("project/apps/code/src")).unwrap();
    std::fs::write(dir.join("project/apps/code/src/main.rs"), "use x;\n\napp_main!(App);\n\nfn main() {}\n").unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    dir
}

struct Session {
    pty: Pty,
    out: Arc<Mutex<Vec<u8>>>,
    child: std::process::Child,
    seen: usize,
}

impl Drop for Session {
    /// A failed test must not leave its child running.
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Session {
    fn start(dir: &Path, term: &str) -> Session {
        let pty = open_pty();
        // The size lives with the open slave (macOS resets it when the last
        // slave closes): set it on one kept open, then hand out clones.
        let held = std::fs::OpenOptions::new().read(true).write(true).open(&pty.slave).unwrap();
        set_size(&held, 100, 30);
        let slave = || held.try_clone().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_makepad-code"))
            .arg("--root")
            .arg(dir.join("project"))
            .env("CODE_MOCK", "1")
            .env("HOME", dir.join("home"))
            .env("TERM", term)
            .env("LANG", "C.UTF-8")
            .env_remove("NO_COLOR")
            .env_remove("LC_ALL")
            .env_remove("LC_CTYPE")
            .stdin(slave())
            .stdout(slave())
            .stderr(slave())
            .spawn()
            .unwrap();
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink = out.clone();
        let mut reader = pty.master.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => sink.lock().unwrap().extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        eprintln!("pty reader: {e}");
                        break;
                    }
                }
            }
        });
        Session { pty, out, child, seen: 0 }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.pty.master.write_all(bytes).unwrap();
        self.pty.master.flush().unwrap();
        std::thread::sleep(Duration::from_millis(80));
    }

    fn raw(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().unwrap()).into_owned()
    }

    /// Wait for `what` in the output after the last thing waited for. The child takes as long as it
    /// needs (a first run under the JIT compiles as it goes): this fails only once it has exited
    /// without writing `what`, or has written nothing for SILENCE.
    fn expect(&mut self, what: &str) -> String {
        let mut watch = Watch::new(self.raw().len());
        let mut exited: Option<Instant> = None;
        loop {
            let raw = self.raw();
            let all = plain(&raw);
            if let Some(at) = all[self.seen.min(all.len())..].find(what) {
                self.seen += at + what.len();
                return all;
            }
            // what it wrote just before it exited may still be on its way through the reader
            if exited.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
                panic!("exited without writing {what:?}; output so far:\n{}", &all[self.seen.min(all.len())..]);
            }
            if exited.is_none() && self.child.try_wait().unwrap().is_some() {
                exited = Some(Instant::now());
            }
            if watch.silent(raw.len()) {
                panic!("did not see {what:?}: no output for {SILENCE:?}; output so far:\n{}", &all[self.seen.min(all.len())..]);
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let mut watch = Watch::new(self.raw().len());
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            let raw = self.raw();
            if watch.silent(raw.len()) {
                let _ = self.child.kill();
                panic!("did not exit: no output for {SILENCE:?}; output:\n{}", plain(&raw));
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

/// How long a live child may write nothing before the test calls it hung.
const SILENCE: Duration = Duration::from_secs(300);

struct Watch { len: usize, since: Instant }

impl Watch {
    fn new(len: usize) -> Watch { Watch { len, since: Instant::now() } }
    fn silent(&mut self, len: usize) -> bool {
        if len != self.len {
            *self = Watch::new(len);
        }
        self.since.elapsed() > SILENCE
    }
}

#[test]
fn interactive_session_in_a_pty() {
    let dir = scratch("pty");
    let mut s = Session::start(&dir, "xterm-256color");
    s.expect("scripted / offline");
    s.expect("allowlist");
    // A request; the scripted agent streams, runs tools, asks for the edit.
    s.send(b"fix the handler\r");
    s.expect("fix the handler");
    s.expect("I'll start by finding where this is handled.");
    s.expect("Applying the change now.");
    s.expect("edit apps/code/src/main.rs");
    let all = s.expect("[y] apply ticked");
    assert!(all.contains("@@ -1,5 +1,5 @@") || all.contains("@@"), "the edit's diff is shown");
    assert!(all.contains("+app_main!(App); // the agent was here"), "the added line is shown");
    // A hunk toggled off and on again, then applied.
    s.send(b"1");
    s.send(b"1");
    s.send(b"y");
    s.expect("allowed");
    s.expect("cargo check --release");
    s.expect("Finished `release` profile");
    s.expect("Want me to apply it?");
    // Coloured diff lines went out as SGR green/red.
    assert!(s.raw().contains("\x1b[32m+app_main!(App); // the agent was here"));
    // A second request, interrupted with Esc while it streams.
    std::thread::sleep(Duration::from_millis(300));
    s.send(b"again please\r");
    s.expect("again please");
    s.expect("Looking for where");
    s.send(b"\x1b");
    s.expect("interrupted");
    // The first turn was saved; /resume lists it and picks it up.
    std::thread::sleep(Duration::from_millis(300));
    s.send(b"/resume\r");
    s.expect("1  ");
    s.expect("fix the handler");
    s.send(b"/resume 1\r");
    s.expect("-- resumed");
    s.expect("end of the saved conversation");
    // History: Up brings the last line back.
    s.send(b"\x1b[A");
    s.expect("/resume 1");
    s.send(b"\x15");
    // Ctrl+D at an empty prompt leaves, and the terminal is put back.
    s.send(b"\x04");
    let status = s.wait_exit();
    assert!(status.success());
    assert!(s.raw().contains("\x1b[?2004l"), "bracketed paste is turned off on the way out");
    let history = std::fs::read_to_string(dir.join("home/.makepad/code/cli-history")).unwrap();
    assert!(history.contains("fix the handler"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bare_console_uses_ascii() {
    let dir = scratch("linux");
    let mut s = Session::start(&dir, "linux");
    s.expect("allowlist");
    s.send(b"/help\r");
    s.expect("/resume");
    s.send(b"/approve auto\r");
    s.expect("approval: auto");
    s.send(b"\x03");
    s.expect("press Ctrl+C again");
    s.send(b"\x03");
    assert!(s.wait_exit().success());
    let raw = s.raw();
    assert!(!raw.contains('›'), "no Unicode prompt on TERM=linux");
    assert!(raw.contains("> "));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn line_mode_from_a_pipe() {
    let dir = scratch("pipe");
    let mut child = Command::new(env!("CARGO_BIN_EXE_makepad-code"))
        .arg("--root")
        .arg(dir.join("project"))
        .env("CODE_MOCK", "1")
        .env("HOME", dir.join("home"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The request, then the approval answer (taken when it is asked).
    child.stdin.as_mut().unwrap().write_all(b"fix the handler\ny\n").unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(!text.contains('\x1b'), "plain text when stdout is a pipe");
    assert!(text.contains("I'll start by finding where this is handled."), "{text}");
    assert!(text.contains("+app_main!(App); // the agent was here"), "{text}");
    assert!(text.contains("allowed"), "{text}");
    assert!(text.contains("Want me to apply it?"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn print_mode_runs_one_task() {
    let dir = scratch("print");
    let out = Command::new(env!("CARGO_BIN_EXE_makepad-code"))
        .args(["--approve", "auto", "--root"])
        .arg(dir.join("project"))
        .args(["-p", "fix", "the", "handler"])
        .env("CODE_MOCK", "1")
        .env("HOME", dir.join("home"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("Want me to apply it?"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
