//! A launched session's process group, per OS: start it detached in a group of its own, signal
//! the group, and read a process's start time (pid + start time name one process for good: a
//! reused pid has a different start).
//!
//! - Unix: `process_group(0)`; the group is signalled with `kill(-pid)` (TERM, then KILL).
//! - Windows: `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` (no console flashes up for a console
//!   program); there is no TERM, so both ends stop the process and its descendants (Toolhelp
//!   snapshot, pid + start time checked before each stop).

use std::process::Command;

/// A child command suitable for a GUI/background caller: Windows console programs
/// run without a console window. Other platforms keep std's default behavior.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    no_console_window(&mut command);
    command
}

/// Sets CREATE_NO_WINDOW on Windows. Call before configuring any additional flags.
pub fn no_console_window(command: &mut Command) {
    #[cfg(windows)]
    windows::process::no_window(command, 0);
    #[cfg(not(windows))]
    let _ = command;
}

/// Starts `cmd` in a group of its own, away from this process's console / terminal.
pub fn detach(cmd: &mut Command) {
    imp::detach(cmd)
}

/// Signals the group led by `pid`: `hard` = KILL (Unix) / stop; otherwise TERM (Unix) / stop.
/// False when nothing could be signalled.
pub fn signal(pid: u32, hard: bool) -> bool {
    pid > 1 && pid != std::process::id() && imp::signal(pid, hard)
}

/// The process's start time (Linux: clock ticks since boot; macOS and Windows: µs since the
/// epoch), None when there is no such (live) process.
pub fn start_time(pid: u32) -> Option<u64> {
    imp::start_time(pid)
}

#[cfg(unix)]
mod imp {
    use std::process::Command;

    pub fn detach(cmd: &mut Command) {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    pub fn signal(pid: u32, hard: bool) -> bool {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe { kill(-(pid as i32), if hard { 9 } else { 15 }) == 0 }
    }

    pub fn start_time(pid: u32) -> Option<u64> {
        crate::relay::proc_start(pid)
    }
}

#[cfg(windows)]
mod imp {
    use std::process::Command;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, TerminateProcess, CREATE_NEW_PROCESS_GROUP, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};

    pub fn detach(cmd: &mut Command) {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP.0 | windows::session::Win32::System::Threading::DETACHED_PROCESS.0);
    }

    fn ft(t: &FILETIME) -> u64 {
        ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
    }

    /// The handle closes when this drops.
    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// µs since the Unix epoch (as macOS: a ledger's JSON number holds it exactly).
    pub fn start_time(pid: u32) -> Option<u64> {
        let h = Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
        let mut code = 0u32;
        // STILL_ACTIVE
        if unsafe { GetExitCodeProcess(h.0, &mut code) }.is_err() || code != 259 {
            return None;
        }
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) }.ok()?;
        // FILETIME: 100 ns since 1601
        Some(ft(&c).saturating_sub(116_444_736_000_000_000) / 10)
    }

    /// (pid, parent pid) of every process.
    pub fn snapshot() -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return out };
        let snap = Owned(snap);
        let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = unsafe { Process32FirstW(snap.0, &mut e) }.is_ok();
        while ok {
            out.push((e.th32ProcessID, e.th32ParentProcessID));
            ok = unsafe { Process32NextW(snap.0, &mut e) }.is_ok();
        }
        out
    }

    /// `pid` and its descendants (a parent pid can be stale on Windows: a child counts only when
    /// it started after its parent).
    pub fn tree(pid: u32) -> Vec<u32> {
        let all = snapshot();
        let mut out = vec![pid];
        let mut i = 0;
        while i < out.len() {
            let p = out[i];
            let ps = start_time(p);
            for (c, pp) in &all {
                if *pp == p && !out.contains(c) && *c != 0 && ps.is_some_and(|ps| start_time(*c).is_some_and(|cs| cs >= ps)) {
                    out.push(*c);
                }
            }
            i += 1;
        }
        out
    }

    pub fn stop(pid: u32) -> bool {
        let Ok(h) = (unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }) else { return false };
        let h = Owned(h);
        unsafe { TerminateProcess(h.0, 1) }.is_ok()
    }

    pub fn signal(pid: u32, _hard: bool) -> bool {
        let me = std::process::id();
        // children first: a parent that is stopped first can no longer be followed to them
        let mut any = false;
        for p in tree(pid).into_iter().rev().filter(|p| *p != me) {
            any |= stop(p);
        }
        any
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::process::Command;
    pub fn detach(_cmd: &mut Command) {}
    pub fn signal(_pid: u32, _hard: bool) -> bool {
        false
    }
    pub fn start_time(_pid: u32) -> Option<u64> {
        None
    }
}

#[cfg(windows)]
pub use imp::tree;
