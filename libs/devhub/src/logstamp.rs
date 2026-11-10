//! The master's log with the time on every line. Its output (its own lines, and those of the
//! children that write to its stdout or stderr) goes through a pipe to a child of its own,
//! `makepad-devhub log-stamp`, which writes each line on to the log with the UTC time it came in:
//! `2026-11-08T20:31:05.123Z devhub autosync: ...`. A log without them could not be lined up with
//! the hubs' logs or the machine's own (11-08). The stamper outlives the master's self-update exec
//! (the new image finds it, [`KEPT_ENV`]) and a crash of the master: it writes what is left in the
//! pipe, and ends when the last writer has. Should it end first, the master puts its output back
//! on the log itself and says so.

use std::io::{Read, Write};

/// `<pid>:<stamper pid>:<log fd>`: the stamper and the log a master's image left this pid over
/// its exec ([`keep_over_exec`]).
pub const KEPT_ENV: &str = "DEVHUB_LOG_STAMPER";

#[cfg(unix)]
static STAMPER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(unix)]
static LOG: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// `ms` (since the epoch) as a log line's time: `2026-11-08T20:31:05.123Z`.
pub fn stamp(ms: u64) -> String {
    let t = makepad_civil_time::utc((ms / 1000) as i64);
    format!("{:04}-{:02}-{:02}T{}.{:03}Z", t.year, t.month, t.day, t.hms(), ms % 1000)
}

/// Copies `input` to `out` a line at a time, each line starting with the time its first byte came
/// in (`now`, ms since the epoch), until `input` ends; a last line without its newline gets one.
pub fn copy_stamped(mut input: impl Read, mut out: impl Write, now: &dyn Fn() -> u64) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    let mut at_start = true;
    loop {
        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let stamp = stamp(now());
        let mut text = Vec::with_capacity(n + 64);
        for &b in &buf[..n] {
            if at_start {
                text.extend_from_slice(stamp.as_bytes());
                text.push(b' ');
            }
            text.push(b);
            at_start = b == b'\n';
        }
        out.write_all(&text)?;
        out.flush()?;
    }
    if !at_start {
        out.write_all(b"\n")?;
    }
    out.flush()
}

/// `log-stamp`: stdin to stdout, stamped ([`copy_stamped`]).
pub fn log_stamp() -> i32 {
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    match copy_stamped(std::io::stdin().lock(), std::io::stdout().lock(), &now) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("devhub log-stamp: {e}");
            1
        }
    }
}

/// The stamper a master runs: its own `log-stamp` word, by the devhub binary it runs its words
/// with once one is built. Never through cargo: the master's lines would wait on its build once
/// the pipe's 64 KB filled, and cargo's would go to the log unstamped.
#[cfg(all(unix, feature = "checkout"))]
pub fn stamper() -> Result<std::process::Command, String> {
    let own = crate::cli::own_argv(&crate::lease::wt::default_repo());
    let [bin] = own.as_slice() else { return Err("no makepad-devhub is built yet to stamp it".into()) };
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("log-stamp");
    Ok(cmd)
}

/// Puts this process's stdout and stderr through the one `stamper` makes (a [`log_stamp`] that
/// writes on to where they went), first thing in a master. An image this process exec'd into from
/// one that did so already has its stamper: it takes that one on ([`KEPT_ENV`]), and makes none.
#[cfg(unix)]
pub fn stamp_output(stamper: impl FnOnce() -> Result<std::process::Command, String>) -> Result<(), String> {
    use std::os::fd::{AsFd, AsRawFd, IntoRawFd};
    if let Some((pid, log)) = kept() {
        // (open over the exec that brought it here: from here on, as in the image that made it, it
        // goes to no child)
        let _ = crate::node::close_on_exec(log, true);
        watch(pid, log);
        return Ok(());
    }
    let mut stamper = stamper()?;
    let (reader, writer) = std::io::pipe().map_err(|e| format!("its pipe: {e}"))?;
    let log = std::io::stderr().as_fd().try_clone_to_owned().map_err(|e| format!("its log: {e}"))?;
    let (out, err) = (log.try_clone().map_err(|e| format!("its log: {e}"))?, log.try_clone().map_err(|e| format!("its log: {e}"))?);
    let child = stamper.stdin(reader).stdout(out).stderr(err).spawn().map_err(|e| format!("its stamper: {e}"))?;
    let log = log.into_raw_fd();
    for fd in [1, 2] {
        if let Err(e) = dup2(writer.as_raw_fd(), fd) {
            let _ = dup2(log, 1);
            let _ = dup2(log, 2);
            return Err(format!("its output onto the pipe: {e}"));
        }
    }
    // (its Child is never waited on: [`watch`] waits for its pid, in this image and the next)
    watch(child.id(), log);
    Ok(())
}

/// The stamper and the log this pid's previous image left it.
#[cfg(unix)]
fn kept() -> Option<(u32, i32)> {
    let v = std::env::var(KEPT_ENV).ok()?;
    let mut w = v.split(':');
    let (pid, stamper, log) = (w.next()?.parse::<u32>().ok()?, w.next()?.parse::<u32>().ok()?, w.next()?.parse::<i32>().ok()?);
    (pid == std::process::id()).then_some((stamper, log))
}

/// Waits for the stamper's end, then puts the output back on the log.
#[cfg(unix)]
fn watch(pid: u32, log: i32) {
    use std::sync::atomic::Ordering;
    STAMPER.store(pid, Ordering::Relaxed);
    LOG.store(log, Ordering::Relaxed);
    let _ = std::thread::Builder::new().name("devhub-log-stamper".into()).spawn(move || {
        extern "C" {
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        }
        let mut status = 0i32;
        // (an end the reaper of an earlier image's children took is no child here: ECHILD)
        // SAFETY: waits for one pid, the status into a local
        while unsafe { waitpid(pid as i32, &mut status, 0) } < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {}
        let _ = dup2(log, 1);
        let _ = dup2(log, 2);
        eprintln!("devhub: its log stamper (pid {pid}) ended; the log goes on without times");
        // (none once the output is back on the log: an exec now keeps no stamper)
        STAMPER.store(0, Ordering::Relaxed);
    });
}

/// The stamper this process's output goes through (0: none).
#[cfg(unix)]
pub fn stamper_pid() -> u32 {
    STAMPER.load(std::sync::atomic::Ordering::Relaxed)
}

/// Keeps the stamper and the log over `cmd`'s exec of this process ([`crate::node::exec`]).
#[cfg(unix)]
pub fn keep_over_exec(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    let (pid, log) = (stamper_pid(), LOG.load(std::sync::atomic::Ordering::Relaxed));
    if pid == 0 || log < 0 {
        cmd.env_remove(KEPT_ENV);
        return;
    }
    cmd.env(KEPT_ENV, format!("{}:{pid}:{log}", std::process::id()));
    // SAFETY: close_on_exec is fcntl only
    unsafe {
        cmd.pre_exec(move || crate::node::close_on_exec(log, false));
    }
}

/// After an exec that failed: the log's descriptor goes to no child ([`keep_over_exec`]).
#[cfg(unix)]
pub fn exec_failed() {
    let log = LOG.load(std::sync::atomic::Ordering::Relaxed);
    if log >= 0 {
        let _ = crate::node::close_on_exec(log, true);
    }
}

#[cfg(unix)]
fn dup2(from: i32, to: i32) -> std::io::Result<()> {
    extern "C" {
        #[link_name = "dup2"]
        fn c_dup2(from: i32, to: i32) -> i32;
    }
    // SAFETY: descriptors only; a bad one fails (EBADF)
    if unsafe { c_dup2(from, to) } < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line's time is when its first byte came in, whatever reads split it; a last line without
    /// its newline ends with one.
    #[test]
    fn a_line_is_stamped_with_the_time_its_first_byte_came_in() {
        struct Chunks(Vec<&'static [u8]>);
        impl Read for Chunks {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() { return Ok(0); }
                let c = self.0.remove(0);
                buf[..c.len()].copy_from_slice(c);
                Ok(c.len())
            }
        }
        let clock = std::cell::Cell::new(1_794_169_865_000u64);
        let now = || { let t = clock.get(); clock.set(t + 1_250); t };
        let mut out = Vec::new();
        copy_stamped(Chunks(vec![b"one\ntw", b"o\nthree"]), &mut out, &now).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "2026-11-08T20:31:05.000Z one\n2026-11-08T20:31:05.000Z two\n2026-11-08T20:31:06.250Z three\n");
        assert_eq!(stamp(0), "1970-01-01T00:00:00.000Z");
    }

    /// The role of [`a_masters_output_is_stamped_over_its_exec_until_its_stamper_ends`].
    #[cfg(unix)]
    #[test]
    fn logstamp_role() {
        let Ok(role) = std::env::var("DEVHUB_LOGSTAMP_ROLE") else { return };
        let base = std::path::PathBuf::from(std::env::var_os("DEVHUB_LOGSTAMP_BASE").unwrap());
        let stamper = || {
            let mut cmd = crate::testenv::role("logstamp::tests::logstamp_role", &base);
            cmd.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_LOGSTAMP_ROLE", "stamper");
            cmd
        };
        match role.as_str() {
            "stamper" => {
                std::fs::write(base.join(format!("stamper {}", std::process::id())), "").unwrap();
                std::process::exit(log_stamp());
            }
            "before" => {
                stamp_output(|| Ok(stamper())).unwrap();
                eprintln!("before line");
                assert!(std::process::Command::new("sh").args(["-c", "echo child line >&2"]).status().unwrap().success());
                let mut cmd = crate::testenv::role("logstamp::tests::logstamp_role", &base);
                cmd.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_LOGSTAMP_ROLE", "after");
                panic!("exec: {}", crate::node::exec(&mut cmd));
            }
            "after" => {
                stamp_output(|| Err("the image after the exec made a stamper of its own".into())).unwrap();
                let pid = stamper_pid();
                eprintln!("after line");
                // a child of this image gets no descriptor of the log
                let fd = LOG.load(std::sync::atomic::Ordering::Relaxed);
                let child = std::process::Command::new("sh").args(["-c", &format!(": >&{fd}")]).stderr(std::process::Stdio::null()).status().unwrap();
                assert!(!child.success(), "the log's descriptor {fd} went to a child");
                let log = base.join("log");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !std::fs::read_to_string(&log).unwrap().contains("after line") {
                    assert!(std::time::Instant::now() < deadline, "the stamper never wrote the line");
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                extern "C" {
                    fn kill(pid: i32, sig: i32) -> i32;
                }
                // SAFETY: a signal to the stamper's pid
                assert_eq!(unsafe { kill(pid as i32, 9) }, 0);
                while stamper_pid() != 0 {
                    assert!(std::time::Instant::now() < deadline, "the log was never put back");
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                eprintln!("unstamped line");
            }
            _ => {}
        }
    }

    /// A master's lines and its child's go to its log stamped, by one stamper over its exec into
    /// its next image, which makes none of its own; once the stamper ends, its lines go on to the
    /// log without times.
    #[cfg(unix)]
    #[test]
    fn a_masters_output_is_stamped_over_its_exec_until_its_stamper_ends() {
        let base = std::env::temp_dir().join(format!("devhub-logstamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let log = std::fs::File::create(base.join("log")).unwrap();
        let mut role = crate::testenv::role("logstamp::tests::logstamp_role", &base);
        role.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_LOGSTAMP_ROLE", "before").env("DEVHUB_LOGSTAMP_BASE", &base).env_remove(KEPT_ENV);
        let status = role.stdin(std::process::Stdio::null()).stdout(log.try_clone().unwrap()).stderr(log).status().unwrap();
        let text = std::fs::read_to_string(base.join("log")).unwrap();
        assert!(status.success(), "{text}");
        let stamped = |line: &str| text.lines().any(|l| l.len() > 25 && l.as_bytes()[23] == b'Z' && l.as_bytes()[10] == b'T' && &l[25..] == line);
        for line in ["before line", "child line", "after line"] {
            assert!(stamped(line), "{line:?} stamped:\n{text}");
        }
        assert!(text.lines().any(|l| l.starts_with("devhub: its log stamper (pid ") && l.ends_with(" ended; the log goes on without times")), "{text}");
        assert!(text.lines().any(|l| l == "unstamped line"), "{text}");
        let stampers = std::fs::read_dir(&base).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("stamper ")).count();
        assert_eq!(stampers, 1, "the next image takes the stamper on");
        let _ = std::fs::remove_dir_all(&base);
    }
}
