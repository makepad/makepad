//! std::process (unix half, selected by process.rs): exit/abort/id and Command over posix_spawnp (+ poll for output()).

use alloc::ffi::CString;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_short, c_void};
use core::fmt;

use crate::ffi::{OsStr, OsString};
use crate::fs::File;
use crate::io::{self, Read, Write};
use crate::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::path::Path;
use crate::sys;

pub fn id() -> u32 {
    unsafe { sys::getpid() as u32 }
}

pub fn exit(code: i32) -> ! {
    crate::io::cleanup();
    unsafe { sys::exit(code) }
}

pub fn abort() -> ! {
    unsafe { sys::abort() }
}

// ---- Stdio

enum StdioKind {
    Inherit,
    Null,
    Piped,
    Fd(OwnedFd),
}

pub struct Stdio(StdioKind);

impl Stdio {
    pub fn piped() -> Stdio {
        Stdio(StdioKind::Piped)
    }
    pub fn inherit() -> Stdio {
        Stdio(StdioKind::Inherit)
    }
    pub fn null() -> Stdio {
        Stdio(StdioKind::Null)
    }
}

impl From<File> for Stdio {
    fn from(f: File) -> Stdio {
        Stdio(StdioKind::Fd(OwnedFd::from(f)))
    }
}

impl From<io::PipeReader> for Stdio {
    fn from(p: io::PipeReader) -> Stdio {
        Stdio::from(OwnedFd::from(p))
    }
}
impl From<io::PipeWriter> for Stdio {
    fn from(p: io::PipeWriter) -> Stdio {
        Stdio::from(OwnedFd::from(p))
    }
}
impl From<OwnedFd> for Stdio {
    fn from(fd: OwnedFd) -> Stdio {
        Stdio(StdioKind::Fd(fd))
    }
}

impl fmt::Debug for Stdio {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stdio").finish_non_exhaustive()
    }
}

// ---- ExitStatus / Output

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct ExitStatus(c_int); // raw wait status

impl ExitStatus {
    pub fn success(&self) -> bool {
        self.code() == Some(0)
    }
    pub fn code(&self) -> Option<i32> {
        if self.0 & 0x7f == 0 {
            Some((self.0 >> 8) & 0xff)
        } else {
            None
        }
    }
    pub(crate) fn signal_number(&self) -> Option<i32> {
        let sig = self.0 & 0x7f;
        if sig != 0 && sig != 0x7f {
            Some(sig)
        } else {
            None
        }
    }
    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        if self.success() {
            Ok(())
        } else {
            Err(ExitStatusError(*self))
        }
    }
    pub(crate) fn raw(&self) -> c_int {
        self.0
    }
    pub(crate) fn from_raw_status(raw: c_int) -> ExitStatus {
        ExitStatus(raw)
    }
}

impl Default for ExitStatus {
    fn default() -> ExitStatus {
        ExitStatus(0)
    }
}

impl fmt::Debug for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ExitStatus(unix_wait_status({}))", self.0)
    }
}

fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => " (SIGHUP)",
        2 => " (SIGINT)",
        3 => " (SIGQUIT)",
        4 => " (SIGILL)",
        5 => " (SIGTRAP)",
        6 => " (SIGABRT)",
        8 => " (SIGFPE)",
        9 => " (SIGKILL)",
        11 => " (SIGSEGV)",
        13 => " (SIGPIPE)",
        14 => " (SIGALRM)",
        15 => " (SIGTERM)",
        _ => "",
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.code() {
            Some(code) => write!(f, "exit status: {}", code),
            None => match self.signal_number() {
                Some(sig) => {
                    if self.0 & 0x80 != 0 {
                        write!(f, "signal: {}{} (core dumped)", sig, signal_name(sig))
                    } else {
                        write!(f, "signal: {}{}", sig, signal_name(sig))
                    }
                }
                None => write!(f, "unrecognised wait status: {} {:#x}", self.0, self.0),
            },
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitStatusError(ExitStatus);

impl ExitStatusError {
    pub fn code(&self) -> Option<i32> {
        self.0.code()
    }
    pub fn into_status(&self) -> ExitStatus {
        self.0
    }
}

impl fmt::Display for ExitStatusError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "process exited unsuccessfully: {}", self.0)
    }
}

impl core::error::Error for ExitStatusError {}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitCode(u8);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);
    pub fn exit_process(self) -> ! {
        exit(self.0 as i32)
    }
}

impl ExitCode {
    #[doc(hidden)]
    pub fn to_i32(self) -> i32 {
        self.0 as i32
    }
}

/// rustc library/std/src/process.rs: what `fn main` may return.
#[cfg_attr(not(relax_check), lang = "termination")]
pub trait Termination {
    fn report(self) -> ExitCode;
}

impl Termination for () {
    fn report(self) -> ExitCode {
        ExitCode::SUCCESS
    }
}

// `!` as a type is unstable under rustc; the shim (check_os) builds without this impl.
#[cfg(not(relax_shim))]
impl Termination for ! {
    fn report(self) -> ExitCode {
        self
    }
}

impl Termination for core::convert::Infallible {
    fn report(self) -> ExitCode {
        match self {}
    }
}

impl Termination for ExitCode {
    fn report(self) -> ExitCode {
        self
    }
}

impl<T: Termination, E: fmt::Debug> Termination for Result<T, E> {
    fn report(self) -> ExitCode {
        match self {
            Ok(val) => val.report(),
            Err(err) => {
                eprintln!("Error: {:?}", err);
                ExitCode::FAILURE
            }
        }
    }
}

/// `makepad run` calls this instead of `main` when main returns a value (rustc rt.rs lang_start:
/// `main().report().to_i32()` becomes the process exit status).
#[doc(hidden)]
pub fn __relax_lang_start<T: Termination>(main: fn() -> T) {
    let code = main().report().to_i32();
    if code != 0 {
        exit(code)
    }
}

impl From<u8> for ExitCode {
    fn from(c: u8) -> ExitCode {
        ExitCode(c)
    }
}

#[derive(PartialEq, Eq, Clone)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl fmt::Debug for Output {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("Output");
        d.field("status", &self.status);
        match core::str::from_utf8(&self.stdout) {
            Ok(s) => d.field("stdout", &s),
            Err(_) => d.field("stdout", &self.stdout),
        };
        match core::str::from_utf8(&self.stderr) {
            Ok(s) => d.field("stderr", &s),
            Err(_) => d.field("stderr", &self.stderr),
        };
        d.finish()
    }
}

// ---- Command

pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    env_clear: bool,
    env: Vec<(OsString, Option<OsString>)>,
    cwd: Option<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    pgroup: Option<sys::pid_t>,
    /// CommandExt::pre_exec closures, run in the child between fork and exec
    closures: Vec<alloc::boxed::Box<dyn FnMut() -> io::Result<()> + Send + Sync>>,
}

impl Command {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Command {
        let program = program.as_ref().to_os_string();
        Command { args: alloc::vec![program.clone()], program, env_clear: false, env: Vec::new(), cwd: None, stdin: None, stdout: None, stderr: None, pgroup: None, closures: Vec::new() }
    }
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Command {
        self.args.push(arg.as_ref().to_os_string());
        self
    }
    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(&mut self, args: I) -> &mut Command {
        for a in args {
            self.arg(a);
        }
        self
    }
    pub fn env<K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, key: K, val: V) -> &mut Command {
        self.set_env(key.as_ref().to_os_string(), Some(val.as_ref().to_os_string()));
        self
    }
    pub fn envs<I: IntoIterator<Item = (K, V)>, K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, vars: I) -> &mut Command {
        for (k, v) in vars {
            self.env(k, v);
        }
        self
    }
    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Command {
        self.set_env(key.as_ref().to_os_string(), None);
        self
    }
    pub fn env_clear(&mut self) -> &mut Command {
        self.env_clear = true;
        self.env.clear();
        self
    }
    fn set_env(&mut self, key: OsString, val: Option<OsString>) {
        let mut i = 0;
        while i < self.env.len() {
            if self.env[i].0 == key {
                self.env[i].1 = val;
                return;
            }
            i += 1;
        }
        self.env.push((key, val));
    }
    pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Command {
        self.cwd = Some(dir.as_ref().as_os_str().to_os_string());
        self
    }
    pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Command {
        self.stdin = Some(cfg.into());
        self
    }
    pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Command {
        self.stdout = Some(cfg.into());
        self
    }
    pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Command {
        self.stderr = Some(cfg.into());
        self
    }
    pub fn get_program(&self) -> &OsStr {
        &self.program
    }
    /// The arguments after the program (std: `get_args`).
    pub fn get_args(&self) -> CommandArgs<'_> {
        CommandArgs { inner: self.args[1..].iter() }
    }
    /// The variables set or removed explicitly, in key order (std keeps them in a BTreeMap).
    pub fn get_envs(&self) -> CommandEnvs<'_> {
        let mut v: Vec<(&OsStr, Option<&OsStr>)> = self.env.iter().map(|(k, x)| (k.as_os_str(), x.as_deref())).collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        CommandEnvs { inner: v.into_iter() }
    }
    pub fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_ref().map(|d| Path::new(d))
    }
    pub(crate) fn set_arg0(&mut self, arg0: &OsStr) {
        self.args[0] = arg0.to_os_string();
    }
    pub(crate) fn set_pgroup(&mut self, pgroup: sys::pid_t) {
        self.pgroup = Some(pgroup);
    }
    pub(crate) fn add_pre_exec(&mut self, f: alloc::boxed::Box<dyn FnMut() -> io::Result<()> + Send + Sync>) {
        self.closures.push(f);
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        self.spawn_with(Stdio(StdioKind::Inherit))
    }

    pub fn output(&mut self) -> io::Result<Output> {
        let mut child = self.spawn_with(Stdio(StdioKind::Piped))?;
        drop(child.stdin.take());
        let (out, err) = read2(child.stdout.take(), child.stderr.take())?;
        let status = child.wait()?;
        Ok(Output { status, stdout: out, stderr: err })
    }

    pub fn status(&mut self) -> io::Result<ExitStatus> {
        let mut child = self.spawn_with(Stdio(StdioKind::Inherit))?;
        child.wait()
    }

    fn envp(&self) -> io::Result<Option<Vec<CString>>> {
        if !self.env_clear && self.env.is_empty() {
            return Ok(None);
        }
        let mut map: Vec<(OsString, OsString)> = Vec::new();
        if !self.env_clear {
            for kv in crate::env::vars_os() {
                map.push(kv);
            }
        }
        for (k, v) in &self.env {
            let mut j = 0;
            while j < map.len() {
                if map[j].0 == *k {
                    map.remove(j);
                } else {
                    j += 1;
                }
            }
            if let Some(v) = v {
                map.push((k.clone(), v.clone()));
            }
        }
        let mut out = Vec::new();
        for (k, v) in map {
            let mut b = k.into_encoded_bytes();
            b.push(b'=');
            b.extend_from_slice(v.as_encoded_bytes());
            out.push(cstring(b)?);
        }
        Ok(Some(out))
    }

    /// Under `makepad run` the program's executable is Relax itself: spawning it runs this
    /// program again (Relax's command line up to `--`, then the new arguments). A spawn of the
    /// loader whose first argument is `run` or `app` is a launcher starting another program
    /// through it (`$MAKEPAD_RUN_LOADER run [flags] <package> ...`, `... app <id>`: a launch
    /// record): that one runs as written.
    fn self_exec_args(&self) -> Option<Vec<OsString>> {
        let pre = crate::env::var("MAKEPAD_RUN_SELF_EXEC").ok()?;
        if self.args.get(1).is_some_and(|a| matches!(a.as_encoded_bytes(), b"run" | b"app")) {
            return None;
        }
        // inside a build script (MAKEPAD_RUN_AS_RUSTC): its spawns of this executable are RUSTC
        // questions, answered as written (relax_as_rustc); re-running the script would recurse
        if crate::env::var_os("MAKEPAD_RUN_AS_RUSTC").is_some() {
            return None;
        }
        let parts: Vec<&str> = pre.split('\u{1f}').collect();
        let exe = crate::path::Path::new(parts[0]);
        let p = crate::fs::canonicalize(crate::path::Path::new(&self.program)).ok()?;
        // the Relax executable itself runs this program
        if p.as_path() != exe {
            return None;
        }
        let mut out: Vec<OsString> = parts.iter().map(|x| OsString::from(*x)).collect();
        out[0] = self.program.clone();
        for a in self.args.iter().skip(1) {
            out.push(a.clone());
        }
        Some(out)
    }

    /// CommandExt::exec: replaces this process (real std sys/process/unix/unix.rs exec:
    /// stdio inherited by default, cwd, process group, SIGPIPE default, then execvp).
    pub(crate) fn exec_replace(&mut self) -> io::Error {
        match self.spawn_with_mode(Stdio(StdioKind::Inherit), true) {
            Err(e) => e,
            Ok(_) => io::const_error!(io::ErrorKind::Other, "exec returned"),
        }
    }

    fn spawn_with(&mut self, default: Stdio) -> io::Result<Child> {
        self.spawn_with_mode(default, false)
    }

    /// A current folder under Relax's VFS (a git repository run) is no folder the OS knows: the
    /// child keeps this process's real folder and gets the virtual one as MAKEPAD_RUN_WORKSPACE plus the
    /// mount table (MAKEPAD_RUN_VFS), which a Relax program's std and Relax itself serve paths from. A
    /// real current folder clears an inherited virtual one.
    fn spawn_with_mode(&mut self, default: Stdio, exec: bool) -> io::Result<Child> {
        // NDK r28 spawn.h: addchdir_np requires API 34; spawning itself requires API 28.
        // The launcher will provide --cwd; never change the Activity process's cwd or fork it.
        #[cfg(target_os = "android")]
        if self.cwd.is_some() || !self.closures.is_empty() {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "Android posix_spawn does not support cwd or pre_exec"));
        }
        let Some(dir) = self.cwd.clone() else {
            return self.spawn_now(default, exec);
        };
        let saved_env = self.env.clone();
        let virt = crate::fs::vfs::kind(Path::new(&dir));
        let r = match virt {
            Some((abs, _)) => {
                self.cwd = None;
                self.env("MAKEPAD_RUN_WORKSPACE", abs.as_os_str());
                self.env("MAKEPAD_RUN_VFS", crate::fs::vfs::mounts());
                let r = self.spawn_now(default, exec);
                self.cwd = Some(dir);
                r
            }
            None => {
                if crate::env::var_os("MAKEPAD_RUN_WORKSPACE").is_some() {
                    self.env_remove("MAKEPAD_RUN_WORKSPACE");
                }
                self.spawn_now(default, exec)
            }
        };
        self.env = saved_env;
        r
    }

    fn spawn_now(&mut self, default: Stdio, exec: bool) -> io::Result<Child> {
        let mut self_exec = self.self_exec_args();
        // replacing this process with the program again (an exec, not a spawn) continues the
        // live session: the `--silent` Relax added for re-runs (it ends live editing) goes
        if let Some(v) = self_exec.as_mut().filter(|_| exec && crate::env::var_os("MAKEPAD_RUN_SELF_EXEC_SILENT_ADDED").is_some()) {
            if v.get(2).is_some_and(|a| a.as_encoded_bytes() == b"--silent") {
                v.remove(2);
            }
        }
        // ... and its clocks go on from this process's start (relax base clock.rs EXEC_ORIGIN_ENV)
        if exec && self_exec.is_some() {
            if let Some(t0) = crate::env::var_os("MAKEPAD_RUN_STARTUP_T0") {
                self.env("MAKEPAD_RUN_EXEC_ORIGIN_EPOCH", &t0);
            }
        }
        let prog = match &self_exec {
            Some(v) => cstring(v[0].as_encoded_bytes().to_vec())?,
            None => cstring(self.program.as_encoded_bytes().to_vec())?,
        };
        let mut argv_c = Vec::new();
        for a in self_exec.as_ref().unwrap_or(&self.args) {
            argv_c.push(cstring(a.as_encoded_bytes().to_vec())?);
        }
        let mut argv: Vec<*const c_char> = Vec::new();
        for a in &argv_c {
            argv.push(a.as_ptr());
        }
        argv.push(core::ptr::null());
        let envp_c = self.envp()?;
        let mut envp: Vec<*const c_char> = Vec::new();
        let envp_ptr = match &envp_c {
            Some(list) => {
                for e in list {
                    envp.push(e.as_ptr());
                }
                envp.push(core::ptr::null());
                envp.as_ptr()
            }
            None => sys::os::environ(),
        };
        let cwd = match &self.cwd {
            Some(d) => Some(cstring(d.as_encoded_bytes().to_vec())?),
            None => None,
        };

        let default_piped = match default.0 {
            StdioKind::Piped => true,
            _ => false,
        };
        let (stdin_child, stdin_parent) = setup_stdio(self.stdin.take(), default_piped, true)?;
        let (stdout_child, stdout_parent) = setup_stdio(self.stdout.take(), default_piped, false)?;
        let (stderr_child, stderr_parent) = setup_stdio(self.stderr.take(), default_piped, false)?;

        #[cfg(not(target_os = "android"))]
        if !self.closures.is_empty() {
            if exec {
                // CommandExt::exec: real std runs the closures in this process before the exec
                for f in self.closures.iter_mut() {
                    f()?;
                }
            } else {
                // real std: no posix_spawn when there are pre_exec closures -- fork, set the child
                // up, run the closures, exec; a CLOEXEC pipe carries a failure's errno back
                let pid = unsafe { self.fork_exec(&prog, &argv, envp_ptr, &cwd, &stdin_child, &stdout_child, &stderr_child)? };
                drop(stdin_child);
                drop(stdout_child);
                drop(stderr_child);
                return Ok(Child {
                    pid,
                    status: None,
                    stdin: match stdin_parent {
                        Some(fd) => Some(ChildStdin { fd }),
                        None => None,
                    },
                    stdout: match stdout_parent {
                        Some(fd) => Some(ChildStdout { fd }),
                        None => None,
                    },
                    stderr: match stderr_parent {
                        Some(fd) => Some(ChildStderr { fd }),
                        None => None,
                    },
                });
            }
        }
        let mut actions = SpawnFileActions { storage: [0; 32] };
        let mut attr = SpawnAttr { storage: [0; 64] };
        let mut pid: sys::pid_t = 0;
        unsafe {
            posix_spawn_file_actions_init(&mut actions as *mut SpawnFileActions);
            posix_spawnattr_init(&mut attr as *mut SpawnAttr);
            if let Some(fd) = &stdin_child {
                posix_spawn_file_actions_adddup2(&mut actions as *mut SpawnFileActions, fd.as_raw_fd(), 0);
            }
            if let Some(fd) = &stdout_child {
                posix_spawn_file_actions_adddup2(&mut actions as *mut SpawnFileActions, fd.as_raw_fd(), 1);
            }
            if let Some(fd) = &stderr_child {
                posix_spawn_file_actions_adddup2(&mut actions as *mut SpawnFileActions, fd.as_raw_fd(), 2);
            }
            #[cfg(not(target_os = "android"))]
            if let Some(c) = &cwd {
                posix_spawn_file_actions_addchdir_np(&mut actions as *mut SpawnFileActions, c.as_ptr());
            }
            // real std (sys/process/unix/unix.rs posix_spawn): the process group, and SIGPIPE
            // back to its default in the child
            let mut flags: c_short = 0;
            if let Some(pgroup) = self.pgroup {
                flags |= POSIX_SPAWN_SETPGROUP;
                let r = posix_spawnattr_setpgroup(&mut attr as *mut SpawnAttr, pgroup);
                if r != 0 {
                    posix_spawn_file_actions_destroy(&mut actions as *mut SpawnFileActions);
                    posix_spawnattr_destroy(&mut attr as *mut SpawnAttr);
                    return Err(io::Error::from_raw_os_error(r));
                }
            }
            let mut default_set = SigSet { storage: [0; 16] };
            sigemptyset(&mut default_set as *mut SigSet);
            sigaddset(&mut default_set as *mut SigSet, SIGPIPE);
            posix_spawnattr_setsigdefault(&mut attr as *mut SpawnAttr, &default_set as *const SigSet);
            flags |= POSIX_SPAWN_SETSIGDEF;
            if exec {
                // Darwin: posix_spawn with POSIX_SPAWN_SETEXEC is execve with the file actions
                // and attributes applied to this process; it returns only on failure
                #[cfg(target_os = "macos")]
                {
                    flags |= POSIX_SPAWN_SETEXEC;
                }
                #[cfg(not(target_os = "macos"))]
                {
                    posix_spawn_file_actions_destroy(&mut actions as *mut SpawnFileActions);
                    posix_spawnattr_destroy(&mut attr as *mut SpawnAttr);
                    for (fd, to) in [(&stdin_child, 0), (&stdout_child, 1), (&stderr_child, 2)] {
                        if let Some(fd) = fd {
                            if dup2(fd.as_raw_fd(), to) < 0 {
                                return Err(io::Error::last_os_error());
                            }
                        }
                    }
                    if let Some(c) = &cwd {
                        if chdir(c.as_ptr()) != 0 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    if let Some(pgroup) = self.pgroup {
                        if setpgid(0, pgroup) != 0 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    signal(SIGPIPE, 0);
                    execvpe(prog.as_ptr(), argv.as_ptr(), envp_ptr);
                    return Err(io::Error::last_os_error());
                }
            }
            posix_spawnattr_setflags(&mut attr as *mut SpawnAttr, flags);
            let r = posix_spawnp(&mut pid as *mut sys::pid_t, prog.as_ptr(), &actions as *const SpawnFileActions, &attr as *const SpawnAttr, argv.as_ptr(), envp_ptr);
            posix_spawn_file_actions_destroy(&mut actions as *mut SpawnFileActions);
            posix_spawnattr_destroy(&mut attr as *mut SpawnAttr);
            if r != 0 {
                return Err(io::Error::from_raw_os_error(r));
            }
        }
        drop(stdin_child);
        drop(stdout_child);
        drop(stderr_child);
        Ok(Child {
            pid,
            status: None,
            stdin: match stdin_parent {
                Some(fd) => Some(ChildStdin { fd }),
                None => None,
            },
            stdout: match stdout_parent {
                Some(fd) => Some(ChildStdout { fd }),
                None => None,
            },
            stderr: match stderr_parent {
                Some(fd) => Some(ChildStderr { fd }),
                None => None,
            },
        })
    }
}

impl Command {
    /// fork + exec for commands with pre_exec closures (real std's do_exec order: stdio, cwd,
    /// process group, SIGPIPE default, closures, exec). Returns the child's pid, or the error the
    /// child reported through the CLOEXEC pipe (the child is reaped then).
    #[cfg(not(target_os = "android"))]
    unsafe fn fork_exec(&mut self, prog: &CString, argv: &Vec<*const c_char>, envp: *const *const c_char, cwd: &Option<CString>, stdin_child: &Option<OwnedFd>, stdout_child: &Option<OwnedFd>, stderr_child: &Option<OwnedFd>) -> io::Result<sys::pid_t> {
        let (err_r, err_w) = anon_pipe()?;
        let pid = fork();
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            // child
            let w = err_w.as_raw_fd();
            for (fd, to) in [(stdin_child, 0), (stdout_child, 1), (stderr_child, 2)] {
                if let Some(fd) = fd {
                    if sys::dup2(fd.as_raw_fd(), to) < 0 {
                        child_fail(w, io::Error::last_os_error());
                    }
                }
            }
            if let Some(c) = cwd {
                if sys::chdir(c.as_ptr()) != 0 {
                    child_fail(w, io::Error::last_os_error());
                }
            }
            if let Some(pgroup) = self.pgroup {
                if fork_setpgid(0, pgroup) != 0 {
                    child_fail(w, io::Error::last_os_error());
                }
            }
            fork_signal(SIGPIPE, 0);
            for f in self.closures.iter_mut() {
                if let Err(e) = f() {
                    child_fail(w, e);
                }
            }
            exec_path_env(prog.as_ptr(), argv.as_ptr(), envp);
            child_fail(w, io::Error::last_os_error());
        }
        drop(err_w);
        let mut buf = [0u8; 4];
        let mut got = 0usize;
        loop {
            let n = sys::read(err_r.as_raw_fd(), buf.as_mut_ptr().add(got) as *mut c_void, 4 - got);
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if n == 0 {
                break;
            }
            got += n as usize;
            if got == 4 {
                break;
            }
        }
        if got == 0 {
            return Ok(pid); // exec closed the pipe
        }
        let mut status: c_int = 0;
        while sys::waitpid(pid, &mut status as *mut c_int, 0) < 0 {
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
        if got == 4 {
            Err(io::Error::from_raw_os_error(i32::from_be_bytes(buf)))
        } else {
            Err(io::const_error!(io::ErrorKind::Other, "short read on the CLOEXEC pipe"))
        }
    }
}

/// In a forked child: report `e`'s errno through the CLOEXEC pipe and exit (EINVAL for errors
/// without an OS code, as real std).
unsafe fn child_fail(fd: c_int, e: io::Error) -> ! {
    let code: i32 = match e.raw_os_error() {
        Some(c) => c,
        None => 22,
    };
    let bytes = code.to_be_bytes();
    sys::write(fd, bytes.as_ptr() as *const c_void, 4);
    sys::_exit(1)
}

/// execvp with an explicit environment: execvpe on Linux; on macOS (no execvpe)
/// posix_spawnp with POSIX_SPAWN_SETEXEC, which is execve after the PATH search.
unsafe fn exec_path_env(file: *const c_char, argv: *const *const c_char, envp: *const *const c_char) {
    #[cfg(not(target_os = "macos"))]
    {
        execvpe(file, argv, envp);
    }
    #[cfg(target_os = "macos")]
    {
        let mut attr = SpawnAttr { storage: [0; 64] };
        posix_spawnattr_init(&mut attr as *mut SpawnAttr);
        posix_spawnattr_setflags(&mut attr as *mut SpawnAttr, POSIX_SPAWN_SETEXEC);
        let mut pid: sys::pid_t = 0;
        let r = posix_spawnp(&mut pid as *mut sys::pid_t, file, core::ptr::null(), &attr as *const SpawnAttr, argv, envp);
        if r != 0 {
            set_errno(r);
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn set_errno(e: c_int) {
    *__error() = e;
}

/// Iterator over a Command's arguments (Command::get_args).
pub struct CommandArgs<'a> {
    inner: core::slice::Iter<'a, OsString>,
}

impl<'a> Iterator for CommandArgs<'a> {
    type Item = &'a OsStr;
    fn next(&mut self) -> Option<&'a OsStr> {
        self.inner.next().map(|s| s.as_os_str())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a> ExactSizeIterator for CommandArgs<'a> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<'a> fmt::Debug for CommandArgs<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandArgs").field("inner", &self.inner.as_slice()).finish()
    }
}

/// Iterator over a Command's explicitly set / removed variables (Command::get_envs).
pub struct CommandEnvs<'a> {
    inner: alloc::vec::IntoIter<(&'a OsStr, Option<&'a OsStr>)>,
}

impl<'a> Iterator for CommandEnvs<'a> {
    type Item = (&'a OsStr, Option<&'a OsStr>);
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a> ExactSizeIterator for CommandEnvs<'a> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut first = true;
        for a in &self.args {
            if !first {
                f.write_str(" ")?;
            }
            first = false;
            fmt::Debug::fmt(a, f)?;
        }
        Ok(())
    }
}

fn cstring(b: Vec<u8>) -> io::Result<CString> {
    match CString::new(b) {
        Ok(c) => Ok(c),
        Err(_) => Err(io::const_error!(io::ErrorKind::InvalidInput, "nul byte found in provided data")),
    }
}

/// (fd for the child, parent end of a pipe)
fn setup_stdio(cfg: Option<Stdio>, default_piped: bool, readable_by_child: bool) -> io::Result<(Option<OwnedFd>, Option<OwnedFd>)> {
    let kind = match cfg {
        Some(s) => s.0,
        None => {
            if default_piped && readable_by_child {
                StdioKind::Null
            } else if default_piped {
                StdioKind::Piped
            } else {
                StdioKind::Inherit
            }
        }
    };
    match kind {
        StdioKind::Inherit => Ok((None, None)),
        StdioKind::Fd(fd) => Ok((Some(fd), None)),
        StdioKind::Null => {
            let mut opts = crate::fs::OpenOptions::new();
            opts.read(readable_by_child).write(!readable_by_child);
            let f = opts.open("/dev/null")?;
            Ok((Some(OwnedFd::from(f)), None))
        }
        StdioKind::Piped => {
            let (r, w) = anon_pipe()?;
            if readable_by_child {
                Ok((Some(r), Some(w)))
            } else {
                Ok((Some(w), Some(r)))
            }
        }
    }
}

fn anon_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as c_int; 2];
    if unsafe { sys::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        sys::fcntl(fds[0], sys::F_SETFD, sys::FD_CLOEXEC);
        sys::fcntl(fds[1], sys::F_SETFD, sys::FD_CLOEXEC);
        Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])))
    }
}

#[repr(C)]
struct SpawnFileActions {
    storage: [u64; 32],
}

#[repr(C)]
struct SpawnAttr {
    storage: [u64; 64],
}

/// sigset_t (4 bytes on Darwin, 128 on Linux glibc): storage for either.
#[repr(C)]
struct SigSet {
    storage: [u64; 16],
}

const POSIX_SPAWN_SETPGROUP: c_short = 0x02;
const POSIX_SPAWN_SETSIGDEF: c_short = 0x04;
#[cfg(target_os = "macos")]
const POSIX_SPAWN_SETEXEC: c_short = 0x40;
const SIGPIPE: c_int = 13;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

const POLLIN: c_short = 1;

extern "C" {
    fn posix_spawnp(pid: *mut sys::pid_t, file: *const c_char, actions: *const SpawnFileActions, attr: *const SpawnAttr, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    fn posix_spawn_file_actions_init(a: *mut SpawnFileActions) -> c_int;
    fn posix_spawn_file_actions_destroy(a: *mut SpawnFileActions) -> c_int;
    fn posix_spawn_file_actions_adddup2(a: *mut SpawnFileActions, fd: c_int, newfd: c_int) -> c_int;
    #[cfg(not(target_os = "android"))]
    fn posix_spawn_file_actions_addchdir_np(a: *mut SpawnFileActions, path: *const c_char) -> c_int;
    fn posix_spawnattr_init(a: *mut SpawnAttr) -> c_int;
    fn posix_spawnattr_destroy(a: *mut SpawnAttr) -> c_int;
    fn posix_spawnattr_setflags(a: *mut SpawnAttr, flags: c_short) -> c_int;
    fn posix_spawnattr_setpgroup(a: *mut SpawnAttr, pgroup: sys::pid_t) -> c_int;
    fn posix_spawnattr_setsigdefault(a: *mut SpawnAttr, set: *const SigSet) -> c_int;
    fn sigemptyset(set: *mut SigSet) -> c_int;
    #[cfg(not(target_os = "macos"))]
    fn dup2(fd: c_int, to: c_int) -> c_int;
    #[cfg(not(target_os = "macos"))]
    fn chdir(path: *const c_char) -> c_int;
    #[cfg(not(target_os = "macos"))]
    fn setpgid(pid: sys::pid_t, pgid: sys::pid_t) -> c_int;
    #[cfg(not(target_os = "macos"))]
    fn signal(sig: c_int, handler: usize) -> usize;
    #[cfg(not(target_os = "macos"))]
    fn execvpe(file: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
    fn sigaddset(set: *mut SigSet, sig: c_int) -> c_int;
    #[cfg(not(target_os = "android"))]
    fn fork() -> sys::pid_t;
    #[cfg(not(target_os = "android"))]
    #[link_name = "setpgid"]
    fn fork_setpgid(pid: sys::pid_t, pgid: sys::pid_t) -> c_int;
    #[cfg(not(target_os = "android"))]
    #[link_name = "signal"]
    fn fork_signal(sig: c_int, handler: usize) -> usize;
    #[cfg(target_os = "macos")]
    fn __error() -> *mut c_int;
    fn poll(fds: *mut PollFd, n: u32, timeout: c_int) -> c_int;
}

/// Reads both pipes to the end without deadlocking (poll), like real std's read2.
fn read2(out: Option<ChildStdout>, err: Option<ChildStderr>) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    let mut out_fd = match &out {
        Some(o) => o.fd.as_raw_fd(),
        None => -1,
    };
    let mut err_fd = match &err {
        Some(e) => e.fd.as_raw_fd(),
        None => -1,
    };
    let mut chunk = [0u8; 8192];
    while out_fd >= 0 || err_fd >= 0 {
        let mut fds = [PollFd { fd: out_fd, events: POLLIN, revents: 0 }, PollFd { fd: err_fd, events: POLLIN, revents: 0 }];
        let r = unsafe { poll(fds.as_mut_ptr(), 2, -1) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.is_interrupted() {
                continue;
            }
            return Err(e);
        }
        let mut k = 0;
        while k < 2 {
            if fds[k].fd >= 0 && fds[k].revents != 0 {
                let n = unsafe { sys::read(fds[k].fd, chunk.as_mut_ptr() as *mut c_void, chunk.len()) };
                if n < 0 {
                    let e = io::Error::last_os_error();
                    if !e.is_interrupted() && e.kind() != io::ErrorKind::WouldBlock {
                        return Err(e);
                    }
                } else if n == 0 {
                    if k == 0 {
                        out_fd = -1;
                    } else {
                        err_fd = -1;
                    }
                } else if k == 0 {
                    out_buf.extend_from_slice(&chunk[..n as usize]);
                } else {
                    err_buf.extend_from_slice(&chunk[..n as usize]);
                }
            }
            k += 1;
        }
    }
    drop(out);
    drop(err);
    Ok((out_buf, err_buf))
}

// ---- Child

pub struct Child {
    pid: sys::pid_t,
    status: Option<ExitStatus>,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl Child {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }
    pub fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        if unsafe { sys::kill(self.pid, sys::SIGKILL) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        if let Some(s) = self.status {
            return Ok(s);
        }
        let mut status: c_int = 0;
        loop {
            let r = unsafe { sys::waitpid(self.pid, &mut status as *mut c_int, 0) };
            if r >= 0 {
                break;
            }
            let e = io::Error::last_os_error();
            if !e.is_interrupted() {
                return Err(e);
            }
        }
        let s = ExitStatus(status);
        self.status = Some(s);
        Ok(s)
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(s) = self.status {
            return Ok(Some(s));
        }
        let mut status: c_int = 0;
        let r = unsafe { sys::waitpid(self.pid, &mut status as *mut c_int, sys::WNOHANG) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        if r == 0 {
            return Ok(None);
        }
        let s = ExitStatus(status);
        self.status = Some(s);
        Ok(Some(s))
    }
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        let (out, err) = read2(self.stdout.take(), self.stderr.take())?;
        let status = self.wait()?;
        Ok(Output { status, stdout: out, stderr: err })
    }
}

impl fmt::Debug for Child {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Child")
            .field("stdin", &self.stdin)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .finish_non_exhaustive()
    }
}

pub struct ChildStdin {
    fd: OwnedFd,
}
pub struct ChildStdout {
    fd: OwnedFd,
}
pub struct ChildStderr {
    fd: OwnedFd,
}

fn fd_read(fd: &OwnedFd, buf: &mut [u8]) -> io::Result<usize> {
    let r = unsafe { sys::read(fd.as_raw_fd(), buf.as_mut_ptr() as *mut c_void, buf.len()) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r as usize)
    }
}

fn fd_write(fd: &OwnedFd, buf: &[u8]) -> io::Result<usize> {
    let r = unsafe { sys::write(fd.as_raw_fd(), buf.as_ptr() as *const c_void, buf.len()) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r as usize)
    }
}

impl Write for ChildStdin {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        fd_write(&self.fd, buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Read for ChildStdout {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        fd_read(&self.fd, buf)
    }
}
impl Read for ChildStderr {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        fd_read(&self.fd, buf)
    }
}

impl AsRawFd for ChildStdin {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
impl AsRawFd for ChildStdout {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
impl AsRawFd for ChildStderr {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
impl IntoRawFd for ChildStdout {
    fn into_raw_fd(self) -> RawFd {
        self.fd.into_raw_fd()
    }
}
impl From<ChildStdin> for Stdio {
    fn from(c: ChildStdin) -> Stdio {
        Stdio(StdioKind::Fd(c.fd))
    }
}
impl From<ChildStdout> for Stdio {
    fn from(c: ChildStdout) -> Stdio {
        Stdio(StdioKind::Fd(c.fd))
    }
}
impl From<ChildStderr> for Stdio {
    fn from(c: ChildStderr) -> Stdio {
        Stdio(StdioKind::Fd(c.fd))
    }
}

impl fmt::Debug for ChildStdin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ChildStdin").finish_non_exhaustive()
    }
}
impl fmt::Debug for ChildStdout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ChildStdout").finish_non_exhaustive()
    }
}
impl fmt::Debug for ChildStderr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ChildStderr").finish_non_exhaustive()
    }
}
