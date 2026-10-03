//! std::process: exit/abort/id and Command over posix_spawnp (+ poll for output()).

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
}

impl Command {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Command {
        let program = program.as_ref().to_os_string();
        Command { args: alloc::vec![program.clone()], program, env_clear: false, env: Vec::new(), cwd: None, stdin: None, stdout: None, stderr: None }
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
    pub(crate) fn set_arg0(&mut self, arg0: &OsStr) {
        self.args[0] = arg0.to_os_string();
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

    fn spawn_with(&mut self, default: Stdio) -> io::Result<Child> {
        let prog = cstring(self.program.as_encoded_bytes().to_vec())?;
        let mut argv_c = Vec::new();
        for a in &self.args {
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
            if let Some(c) = &cwd {
                posix_spawn_file_actions_addchdir_np(&mut actions as *mut SpawnFileActions, c.as_ptr());
            }
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
        Err(_) => Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "nul byte found in provided data")),
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
    fn posix_spawn_file_actions_addchdir_np(a: *mut SpawnFileActions, path: *const c_char) -> c_int;
    fn posix_spawnattr_init(a: *mut SpawnAttr) -> c_int;
    fn posix_spawnattr_destroy(a: *mut SpawnAttr) -> c_int;
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
