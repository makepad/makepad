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

/// One running process: its parent and name (None once it is gone).
pub fn process(pid: u32) -> Option<Proc> {
    process_impl(pid)
}

/// Who a process is, for a log line that names who asked for something: its pid and name, then
/// its parents' up to init (`pid 812 kill, from 811 zsh, from 790 node`). Natively and at once,
/// so a short-lived sender such as a `kill` is still there to be read.
pub fn said(pid: u32) -> String {
    let Some(first) = process(pid) else { return format!("pid {pid} (gone already)") };
    let mut text = format!("pid {pid} {}", first.name);
    let mut parent = first.ppid;
    for _ in 0..4 {
        if parent <= 1 {
            break;
        }
        let Some(p) = process(parent) else {
            text.push_str(&format!(", from {parent}"));
            break;
        };
        text.push_str(&format!(", from {parent} {}", p.name));
        parent = p.ppid;
    }
    text
}

/// `root` and every process under it, root first.
pub fn tree(root: u32) -> Vec<u32> {
    let mut out = vec![root];
    if let Ok(procs) = list_processes() {
        collect_descendants(&procs, root, &mut out);
    }
    out
}

/// The CPU time `pid` has run, its user and system time, in ns: None once it is gone, or where
/// the platform cannot say.
pub fn cpu_ns(pid: u32) -> Option<u64> {
    cpu_ns_impl(pid)
}

/// The time the threads of `pid` have waited on a run queue for a CPU, in ns: the second field
/// of each `/proc/<pid>/task/<tid>/schedstat`. A process that wants to run on a saturated box
/// waits there and runs next to nothing, yet it is not idle. None once it is gone, or where the
/// platform (or a kernel without scheduler statistics) cannot say. A thread that ended takes its
/// wait with it: compare two readings with a saturating difference.
pub fn run_wait_ns(pid: u32) -> Option<u64> {
    run_wait_ns_impl(pid)
}

/// The run-queue wait of one `schedstat` line (`<ns on a CPU> <ns waited on a run queue>
/// <timeslices run>`).
pub fn schedstat_wait_ns(line: &str) -> Option<u64> {
    let mut fields = line.split_whitespace();
    fields.next()?.parse::<u64>().ok()?;
    let waited = fields.next()?.parse().ok()?;
    fields.next()?.parse::<u64>().ok()?;
    Some(waited)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn run_wait_ns_impl(pid: u32) -> Option<u64> {
    let tasks = std::fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    let mut waited = None;
    for task in tasks.flatten() {
        if let Some(w) = std::fs::read_to_string(task.path().join("schedstat")).ok().and_then(|line| schedstat_wait_ns(&line)) {
            *waited.get_or_insert(0) += w;
        }
    }
    waited
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn run_wait_ns_impl(_pid: u32) -> Option<u64> {
    None
}

/// A thread in an uninterruptible kernel wait ([`blocked_threads`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Blocked {
    pub pid: u32,
    pub tid: u32,
    pub name: String,
    /// the kernel function it sleeps in, empty where the kernel names none
    pub wchan: String,
}

/// The threads of `pid` in an uninterruptible kernel wait now (state `D`), each with the kernel
/// function it sleeps in where the kernel names it (`wchan`). A thread that waits on the disk or
/// a filesystem lock runs no CPU, yet its process is held there, not idle. Empty once it is gone,
/// or where the platform cannot say.
pub fn blocked_threads(pid: u32) -> Vec<Blocked> {
    blocked_threads_impl(pid)
}

/// Thread `tid` of `pid` as its `/proc/<pid>/task/<tid>/stat` line says, when its state is `D`
/// ([`thread_of`]).
pub fn blocked_of(pid: u32, tid: u32, stat: &str, wchan: &str) -> Option<Blocked> {
    let t = thread_of(tid, stat, wchan)?;
    (t.state == 'D').then(|| Blocked { pid, tid, name: t.name, wchan: t.wchan })
}

/// One thread of a process and what it does now ([`threads`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    pub tid: u32,
    pub name: String,
    /// the kernel's state letter: `R` runs or waits for a CPU, `S` sleeps, `D` sleeps
    /// uninterruptibly, `T` is stopped, `Z` has ended
    pub state: char,
    /// the kernel function it sleeps in, empty where the kernel names none
    pub wchan: String,
}

/// One row of Linux `/proc/locks`. The blocked row shares an ordinal with its blocker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockEntry {
    pub ordinal: u64,
    pub blocked: bool,
    pub kind: String,
    pub mode: String,
    pub access: String,
    /// None for a pid-less lock or an owner outside the PID namespace.
    pub pid: Option<u32>,
    pub dev_major: u32,
    pub dev_minor: u32,
    pub ino: u64,
}

/// An active lock holder, with the process details readable at inspection time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockHolder {
    pub pid: Option<u32>,
    pub kind: String,
    pub access: String,
    pub name: Option<String>,
    pub cmdline: Vec<String>,
    pub threads: Vec<Thread>,
}

/// A blocked lock request. The holder with the waiter's ordinal appears first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockWait {
    pub waiter_pid: u32,
    pub kind: String,
    pub want: String,
    pub dev_major: u32,
    pub dev_minor: u32,
    pub ino: u64,
    /// Target of an open descriptor, including the kernel's ` (deleted)` suffix.
    pub path: Option<PathBuf>,
    pub holders: Vec<LockHolder>,
}

/// Parses active and blocked rows of Linux `/proc/locks`, skipping malformed rows.
pub fn parse_proc_locks(text: &str) -> Vec<LockEntry> {
    text.lines().filter_map(|line| {
        let mut fields = line.split_ascii_whitespace();
        let ordinal = fields.next()?.strip_suffix(':')?.parse().ok()?;
        let first = fields.next()?;
        let blocked = first == "->";
        let kind = if blocked { fields.next()? } else { first };
        let mode = fields.next()?;
        let access = fields.next()?;
        let pid: i32 = fields.next()?.parse().ok()?;
        let mut key = fields.next()?.split(':');
        let dev_major = u32::from_str_radix(key.next()?, 16).ok()?;
        let dev_minor = u32::from_str_radix(key.next()?, 16).ok()?;
        let ino = key.next()?.parse().ok()?;
        if key.next().is_some() {
            return None;
        }
        Some(LockEntry {
            ordinal, blocked, kind: kind.into(), mode: mode.into(), access: access.into(),
            pid: (pid > 0).then_some(pid as u32), dev_major, dev_minor, ino,
        })
    }).collect()
}

/// Pairs each blocked request from `waiter_pids` with conflicting active locks on its inode.
pub fn match_lock_waits(entries: &[LockEntry], waiter_pids: &[u32]) -> Vec<(LockEntry, Vec<LockEntry>)> {
    let mut waits = Vec::new();
    for w in entries.iter().filter(|w| w.blocked && w.pid.is_some_and(|p| waiter_pids.contains(&p))) {
        let mut holders: Vec<_> = entries.iter().filter(|h| {
            let same_family = h.kind == w.kind ||
                (matches!(h.kind.as_str(), "POSIX" | "OFDLCK") && matches!(w.kind.as_str(), "POSIX" | "OFDLCK"));
            let conflicts = match w.access.as_str() {
                "WRITE" | "RW" => matches!(h.access.as_str(), "READ" | "WRITE" | "RW"),
                "READ" => matches!(h.access.as_str(), "WRITE" | "RW"),
                _ => false,
            };
            !h.blocked && (h.dev_major, h.dev_minor, h.ino) == (w.dev_major, w.dev_minor, w.ino)
                && same_family && (h.ordinal == w.ordinal || conflicts)
        }).collect();
        holders.sort_by_key(|&h| (h.ordinal != w.ordinal, h.pid, h.access.as_str()));
        let mut seen = std::collections::HashSet::new();
        holders.retain(|&h| seen.insert((h.pid, h.kind.as_str(), h.access.as_str())));
        waits.push((w.clone(), holders.into_iter().cloned().collect()));
    }
    waits
}

/// The Linux device major and minor encoded in `st_dev`.
pub fn dev_major_minor(dev: u64) -> (u32, u32) {
    let major = (((dev & 0x0000_0000_000f_ff00) >> 8) | ((dev & 0xffff_f000_0000_0000) >> 32)) as u32;
    let minor = ((dev & 0x0000_0000_0000_00ff) | ((dev & 0x0000_0fff_fff0_0000) >> 12)) as u32;
    (major, minor)
}

/// Arguments from `/proc/<pid>/cmdline`, empty where unavailable.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn cmdline(pid: u32) -> Vec<String> {
    let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return Vec::new() };
    if bytes.is_empty() { return Vec::new() }
    bytes.strip_suffix(&[0]).unwrap_or(&bytes).split(|&b| b == 0)
        .map(|arg| String::from_utf8_lossy(arg).into_owned()).collect()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn cmdline(_pid: u32) -> Vec<String> {
    Vec::new()
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn resolve_lock_path(waiter_pid: u32, lock: &LockEntry, holders: &[LockEntry]) -> Option<PathBuf> {
    use std::collections::HashSet;
    use std::os::unix::fs::MetadataExt;

    let mut scanned = HashSet::new();
    let mut waiter_paths = HashSet::new();
    let mut shared_paths = HashSet::new();
    for pid in std::iter::once(waiter_pid).chain(holders.iter().filter_map(|h| h.pid)) {
        if !scanned.insert(pid) { continue }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        for fd in fds.flatten() {
            let fd = fd.path();
            let Ok(meta) = std::fs::metadata(&fd) else { continue };
            if meta.ino() != lock.ino { continue }
            let Ok(path) = std::fs::read_link(fd) else { continue };
            if dev_major_minor(meta.dev()) == (lock.dev_major, lock.dev_minor) {
                return Some(path);
            }
            // A subvolume or overlay can report another device for the same inode.
            if pid == waiter_pid {
                waiter_paths.insert(path);
            } else if waiter_paths.contains(&path) {
                shared_paths.insert(path);
            }
        }
    }
    if waiter_paths.len() == 1 {
        waiter_paths.into_iter().next()
    } else if holders.iter().any(|h| h.pid == Some(waiter_pid)) {
        // Every waiter path is also a holder path in a self-wait.
        None
    } else if shared_paths.len() == 1 {
        shared_paths.into_iter().next()
    } else {
        None
    }
}

/// Observed file-lock waits, with the path and holder details that procfs still exposes.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn lock_waits(waiter_pids: &[u32]) -> Vec<LockWait> {
    let Ok(text) = std::fs::read_to_string("/proc/locks") else { return Vec::new() };
    let mut processes = std::collections::HashMap::new();
    let mut waits = Vec::new();
    for (w, active) in match_lock_waits(&parse_proc_locks(&text), waiter_pids) {
        let Some(waiter_pid) = w.pid else { continue };
        let path = resolve_lock_path(waiter_pid, &w, &active);
        let holders = active.into_iter().map(|h| {
            let (name, cmdline, threads) = h.pid.map(|pid| {
                processes.entry(pid).or_insert_with(|| (process(pid).map(|p| p.name), cmdline(pid), threads(pid))).clone()
            }).unwrap_or_default();
            LockHolder { pid: h.pid, kind: h.kind, access: h.access, name, cmdline, threads }
        }).collect();
        waits.push(LockWait {
            waiter_pid, kind: w.kind, want: w.access, dev_major: w.dev_major, dev_minor: w.dev_minor, ino: w.ino, path, holders,
        });
    }
    waits
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn lock_waits(_waiter_pids: &[u32]) -> Vec<LockWait> {
    Vec::new()
}

/// Thread `tid` as its `/proc/<pid>/task/<tid>/stat` line says: its name (inside the
/// parentheses, which may hold spaces and parentheses) and its state (the field after them);
/// `wchan` read beside it (`0`: the kernel names none).
pub fn thread_of(tid: u32, stat: &str, wchan: &str) -> Option<Thread> {
    let (start, end) = (stat.find('(')?, stat.rfind(')')?);
    let state = stat.get(end + 1..)?.split_whitespace().next()?.chars().next()?;
    let wchan = wchan.trim();
    Some(Thread { tid, name: stat.get(start + 1..end)?.to_string(), state, wchan: if wchan == "0" { String::new() } else { wchan.to_string() } })
}

/// Every thread of `pid` with its state and the kernel function it sleeps in, by tid: what a
/// process that seems stuck does. Empty once it is gone, or where the platform cannot say.
pub fn threads(pid: u32) -> Vec<Thread> {
    threads_impl(pid)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn threads_impl(pid: u32) -> Vec<Thread> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return Vec::new() };
    let mut out = Vec::new();
    for task in tasks.flatten() {
        let Some(tid) = task.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(task.path().join("stat")) else { continue };
        let wchan = std::fs::read_to_string(task.path().join("wchan")).unwrap_or_default();
        out.extend(thread_of(tid, &stat, &wchan));
    }
    out.sort_by_key(|t| t.tid);
    out
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn threads_impl(_pid: u32) -> Vec<Thread> {
    Vec::new()
}

/// Each thread of `/proc/<pid>/task` by [`blocked_of`].
#[cfg(any(target_os = "linux", target_os = "android"))]
fn blocked_threads_impl(pid: u32) -> Vec<Blocked> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return Vec::new() };
    let mut out = Vec::new();
    for task in tasks.flatten() {
        let Some(tid) = task.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(task.path().join("stat")) else { continue };
        // (the wchan only of a thread that sleeps so: one read for the rest)
        if !stat.rsplit_once(')').is_some_and(|(_, rest)| rest.trim_start().starts_with('D')) {
            continue;
        }
        let wchan = std::fs::read_to_string(task.path().join("wchan")).unwrap_or_default();
        out.extend(blocked_of(pid, tid, &stat, &wchan));
    }
    out.sort_by_key(|b| b.tid);
    out
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn blocked_threads_impl(_pid: u32) -> Vec<Blocked> {
    Vec::new()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proc_locks_handles_active_waiters_hex_dev_and_ofd_pids() {
        let entries = parse_proc_locks("7: FLOCK ADVISORY WRITE 10 fd:0a:987654 0 EOF
            7: -> FLOCK ADVISORY READ 20 fd:0a:987654 0 EOF
            9: POSIX ADVISORY WRITE 30 08:02:321 5 9
            10: OFDLCK ADVISORY WRITE -1 08:02:321 20 EOF
            11: FLOCK ADVISORY READ 15 fd:0a:12:34 0 EOF");
        assert_eq!(entries.len(), 4, "only complete, valid device/inode triples parse");
        let active = LockEntry {
            ordinal: 7, blocked: false, kind: "FLOCK".into(), mode: "ADVISORY".into(),
            access: "WRITE".into(), pid: Some(10), dev_major: 0xfd, dev_minor: 0x0a, ino: 987654,
        };
        assert_eq!(entries[0], active);
        assert_eq!(entries[1], LockEntry { blocked: true, access: "READ".into(), pid: Some(20), ..active.clone() });
        let posix = LockEntry { ordinal: 9, kind: "POSIX".into(), pid: Some(30), dev_major: 8, dev_minor: 2, ino: 321, ..active };
        assert_eq!(entries[2], posix);
        assert_eq!(entries[3], LockEntry { ordinal: 10, kind: "OFDLCK".into(), pid: None, ..posix });
    }

    #[test]
    fn match_lock_waits_pairs_waiter_with_single_and_multiple_shared_holders() {
        let entries = parse_proc_locks("1: FLOCK ADVISORY READ 11 103:02:555 0 EOF
            2: FLOCK ADVISORY READ 10 103:02:555 0 EOF
            2: -> FLOCK ADVISORY WRITE 20 103:02:555 0 EOF
            3: FLOCK ADVISORY READ 12 103:02:556 0 EOF
            4: POSIX ADVISORY WRITE 13 103:02:555 0 EOF
            5: FLOCK ADVISORY WRITE 30 103:02:777 0 EOF
            5: -> FLOCK ADVISORY WRITE 30 103:02:777 0 EOF
            6: FLOCK ADVISORY READ 11 103:02:555 0 EOF
            1: -> FLOCK ADVISORY WRITE 21 103:02:555 0 EOF");
        let waits = match_lock_waits(&entries, &[20, 30]);
        assert_eq!(waits.len(), 2, "only waits in the requested tree");
        assert_eq!(waits[0].0.pid, Some(20));
        assert_eq!(waits[0].1, [entries[1].clone(), entries[0].clone()], "both shared holders, blocker first");
        assert_eq!(waits[1].1, [entries[5].clone()], "a second open description can block its own process");
        let waits = match_lock_waits(&entries, &[21]);
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].1, [entries[0].clone(), entries[1].clone()], "a non-adjacent duplicate is still one holder");
    }

    #[test]
    fn dev_major_minor_decodes_standard_and_extended_linux_devs() {
        assert_eq!(dev_major_minor(0x10302), (0x103, 2));
        assert_eq!(dev_major_minor(0xfd01), (0xfd, 1));
        assert_eq!(dev_major_minor(0x0000_1000_abc2_34de), (0x1234, 0xabcde));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lock_waits_reports_real_linux_flock_path_holder_and_threads() {
        use std::fs::File;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        struct TempFile(PathBuf);
        impl Drop for TempFile {
            fn drop(&mut self) {
                std::fs::remove_file(&self.0).unwrap();
            }
        }
        let pid = std::process::id();
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("flock-{pid}-{nonce}"));
        let f1 = File::create_new(&path).unwrap();
        let temp = TempFile(path);
        let path = temp.0.canonicalize().unwrap();
        f1.lock().unwrap();
        let ready = AtomicBool::new(false);
        let waits = std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                let f2 = File::open(&path)?;
                ready.store(true, Ordering::Release);
                f2.lock()
            });
            let start = Instant::now();
            let waits = loop {
                let waits = if ready.load(Ordering::Acquire) { lock_waits(&[pid]) } else { Vec::new() };
                if !waits.is_empty() || start.elapsed() >= Duration::from_millis(500) {
                    break waits;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            drop(f1);
            waiter.join().unwrap().unwrap();
            waits
        });
        let resolved = waits.first().and_then(|w| w.path.as_deref()).and_then(|p| p.canonicalize().ok());
        drop(temp);
        assert_eq!(waits.len(), 1, "one blocked flock in this process");
        assert_eq!(waits[0].kind, "FLOCK");
        assert_eq!(waits[0].want, "WRITE");
        assert_eq!(resolved.as_ref(), Some(&path));
        let holder = waits[0].holders.iter().find(|h| h.pid == Some(pid) && h.access == "WRITE").expect("the self-holder");
        assert!(!holder.threads.is_empty(), "the holder's threads are readable");
        if holder.threads.iter().any(|t| !t.wchan.is_empty()) {
            assert!(holder.threads.iter().any(|t| t.state == 'S' && t.wchan.contains("lock_inode_wait")), "{:?}", holder.threads);
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
    const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = !0usize as *mut std::ffi::c_void;

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
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut std::ffi::c_void;
        fn Process32FirstW(snapshot: *mut std::ffi::c_void, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: *mut std::ffi::c_void, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut std::ffi::c_void;
        fn TerminateProcess(process: *mut std::ffi::c_void, exit_code: u32) -> i32;
        fn GetExitCodeProcess(process: *mut std::ffi::c_void, exit_code: *mut u32) -> i32;
        fn GetProcessTimes(process: *mut std::ffi::c_void, creation: *mut u64, exit: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
        fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
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

    /// Kernel and user time, FILETIMEs of 100 ns (read as u64: same size, and 4-aligned is
    /// enough for the call).
    pub fn cpu_ns(pid: u32) -> Option<u64> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let (mut creation, mut exit, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
            let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            CloseHandle(handle);
            (ok != 0).then(|| (kernel + user) * 100)
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
#[cfg(windows)]
fn process_impl(pid: u32) -> Option<Proc> {
    win::list().ok()?.into_iter().find(|p| p.pid == pid)
}
#[cfg(windows)]
fn cpu_ns_impl(pid: u32) -> Option<u64> {
    win::cpu_ns(pid)
}

/// `struct proc_taskinfo` (96 bytes): `pti_total_user` at 16, `pti_total_system` at 24, in Mach
/// time units the timebase turns into ns (on Apple silicon a unit is not a ns).
#[cfg(target_os = "macos")]
fn cpu_ns_impl(pid: u32) -> Option<u64> {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    const PROC_PIDTASKINFO: i32 = 4;
    let mut buf = [0u8; 96];
    let n = unsafe { proc_pidinfo(i32::try_from(pid).ok()?, PROC_PIDTASKINFO, 0, buf.as_mut_ptr(), buf.len() as i32) };
    if n != buf.len() as i32 {
        return None;
    }
    let field = |at: usize| u64::from_ne_bytes(buf[at..at + 8].try_into().unwrap());
    let mut tb = Timebase { numer: 1, denom: 1 };
    unsafe { mach_timebase_info(&mut tb) };
    Some(((field(16) + field(24)) as u128 * tb.numer as u128 / tb.denom.max(1) as u128) as u64)
}

/// `utime` and `stime` of `/proc/<pid>/stat` (its 14th and 15th fields), in clock ticks of
/// USER_HZ, which is 100 on every Linux ABI.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn cpu_ns_impl(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // after `pid (comm)` the fields start at the 3rd (state): utime is the 12th of them
    let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace().skip(11);
    let utime: u64 = fields.next()?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some((utime + stime) * 10_000_000)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android", target_os = "macos"))))]
fn cpu_ns_impl(_pid: u32) -> Option<u64> {
    None
}

/// `proc_pidinfo`'s `struct proc_bsdinfo` (136 bytes) of `pid`, None once it is gone.
#[cfg(target_os = "macos")]
fn bsd_info(pid: i32) -> Option<[u8; 136]> {
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
    }
    const PROC_PIDTBSDINFO: i32 = 3;
    let mut buf = [0u8; 136];
    let n = unsafe { proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, buf.as_mut_ptr(), buf.len() as i32) };
    (n == buf.len() as i32).then_some(buf)
}

/// A NUL-ended name field of `struct proc_bsdinfo`.
#[cfg(target_os = "macos")]
fn bsd_name(field: &[u8]) -> String {
    let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

#[cfg(target_os = "macos")]
fn process_impl(pid: u32) -> Option<Proc> {
    let buf = bsd_info(i32::try_from(pid).ok()?)?;
    // pbi_ppid at 16, pbi_comm[16] at 48, pbi_name[32] (the longer name, when set) at 64
    let ppid = u32::from_ne_bytes(buf[16..20].try_into().unwrap());
    let name = Some(bsd_name(&buf[64..96])).filter(|n| !n.is_empty()).unwrap_or_else(|| bsd_name(&buf[48..64]));
    Some(Proc { pid, ppid, name })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn process_impl(pid: u32) -> Option<Proc> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ppid ...`: comm may hold spaces and parens, so it ends at the last `)`
    let (start, end) = (stat.find('(')? + 1, stat.rfind(')')?);
    let ppid = stat.get(end + 1..)?.split_whitespace().nth(1)?.parse().ok()?;
    Some(Proc { pid, ppid, name: stat.get(start..end)?.to_string() })
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android", target_os = "macos"))))]
fn process_impl(_pid: u32) -> Option<Proc> {
    None
}

#[cfg(target_os = "macos")]
fn list_processes_impl() -> io::Result<Vec<Proc>> {
    extern "C" {
        fn proc_listallpids(buffer: *mut i32, size: i32) -> i32;
    }
    // The pid list can grow between the size query and the fill, so ask with slack and retry.
    let mut pids: Vec<i32>;
    loop {
        let want = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
        if want <= 0 {
            return Err(io::Error::last_os_error());
        }
        pids = vec![0i32; want as usize + 64];
        let bytes = unsafe { proc_listallpids(pids.as_mut_ptr(), (pids.len() * 4) as i32) };
        if bytes <= 0 {
            return Err(io::Error::last_os_error());
        }
        let n = bytes as usize / 4;
        if n < pids.len() {
            pids.truncate(n);
            break;
        }
    }
    Ok(pids.into_iter().filter(|pid| *pid > 0).filter_map(|pid| process_impl(pid as u32)).collect())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
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

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android", target_os = "macos"))))]
fn list_processes_impl() -> io::Result<Vec<Proc>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no process list on this platform"))
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
