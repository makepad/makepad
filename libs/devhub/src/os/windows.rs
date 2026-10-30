//! Windows: a lease run is a Job Object (KILL_ON_JOB_CLOSE): the child is created suspended,
//! put in its job, then resumed, so every process it starts is in the job from its first
//! instruction; end = TerminateJobObject (there is no TERM to send a process tree). The run is
//! BELOW_NORMAL priority (its children inherit the class), without a console window. If the hub
//! dies the job handle closes and the run ends with it.
//!
//! Also the machine's numbers (memory, CPU times, processes, disk, CPU name, battery) through the
//! generated bindings (makepad-windows-sys).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next, PROCESSENTRY32W, TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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

/// A run's Job Object.
pub struct Job(Owned);

impl Job {
    fn new() -> std::io::Result<Job> {
        let h = unsafe { CreateJobObjectW(None, None) }.map_err(|_| std::io::Error::last_os_error())?;
        let job = Job(Owned(h));
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
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
}

/// What keeps a run's processes together: its job (None for a run taken back from the ledger:
/// its job closed with the hub that made it, which ended it).
#[derive(Default)]
pub struct Hold(Option<Job>);

impl Hold {
    pub fn none() -> Hold {
        Hold(None)
    }
}

/// The argv a lease run executes: as given (priority is the creation class; no pinning).
pub fn lease_argv(argv: Vec<String>, _unit: Option<&str>, _cpus: &str) -> Vec<String> {
    argv
}

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

/// Starts a lease run in a job of its own (see the module doc).
pub fn lease_spawn(cmd: &mut Command) -> std::io::Result<(Child, Hold)> {
    windows::process::no_window(cmd, CREATE_SUSPENDED.0 | BELOW_NORMAL_PRIORITY_CLASS.0 | CREATE_NEW_PROCESS_GROUP.0);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let job = Job::new().and_then(|j| j.assign(pid).map(|_| j)).and_then(|j| resume(pid).map(|_| j));
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

/// Whether the run still has a live process: its job's count, else (taken back from the
/// ledger) the recorded process or a descendant of it.
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

/// `program` at BELOW_NORMAL priority (background builds), without a console window.
pub fn niced(program: &std::ffi::OsStr) -> Command {
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
