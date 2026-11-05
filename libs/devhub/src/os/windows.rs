//! Windows: a lease run is a named Job Object: its wrapper ([`metered_argv`]) is created
//! suspended, put in its job, then resumed, so every process it starts is in the job from its
//! first instruction. There is no TERM to send a process tree: a lease's end asks the apps in the
//! job to `quit` over their control endpoints and waits while they wind down, then
//! TerminateJobObject ends what is left (lease/exec.rs `stop_runs`). The run
//! is BELOW_NORMAL priority (its children inherit the class), without a console window. No process
//! leaves its job, so [`with_env`] has nothing to find.
//!
//! A run outlives the hub that started it, as on Linux: the job does not end when the hub's handle
//! closes, a restarted hub opens it again by its name ([`Hold::taken_back`]), and the run's exit is
//! the one its wrapper wrote beside the ledger, with no hub there. (A name lasts while a handle to
//! its job is open: the wrapper holds one that the run's processes inherit.) So a self-update waits for no
//! run (it waited for a moment with none running, and a busy box never had one: the Windows node
//! stayed an update behind for an hour, 11-04). A hub that crashes under `cargo run` still takes its runs
//! along: cargo's own job ends its tree when cargo exits on an error, not on a clean exit.
//!
//! The job is also the run's resource envelope (Linux gets it from its systemd scope): at most
//! [`RUN_RAM_SHARE`] of the node's RAM may be committed by the whole tree of the run, so a build
//! or test sweep that fans out can fail its own allocations but never leaves the hub and the OS
//! without memory. Its processors are never capped: a run uses the whole box when it is idle,
//! and BELOW_NORMAL keeps the hub and the desktop ahead of it under contention.
//!
//! Also the machine's numbers (memory, CPU times, processes, disk, CPU name, battery) through the
//! generated bindings (makepad-windows-sys).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, SetHandleInformation_raw, FILETIME, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next, PROCESSENTRY32W, TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetProcessTimes, GetSystemTimes, OpenProcess, OpenThread, QueryFullProcessImageNameW, ResumeThread, BELOW_NORMAL_PRIORITY_CLASS, CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE, THREAD_SUSPEND_RESUME,
};

/// A handle closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// a job handle is used from the lease threads (the waiter, the watchdog, end)
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

/// The share of the node's RAM one run may commit in total (its job's memory limit): the rest is
/// what the hub, the OS and the other leases keep when a run's builds or tests fan out. A run
/// that reaches it fails its allocations (its own tests and compiles fail); the node does not.
pub const RUN_RAM_SHARE: f64 = 0.75;

// winnt.h JOB_OBJECT_LIMIT_JOB_MEMORY (the generated bindings carry only the kill-on-close
// flag of the set, which a run's job does not have: it outlives the hub)
const LIMIT_JOB_MEMORY: JOB_OBJECT_LIMIT = JOB_OBJECT_LIMIT(0x0000_0200);
// winnt.h JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: every process in the job runs with
// SEM_NOGPFAULTERRORBOX, whatever it sets itself. A program that faults ends at once, with its
// fault's code, instead of waiting in the system's error report until its job times out.
const LIMIT_DIE_ON_FAULT: JOB_OBJECT_LIMIT = JOB_OBJECT_LIMIT(0x0000_0400);

/// A run's Job Object.
pub struct Job(Owned);

impl Job {
    /// The job named for `exit` (a run's exit file, [`job_name`]), anonymous without one: an
    /// existing one of that name, or a new one.
    fn named(exit: Option<&Path>) -> std::io::Result<Job> {
        let name = exit.map(job_name);
        let h = unsafe { CreateJobObjectW(None, name.as_ref().map(|n| PCWSTR(n.as_ptr()))) }.map_err(|_| std::io::Error::last_os_error())?;
        Ok(Job(Owned(h)))
    }

    /// A handle of the run's own job that the processes it starts inherit (the wrapper's): the job
    /// keeps its name while one of them lives, after its hub's handle closed (Windows drops an
    /// object's name with its last handle, whatever processes the job still holds).
    fn inherited(exit: &Path) -> std::io::Result<Job> {
        let name = job_name(exit);
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: windows::core::BOOL(1) };
        let h = unsafe { CreateJobObjectW(Some(&sa as *const SECURITY_ATTRIBUTES), Some(PCWSTR(name.as_ptr()))) }.map_err(|_| std::io::Error::last_os_error())?;
        Ok(Job(Owned(h)))
    }

    /// A new run's job: no other process in it yet, at most [`RUN_RAM_SHARE`] of the RAM, and a
    /// fault ends its program ([`LIMIT_DIE_ON_FAULT`]).
    fn new(exit: Option<&Path>) -> std::io::Result<Job> {
        let job = Job::named(exit)?;
        if job.active() > 0 {
            return Err(std::io::Error::other(format!("the job of {} already runs processes", exit.map_or(String::new(), |p| p.display().to_string()))));
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = LIMIT_DIE_ON_FAULT;
        let (ram, _) = memory();
        if ram > 0 {
            info.BasicLimitInformation.LimitFlags = LIMIT_DIE_ON_FAULT | LIMIT_JOB_MEMORY;
            info.JobMemoryLimit = (ram as f64 * RUN_RAM_SHARE) as usize;
        }
        unsafe { SetInformationJobObject(job.0 .0, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32) }.map_err(|_| std::io::Error::last_os_error())?;
        Ok(job)
    }

    fn assign(&self, pid: u32) -> std::io::Result<()> {
        let p = Owned(unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }.map_err(|_| std::io::Error::last_os_error())?);
        unsafe { AssignProcessToJobObject(self.0 .0, p.0) }.map_err(|_| std::io::Error::last_os_error())
    }

    /// Live processes in the job.
    pub fn active(&self) -> u32 {
        let mut a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        match unsafe { QueryInformationJobObject(Some(self.0 .0), JobObjectBasicAccountingInformation, &mut a as *mut _ as *mut _, std::mem::size_of_val(&a) as u32, None) } {
            Ok(()) => a.ActiveProcesses,
            Err(_) => 0,
        }
    }

    pub fn terminate(&self) {
        unsafe {
            let _ = TerminateJobObject(self.0 .0, 1);
        }
    }

    /// The processes in the job now (everything a run started: children join their parent's job).
    fn pids(&self) -> Vec<u32> {
        // the list: its two counts, then one id per word
        let word = std::mem::size_of::<usize>();
        let first = (std::mem::size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() - word) / word;
        let mut room = 64usize;
        loop {
            let mut buf = vec![0usize; first + room];
            let asked = unsafe { QueryInformationJobObject(Some(self.0 .0), JobObjectBasicProcessIdList, buf.as_mut_ptr() as *mut _, (buf.len() * word) as u32, None) };
            // SAFETY: the buffer starts with the list's header and is word-aligned like it
            let head = unsafe { &*(buf.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST) };
            let (assigned, listed) = (head.NumberOfAssignedProcesses as usize, head.NumberOfProcessIdsInList as usize);
            // more processes than room (ERROR_MORE_DATA): asked again with room for all
            if asked.is_err() && assigned > room {
                room = assigned + 16;
                continue;
            }
            return if asked.is_ok() { buf[first..first + listed.min(room)].iter().map(|&p| p as u32).collect() } else { Vec::new() };
        }
    }

    /// The CPU its processes used so far, ended ones too (100 ns units).
    fn cpu(&self) -> Option<u64> {
        let mut a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        unsafe { QueryInformationJobObject(Some(self.0 .0), JobObjectBasicAccountingInformation, &mut a as *mut _ as *mut _, std::mem::size_of_val(&a) as u32, None) }.ok()?;
        Some((a.TotalUserTime + a.TotalKernelTime).max(0) as u64)
    }
}

/// What keeps a run's processes together: its job (None: an in-hub run with no processes, or a
/// run taken back whose processes all ended).
#[derive(Default)]
pub struct Hold(Option<Job>);

impl Hold {
    pub fn none() -> Hold {
        Hold(None)
    }

    /// The job of a run an earlier hub started ([`job_name`] of its exit file): None when no
    /// process of it is left (the job went with the last one; the name opened a new, empty one).
    pub fn taken_back(exit: &Path) -> Hold {
        Hold(Job::named(Some(exit)).ok().filter(|j| j.active() > 0))
    }
}

/// A run's job's name: its exit file's, which the hub that takes the run back knows (one per
/// state dir, lease and run), in this session's namespace (the keeper restarts the hub there).
fn job_name(exit: &Path) -> Vec<u16> {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in exit.as_os_str().as_encoded_bytes() {
        hash = (hash ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("Local\\makepad-devhub-run-{hash:016x}").encode_utf16().chain(std::iter::once(0)).collect()
}

/// The argv a lease run executes: as given (priority is the creation class; no pinning).
pub fn lease_argv(argv: Vec<String>, _unit: Option<&str>, _nice: i32, _cpus: &str, _share: Option<crate::lease::Cost>, _ram_max_gb: Option<f64>) -> Vec<String> {
    argv
}

/// The handles this process got from its parent stay its own: none is inheritable from here on,
/// so a run gets only the handles it is given (its stdio, its job), never the hub's. The node's
/// start loop's log came in so (cmd's `>> node.log`, which shares no writing): a run that
/// outlived its hub kept it open, the loop could not open it again, and no hub started (the
/// Windows node down for an hour, 11-04). Called at the hub's start, before it starts anything;
/// what Rust opens is not inheritable (but for what `TcpStream::try_clone` returns:
/// `makepad_network::clone_tcp`), and its spawns pass their stdio as inheritable copies.
/// Windows has no call in the bindings that lists a process's handles: every handle value below
/// [`HANDLE_VALUES`] is asked (an inherited handle has its value in the parent, and the start
/// loop's processes hold a few hundred).
pub fn keep_handles_to_self() {
    // (what keeps the node's log start in the hub: before_main)
    let _ = std::hint::black_box(&WRAPPER);
    for value in (4..HANDLE_VALUES).step_by(4) {
        // (a value that is no handle of this process: ERROR_INVALID_HANDLE, nothing else)
        let _ = unsafe { SetHandleInformation_raw(HANDLE(value as *mut core::ffi::c_void), HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) };
    }
}

const HANDLE_VALUES: usize = 1 << 16;

/// Windows has no transient scope units to clear.
pub fn reset_failed_units(_pattern: &str) {}

/// The first argument of a run's wrapper: a program with this crate in it runs `argv` and records
/// its exit instead of starting ([`wrapper`]).
const WRAP: &str = "--makepad-devhub-run-wrapper";

/// `program` as the launch runs it: Windows' own search finds it (the unix hub searches its PATH).
pub fn hub_program(program: &str, _path: &std::ffi::OsStr) -> Result<std::path::PathBuf, String> {
    Ok(program.into())
}

/// Every run goes under a small wrapper, as on Unix: it writes the run's pid beside its usage
/// file (`<id>-<n>.usage.pid`, what [`crate::lease`] says it runs) and its exit code beside its
/// ledger (`<id>-<n>.exit`) as it ends, so a restarted hub reads the exit of a run that ended
/// without one. The wrapper is this hub's own image ([`run_image`]) before its `main`.
pub fn metered_argv(argv: Vec<String>, _unit: Option<&str>, usage: &Path) -> Vec<String> {
    // (what keeps the wrapper in every program this crate is in)
    let _ = std::hint::black_box(&WRAPPER);
    let mut full = vec![run_image().to_string_lossy().into_owned(), WRAP.into(), usage.to_string_lossy().into_owned()];
    full.extend(argv);
    full
}

/// What the wrappers run: this hub's image, under the name it has now. Its build moves the
/// running image aside ([`image_moved`]) and writes the new one in its place: a wrapper started
/// meanwhile runs the image this hub runs, never a half-written one.
pub fn run_image() -> PathBuf {
    IMAGE.lock().unwrap_or_else(|e| e.into_inner()).clone().or_else(|| std::env::current_exe().ok()).unwrap_or_default()
}

/// The running image's name after a move aside (None: back at its own name).
pub fn image_moved(to: Option<PathBuf>) {
    *IMAGE.lock().unwrap_or_else(|e| e.into_inner()) = to;
}

static IMAGE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Under `makepad run` this program's image is Relax, and nothing runs before its `main`: a spawn
/// of it is the program again, never a [`wrapper`] (it hung a CI leg on a pid file, 11-04). A hub
/// runs natively, so the tests of those roles run under `cargo test`, which `devhub test` uses here.
#[cfg(test)]
pub fn no_image_roles() -> bool {
    let jit = std::env::var_os("MAKEPAD_RUN_SELF_EXEC").is_some();
    if jit { eprintln!("not under the JIT: this tests the image's roles before main"); }
    jit
}

// the C runtime calls it before `main` in every program with this crate (tests too)
#[used]
#[link_section = ".CRT$XCU"]
static WRAPPER: extern "C" fn() = before_main;

/// This image's other two roles, by its first argument: a run's [`wrapper`], or the node's start
/// with its own log ([`log_start`]). Anything else: returns, and the program's own `main` runs.
extern "C" fn before_main() {
    let mut args = std::env::args_os().skip(1);
    match args.next() {
        Some(first) if first == WRAP => wrapper(args),
        Some(first) if first == LOG => log_start(args),
        _ => {}
    }
}

/// The first argument of the node's start (install-node.ps1's node-start.cmd): the hub writes
/// its own log ([`log_start`]).
const LOG: &str = "--makepad-devhub-log";

/// `<image> --makepad-devhub-log <file> <args..>`: runs this image with `<args..>`, its output
/// appended to `<file>`, and exits with its code. So the node's start loop redirects nothing: a
/// process the hub starts can never hold the loop's own redirect open (cmd's `>>` shares no
/// writing; a run that outlived its hub held it, and the loop started no hub, 11-04). The file is
/// opened shared (Rust's default): whoever else has it open never keeps a new hub from writing.
fn log_start(mut args: impl Iterator<Item = std::ffi::OsString>) -> ! {
    let Some(file) = args.next() else {
        eprintln!("devhub: {LOG} needs its file");
        std::process::exit(2);
    };
    let log = match std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        Ok(log) => log,
        Err(e) => {
            eprintln!("devhub: {}: {e}", file.to_string_lossy());
            std::process::exit(1);
        }
    };
    let started = std::env::current_exe().and_then(|exe| Command::new(exe).args(args).stdout(log.try_clone()?).stderr(log).status());
    std::process::exit(match started {
        Ok(status) => exit_code(status),
        Err(e) => {
            eprintln!("devhub: starting the hub: {e}");
            1
        }
    });
}

/// A run's wrapper ([`metered_argv`]): `<image> --makepad-devhub-run-wrapper <usage> <program>
/// <args..>` starts the program as the hub would have (found on the run's PATH first, no
/// console window, its stdio and directory), writes its pid, waits for it and writes its exit
/// code whole (aside, then renamed), then exits with that code.
fn wrapper(mut args: impl Iterator<Item = std::ffi::OsString>) -> ! {
    let (Some(usage), Some(program)) = (args.next(), args.next()) else {
        eprintln!("devhub: a run wrapper needs its usage file and a program");
        std::process::exit(2);
    };
    let usage = PathBuf::from(usage);
    let exit = usage.with_extension("exit");
    // its job, held open for the run by it and what it starts (a hub that started it has it open
    // now: this is that job)
    let _job = Job::inherited(&exit);
    let mut cmd = Command::new(&program);
    cmd.args(args);
    // (a PATH set for the child is searched first, as the hub's own start of it searched it)
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    windows::process::no_window(&mut cmd, 0);
    let code = match cmd.spawn() {
        Ok(mut child) => {
            let mut pid = usage.as_os_str().to_owned();
            pid.push(".pid");
            let _ = std::fs::write(PathBuf::from(pid), child.id().to_string());
            child.wait().map_or(-1, exit_code)
        }
        Err(e) => {
            eprintln!("devhub: {}: {e}", program.to_string_lossy());
            127
        }
    };
    let mut part = exit.as_os_str().to_owned();
    part.push(".wrapped");
    let part = PathBuf::from(part);
    if std::fs::write(&part, code.to_string()).and_then(|_| std::fs::rename(&part, &exit)).is_err() {
        let _ = std::fs::remove_file(&part);
    }
    std::process::exit(code);
}

pub fn scope_usage(_pid: u32, _start: u64, _unit: Option<&str>, _usage: &Path) -> (Option<u64>, Option<f64>) { (None, None) }

/// A run's processes are its job's: no group to measure them by.
pub const MEASURES_GROUPS: bool = false;

pub fn groups_rss(_groups: &[(u32, u64)]) -> Option<u64> { None }

pub fn groups_held(_groups: &[(u32, u64)]) -> Option<u64> { None }

pub fn scope_now(_pid: u32, _start: u64, _unit: Option<&str>, _usage: &Path) -> Option<u64> { None }

/// What a run has done so far ([`super::Activity`]): its job's CPU (its I/O is not counted).
/// None: a run taken back from the ledger has no job to ask.
pub fn activity(_pid: u32, _start: u64, _unit: Option<&str>, _usage: &Path, hold: &Hold) -> Option<super::Activity> {
    Some(super::Activity { cpu_us: hold.0.as_ref()?.cpu()? / 10, io_bytes: 0 })
}

pub fn wait_usage(child: &mut Child) -> std::io::Result<(ExitStatus, Option<f64>, Option<u64>)> { child.wait().map(|s| (s, None, None)) }

/// Resumes the (one) thread of a process created suspended.
fn resume(pid: u32) -> std::io::Result<()> {
    let snap = Owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(|_| std::io::Error::last_os_error())?);
    let mut t: THREADENTRY32 = unsafe { std::mem::zeroed() };
    t.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut ok = unsafe { Thread32First(snap.0, &mut t) }.is_ok();
    let mut resumed = false;
    while ok {
        if t.th32OwnerProcessID == pid {
            if let Ok(h) = unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, t.th32ThreadID) } {
                let h = Owned(h);
                resumed |= unsafe { ResumeThread(h.0) } != u32::MAX;
            }
        }
        ok = unsafe { Thread32Next(snap.0, &mut t) }.is_ok();
    }
    if resumed { Ok(()) } else { Err(std::io::Error::other(format!("pid {pid}: no thread to resume"))) }
}

/// Starts a lease run in a job of its own (see the module doc), named for its exit file `exit` so
/// a restarted hub takes it back (anonymous without one: a build of the hub's own).
pub fn lease_spawn(cmd: &mut Command, exit: Option<&Path>) -> std::io::Result<(Child, Hold)> {
    let job = Job::new(exit)?;
    windows::process::no_window(cmd, CREATE_SUSPENDED.0 | BELOW_NORMAL_PRIORITY_CLASS.0 | CREATE_NEW_PROCESS_GROUP.0);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let job = job.assign(pid).map(|_| job).and_then(|j| resume(pid).map(|_| j));
    match job {
        Ok(j) => Ok((child, Hold(Some(j)))),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(e)
        }
    }
}

pub fn exit_code(s: ExitStatus) -> i32 {
    s.code().unwrap_or(-1)
}

/// The creation time (µs since the epoch); None when the process is gone.
pub fn proc_start(pid: u32) -> Option<u64> {
    makepad_remote::group::start_time(pid)
}

/// The recorded process still is the one we started (not gone, not a reused pid).
pub fn same(pid: u32, start: u64) -> bool {
    proc_start(pid) == Some(start)
}

/// Whether `pid` is one of the run's processes: its leader or a member of its job (a JIT under
/// cargo or a shell, which a stalled lease's capture asks for; with the leader alone, a Windows
/// stall captured nothing, 11-04). A kill of one of them is [`signal_one`].
pub fn in_group(pid: u32, leader: u32, hold: &Hold) -> bool {
    pid == leader || job_pids(hold).contains(&pid)
}

/// The processes in the run's job (none for a run with no job left).
pub fn group_pids(_pid: u32, _start: u64, hold: &Hold) -> Vec<u32> {
    job_pids(hold)
}

/// (No thread view here: the processes alone.)
pub fn processes_view(pids: &[u32]) -> String {
    pids.iter().map(|p| format!("pid {p}\n")).collect()
}

pub fn job_pids(hold: &Hold) -> Vec<u32> {
    hold.0.as_ref().map(Job::pids).unwrap_or_default()
}

/// Whether the run still has a live process: its job's count, else (no job left) the recorded
/// process.
pub fn group_alive(pid: u32, start: u64, hold: &Hold) -> bool {
    match &hold.0 {
        Some(j) => j.active() > 0,
        None => same(pid, start),
    }
}

/// Ends the run: its job, else the recorded process and its descendants.
pub fn signal_group(pid: u32, _hard: bool, hold: &Hold) {
    match &hold.0 {
        Some(j) => j.terminate(),
        None => {
            makepad_remote::group::signal(pid, true);
        }
    }
}

/// No TERM for a process tree here: a lease's end asks the apps in the run's job over their
/// control endpoints instead (lease/exec.rs `ask_exit`).
pub fn term_group(_pid: u32, _hold: &Hold) -> bool {
    false
}

/// `program` at BELOW_NORMAL priority (background builds), without a console window.
pub fn background(program: &std::ffi::OsStr) -> Command {
    let mut cmd = Command::new(program);
    windows::process::no_window(&mut cmd, BELOW_NORMAL_PRIORITY_CLASS.0);
    cmd
}

pub fn pid_alive(pid: u32) -> bool {
    proc_start(pid).is_some()
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\"))
}

pub fn login() -> Option<String> {
    std::env::var("USERNAME").ok()
}

/// (bytes, mtime in s) of one directory entry (the file length: NTFS allocation is not asked).
pub fn on_disk(m: &std::fs::Metadata) -> (u64, u64) {
    let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    (if m.is_file() { m.len() } else { 0 }, mtime)
}

/// Background work keeps the normal thread priority (the disk walk is I/O bound).
pub fn nice_this_thread() {}

/// Windows has no nice level that a child inherits.
pub fn own_nice() -> i32 {
    0
}

// ---- the machine's numbers

fn ft(t: &FILETIME) -> u64 {
    ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
}

/// (total, available) physical memory in bytes.
pub fn memory() -> (u64, u64) {
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    match unsafe { GlobalMemoryStatusEx(&mut m) } {
        Ok(()) => (m.ullTotalPhys, m.ullAvailPhys),
        Err(_) => (0, 0),
    }
}

/// (busy, total) CPU time of all cores in 100 ns (as `/proc/stat`'s pair: compare two samples).
pub fn cpu_times() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    // kernel time includes the idle time
    let total = ft(&kernel) + ft(&user);
    Some((total.saturating_sub(ft(&idle)), total))
}

pub fn uptime_s() -> u64 {
    unsafe { GetTickCount64() / 1000 }
}

/// One process's (user + kernel CPU ns, working set bytes).
pub fn proc_numbers(pid: u32) -> Option<(u64, u64)> {
    let h = Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) }.ok()?;
    let mut m: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let rss = if unsafe { K32GetProcessMemoryInfo(h.0, &mut m, m.cb) }.0 != 0 { m.WorkingSetSize as u64 } else { 0 };
    Some(((ft(&k) + ft(&u)) * 100, rss))
}

/// Every process: (pid, exe name, threads).
pub fn processes() -> Vec<(u32, String, u32)> {
    let mut out = Vec::new();
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return out };
    let snap = Owned(snap);
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snap.0, &mut e) }.is_ok();
    while ok {
        let n = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
        out.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..n]), e.cntThreads));
        ok = unsafe { Process32NextW(snap.0, &mut e) }.is_ok();
    }
    out
}

/// The program's file name (`makepad.exe`).
pub fn exe_name(pid: u32) -> Option<String> {
    let h = Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_FORMAT(0), windows::core::PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
}

/// (free to this user, total) bytes of the volume holding `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let w: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(Some(PCWSTR(w.as_ptr())), Some(&mut free), Some(&mut total), None) }.ok()?;
    Some((free, total))
}

/// The CPU's brand string (cpuid 0x80000002..4).
pub fn cpu_name() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        if __cpuid(0x8000_0000).eax < 0x8000_0004 {
            return String::new();
        }
        let mut b = Vec::with_capacity(48);
        for leaf in 0x8000_0002u32..=0x8000_0004 {
            let r = __cpuid(leaf);
            for x in [r.eax, r.ebx, r.ecx, r.edx] {
                b.extend_from_slice(&x.to_le_bytes());
            }
        }
        String::from_utf8_lossy(&b).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        String::new()
    }
}

/// The machine's battery: (percent, charging); None without one.
pub fn battery() -> Option<(u32, bool)> {
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    // 128: no system battery, 255: unknown
    if s.BatteryFlag & 128 != 0 || s.BatteryFlag == 255 || s.BatteryLifePercent > 100 {
        return None;
    }
    Some((s.BatteryLifePercent as u32, s.ACLineStatus == 1))
}

/// No descriptor limit to raise here.
pub fn raise_fd_limit() -> u64 {
    0
}

/// The processes that left their run's group: none (see the top of this file).
pub fn env_values(_prefix: &str) -> Vec<(u32, String)> {
    Vec::new()
}

pub fn with_env(_marker: &str) -> Vec<u32> {
    Vec::new()
}

/// Ends one process (there is no TERM here), not its job.
pub fn signal_one(pid: u32, _hard: bool) {
    let _ = makepad_remote::procs::kill_pid(pid);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Foundation::SetHandleInformation;
    use std::os::windows::io::AsRawHandle;

    /// The node start loop's `>> node.log` as the hub got it (inheritable, shared for reading
    /// only) reaches no run once the hub keeps its handles to itself, so a run that outlives the
    /// hub never keeps the loop from opening its log again. Before, the run held it and the log
    /// could not be opened for writing while it lived (11-04).
    #[test]
    fn a_run_gets_none_of_the_hubs_handles() {
        let dir = std::env::temp_dir().join(format!("devhub-handles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // a run started while the hub holds the loop's log; then the hub ends: can the log be
        // opened for writing again? (A log of its own each time: what the other tests start
        // meanwhile inherits it too, as long as it is inheritable here.)
        let reopens = |keep_to_self: bool| {
            let log = dir.join(format!("node-{keep_to_self}.log"));
            // FILE_SHARE_READ: cmd's redirect shares no writing
            let held = std::fs::OpenOptions::new().create(true).append(true).share_mode(1).open(&log).unwrap();
            assert!(unsafe { SetHandleInformation(HANDLE(held.as_raw_handle()), HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }.is_ok());
            if keep_to_self {
                keep_handles_to_self();
            }
            let mut run = Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
            drop(held);
            let again = std::fs::OpenOptions::new().append(true).open(&log).map(|_| ()).map_err(|e| e.raw_os_error());
            let _ = run.kill();
            let _ = run.wait();
            again
        };
        // ERROR_SHARING_VIOLATION while the run lives
        assert_eq!(reopens(false), Err(Some(32)), "a run holds what it inherits");
        assert_eq!(reopens(true), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A run's program that faults ends at once, with its fault's code. It used to wait in the
    /// system's error report until its job timed out (11-05). This test's program is the one that
    /// faults: the system's own report is what is tested, so it has to really fault.
    #[test]
    fn a_run_that_faults_ends_at_once() {
        const FAULT: &str = "DEVHUB_TEST_FAULT";
        if std::env::var_os(FAULT).is_some() {
            unsafe { (8usize as *mut u8).write_volatile(1) };
        }
        if no_image_roles() {
            return;
        }
        let mut run = Command::new(std::env::current_exe().unwrap());
        run.arg("a_run_that_faults_ends_at_once").env(FAULT, "1").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let (mut child, hold) = lease_spawn(&mut run, None).unwrap();
        let t0 = std::time::Instant::now();
        let code = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status.code();
            }
            if t0.elapsed() > std::time::Duration::from_secs(60) {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        if let Hold(Some(job)) = &hold {
            job.terminate();
        }
        let _ = child.wait();
        // STATUS_ACCESS_VIOLATION
        assert_eq!(code, Some(0xC000_0005u32 as i32), "after {:?}", t0.elapsed());
    }

    /// The node's start (`--makepad-devhub-log <file> <args..>`): this image runs with `<args..>`,
    /// its output appended to the file, and the start exits with its code. Here the image is this
    /// test program, listing its tests.
    #[test]
    fn the_hub_writes_its_own_log() {
        if super::no_image_roles() { return; }
        let dir = std::env::temp_dir().join(format!("devhub-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("node.log");
        std::fs::write(&log, "earlier\n").unwrap();
        let status = Command::new(std::env::current_exe().unwrap()).arg(LOG).arg(&log).arg("--list").stdout(std::process::Stdio::null()).status().unwrap();
        assert!(status.success(), "{status:?}");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with("earlier\n") && text.contains("the_hub_writes_its_own_log: test"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
