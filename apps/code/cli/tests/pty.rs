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
        pub fn ptsname(fd: c_int) -> *const c_char;
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
        let name = std::ffi::CStr::from_ptr(ffi::ptsname(fd)).to_string_lossy().into_owned();
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

    /// Wait for `what` in the output after the last thing waited for.
    fn expect(&mut self, what: &str, secs: u64) -> String {
        let t0 = Instant::now();
        loop {
            let all = plain(&self.raw());
            if let Some(at) = all[self.seen.min(all.len())..].find(what) {
                self.seen += at + what.len();
                return all;
            }
            if t0.elapsed() > Duration::from_secs(secs) {
                panic!("did not see {what:?}; output so far:\n{}", &all[self.seen.min(all.len())..]);
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn wait_exit(&mut self, secs: u64) -> std::process::ExitStatus {
        let t0 = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if t0.elapsed() > Duration::from_secs(secs) {
                let _ = self.child.kill();
                panic!("did not exit; output:\n{}", plain(&self.raw()));
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

#[test]
fn interactive_session_in_a_pty() {
    let dir = scratch("pty");
    let mut s = Session::start(&dir, "xterm-256color");
    s.expect("scripted / offline", 10);
    s.expect("allowlist", 5);
    // A request; the scripted agent streams, runs tools, asks for the edit.
    s.send(b"fix the handler\r");
    s.expect("fix the handler", 5);
    s.expect("I'll start by finding where this is handled.", 10);
    s.expect("Applying the change now.", 15);
    s.expect("edit apps/code/src/main.rs", 10);
    let all = s.expect("[y] apply ticked", 10);
    assert!(all.contains("@@ -1,5 +1,5 @@") || all.contains("@@"), "the edit's diff is shown");
    assert!(all.contains("+app_main!(App); // the agent was here"), "the added line is shown");
    // A hunk toggled off and on again, then applied.
    s.send(b"1");
    s.send(b"1");
    s.send(b"y");
    s.expect("allowed", 5);
    s.expect("cargo check --release", 10);
    s.expect("Finished `release` profile", 10);
    s.expect("Want me to apply it?", 20);
    // Coloured diff lines went out as SGR green/red.
    assert!(s.raw().contains("\x1b[32m+app_main!(App); // the agent was here"));
    // A second request, interrupted with Esc while it streams.
    std::thread::sleep(Duration::from_millis(300));
    s.send(b"again please\r");
    s.expect("again please", 5);
    s.expect("Looking for where", 10);
    s.send(b"\x1b");
    s.expect("interrupted", 5);
    // The first turn was saved; /resume lists it and picks it up.
    std::thread::sleep(Duration::from_millis(300));
    s.send(b"/resume\r");
    s.expect("1  ", 5);
    s.expect("fix the handler", 5);
    s.send(b"/resume 1\r");
    s.expect("-- resumed", 5);
    s.expect("end of the saved conversation", 5);
    // History: Up brings the last line back.
    s.send(b"\x1b[A");
    s.expect("/resume 1", 5);
    s.send(b"\x15");
    // Ctrl+D at an empty prompt leaves, and the terminal is put back.
    s.send(b"\x04");
    let status = s.wait_exit(10);
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
    s.expect("allowlist", 10);
    s.send(b"/help\r");
    s.expect("/resume", 5);
    s.send(b"/approve auto\r");
    s.expect("approval: auto", 5);
    s.send(b"\x03");
    s.expect("press Ctrl+C again", 5);
    s.send(b"\x03");
    assert!(s.wait_exit(10).success());
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
