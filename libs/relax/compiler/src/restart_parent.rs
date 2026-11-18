//! A run that can restart as a new process (a live `makepad run`, a live test harness) is, on Windows
//! and macOS, the child of a thin parent: the first `makepad` process, which starts the run, waits for
//! it and passes on how it ended. To restart, the run writes the next run's command to a file and
//! ends; the parent starts that run. Windows has no exec. On macOS an exec in place keeps a pid whose
//! window-server session the system has marked "pending exit", for good: a menu-bar app's status
//! item came back blank after a restart into a rebuilt runner (11-17). The old process is gone before
//! the new one opens the disk cache, the waiters see one process, and the run's pid changes with each
//! restart. The parent's pid is the one that stays, and the one whoever started the run holds: the
//! run's discovery file names it, so that pid finds the run
//! (`makepad_remote::discovery::App::answers_to`).
//!
//! On Unix the file is the request (an exit code has 8 bits): the parent removes it before it starts
//! a run and restarts only when it is there after the run ended. It then execs the next run's program
//! itself (exec-through) and so becomes the next run's parent on the next run's binary: its pid stays,
//! it never held a window, and the file's writer and reader are always one binary. A signal sent to
//! the parent alone is passed on to the run once; a parent that is killed ends its run through a pipe
//! only the parent holds open (the lifeline). Linux keeps its exec in place ([`replace`]): its window
//! manager's connection crosses the exec (hal_run.rs prepare_exec), which a new process could not
//! inherit. The parent's code builds on Linux too, where its tests run it.
#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
use std::ffi::{OsStr, OsString};

/// The exit code with which a run asks its parent to start the next one.
#[cfg(windows)]
const RESTART_EXIT: u32 = 0x4D50_5253;

/// The environment variable naming the file the next run's command goes to: set by the parent for
/// the run it starts, which takes it out so the programs it starts get parents of their own. No
/// child takes it, or [`PARENT_ENV`] or the lifeline's, from a run's environment or build
/// environment (relax-base progenv.rs `RESTART`): under the JIT those reach a run's children.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
const FILE_ENV: &str = "RELAX_RESTART_FILE";

/// The parent's pid, set and taken out with [`FILE_ENV`].
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
const PARENT_ENV: &str = "RELAX_RESTART_PARENT";

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
static FILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

static PARENT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// The pid of the parent this run stands behind; None for a process without one.
pub(crate) fn parent_pid() -> Option<u32> {
    PARENT.get().copied()
}

/// Whether the command line `args` (with the program's name first) is a run that can restart: the
/// same runs as `driver_options.live` in run.rs, as far as a command line tells.
#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
fn can_restart(args: &[Vec<u8>]) -> bool {
    if args.get(1).map(Vec::as_slice) != Some(b"run".as_slice()) {
        return false;
    }
    let has = |names: &[&[u8]]| args[2..].iter().take_while(|a| a.as_slice() != b"--").any(|a| names.contains(&a.as_slice()));
    has(&[b"--live"]) || !has(&[b"--check", b"--harness", b"--plain-test", b"--no-live", b"--silent", b"--src"]) && std::env::var_os("RELAX_SCRIPT_DEPTH").is_none()
}

/// The next run as it is written to the file: what an exec would keep and change, the process's
/// working directory and whole environment included (the parent has neither as the run left them).
#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
#[derive(Debug, PartialEq)]
struct Next {
    program: OsString,
    args: Vec<OsString>,
    dir: OsString,
    envs: Vec<(OsString, OsString)>,
}

#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
fn put(out: &mut Vec<u8>, word: &OsStr) {
    let bytes = word.as_encoded_bytes();
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
impl Next {
    /// The first run: this process's binary with the loader's command line `args` (its name
    /// first), working directory and environment.
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn first(args: &[Vec<u8>]) -> std::io::Result<Next> {
        let exe = std::env::current_exe()?;
        // SAFETY: args_bytes gives the encoded bytes of OsStrings
        let words = args[1..].iter().map(|a| unsafe { OsStr::from_encoded_bytes_unchecked(a) }.to_os_string()).collect();
        let dir = std::env::current_dir().map(std::path::PathBuf::into_os_string).unwrap_or_default();
        Ok(Next { program: exe.into_os_string(), args: words, dir, envs: std::env::vars_os().collect() })
    }

    /// This process's directory and environment with `cmd`'s changes (its folder, when it names one).
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn of(cmd: &std::process::Command) -> std::io::Result<Next> {
        let same = |a: &OsStr, b: &OsStr| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b };
        let mut envs: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        for (k, v) in cmd.get_envs() {
            envs.retain(|(e, _)| !same(e, k));
            if let Some(v) = v {
                envs.push((k.to_os_string(), v.to_os_string()));
            }
        }
        Ok(Next {
            program: cmd.get_program().to_os_string(),
            args: cmd.get_args().map(OsStr::to_os_string).collect(),
            dir: match cmd.get_current_dir() { Some(dir) => dir.as_os_str().to_os_string(), None => std::env::current_dir()?.into_os_string() },
            envs,
        })
    }

    /// The command that starts this run (`cmd` as its program, arguments, folder and environment).
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.program);
        cmd.args(&self.args).env_clear().envs(self.envs.iter().map(|(k, v)| (k, v)));
        if !self.dir.is_empty() {
            cmd.current_dir(&self.dir);
        }
        cmd
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put(&mut out, &self.program);
        out.extend_from_slice(&(self.args.len() as u64).to_le_bytes());
        for a in &self.args {
            put(&mut out, a);
        }
        put(&mut out, &self.dir);
        out.extend_from_slice(&(self.envs.len() as u64).to_le_bytes());
        for (k, v) in &self.envs {
            put(&mut out, k);
            put(&mut out, v);
        }
        out
    }

    /// None: not what `encode` writes. The words come back as the run's own encoding wrote them:
    /// the file is this binary's, from the run this process started.
    fn decode(data: &[u8]) -> Option<Next> {
        struct Reader<'a>(&'a [u8]);
        impl<'a> Reader<'a> {
            fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
                if self.0.len() < n {
                    return None;
                }
                let (head, tail) = self.0.split_at(n);
                self.0 = tail;
                Some(head)
            }
            fn len(&mut self) -> Option<usize> {
                Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?) as usize)
            }
            fn word(&mut self) -> Option<OsString> {
                let n = self.len()?;
                // SAFETY: as_encoded_bytes wrote them (put), in this same binary
                Some(unsafe { OsStr::from_encoded_bytes_unchecked(self.bytes(n)?) }.to_os_string())
            }
        }
        let mut r = Reader(data);
        let program = r.word()?;
        let n = r.len()?;
        let args = (0..n).map(|_| r.word()).collect::<Option<Vec<_>>>()?;
        let dir = r.word()?;
        let n = r.len()?;
        let envs = (0..n).map(|_| Some((r.word()?, r.word()?))).collect::<Option<Vec<_>>>()?;
        r.0.is_empty().then_some(Next { program, args, dir, envs })
    }
}

/// This process ends and `cmd` runs as the next run: through this run's parent where it has one
/// (Windows, macOS: [`request`]), else by an exec in place (Linux, whose window manager's connection
/// crosses the exec and whose runs get no parent; a macOS run that cannot restart, which has none).
/// The caller has ended the run as at its exit (neither an exec nor the parent's request runs the
/// exit handlers). Returns only on failure, with why: this run goes on.
pub(crate) fn replace(cmd: &mut std::process::Command) -> String {
    #[cfg(windows)]
    return request(cmd);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if FILE.get().is_some() {
        return request(cmd);
    }
    #[cfg(unix)]
    return std::os::unix::process::CommandExt::exec(cmd).to_string();
    #[cfg(not(any(unix, windows)))]
    {
        let _ = cmd;
        "this platform cannot start a process in this one's place".to_string()
    }
}

/// A run asks for the next one: `cmd` is written for the parent and this process ends. Returns only
/// on failure, with why: this run goes on.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub(crate) fn request(cmd: &std::process::Command) -> String {
    let Some(file) = FILE.get() else {
        return "this run has no parent process to start it again".to_string();
    };
    let next = match Next::of(cmd) {
        Ok(n) => n,
        Err(e) => return format!("this run's directory is unknown: {e}"),
    };
    #[cfg(windows)]
    {
        if let Err(e) = std::fs::write(file, next.encode()) {
            return format!("cannot write {}: {e}", file.display());
        }
        // the next run's window may take the focus this one has
        const ASFW_ANY: u32 = u32::MAX;
        let _ = unsafe { makepad_windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(ASFW_ANY) };
        // an exec does not run the program's exit handlers either; the process's own exit work does run
        // (the discovery file of this pid goes: the hub reads one left behind as a crash)
        makepad_core_util::process_end::end_without_c_handlers(RESTART_EXIT as i32)
    }
    #[cfg(unix)]
    {
        // the file is the request: made new (the parent removed it before it started this run), 0600,
        // in this user's own folder (it is a command line that runs)
        if let Err(e) = crate::reload::write_new_private(file, &next.encode()) {
            return format!("cannot write {}: {e}", file.display());
        }
        makepad_core_util::process_end::end_without_c_handlers(0)
    }
}

/// On entry of the loader with the resolved command line `args` (Windows and macOS): the run its
/// parent started takes the parent's file and goes on; a run that can restart becomes that parent
/// and never returns; anything else goes on as a plain process.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub fn enter(args: &[Vec<u8>]) {
    if started_by_parent() {
        return;
    }
    if can_restart(args) {
        #[cfg(windows)]
        parent(args);
        #[cfg(unix)]
        {
            let why = match Next::first(args) {
                Ok(first) => unix::parent(first),
                Err(e) => format!("the {tool} binary's path is unknown: {e}", tool = crate::tool_name()),
            };
            eprintln!("relax: {why}; this run goes on without a parent to start it again");
        }
    }
}

/// The run a parent started: it keeps the parent's file and pid, out of the environment its
/// programs get. False: no parent started this process.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn started_by_parent() -> bool {
    let Some(file) = std::env::var_os(FILE_ENV) else {
        // a loader run as the program of another (`cfg(relax)`: relax under relax): the runtime's
        // own runner took the variables out of the environment before this code ran (macOS,
        // Windows: `enter`), and its run is this program. Without them here this run took itself
        // for a first process and became a parent, and so did its run, without end.
        #[cfg(relax)]
        if let Some((file, pid)) = host_took() {
            let _ = FILE.set(file);
            if pid != 0 {
                let _ = PARENT.set(pid);
            }
            return true;
        }
        return false;
    };
    std::env::remove_var(FILE_ENV);
    let _ = FILE.set(std::path::PathBuf::from(file));
    if let Some(pid) = std::env::var_os(PARENT_ENV).and_then(|p| p.to_str()?.parse().ok()) {
        let _ = PARENT.set(pid);
    }
    std::env::remove_var(PARENT_ENV);
    #[cfg(unix)]
    unix::hold_lifeline();
    true
}

/// The parent's file and pid that the runtime running this code took (`started_by_parent` there):
/// none when no parent started this process.
#[cfg(all(relax, any(windows, target_os = "macos", target_os = "linux")))]
fn host_took() -> Option<(std::path::PathBuf, u32)> {
    extern "C" {
        fn __relax_host_restart_parent(buf: *mut u8, cap: usize, pid: *mut u32) -> usize;
    }
    let mut buf = vec![0u8; 1024];
    let mut pid = 0u32;
    let mut n = unsafe { __relax_host_restart_parent(buf.as_mut_ptr(), buf.len(), &mut pid) };
    if n > buf.len() {
        buf.resize(n, 0);
        n = unsafe { __relax_host_restart_parent(buf.as_mut_ptr(), buf.len(), &mut pid) };
    }
    if n == 0 || n > buf.len() {
        return None;
    }
    buf.truncate(n);
    // (the bytes as_encoded_bytes gave in this process: the same platform's encoding)
    Some((std::path::PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(buf) }), pid))
}

/// `__relax_host_restart_parent` (jit.rs): the parent's file and pid this process took, for a
/// loader that runs as this one's program (its own `started_by_parent`). The file's path goes to
/// `buf` when it fits in `cap` bytes, the pid to `pid`; returns the path's length (more than `cap`:
/// ask again with that much room), 0 when no parent started this process.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub(crate) extern "C" fn relax_host_restart_parent(buf: *mut u8, cap: usize, pid: *mut u32) -> usize {
    let Some(file) = FILE.get() else { return 0 };
    let bytes = file.as_os_str().as_encoded_bytes();
    if bytes.len() <= cap {
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
            *pid = PARENT.get().copied().unwrap_or(0);
        }
    }
    bytes.len()
}

#[cfg(windows)]
fn parent(args: &[Vec<u8>]) -> ! {
    use std::os::windows::io::AsRawHandle;
    /// A console's Ctrl+C reaches the run too, which decides what it means; this process waits for it.
    extern "system" fn ignore_console_event(_: u32) -> i32 {
        1
    }
    unsafe { relax_session::win32::SetConsoleCtrlHandler(Some(ignore_console_event), 1) };
    let mut next = match Next::first(args) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("relax: the {tool} binary's path is unknown: {e}", tool = crate::tool_name());
            std::process::exit(1);
        }
    };
    let job = kill_on_close_job();
    let file = std::env::temp_dir().join(format!("relax-restart-{}", std::process::id()));
    loop {
        let mut cmd = next.command();
        cmd.env(FILE_ENV, &file).env(PARENT_ENV, std::process::id().to_string());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => next_failed(&next, &format!("cannot start the next run: {e}")),
        };
        if let Some(job) = &job {
            let handle = makepad_windows_sys::Win32::Foundation::HANDLE(child.as_raw_handle() as _);
            if unsafe { makepad_windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(*job, handle) }.is_err() {
                eprintln!("relax: this process's end will not end the run: it could not join the job");
            }
        }
        let code = match child.wait() {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => next_failed(&next, &format!("cannot wait for the run: {e}")),
        };
        if code as u32 != RESTART_EXIT {
            let _ = std::fs::remove_file(&file);
            std::process::exit(code);
        }
        let read = std::fs::read(&file).map_err(|e| e.to_string()).and_then(|d| Next::decode(&d).ok_or("not a run's command".to_string()));
        let _ = std::fs::remove_file(&file);
        next = match read {
            Ok(n) => n,
            Err(e) => next_failed(&next, &format!("cannot read the next run's command in {}: {e}", file.display())),
        };
    }
}

/// The job this process's end ends the run with (a run outliving a parent that was killed would
/// hold its window and its files). What the run starts stays out of it, as without a parent: those
/// programs' ends are not this process's. None: it could not be made; the run goes on without it.
#[cfg(windows)]
fn kill_on_close_job() -> Option<makepad_windows_sys::Win32::Foundation::HANDLE> {
    use makepad_windows_sys::Win32::System::JobObjects::*;
    // winnt.h (the generated bindings do not carry it)
    const SILENT_BREAKAWAY_OK: u32 = 0x0000_1000;
    // SAFETY: the generated metadata supplies the Win32 structures and ABI
    let job = unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.0 | SILENT_BREAKAWAY_OK);
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const std::ffi::c_void, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32).ok()?;
        job
    };
    Some(job)
}

/// The run that asked for a restart has ended and the next did not start: only this process can say.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn next_failed(next: &Next, why: &str) -> ! {
    record_failure(next, why);
    std::process::exit(1)
}

/// The next run `next` did not start, for `why`: said, and the recovery its file describes closed.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn record_failure(next: &Next, why: &str) {
    eprintln!("relax live: restart failed: {why}");
    if let Some((_, path)) = next.envs.iter().find(|(k, _)| k == "RELAX_JIT_RECOVERY") {
        crate::hot_failure::next_run_failed(std::path::Path::new(path), why);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod unix {
    use super::*;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::sync::atomic::{AtomicI32, Ordering::SeqCst};

    /// The run's end of the lifeline pipe (its descriptor), set by the parent and taken out with
    /// [`FILE_ENV`].
    const LIFELINE_ENV: &str = "RELAX_RESTART_LIFELINE";

    const SIGHUP: i32 = 1;
    const SIGINT: i32 = 2;
    const SIGQUIT: i32 = 3;
    const SIGKILL: i32 = 9;
    const SIGTERM: i32 = 15;
    /// The signals the parent passes on to its run.
    const PASSED_ON: [i32; 4] = [SIGHUP, SIGINT, SIGQUIT, SIGTERM];
    const F_SETFD: i32 = 2;
    const FD_CLOEXEC: i32 = 1;
    const EINTR: i32 = 4;

    #[cfg(target_os = "macos")]
    mod abi {
        pub const SA_SIGINFO: i32 = 0x40;
        pub const SA_RESTART: i32 = 0x2;
        pub const SIG_BLOCK: i32 = 1;
        pub const SIG_UNBLOCK: i32 = 2;
        /// siginfo_t: si_signo, si_errno, si_code, si_pid
        pub const SI_PID: usize = 12;
        /// Darwin's struct sigaction (the user-space one)
        #[repr(C)]
        pub struct Act {
            pub handler: usize,
            pub mask: u32,
            pub flags: i32,
        }
        pub fn act(handler: usize, flags: i32) -> Act {
            Act { handler, mask: 0, flags }
        }
    }

    #[cfg(target_os = "linux")]
    mod abi {
        pub const SA_SIGINFO: i32 = 4;
        pub const SA_RESTART: i32 = 0x1000_0000;
        pub const SIG_BLOCK: i32 = 0;
        pub const SIG_UNBLOCK: i32 = 1;
        /// siginfo_t: si_signo, si_errno, si_code, the union at 8-byte alignment, si_pid first in it
        pub const SI_PID: usize = 16;
        /// glibc's struct sigaction
        #[repr(C)]
        pub struct Act {
            pub handler: usize,
            pub mask: [u64; 16],
            pub flags: i32,
            pub pad: i32,
            pub restorer: usize,
        }
        pub fn act(handler: usize, flags: i32) -> Act {
            Act { handler, mask: [0; 16], flags, pad: 0, restorer: 0 }
        }
    }

    extern "C" {
        fn pipe(fds: *mut i32) -> i32;
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn close(fd: i32) -> i32;
        fn read(fd: i32, buf: *mut core::ffi::c_void, n: usize) -> isize;
        fn kill(pid: i32, sig: i32) -> i32;
        fn getpid() -> i32;
        fn raise(sig: i32) -> i32;
        // (declared as the crate's other users declare them: the layouts are abi::Act's, the mask's
        // first word is the set)
        fn sigaction(sig: i32, act: *const crate::os::fault::SigAction, old: *mut crate::os::fault::SigAction) -> i32;
        fn pthread_sigmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32;
    }

    const _: () = assert!(std::mem::size_of::<abi::Act>() == std::mem::size_of::<crate::os::fault::SigAction>());

    /// The run's pid while it runs (0: none).
    static CHILD: AtomicI32 = AtomicI32::new(0);
    /// The first signal the parent passed on (0: none): no run starts after it.
    static STOP: AtomicI32 = AtomicI32::new(0);

    /// A signal sent to the parent: passed on to its run. A terminal's Ctrl+C and Ctrl+\ reach the
    /// run themselves (the process group's every member), so those are passed on only when a
    /// process sent them.
    extern "C" fn pass_on(sig: i32, info: *const u8, _: *mut u8) {
        // SAFETY: the kernel's siginfo_t for this signal
        if (sig == SIGINT || sig == SIGQUIT) && (info.is_null() || unsafe { *(info.add(abi::SI_PID) as *const i32) } == 0) {
            return;
        }
        let _ = STOP.compare_exchange(0, sig, SeqCst, SeqCst);
        let child = CHILD.load(SeqCst);
        if child > 0 {
            unsafe { kill(child, sig) };
        }
    }

    fn set_action(sig: i32, act: &abi::Act) {
        unsafe { sigaction(sig, act as *const abi::Act as *const crate::os::fault::SigAction, std::ptr::null_mut()) };
    }

    /// Blocks (or lets through) the signals the parent passes on, for this thread; an exec keeps
    /// them blocked and pending, so one that comes between a run's end and the next run's start
    /// reaches the next image's handler.
    fn mask(how: i32) {
        let mut set = [0u64; 16];
        for sig in PASSED_ON {
            set[0] |= 1 << (sig - 1);
        }
        unsafe { pthread_sigmask(how, &set, std::ptr::null_mut()) };
    }

    /// Ends this process as `sig` would with its default action (the launcher's wait status says
    /// it), or with 128 + `sig` for one whose default writes a crash report: the run's is the one.
    fn end_by(sig: i32) -> ! {
        if [SIGTERM, SIGINT, SIGHUP, SIGKILL, SIGQUIT].contains(&sig) {
            set_action(sig, &abi::act(0, 0));
            mask(abi::SIG_UNBLOCK);
            unsafe { raise(sig) };
        }
        std::process::exit(128 + sig)
    }

    /// The run (`pid`) ended without asking for another: this process ends as it did. A signal
    /// that did not come through this process (a kill, a crash, the system's at memory pressure)
    /// is said first: the launcher sees only this process's end, as if it had been sent to it.
    /// (A terminal's Ctrl+C, Ctrl+\ and hang-up reach the whole process group: not said.)
    fn finish(status: std::process::ExitStatus, pid: u32) -> ! {
        match (status.code(), status.signal()) {
            (Some(code), _) => std::process::exit(code),
            (None, Some(sig)) => {
                if STOP.load(SeqCst) != sig && ![SIGINT, SIGQUIT, SIGHUP].contains(&sig) {
                    let name = match sig {
                        SIGKILL => " (SIGKILL)",
                        SIGTERM => " (SIGTERM)",
                        6 => " (SIGABRT)",
                        11 => " (SIGSEGV)",
                        _ => "",
                    };
                    eprintln!("relax: the run (pid {pid}) was ended by signal {sig}{name}, which did not come through its parent: the parent (pid {}) ends the same way", std::process::id());
                }
                end_by(sig)
            }
            (None, None) => std::process::exit(1),
        }
    }

    /// Starts `next` with the parent's file, pid and lifeline, and waits for it: how it ended, and
    /// its pid.
    fn run(next: &Next, file: &std::path::Path) -> Result<(std::process::ExitStatus, u32), String> {
        let mut fds = [0i32; 2];
        if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
            return Err(format!("no lifeline pipe: {}", std::io::Error::last_os_error()));
        }
        let (lifeline, held) = (fds[0], fds[1]);
        // the write end is this process's alone (not past its exec); the read end goes to the run
        unsafe { fcntl(held, F_SETFD, FD_CLOEXEC) };
        let mut cmd = next.command();
        cmd.env(FILE_ENV, file).env(PARENT_ENV, std::process::id().to_string()).env(LIFELINE_ENV, lifeline.to_string());
        let spawned = cmd.spawn();
        unsafe { close(lifeline) };
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                unsafe { close(held) };
                return Err(format!("cannot start {}: {e}", std::path::Path::new(&next.program).display()));
            }
        };
        CHILD.store(child.id() as i32, SeqCst);
        // a signal that came while the run was being started is passed on now
        if let s @ 1.. = STOP.load(SeqCst) {
            unsafe { kill(child.id() as i32, s) };
        }
        let status = child.wait();
        CHILD.store(0, SeqCst);
        unsafe { close(held) };
        status.map(|s| (s, child.id())).map_err(|e| format!("cannot wait for the run: {e}"))
    }

    /// The first `makepad` of a run that can restart: starts the run, waits for it, then ends as it
    /// ended, or becomes the next run's binary (exec-through) when the run asked for another.
    /// Returns only when it could not be a parent, before it started anything, with why.
    pub(super) fn parent(first: Next) -> String {
        let _k = crate::zone_alloc::kept();
        let dir = match makepad_remote::carrier::unix::own_socket_dir() {
            Ok(d) => d,
            Err(e) => return format!("no private folder for a restart's request: {e}"),
        };
        let file = dir.join(format!("relax-restart-{}", std::process::id()));
        for sig in PASSED_ON {
            set_action(sig, &abi::act(pass_on as *const () as usize, abi::SA_SIGINFO | abi::SA_RESTART));
        }
        let mut next = first;
        loop {
            // (a signal kept pending across the exec-through runs the handler now)
            mask(abi::SIG_UNBLOCK);
            if let s @ 1.. = STOP.load(SeqCst) {
                // asked to end between two runs: as if the next run had taken it
                end_by(s);
            }
            // the file is the request: only the run started next can make it
            let _ = std::fs::remove_file(&file);
            let (status, pid) = match run(&next, &file) {
                Ok(s) => s,
                Err(e) => next_failed(&next, &e),
            };
            mask(abi::SIG_BLOCK);
            let asked = std::fs::read(&file);
            let _ = std::fs::remove_file(&file);
            let data = match asked {
                Ok(d) if STOP.load(SeqCst) == 0 => d,
                // no request, or this process was asked to end: no next run
                _ => finish(status, pid),
            };
            let Some(n) = Next::decode(&data) else {
                next_failed(&next, &format!("{} is not a run's command", file.display()));
            };
            let e = n.command().exec();
            // the next program cannot start: the previous one does, with the next run's state
            let why = format!("cannot start {}: {e}; the run starts again on the {tool} it ran", std::path::Path::new(&n.program).display(), tool = crate::tool_name());
            record_failure(&n, &why);
            let envs = n.envs.into_iter().filter(|(k, _)| k != "RELAX_JIT_RECOVERY").collect();
            next = Next { program: next.program, args: n.args, dir: n.dir, envs };
        }
    }

    /// The run a parent started: one of its threads waits on the lifeline, whose other end only
    /// the parent holds, and ends the run as a SIGTERM would once the parent is gone (killed:
    /// nothing else passes the run a signal).
    pub(super) fn hold_lifeline() {
        let Some(fd) = std::env::var_os(LIFELINE_ENV).and_then(|v| v.to_str()?.parse::<i32>().ok()) else { return };
        std::env::remove_var(LIFELINE_ENV);
        // not for the programs this run starts
        unsafe { fcntl(fd, F_SETFD, FD_CLOEXEC) };
        let _k = crate::zone_alloc::kept();
        let spawned = std::thread::Builder::new().name("run-lifeline".into()).spawn(move || {
            let _k = crate::zone_alloc::kept();
            let mut byte = 0u8;
            loop {
                match unsafe { read(fd, &mut byte as *mut u8 as *mut core::ffi::c_void, 1) } {
                    0 => break,
                    n if n < 0 && std::io::Error::last_os_error().raw_os_error() != Some(EINTR) => return,
                    _ => {}
                }
            }
            eprintln!("relax: the run's parent process (pid {}) has ended: the run ends as a SIGTERM ends it", PARENT.get().copied().unwrap_or(0));
            unsafe { kill(getpid(), SIGTERM) };
        });
        if let Err(e) = spawned {
            eprintln!("relax: the run will outlive a parent that is killed: no thread for its lifeline ({e})");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::path::Path;

        const NAME: &str = "restart_parent::unix::tests::a_run_restarts_as_a_new_process_behind_a_parent_that_stays";
        /// The folder of a scenario: set for the processes of this test that play the loader.
        const ROLE: &str = "RELAX_TEST_RESTART_PARENT_DIR";
        const SCENARIO: &str = "RELAX_TEST_RESTART_PARENT_SCENARIO";
        const SIGUSR1: i32 = if cfg!(target_os = "macos") { 30 } else { 10 };

        extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }

        static TERMS: AtomicI32 = AtomicI32::new(0);
        extern "C" fn count_term(_: i32) {
            TERMS.fetch_add(1, SeqCst);
        }

        /// A process of this test that plays the loader: the first becomes a parent (`enter`); each
        /// run its parent starts writes what it got to `run-<n>`, then does its scenario's step.
        fn play(dir: &Path) {
            if !started_by_parent() {
                let args: Vec<Vec<u8>> = std::env::args_os().map(|a| a.into_encoded_bytes()).collect();
                panic!("not a parent: {}", parent(Next::first(&args).unwrap()));
            }
            let n = std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("run-")).count() + 1;
            let var = |k: &str| std::env::var(k).unwrap_or_else(|_| "-".into());
            let got = format!(
                "pid {} ppid {} parent {:?} restore {} receipt {} file {} lifeline {}",
                std::process::id(),
                std::os::unix::process::parent_id(),
                parent_pid(),
                var("RELAX_RESTORE"),
                var("RELAX_TEST_RECEIPT"),
                var(FILE_ENV),
                var(LIFELINE_ENV),
            );
            std::fs::write(dir.join(format!("run-{n}")), got).unwrap();
            let again = |program: &std::ffi::OsStr, cwd: &Path, restore: &Path| {
                let mut cmd = std::process::Command::new(program);
                cmd.args(std::env::args_os().skip(1)).current_dir(cwd).env("RELAX_RESTORE", restore).env_remove("RELAX_TEST_RECEIPT");
                panic!("the request failed: {}", request(&cmd));
            };
            let exe = std::env::current_exe().unwrap();
            match (var(SCENARIO).as_str(), n) {
                // a restart, then a plain end. The next program is a shell that notes the folder
                // it starts in and execs this one: a run's link under the JIT goes to its launch's
                // folder as it starts (runner launch.rs), so the run cannot say where it was started
                ("chain", 1) => {
                    let mut cmd = std::process::Command::new("/bin/sh");
                    cmd.args(["-c", "pwd -P > started-in && exec \"$0\" \"$@\""]).arg(&exe).args(std::env::args_os().skip(1));
                    cmd.current_dir(dir.join("b")).env("RELAX_RESTORE", dir.join("state")).env_remove("RELAX_TEST_RECEIPT");
                    panic!("the request failed: {}", request(&cmd));
                }
                ("chain", _) => std::process::exit(7),
                // a next program that is not there: the previous one starts with the next run's state
                ("missing", 1) => again(dir.join("missing").as_os_str(), dir, &dir.join("state2")),
                ("missing", _) => {
                    unsafe { raise(SIGUSR1) };
                    unreachable!()
                }
                // waits for SIGTERMs (the parent's, the lifeline's), says how many came, then dies of one
                ("term", _) => {
                    unsafe { signal(SIGTERM, count_term as *const () as usize) };
                    std::fs::write(dir.join("waiting"), "").unwrap();
                    while TERMS.load(SeqCst) == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    std::fs::write(dir.join("terms"), TERMS.load(SeqCst).to_string()).unwrap();
                    set_action(SIGTERM, &abi::act(0, 0));
                    unsafe { raise(SIGTERM) };
                    unreachable!()
                }
                (other, n) => panic!("no step {n} in scenario {other}"),
            }
        }

        fn start(dir: &Path, scenario: &str) -> std::process::Child {
            std::fs::create_dir_all(dir.join("b")).unwrap();
            // (a process this test runs in may be a run of a parent of its own: its scenario starts
            // without that parent's names)
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
                .env_remove(FILE_ENV)
                .env_remove(PARENT_ENV)
                .env_remove(LIFELINE_ENV)
                .env(ROLE, dir)
                .env(SCENARIO, scenario)
                .env("RELAX_TEST_RECEIPT", "r1")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        }

        fn runs(dir: &Path) -> Vec<std::collections::HashMap<String, String>> {
            (1..)
                .map(|n| std::fs::read_to_string(dir.join(format!("run-{n}"))).ok())
                .take_while(Option::is_some)
                .flatten()
                .map(|text| {
                    let w: Vec<&str> = text.split(' ').collect();
                    w.chunks(2).map(|kv| (kv[0].to_string(), kv[1].to_string())).collect()
                })
                .collect()
        }

        /// Waits for `path` while `child` runs (false: it ended first).
        fn appears(path: &Path, child: &mut std::process::Child) -> bool {
            while !path.exists() {
                if child.try_wait().unwrap().is_some() {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            true
        }

        /// The parent on Unix (one setup per scenario, several properties each): a run's request
        /// starts the next run as a new process behind the same parent pid (exec-through keeps it),
        /// with the request's arguments, folder and environment; the parent ends as the last run
        /// did; a next program that cannot start gives way to the previous one with the state;
        /// a SIGTERM to the parent reaches the run once, and a killed parent's run ends through
        /// the lifeline.
        #[test]
        fn a_run_restarts_as_a_new_process_behind_a_parent_that_stays() {
            if let Some(dir) = std::env::var_os(ROLE) {
                return play(Path::new(&dir));
            }
            // (a parent and its run at a time: one child slot)
            let _slot = crate::test_child::slot();
            let base = std::env::temp_dir().join(format!("relax-restart-parent-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);

            let dir = base.join("chain");
            let parent = start(&dir, "chain");
            let pid = parent.id().to_string();
            let out = parent.wait_with_output().unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            let r = runs(&dir);
            assert_eq!(r.len(), 2, "two runs: {r:?}, the parent ended {:?}\n{stderr}", out.status);
            assert_eq!(out.status.code(), Some(7), "the parent ends as the last run did: {stderr}");
            assert_ne!(r[0]["pid"], r[1]["pid"], "the next run is a new process");
            assert!(r.iter().all(|x| x["ppid"] == pid && x["parent"] == format!("Some({pid})")), "both runs stand behind the first process's pid, which stays across the exec-through: {r:?}");
            let started_in = std::fs::read_to_string(dir.join("b/started-in")).unwrap_or_default();
            assert_eq!(started_in.trim_end(), dir.join("b").canonicalize().unwrap().display().to_string(), "the request's folder");
            assert_eq!((r[0]["restore"].as_str(), r[1]["restore"].clone()), ("-", dir.join("state").display().to_string()), "the request's environment");
            assert_eq!((r[0]["receipt"].as_str(), r[1]["receipt"].as_str()), ("r1", "-"), "what the request takes out is out");
            assert!(r.iter().all(|x| x["file"] == "-" && x["lifeline"] == "-"), "the parent's variables are not the programs': {r:?}");

            let dir = base.join("missing");
            let out = start(&dir, "missing").wait_with_output().unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            let r = runs(&dir);
            assert_eq!(r.len(), 2, "the previous program ran again: {r:?}\n{stderr}");
            assert!(stderr.contains("restart failed: cannot start") && stderr.contains("missing"), "the parent says why: {stderr}");
            assert_eq!(r[1]["restore"], dir.join("state2").display().to_string(), "with the next run's state");
            assert_eq!(out.status.code(), Some(128 + SIGUSR1), "a run's other signal ends the parent with 128 + its number: {stderr}");
            assert!(stderr.contains(&format!("was ended by signal {SIGUSR1}, which did not come through its parent")), "the parent says a signal it did not pass on: {stderr}");

            let dir = base.join("term");
            let mut parent = start(&dir, "term");
            assert!(appears(&dir.join("waiting"), &mut parent), "the run started");
            unsafe { kill(parent.id() as i32, SIGTERM) };
            let out = parent.wait_with_output().unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(std::fs::read_to_string(dir.join("terms")).ok().as_deref(), Some("1"), "the parent's SIGTERM reaches the run once: {stderr}");
            assert_eq!(out.status.signal(), Some(SIGTERM), "the parent ends of the signal its run ended of");
            assert!(!stderr.contains("did not come through"), "a signal the parent passed on is not said: {stderr}");

            let dir = base.join("killed");
            let mut parent = start(&dir, "term");
            assert!(appears(&dir.join("waiting"), &mut parent), "the run started");
            parent.kill().unwrap();
            let out = parent.wait_with_output().unwrap();
            // (the run, orphaned, says so in its folder: a reaped process cannot be waited for)
            let mut waited = 0;
            while !dir.join("terms").exists() && waited < 30_000 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                waited += 20;
            }
            assert_eq!(std::fs::read_to_string(dir.join("terms")).ok().as_deref(), Some("1"), "a killed parent's run ends through the lifeline: {}", String::from_utf8_lossy(&out.stderr));
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<Vec<u8>> {
        args.iter().map(|a| a.as_bytes().to_vec()).collect()
    }

    #[test]
    fn only_a_run_that_can_restart_gets_a_parent() {
        for live in [&["makepad", "run", "tool-a"][..], &["makepad", "run", "--quiet", "tool-a"], &["makepad", "run", "--harness", "--live", "p"]] {
            assert!(can_restart(&words(live)), "{live:?}");
        }
        for other in [
            &["makepad", "inspect", "1", "stacks"][..],
            &["makepad", "devhub", "ls"],
            &["makepad", "test", "p"],
            &["makepad", "run", "--no-live", "p"],
            &["makepad", "run", "--check", "p"],
            &["makepad", "run", "--harness", "p"],
            &["makepad", "run", "--silent", "p"],
            &["makepad"],
        ] {
            assert!(!can_restart(&words(other)), "{other:?}");
        }
        assert!(can_restart(&words(&["makepad", "run", "p", "--", "--no-live"])), "the program's own arguments are not the run's");
    }

    #[test]
    fn the_next_run_comes_back_from_the_file_as_it_was_written() {
        let next = Next {
            program: "C:\\a b\\makepad.exe".into(),
            args: vec!["run".into(), "pkg".into(), "".into(), "with space".into()],
            dir: "C:\\work dir".into(),
            envs: vec![("RELAX_RESTART_T0".into(), "12".into()), ("EMPTY".into(), "".into())],
        };
        let data = next.encode();
        assert_eq!(Next::decode(&data), Some(next));
        assert_eq!(Next::decode(&data[..data.len() - 1]), None, "cut short");
        assert_eq!(Next::decode(&[data.as_slice(), &[0]].concat()), None, "something after it");
    }
}
