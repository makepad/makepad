//! Process list/kill used by the remote protocol. Native APIs only — no
//! `cmd` / PowerShell, so nothing flashes a console on the target box.

use std::io;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct SpawnRecord {
    pub pid: u32,
    pub command: String,
    pub log: PathBuf,
}

pub fn list_processes() -> io::Result<Vec<Proc>> {
    list_processes_impl()
}

pub fn kill_pid(pid: u32) -> io::Result<()> {
    if pid == std::process::id() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing to kill the remote server process",
        ));
    }
    kill_pid_impl(pid)
}

pub fn kill_tree(pid: u32) -> io::Result<Vec<u32>> {
    if pid == std::process::id() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing to kill the remote server process",
        ));
    }
    let procs = list_processes()?;
    let mut targets = Vec::new();
    collect_descendants(&procs, pid, &mut targets);
    targets.push(pid);
    let self_pid = std::process::id();
    targets.retain(|p| *p != self_pid);
    let mut killed = Vec::new();
    for target in targets {
        if kill_pid_impl(target).is_ok() {
            killed.push(target);
        }
    }
    if killed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("could not kill {pid}"),
        ));
    }
    Ok(killed)
}

pub fn is_alive(pid: u32) -> bool {
    is_alive_impl(pid)
}

fn collect_descendants(procs: &[Proc], parent: u32, out: &mut Vec<u32>) {
    for proc in procs {
        if proc.ppid == parent && proc.pid != parent {
            collect_descendants(procs, proc.pid, out);
            if !out.contains(&proc.pid) {
                out.push(proc.pid);
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use super::Proc;
    use std::io;
    use std::mem::{size_of, zeroed};

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const INVALID_HANDLE_VALUE: *mut u8 = !0usize as *mut u8;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut u8;
        fn Process32FirstW(snapshot: *mut u8, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: *mut u8, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut u8;
        fn TerminateProcess(process: *mut u8, exit_code: u32) -> i32;
        fn GetExitCodeProcess(process: *mut u8, exit_code: *mut u32) -> i32;
        fn CloseHandle(object: *mut u8) -> i32;
    }

    fn wide_to_string(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    pub fn list() -> io::Result<Vec<Proc>> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut entry: ProcessEntry32W = zeroed();
            entry.dw_size = size_of::<ProcessEntry32W>() as u32;
            let mut out = Vec::new();
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    out.push(Proc {
                        pid: entry.th32_process_id,
                        ppid: entry.th32_parent_process_id,
                        name: wide_to_string(&entry.sz_exe_file),
                    });
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
            Ok(out)
        }
    }

    pub fn kill(pid: u32) -> io::Result<()> {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ok = TerminateProcess(handle, 1);
            CloseHandle(handle);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    pub fn alive(pid: u32) -> bool {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE
        }
    }
}

#[cfg(windows)]
fn list_processes_impl() -> io::Result<Vec<Proc>> {
    win::list()
}
#[cfg(windows)]
fn kill_pid_impl(pid: u32) -> io::Result<()> {
    win::kill(pid)
}
#[cfg(windows)]
fn is_alive_impl(pid: u32) -> bool {
    win::alive(pid)
}

#[cfg(unix)]
fn list_processes_impl() -> io::Result<Vec<Proc>> {
    let mut out = Vec::new();
    let proc_dir = std::fs::read_dir("/proc")?;
    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let comm_end = match stat.rfind(')') {
            Some(i) => i,
            None => continue,
        };
        let comm_start = match stat.find('(') {
            Some(i) => i + 1,
            None => continue,
        };
        let comm = stat[comm_start..comm_end].to_string();
        let rest = stat[comm_end + 1..].split_whitespace();
        let mut rest = rest.skip(1);
        let ppid = rest
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        out.push(Proc {
            pid,
            ppid,
            name: comm,
        });
    }
    Ok(out)
}

#[cfg(unix)]
fn kill_pid_impl(pid: u32) -> io::Result<()> {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;
    unsafe {
        if kill(pid as i32, SIGTERM) != 0 {
            return Err(io::Error::last_os_error());
        }
        if kill(pid as i32, 0) == 0 {
            let _ = kill(pid as i32, SIGKILL);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_alive_impl(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }
    // Spawned processes are forgotten direct children; reap here so exited
    // ones don't linger as zombies (kill(pid,0) reports zombies as alive,
    // which would keep them in the spawn list forever). WNOHANG == 1 on
    // both Linux and macOS; for non-child pids waitpid fails and we fall
    // through to the signal probe.
    const WNOHANG: i32 = 1;
    unsafe {
        if waitpid(pid as i32, std::ptr::null_mut(), WNOHANG) == pid as i32 {
            return false;
        }
        kill(pid as i32, 0) == 0
    }
}
