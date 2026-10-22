//! std::process (Windows half, selected by process.rs): exit/abort/id and Command over
//! CreateProcessW (real std's sys/process/windows.rs: program resolution through the child's PATH,
//! the application directory, the system directories and the parent's PATH; the command line
//! quoted for CommandLineToArgvW, batch files through cmd.exe with real std's escaping; a sorted,
//! case-insensitive UTF-16 environment block; only the child's stdio handles made inheritable,
//! under one lock).
//!
//! Child pipes are CreatePipe pipes (synchronous on both ends; real std uses an overlapped named
//! pipe for our end so read_output can wait on both with overlapped reads): output() and
//! wait_with_output() read stderr on a second thread while this one reads stdout, so a child
//! filling either pipe never deadlocks.

#![allow(non_snake_case, non_camel_case_types)]

use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt;

use crate::ffi::{OsStr, OsString};
use crate::fs::File;
use crate::io::{self, Read, Write};
use crate::os::windows::ffi::{OsStrExt, OsStringExt};
use crate::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use crate::path::Path;
use crate::sys::os as win;

type HANDLE = RawHandle;
type BOOL = i32;

#[repr(C)]
struct STARTUPINFOW {
    cb: u32,
    lpReserved: *mut u16,
    lpDesktop: *mut u16,
    lpTitle: *mut u16,
    dwX: u32,
    dwY: u32,
    dwXSize: u32,
    dwYSize: u32,
    dwXCountChars: u32,
    dwYCountChars: u32,
    dwFillAttribute: u32,
    dwFlags: u32,
    wShowWindow: u16,
    cbReserved2: u16,
    lpReserved2: *mut u8,
    hStdInput: HANDLE,
    hStdOutput: HANDLE,
    hStdError: HANDLE,
}

#[repr(C)]
struct PROCESS_INFORMATION {
    hProcess: HANDLE,
    hThread: HANDLE,
    dwProcessId: u32,
    dwThreadId: u32,
}

#[repr(C)]
struct SECURITY_ATTRIBUTES {
    nLength: u32,
    lpSecurityDescriptor: *mut c_void,
    bInheritHandle: BOOL,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateProcessW(app: *const u16, cmd: *mut u16, process_attrs: *mut SECURITY_ATTRIBUTES, thread_attrs: *mut SECURITY_ATTRIBUTES, inherit: BOOL, flags: u32, env: *mut c_void, cwd: *const u16, si: *mut STARTUPINFOW, pi: *mut PROCESS_INFORMATION) -> BOOL;
    fn GetExitCodeProcess(process: HANDLE, code: *mut u32) -> BOOL;
    fn TerminateProcess(process: HANDLE, code: u32) -> BOOL;
    fn GetProcessId(process: HANDLE) -> u32;
    fn GetCurrentProcess() -> HANDLE;
    fn DuplicateHandle(src_process: HANDLE, src: HANDLE, dst_process: HANDLE, dst: *mut HANDLE, access: u32, inherit: BOOL, options: u32) -> BOOL;
    fn CreatePipe(read: *mut HANDLE, write: *mut HANDLE, attrs: *mut c_void, size: u32) -> BOOL;
    fn SetHandleInformation(h: HANDLE, mask: u32, flags: u32) -> BOOL;
    fn CreateFileW(name: *const u16, access: u32, share: u32, attrs: *const SECURITY_ATTRIBUTES, disposition: u32, flags: u32, template: HANDLE) -> HANDLE;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn GetFullPathNameW(name: *const u16, n: u32, buf: *mut u16, file_part: *mut *mut u16) -> u32;
    fn GetSystemDirectoryW(buf: *mut u16, n: u32) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, n: u32) -> u32;
    fn CompareStringOrdinal(s1: *const u16, n1: i32, s2: *const u16, n2: i32, ignore_case: BOOL) -> i32;
}

extern "C" {
    // the C runtime's abort (real std: __fastfail(FAST_FAIL_FATAL_APP_EXIT), inline asm)
    #[link_name = "abort"]
    fn crt_abort() -> !;
}

const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
const STARTF_USESHOWWINDOW: u32 = 0x0000_0001;
const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
const WAIT_OBJECT_0: u32 = 0;
const DUPLICATE_SAME_ACCESS: u32 = 0x2;
const HANDLE_FLAG_INHERIT: u32 = 0x1;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const FILE_SHARE_DELETE: u32 = 0x4;
const OPEN_EXISTING: u32 = 3;
const INVALID_FILE_ATTRIBUTES: u32 = 0xffff_ffff;
const CSTR_LESS_THAN: i32 = 1;
const CSTR_EQUAL: i32 = 2;
const CSTR_GREATER_THAN: i32 = 3;
const EXE_SUFFIX: &str = ".exe";

pub fn id() -> u32 {
    unsafe { win::GetCurrentProcessId() }
}

extern "C" {
    /// The runtime's exit (Relax run.rs): its at-exit work, then the end of the process without
    /// the exiting thread's fiber-local destructors (as unix's exit() skips the main thread's
    /// key destructors). ExitProcess would run them into JIT code during teardown.
    fn __relax_exit(code: i32);
}

pub fn exit(code: i32) -> ! {
    crate::io::cleanup();
    unsafe {
        __relax_exit(code);
        win::ExitProcess(code as u32)
    }
}

pub fn abort() -> ! {
    unsafe { crt_abort() }
}

// ---- Stdio

enum StdioKind {
    Inherit,
    Null,
    Piped,
    Handle(OwnedHandle),
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
        Stdio(StdioKind::Handle(OwnedHandle::from(f)))
    }
}

impl From<io::PipeReader> for Stdio {
    fn from(p: io::PipeReader) -> Stdio {
        Stdio::from(OwnedHandle::from(p))
    }
}
impl From<io::PipeWriter> for Stdio {
    fn from(p: io::PipeWriter) -> Stdio {
        Stdio::from(OwnedHandle::from(p))
    }
}
impl From<OwnedHandle> for Stdio {
    fn from(h: OwnedHandle) -> Stdio {
        Stdio(StdioKind::Handle(h))
    }
}

impl fmt::Debug for Stdio {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stdio").finish_non_exhaustive()
    }
}

// ---- ExitStatus / Output

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct ExitStatus(u32); // GetExitCodeProcess's code

impl ExitStatus {
    pub fn success(&self) -> bool {
        self.0 == 0
    }
    pub fn code(&self) -> Option<i32> {
        Some(self.0 as i32)
    }
    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        if self.success() {
            Ok(())
        } else {
            Err(ExitStatusError(*self))
        }
    }
    pub(crate) fn from_raw_code(raw: u32) -> ExitStatus {
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
        // real std: the public ExitStatus wraps sys's derived-Debug ExitStatus(u32)
        write!(f, "ExitStatus(ExitStatus({}))", self.0)
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // codes with the high bit set are NTSTATUS-style exceptions: hex is the searchable form
        if self.0 & 0x8000_0000 != 0 {
            write!(f, "exit code: {:#x}", self.0)
        } else {
            write!(f, "exit code: {}", self.0)
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct ExitStatusError(ExitStatus);

impl ExitStatusError {
    pub fn code(&self) -> Option<i32> {
        self.0.code()
    }
    pub fn into_status(&self) -> ExitStatus {
        self.0
    }
}

impl fmt::Debug for ExitStatusError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ExitStatusError(ExitStatusError({}))", self.0 .0)
    }
}

impl fmt::Display for ExitStatusError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "process exited unsuccessfully: {}", self.0)
    }
}

impl core::error::Error for ExitStatusError {}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitCode(u32);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);
    pub fn exit_process(self) -> ! {
        exit(self.0 as i32)
    }
    pub(crate) fn from_raw_code(raw: u32) -> ExitCode {
        ExitCode(raw)
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
        ExitCode(c as u32)
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

/// An argument as given: quoted for CommandLineToArgvW, or (CommandExt::raw_arg) appended as is.
#[derive(Clone)]
enum Arg {
    Regular(OsString),
    Raw(OsString),
}

impl Arg {
    fn as_os_str(&self) -> &OsStr {
        match self {
            Arg::Regular(s) => s,
            Arg::Raw(s) => s,
        }
    }
}

pub struct Command {
    program: OsString,
    args: Vec<Arg>,
    env_clear: bool,
    /// changed variables in the order first set; keys compare case-insensitively (EnvKey)
    env: Vec<(OsString, Option<OsString>)>,
    cwd: Option<OsString>,
    flags: u32,
    show_window: Option<u16>,
    force_quotes: bool,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

impl Command {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Command {
        Command { program: program.as_ref().to_os_string(), args: Vec::new(), env_clear: false, env: Vec::new(), cwd: None, flags: 0, show_window: None, force_quotes: false, stdin: None, stdout: None, stderr: None }
    }
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Command {
        self.args.push(Arg::Regular(arg.as_ref().to_os_string()));
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
    /// real std's CommandEnv::set/remove over a BTreeMap<EnvKey, _>: an existing key (compared
    /// case-insensitively) keeps its spelling and takes the new value.
    fn set_env(&mut self, key: OsString, val: Option<OsString>) {
        let mut i = 0;
        while i < self.env.len() {
            if env_key_eq(&self.env[i].0, &key) {
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
        CommandArgs { inner: self.args.iter() }
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
    pub(crate) fn set_creation_flags(&mut self, flags: u32) {
        self.flags = flags;
    }
    pub(crate) fn set_show_window(&mut self, cmd_show: Option<u16>) {
        self.show_window = cmd_show;
    }
    pub(crate) fn set_force_quotes(&mut self, enabled: bool) {
        self.force_quotes = enabled;
    }
    pub(crate) fn push_raw_arg(&mut self, text: &OsStr) {
        self.args.push(Arg::Raw(text.to_os_string()));
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        self.spawn_with(StdioKind::Inherit, true)
    }

    pub fn output(&mut self) -> io::Result<Output> {
        let mut child = self.spawn_with(StdioKind::Piped, false)?;
        drop(child.stdin.take());
        let (out, err) = read2(child.stdout.take(), child.stderr.take())?;
        let status = child.wait()?;
        Ok(Output { status, stdout: out, stderr: err })
    }

    pub fn status(&mut self) -> io::Result<ExitStatus> {
        let mut child = self.spawn_with(StdioKind::Inherit, true)?;
        child.wait()
    }

    /// real std's CommandEnv::capture_if_changed: None = the child inherits this environment;
    /// otherwise the full environment, sorted by the case-insensitive ordinal key order (the
    /// BTreeMap<EnvKey, OsString> real std builds), an existing key keeping its spelling.
    fn capture_env_if_changed(&self) -> Option<Vec<(OsString, OsString)>> {
        if !self.env_clear && self.env.is_empty() {
            return None;
        }
        let mut map: Vec<(OsString, OsString)> = Vec::new();
        if !self.env_clear {
            for (k, v) in crate::env::vars_os() {
                env_map_set(&mut map, k, Some(v));
            }
        }
        for (k, v) in &self.env {
            env_map_set(&mut map, k.clone(), v.clone());
        }
        map.sort_by(|a, b| env_key_cmp(&a.0, &b.0));
        Some(map)
    }

    /// Under `makepad run` the program's executable is Relax itself: spawning it runs this
    /// program again (Relax's command line up to `--`, then the new arguments). A spawn of the
    /// loader whose first argument is `run` or `app` is a launcher starting another program through
    /// it (`$MAKEPAD_RUN_LOADER run [flags] <package> ...`, `... app <id>`): that one runs as written. `program` is the
    /// resolved executable (NUL-terminated UTF-16, with its `.exe`); the result is the argument
    /// list after argv[0] to run instead of this command's.
    fn self_exec_args(&self, program: &[u16]) -> Option<Vec<Arg>> {
        let pre = crate::env::var("MAKEPAD_RUN_SELF_EXEC").ok()?;
        if self.args.first().is_some_and(|a| matches!(a.as_os_str().as_encoded_bytes(), b"run" | b"app")) {
            return None;
        }
        // inside a build script (MAKEPAD_RUN_AS_RUSTC): its spawns of this executable are RUSTC
        // questions, answered as written (relax_as_rustc); re-running the script would recurse
        if crate::env::var_os("MAKEPAD_RUN_AS_RUSTC").is_some() {
            return None;
        }
        let parts: Vec<&str> = pre.split('\u{1f}').collect();
        let exe = Path::new(parts[0]);
        let resolved = OsString::from_wide(strip_nul(program));
        let p = crate::fs::canonicalize(Path::new(&resolved)).ok()?;
        // the Relax executable itself, or a copy of it (makepad-cef re-execs the program from a
        // copy of its executable inside a synthetic app bundle): the copy runs this program
        if p.as_path() != exe && !same_file_contents(p.as_path(), exe) {
            return None;
        }
        let mut out: Vec<Arg> = Vec::new();
        for x in parts.iter().skip(1) {
            out.push(Arg::Regular(OsString::from(*x)));
        }
        for a in &self.args {
            out.push(a.clone());
        }
        Some(out)
    }

    /// A child started in a virtual folder of a git run (Relax's VFS) cannot get it as its
    /// working directory: it runs from the current one and finds its workspace through
    /// MAKEPAD_RUN_WORKSPACE + the mount table (MAKEPAD_RUN_VFS), as on unix (process_unix.rs).
    fn spawn_with(&mut self, default: StdioKind, needs_stdin: bool) -> io::Result<Child> {
        let Some(dir) = self.cwd.clone() else {
            return self.spawn_now(default, needs_stdin);
        };
        let saved_env = self.env.clone();
        let r = match crate::fs::vfs::kind(Path::new(&dir)) {
            Some((abs, _)) => {
                self.cwd = None;
                self.env("MAKEPAD_RUN_WORKSPACE", abs.as_os_str());
                self.env("MAKEPAD_RUN_VFS", crate::fs::vfs::mounts());
                let r = self.spawn_now(default, needs_stdin);
                self.cwd = Some(dir);
                r
            }
            None => {
                if crate::env::var_os("MAKEPAD_RUN_WORKSPACE").is_some() {
                    self.env_remove("MAKEPAD_RUN_WORKSPACE");
                }
                self.spawn_now(default, needs_stdin)
            }
        };
        self.env = saved_env;
        r
    }

    fn spawn_now(&mut self, default: StdioKind, needs_stdin: bool) -> io::Result<Child> {
        let maybe_env = self.capture_env_if_changed();
        // real std's CommandEnv::have_changed_path: PATH set or removed, or the environment cleared
        let mut env_saw_path = self.env_clear;
        for (k, _) in &self.env {
            if env_key_eq(k, OsStr::new("PATH")) {
                env_saw_path = true;
            }
        }
        let mut child_paths: Option<OsString> = None;
        if env_saw_path {
            if let Some(env) = &maybe_env {
                for (k, v) in env {
                    if env_key_eq(k, OsStr::new("PATH")) {
                        child_paths = Some(v.clone());
                    }
                }
            }
        }
        let program = resolve_exe(&self.program, child_paths.as_deref())?;
        let self_exec = self.self_exec_args(&program);
        let args: &[Arg] = match &self_exec {
            Some(v) => v,
            None => &self.args,
        };
        let is_batch_file = if is_verbatim(&program) {
            has_bat_extension(strip_nul(&program))
        } else {
            let full = win::fill_utf16_buf(|buf, n| unsafe { GetFullPathNameW(program.as_ptr(), n, buf, core::ptr::null_mut()) })?;
            has_bat_extension(&full)
        };
        let (program, mut cmd_str) = if is_batch_file {
            (command_prompt()?, make_bat_command_line(&program, args, self.force_quotes)?)
        } else {
            let cmd_str = make_command_line(&self.program, args, self.force_quotes)?;
            (program, cmd_str)
        };
        cmd_str.push(0);

        let flags = self.flags | CREATE_UNICODE_ENVIRONMENT;
        let mut env_block = make_envp(maybe_env)?;
        let envp: *mut c_void = match &mut env_block {
            Some(b) => b.as_mut_ptr() as *mut c_void,
            None => core::ptr::null_mut(),
        };
        let dir = make_dirp(self.cwd.as_ref())?;
        let dirp: *const u16 = match &dir {
            Some((buf, start)) => buf[*start..].as_ptr(),
            None => core::ptr::null(),
        };

        // The child's stdio handles are made inheritable here, and every handle inheritable
        // while CreateProcessW runs goes to that child: one spawn at a time
        // (https://support.microsoft.com/kb/315939).
        static CREATE_PROCESS_LOCK: crate::sync::Mutex<()> = crate::sync::Mutex::new(());
        let _guard = CREATE_PROCESS_LOCK.lock();

        // the configured stdio stays with the command (a second spawn uses it again)
        let null = StdioKind::Null;
        let stdin_kind: &StdioKind = match &self.stdin {
            Some(s) => &s.0,
            None => {
                if needs_stdin {
                    &default
                } else {
                    &null
                }
            }
        };
        let stdout_kind: &StdioKind = match &self.stdout {
            Some(s) => &s.0,
            None => &default,
        };
        let stderr_kind: &StdioKind = match &self.stderr {
            Some(s) => &s.0,
            None => &default,
        };
        let (stdin_child, stdin_parent) = to_handle(stdin_kind, win::STD_INPUT_HANDLE)?;
        let (stdout_child, stdout_parent) = to_handle(stdout_kind, win::STD_OUTPUT_HANDLE)?;
        let (stderr_child, stderr_parent) = to_handle(stderr_kind, win::STD_ERROR_HANDLE)?;

        let mut si = zeroed_startupinfo();
        // with any of the three set, all three go in (a null one stays null); with none set the
        // OS applies its default
        if stdin_child.is_some() || stdout_child.is_some() || stderr_child.is_some() {
            si.dwFlags |= STARTF_USESTDHANDLES;
            si.hStdInput = raw_or_null(&stdin_child);
            si.hStdOutput = raw_or_null(&stdout_child);
            si.hStdError = raw_or_null(&stderr_child);
        }
        if let Some(cmd_show) = self.show_window {
            si.dwFlags |= STARTF_USESHOWWINDOW;
            si.wShowWindow = cmd_show;
        }
        si.cb = core::mem::size_of::<STARTUPINFOW>() as u32;

        let mut pi = PROCESS_INFORMATION { hProcess: core::ptr::null_mut(), hThread: core::ptr::null_mut(), dwProcessId: 0, dwThreadId: 0 };
        let ok = unsafe { CreateProcessW(program.as_ptr(), cmd_str.as_mut_ptr(), core::ptr::null_mut(), core::ptr::null_mut(), 1, flags, envp, dirp, &mut si, &mut pi) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        drop(stdin_child);
        drop(stdout_child);
        drop(stderr_child);
        unsafe {
            Ok(Child {
                handle: OwnedHandle::from_raw_handle(pi.hProcess),
                main_thread: OwnedHandle::from_raw_handle(pi.hThread),
                stdin: match stdin_parent {
                    Some(h) => Some(ChildStdin { handle: h }),
                    None => None,
                },
                stdout: match stdout_parent {
                    Some(h) => Some(ChildStdout { handle: h }),
                    None => None,
                },
                stderr: match stderr_parent {
                    Some(h) => Some(ChildStderr { handle: h }),
                    None => None,
                },
            })
        }
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&*self.program, f)?;
        for a in &self.args {
            f.write_str(" ")?;
            match a {
                Arg::Regular(s) => fmt::Debug::fmt(&**s, f)?,
                Arg::Raw(s) => f.write_str(&s.to_string_lossy())?,
            }
        }
        Ok(())
    }
}

fn raw_or_null(h: &Option<OwnedHandle>) -> HANDLE {
    match h {
        Some(h) => h.as_raw_handle(),
        None => core::ptr::null_mut(),
    }
}

fn zeroed_startupinfo() -> STARTUPINFOW {
    STARTUPINFOW {
        cb: 0,
        lpReserved: core::ptr::null_mut(),
        lpDesktop: core::ptr::null_mut(),
        lpTitle: core::ptr::null_mut(),
        dwX: 0,
        dwY: 0,
        dwXSize: 0,
        dwYSize: 0,
        dwXCountChars: 0,
        dwYCountChars: 0,
        dwFillAttribute: 0,
        dwFlags: 0,
        wShowWindow: 0,
        cbReserved2: 0,
        lpReserved2: core::ptr::null_mut(),
        hStdInput: core::ptr::null_mut(),
        hStdOutput: core::ptr::null_mut(),
        hStdError: core::ptr::null_mut(),
    }
}

/// `h` duplicated into this process, inheritable.
fn dup_inheritable(h: HANDLE) -> io::Result<OwnedHandle> {
    let mut out: HANDLE = core::ptr::null_mut();
    let ok = unsafe { DuplicateHandle(GetCurrentProcess(), h, GetCurrentProcess(), &mut out, 0, 1, DUPLICATE_SAME_ACCESS) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(out) })
}

/// real std's Stdio::to_handle: (the child's handle, inheritable -- None = null; our end of a
/// pipe).
fn to_handle(kind: &StdioKind, stdio_id: u32) -> io::Result<(Option<OwnedHandle>, Option<OwnedHandle>)> {
    match kind {
        StdioKind::Inherit => {
            let h = unsafe { win::GetStdHandle(stdio_id) };
            if h.is_null() || h == win::INVALID_HANDLE_VALUE {
                // no stdio handle: the child gets null too
                return Ok((None, None));
            }
            Ok((Some(dup_inheritable(h)?), None))
        }
        StdioKind::Handle(h) => Ok((Some(dup_inheritable(h.as_raw_handle())?), None)),
        StdioKind::Piped => {
            let ours_readable = stdio_id != win::STD_INPUT_HANDLE;
            let (ours, theirs) = child_pipe(ours_readable)?;
            Ok((Some(theirs), Some(ours)))
        }
        StdioKind::Null => {
            // \\.\NUL, readable for stdin, writable otherwise, inheritable
            let name: Vec<u16> = r"\\.\NUL".encode_utf16().chain(core::iter::once(0)).collect();
            let sa = SECURITY_ATTRIBUTES { nLength: core::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: core::ptr::null_mut(), bInheritHandle: 1 };
            let access = if stdio_id == win::STD_INPUT_HANDLE { GENERIC_READ } else { GENERIC_WRITE };
            let h = unsafe { CreateFileW(name.as_ptr(), access, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, &sa, OPEN_EXISTING, 0, core::ptr::null_mut()) };
            if h == win::INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            Ok((Some(unsafe { OwnedHandle::from_raw_handle(h) }), None))
        }
    }
}

/// A pipe to a child: (ours, not inheritable; theirs, inheritable). `ours_readable`: we read
/// (the child's stdout / stderr) or write (its stdin). 64 KiB of buffer, as real std's.
fn child_pipe(ours_readable: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut sa = SECURITY_ATTRIBUTES { nLength: core::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: core::ptr::null_mut(), bInheritHandle: 1 };
    let mut r: HANDLE = win::INVALID_HANDLE_VALUE;
    let mut w: HANDLE = win::INVALID_HANDLE_VALUE;
    if unsafe { CreatePipe(&mut r, &mut w, &mut sa as *mut SECURITY_ATTRIBUTES as *mut c_void, 64 * 1024) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let (r, w) = unsafe { (OwnedHandle::from_raw_handle(r), OwnedHandle::from_raw_handle(w)) };
    let (ours, theirs) = if ours_readable { (r, w) } else { (w, r) };
    if unsafe { SetHandleInformation(ours.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((ours, theirs))
}

// ---- program resolution (real std's resolve_exe / search_paths / program_exists)

const fn is_sep_byte(b: u8) -> bool {
    b == b'/' || b == b'\\'
}

fn strip_nul(w: &[u16]) -> &[u16] {
    match w.last() {
        Some(&0) => &w[..w.len() - 1],
        _ => w,
    }
}

fn starts_with_ascii(w: &[u16], prefix: &str) -> bool {
    let p = prefix.as_bytes();
    if w.len() < p.len() {
        return false;
    }
    let mut i = 0;
    while i < p.len() {
        if w[i] != p[i] as u16 {
            return false;
        }
        i += 1;
    }
    true
}

fn is_verbatim(w: &[u16]) -> bool {
    starts_with_ascii(w, r"\\?\") || starts_with_ascii(w, r"\??\")
}

/// Case-insensitive "ends with .bat or .cmd" of a UTF-16 path (no NUL).
fn has_bat_extension(w: &[u16]) -> bool {
    if w.len() < 4 {
        return false;
    }
    let e = &w[w.len() - 4..];
    let lower = |c: u16| if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c };
    let ext = [lower(e[0]), lower(e[1]), lower(e[2]), lower(e[3])];
    ext == [b'.' as u16, b'b' as u16, b'a' as u16, b't' as u16] || ext == [b'.' as u16, b'c' as u16, b'm' as u16, b'd' as u16]
}

/// `dir` joined with the file name `name` (PathBuf::push for a relative file name: a separator
/// unless `dir` ends in one or is a bare drive `C:`).
fn join_dir(dir: &OsStr, name: &OsStr) -> OsString {
    let b = dir.as_encoded_bytes();
    let bare_drive = b.len() == 2 && b[1] == b':';
    let mut out = dir.to_os_string();
    if !b.is_empty() && !is_sep_byte(b[b.len() - 1]) && !bare_drive {
        out.push("\\");
    }
    out.push(name);
    out
}

/// Resolves the program to the executable CreateProcessW gets (NUL-terminated UTF-16): a path
/// is used as given (with `.exe` appended when that file exists), a bare file name is searched
/// in the child's PATH, the application's directory, the system directories and this process's
/// PATH, with `.exe` appended when it has no extension.
fn resolve_exe(exe_path: &OsStr, child_paths: Option<&OsStr>) -> io::Result<Vec<u16>> {
    let bytes = exe_path.as_encoded_bytes();
    let trailing_slash = match bytes.last() {
        Some(&c) => {
            if bytes.starts_with(br"\\?\") {
                c == b'\\'
            } else {
                is_sep_byte(c)
            }
        }
        None => false,
    };
    if bytes.is_empty() || trailing_slash {
        return Err(io::const_error!(io::ErrorKind::InvalidInput, "program path has no file name"));
    }
    let has_exe_suffix = bytes.len() >= EXE_SUFFIX.len() && bytes[bytes.len() - EXE_SUFFIX.len()..].eq_ignore_ascii_case(EXE_SUFFIX.as_bytes());
    let is_file_name = !bytes.iter().any(|&c| is_sep_byte(c));
    if !is_file_name {
        if has_exe_suffix {
            // a path to a .exe: CreateProcessW reports whether it exists
            return to_user_path(exe_path);
        }
        let mut with_exe = exe_path.to_os_string();
        with_exe.push(EXE_SUFFIX);
        if let Some(p) = program_exists(&with_exe) {
            return Ok(p);
        }
        return to_user_path(exe_path);
    }
    ensure_no_nuls(exe_path)?;
    // CreateProcessW: a file name without an extension gets .exe (only when searching)
    let has_extension = bytes.contains(&b'.');
    let exists = |dir: &OsStr| -> Option<Vec<u16>> {
        let mut p = join_dir(dir, exe_path);
        if !has_extension {
            p.push(EXE_SUFFIX);
        }
        program_exists(&p)
    };
    // 1. the child's PATH
    if let Some(paths) = child_paths {
        for dir in crate::env::split_paths(paths) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            if let Some(p) = exists(dir.as_os_str()) {
                return Ok(p);
            }
        }
    }
    // 2. the application's directory
    if let Ok(app) = crate::env::current_exe() {
        let b = app.as_os_str().as_encoded_bytes();
        let mut cut = b.len();
        while cut > 0 && !is_sep_byte(b[cut - 1]) {
            cut -= 1;
        }
        if cut > 0 {
            // PathBuf::pop: the directory (join_dir adds no second separator)
            let dir = unsafe { OsStr::from_encoded_bytes_unchecked(&b[..cut]) };
            if let Some(p) = exists(dir) {
                return Ok(p);
            }
        }
    }
    // 3 & 4. the system directories
    if let Ok(sys_dir) = win::fill_utf16_buf(|buf, n| unsafe { GetSystemDirectoryW(buf, n) }) {
        if let Some(p) = exists(&OsString::from_wide(&sys_dir)) {
            return Ok(p);
        }
    }
    if let Ok(win_dir) = win::fill_utf16_buf(|buf, n| unsafe { GetWindowsDirectoryW(buf, n) }) {
        if let Some(p) = exists(&OsString::from_wide(&win_dir)) {
            return Ok(p);
        }
    }
    // 5. this process's PATH
    if let Some(paths) = crate::env::var_os("PATH") {
        for dir in crate::env::split_paths(&paths) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            if let Some(p) = exists(dir.as_os_str()) {
                return Ok(p);
            }
        }
    }
    Err(io::const_error!(io::ErrorKind::NotFound, "program not found"))
}

/// The path (as to_user_path gives it) when a file is there (GetFileAttributesW, which does not
/// follow symlinks).
fn program_exists(path: &OsStr) -> Option<Vec<u16>> {
    let p = to_user_path(path).ok()?;
    if unsafe { GetFileAttributesW(p.as_ptr()) } == INVALID_FILE_ATTRIBUTES {
        None
    } else {
        Some(p)
    }
}

fn ensure_no_nuls(s: &OsStr) -> io::Result<()> {
    if s.encode_wide().any(|c| c == 0) {
        Err(io::const_error!(io::ErrorKind::InvalidInput, "nul byte found in provided data"))
    } else {
        Ok(())
    }
}

/// real std's to_user_path: the path as NUL-terminated UTF-16 without a verbatim prefix when
/// dropping it resolves to the same path (cmd.exe and CreateProcessW's search take no verbatim
/// paths); a long path gets one.
fn to_user_path(path: &OsStr) -> io::Result<Vec<u16>> {
    let mut path = win::to_wide(path)?;
    const LEGACY_MAX_PATH: usize = 260;
    if path.len() > LEGACY_MAX_PATH {
        return Ok(path);
    }
    let unc = starts_with_ascii(&path, r"\\?\UNC\");
    let drive = !unc && path.len() >= 7 && starts_with_ascii(&path, r"\\?\") && path[5] == b':' as u16 && path[6] == b'\\' as u16;
    if drive || unc {
        // `\\?\C:\...` -> `C:\...`; `\\?\UNC\...` -> `\\...` (the `C` of `UNC` becomes a `\`)
        let start = if unc { 6 } else { 4 };
        if unc {
            path[6] = b'\\' as u16;
        }
        let full = win::fill_utf16_buf(|buf, n| unsafe { GetFullPathNameW(path[start..].as_ptr(), n, buf, core::ptr::null_mut()) })?;
        if full.as_slice() == &path[start..path.len() - 1] {
            let mut out = full;
            out.push(0);
            return Ok(out);
        }
        if unc {
            path[6] = b'C' as u16;
        }
        return Ok(path);
    }
    get_long_path(path)
}

/// real std's get_long_path(path, prefer_verbatim = false): a path too long for the legacy APIs
/// made absolute and verbatim; anything else unchanged.
fn get_long_path(path: Vec<u16>) -> io::Result<Vec<u16>> {
    const LEGACY_MAX_PATH: usize = 248;
    let sep = |c: u16| c == b'\\' as u16 || c == b'/' as u16;
    if is_verbatim(&path) || path.as_slice() == [0] {
        return Ok(path);
    }
    if path.len() < LEGACY_MAX_PATH {
        // `D:`, `D:\...`, `D:/...`
        if path.len() >= 3 && path[1] == b':' as u16 && !sep(path[0]) && (path[2] == 0 || sep(path[2])) {
            return Ok(path);
        }
        // `\\...`
        if path.len() >= 2 && sep(path[0]) && sep(path[1]) {
            return Ok(path);
        }
    }
    let full = win::fill_utf16_buf(|buf, n| unsafe { GetFullPathNameW(path.as_ptr(), n, buf, core::ptr::null_mut()) })?;
    let mut out: Vec<u16> = Vec::new();
    if full.len() + 1 >= LEGACY_MAX_PATH {
        let mut abs: &[u16] = &full;
        if abs.len() >= 3 && abs[1] == b':' as u16 && abs[2] == b'\\' as u16 {
            out.extend(r"\\?\".encode_utf16());
        } else if starts_with_ascii(abs, r"\\.\") {
            abs = &abs[4..];
            out.extend(r"\\?\".encode_utf16());
        } else if starts_with_ascii(abs, r"\\?\") || starts_with_ascii(abs, r"\??\") {
        } else if starts_with_ascii(abs, r"\\") {
            abs = &abs[2..];
            out.extend(r"\\?\UNC\".encode_utf16());
        }
        out.extend_from_slice(abs);
    } else {
        out.extend_from_slice(&full);
    }
    out.push(0);
    Ok(out)
}

/// `GetFullPathNameW(path) == path` (real std's is_absolute_exact; `path` NUL-terminated).
fn is_absolute_exact(path: &[u16]) -> bool {
    if path.is_empty() || path[path.len() - 1] != 0 {
        return false;
    }
    match win::fill_utf16_buf(|buf, n| unsafe { GetFullPathNameW(path.as_ptr(), n, buf, core::ptr::null_mut()) }) {
        Ok(full) => full.as_slice() == &path[..path.len() - 1],
        Err(_) => false,
    }
}

// ---- command lines, environment block, directory

/// The command line (no NUL) CommandLineToArgvW splits back into argv0 and the arguments.
fn make_command_line(argv0: &OsStr, args: &[Arg], force_quotes: bool) -> io::Result<Vec<u16>> {
    let mut cmd: Vec<u16> = Vec::new();
    // the program name always quoted (file names cannot contain quotes)
    cmd.push(b'"' as u16);
    cmd.extend(argv0.encode_wide());
    cmd.push(b'"' as u16);
    for a in args {
        cmd.push(b' ' as u16);
        append_arg(&mut cmd, a, force_quotes)?;
    }
    Ok(cmd)
}

/// real std's append_arg: quoted when empty or holding a space or tab (always with force_quotes,
/// never for a raw argument); backslashes doubled before a quote, quotes escaped.
fn append_arg(cmd: &mut Vec<u16>, arg: &Arg, force_quotes: bool) -> io::Result<()> {
    let (s, quote, escape) = match arg {
        Arg::Regular(s) => {
            let b = s.as_encoded_bytes();
            let q = force_quotes || b.is_empty() || b.iter().any(|&c| c == b' ' || c == b'\t');
            (s, q, true)
        }
        Arg::Raw(s) => (s, false, false),
    };
    ensure_no_nuls(s)?;
    if quote {
        cmd.push(b'"' as u16);
    }
    let mut backslashes: usize = 0;
    for x in s.encode_wide() {
        if escape {
            if x == b'\\' as u16 {
                backslashes += 1;
            } else {
                if x == b'"' as u16 {
                    // n+1 more backslashes: 2n+1 before an inner quote
                    for _ in 0..=backslashes {
                        cmd.push(b'\\' as u16);
                    }
                }
                backslashes = 0;
            }
        }
        cmd.push(x);
    }
    if quote {
        // n more backslashes: 2n before the closing quote
        for _ in 0..backslashes {
            cmd.push(b'\\' as u16);
        }
        cmd.push(b'"' as u16);
    }
    Ok(())
}

/// real std's append_bat_arg: cmd.exe's quoting, `%` defused as `%%cd:~,%`.
fn append_bat_arg(cmd: &mut Vec<u16>, arg: &OsStr, mut quote: bool) -> io::Result<()> {
    ensure_no_nuls(arg)?;
    let b = arg.as_encoded_bytes();
    if b.is_empty() || b[b.len() - 1] == b'\\' {
        quote = true;
    }
    for u in arg.encode_wide() {
        // ASCII outside the known-good set, and C1 controls (surrogates are never controls)
        if u < 0x80 {
            let c = u as u8;
            if !(c.is_ascii_alphanumeric() || br"#$*+-./:?@\_".contains(&c)) {
                quote = true;
            }
        } else if u <= 0x9f {
            quote = true;
        }
    }
    if quote {
        cmd.push(b'"' as u16);
    }
    let mut backslashes: usize = 0;
    for x in arg.encode_wide() {
        if x == b'\\' as u16 {
            backslashes += 1;
        } else {
            if x == b'"' as u16 {
                for _ in 0..backslashes {
                    cmd.push(b'\\' as u16);
                }
                cmd.push(b'"' as u16);
            } else if x == b'%' as u16 || x == b'\r' as u16 {
                cmd.extend("%%cd:~,".encode_utf16());
            }
            backslashes = 0;
        }
        cmd.push(x);
    }
    if quote {
        for _ in 0..backslashes {
            cmd.push(b'\\' as u16);
        }
        cmd.push(b'"' as u16);
    }
    Ok(())
}

/// real std's make_bat_command_line: `cmd.exe /e:ON /v:OFF /d /c ""script" args..."`.
fn make_bat_command_line(script: &[u16], args: &[Arg], force_quotes: bool) -> io::Result<Vec<u16>> {
    let mut cmd: Vec<u16> = "cmd.exe /e:ON /v:OFF /d /c \"".encode_utf16().collect();
    cmd.push(b'"' as u16);
    let script = strip_nul(script);
    if script.contains(&(b'"' as u16)) || script.last() == Some(&(b'\\' as u16)) {
        return Err(io::const_error!(io::ErrorKind::InvalidInput, "Windows file names may not contain `\"` or end with `\\`"));
    }
    cmd.extend_from_slice(script);
    cmd.push(b'"' as u16);
    for a in args {
        cmd.push(b' ' as u16);
        match a {
            Arg::Regular(s) => {
                // \r and \n could truncate the arguments
                if s.as_encoded_bytes().iter().any(|&c| c == b'\r' || c == b'\n') {
                    return Err(io::const_error!(io::ErrorKind::InvalidInput, "batch file arguments are invalid"));
                }
                append_bat_arg(&mut cmd, s, force_quotes)?;
            }
            Arg::Raw(_) => append_arg(&mut cmd, a, force_quotes)?,
        }
    }
    cmd.push(b'"' as u16);
    Ok(cmd)
}

/// `<system directory>\cmd.exe`, NUL-terminated.
fn command_prompt() -> io::Result<Vec<u16>> {
    let mut system = win::fill_utf16_buf(|buf, n| unsafe { GetSystemDirectoryW(buf, n) })?;
    system.extend("\\cmd.exe".encode_utf16());
    system.push(0);
    Ok(system)
}

/// The environment block: `k=v\0` per variable, then a final `\0` (an empty block is `\0\0`).
fn make_envp(env: Option<Vec<(OsString, OsString)>>) -> io::Result<Option<Vec<u16>>> {
    let env = match env {
        Some(e) => e,
        None => return Ok(None),
    };
    let mut blk: Vec<u16> = Vec::new();
    if env.is_empty() {
        blk.push(0);
    }
    for (k, v) in env {
        ensure_no_nuls(&k)?;
        blk.extend(k.encode_wide());
        blk.push(b'=' as u16);
        ensure_no_nuls(&v)?;
        blk.extend(v.encode_wide());
        blk.push(0);
    }
    blk.push(0);
    Ok(Some(blk))
}

/// The child's working directory, NUL-terminated, and where it starts: a verbatim prefix is
/// skipped when the rest names the same directory (current directories cannot be verbatim).
fn make_dirp(d: Option<&OsString>) -> io::Result<Option<(Vec<u16>, usize)>> {
    let dir = match d {
        Some(d) => d,
        None => return Ok(None),
    };
    ensure_no_nuls(dir)?;
    let mut w: Vec<u16> = dir.encode_wide().collect();
    w.push(0);
    if starts_with_ascii(&w, r"\\?\UNC") {
        let start = r"\\?\UN".len();
        w[start] = b'\\' as u16;
        if is_absolute_exact(&w[start..]) {
            return Ok(Some((w, start)));
        }
        w[start] = b'C' as u16;
        return Ok(Some((w, 0)));
    }
    if starts_with_ascii(&w, r"\\?\") {
        let start = r"\\?\".len();
        if is_absolute_exact(&w[start..]) {
            return Ok(Some((w, start)));
        }
    }
    Ok(Some((w, 0)))
}

/// real std's EnvKey order: CompareStringOrdinal ignoring case on the UTF-16 keys.
fn env_key_cmp(a: &OsStr, b: &OsStr) -> core::cmp::Ordering {
    let a: Vec<u16> = a.encode_wide().collect();
    let b: Vec<u16> = b.encode_wide().collect();
    match unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) } {
        CSTR_LESS_THAN => core::cmp::Ordering::Less,
        CSTR_EQUAL => core::cmp::Ordering::Equal,
        CSTR_GREATER_THAN => core::cmp::Ordering::Greater,
        _ => panic!("comparing environment keys failed: {}", io::Error::last_os_error()),
    }
}

fn env_key_eq(a: &OsStr, b: &OsStr) -> bool {
    a.encode_wide().count() == b.encode_wide().count() && env_key_cmp(a, b) == core::cmp::Ordering::Equal
}

/// BTreeMap insert / remove on a Vec map: an existing key keeps its spelling.
fn env_map_set(map: &mut Vec<(OsString, OsString)>, k: OsString, v: Option<OsString>) {
    let mut i = 0;
    while i < map.len() {
        if env_key_eq(&map[i].0, &k) {
            match v {
                Some(v) => map[i].1 = v,
                None => {
                    map.remove(i);
                }
            }
            return;
        }
        i += 1;
    }
    if let Some(v) = v {
        map.push((k, v));
    }
}

/// Whether two files have the same bytes (a copy of an executable).
fn same_file_contents(a: &Path, b: &Path) -> bool {
    match (crate::fs::metadata(a), crate::fs::metadata(b)) {
        (Ok(x), Ok(y)) if x.len() == y.len() => {}
        _ => return false,
    }
    match (crate::fs::read(a), crate::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Reads both pipes to the end without deadlocking: stderr on a second thread while this one
/// reads stdout (the pipes are synchronous; real std uses overlapped reads on both).
fn read2(out: Option<ChildStdout>, err: Option<ChildStderr>) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    match (out, err) {
        (None, None) => {}
        (Some(mut o), None) => {
            o.read_to_end(&mut out_buf)?;
        }
        (None, Some(mut e)) => {
            e.read_to_end(&mut err_buf)?;
        }
        (Some(mut o), Some(mut e)) => {
            let t = crate::thread::spawn(move || -> io::Result<Vec<u8>> {
                let mut v = Vec::new();
                e.read_to_end(&mut v)?;
                Ok(v)
            });
            let r = o.read_to_end(&mut out_buf);
            drop(o);
            match t.join() {
                Ok(Ok(v)) => err_buf = v,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(io::const_error!(io::ErrorKind::Other, "the stderr reader thread panicked")),
            }
            r?;
        }
    }
    Ok((out_buf, err_buf))
}

// ---- Child

pub struct Child {
    handle: OwnedHandle,
    main_thread: OwnedHandle,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl Child {
    pub fn id(&self) -> u32 {
        unsafe { GetProcessId(self.handle.as_raw_handle()) }
    }
    pub fn kill(&mut self) -> io::Result<()> {
        if unsafe { TerminateProcess(self.handle.as_raw_handle(), 1) } == 0 {
            let error = io::Error::last_os_error();
            // ERROR_ACCESS_DENIED when the process has already ended (by us or otherwise): not
            // an error when it has
            if error.raw_os_error() != Some(win::ERROR_ACCESS_DENIED as i32) || self.try_wait().is_err() {
                return Err(error);
            }
        }
        Ok(())
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        unsafe {
            if win::WaitForSingleObject(self.handle.as_raw_handle(), win::INFINITE) != WAIT_OBJECT_0 {
                return Err(io::Error::last_os_error());
            }
            let mut status: u32 = 0;
            if GetExitCodeProcess(self.handle.as_raw_handle(), &mut status) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(ExitStatus(status))
        }
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        unsafe {
            let r = win::WaitForSingleObject(self.handle.as_raw_handle(), 0);
            if r == win::WAIT_TIMEOUT {
                return Ok(None);
            }
            if r != WAIT_OBJECT_0 {
                return Err(io::Error::last_os_error());
            }
            let mut status: u32 = 0;
            if GetExitCodeProcess(self.handle.as_raw_handle(), &mut status) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(ExitStatus(status)))
        }
    }
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        let (out, err) = read2(self.stdout.take(), self.stderr.take())?;
        let status = self.wait()?;
        Ok(Output { status, stdout: out, stderr: err })
    }
    pub(crate) fn process_handle(&self) -> &OwnedHandle {
        &self.handle
    }
    pub(crate) fn main_thread(&self) -> &OwnedHandle {
        &self.main_thread
    }
    pub(crate) fn into_process_handle(self) -> OwnedHandle {
        self.handle
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
    handle: OwnedHandle,
}
pub struct ChildStdout {
    handle: OwnedHandle,
}
pub struct ChildStderr {
    handle: OwnedHandle,
}

/// ReadFile; a closed write end reads as end of file (real std's pipe read).
fn handle_read(h: &OwnedHandle, buf: &mut [u8]) -> io::Result<usize> {
    let len = if buf.len() > u32::MAX as usize { u32::MAX } else { buf.len() as u32 };
    let mut n: u32 = 0;
    if unsafe { win::ReadFile(h.as_raw_handle(), buf.as_mut_ptr(), len, &mut n, core::ptr::null_mut()) } == 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::BrokenPipe {
            return Ok(0);
        }
        return Err(e);
    }
    Ok(n as usize)
}

fn handle_write(h: &OwnedHandle, buf: &[u8]) -> io::Result<usize> {
    let len = if buf.len() > u32::MAX as usize { u32::MAX } else { buf.len() as u32 };
    let mut n: u32 = 0;
    if unsafe { win::WriteFile(h.as_raw_handle(), buf.as_ptr(), len, &mut n, core::ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

impl Write for ChildStdin {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        handle_write(&self.handle, buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Read for ChildStdout {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        handle_read(&self.handle, buf)
    }
}
impl Read for ChildStderr {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        handle_read(&self.handle, buf)
    }
}

impl ChildStdin {
    pub(crate) fn handle(&self) -> &OwnedHandle {
        &self.handle
    }
    pub(crate) fn into_handle(self) -> OwnedHandle {
        self.handle
    }
    pub(crate) fn from_handle(handle: OwnedHandle) -> ChildStdin {
        ChildStdin { handle }
    }
}
impl ChildStdout {
    pub(crate) fn handle(&self) -> &OwnedHandle {
        &self.handle
    }
    pub(crate) fn into_handle(self) -> OwnedHandle {
        self.handle
    }
    pub(crate) fn from_handle(handle: OwnedHandle) -> ChildStdout {
        ChildStdout { handle }
    }
}
impl ChildStderr {
    pub(crate) fn handle(&self) -> &OwnedHandle {
        &self.handle
    }
    pub(crate) fn into_handle(self) -> OwnedHandle {
        self.handle
    }
    pub(crate) fn from_handle(handle: OwnedHandle) -> ChildStderr {
        ChildStderr { handle }
    }
}

impl From<ChildStdin> for Stdio {
    fn from(c: ChildStdin) -> Stdio {
        Stdio(StdioKind::Handle(c.handle))
    }
}
impl From<ChildStdout> for Stdio {
    fn from(c: ChildStdout) -> Stdio {
        Stdio(StdioKind::Handle(c.handle))
    }
}
impl From<ChildStderr> for Stdio {
    fn from(c: ChildStderr) -> Stdio {
        Stdio(StdioKind::Handle(c.handle))
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

/// Iterator over a Command's arguments (Command::get_args).
pub struct CommandArgs<'a> {
    inner: core::slice::Iter<'a, Arg>,
}

impl<'a> Iterator for CommandArgs<'a> {
    type Item = &'a OsStr;
    fn next(&mut self) -> Option<&'a OsStr> {
        self.inner.next().map(|a| a.as_os_str())
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
        f.debug_list().entries(self.inner.clone().map(|a| a.as_os_str())).finish()
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
