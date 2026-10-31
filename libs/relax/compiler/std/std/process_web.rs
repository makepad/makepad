//! std::process for the web targets (rustc's sys/process/unsupported.rs): there are no processes.
//! A `Command` is built and printed as usual, but spawning one is `Err(Unsupported)`; a `Child`
//! can never exist, nor can an `ExitStatusError` or a pipe end. Same public API as process_unix.rs /
//! process_windows.rs.

use alloc::vec::Vec;
use core::fmt;

use crate::collections::BTreeMap;
use crate::ffi::{OsStr, OsString};
use crate::fs::File;
use crate::io::{self, Read, Write};
use crate::path::Path;

/// rustc's `getpid` on this target: there are no process ids.
pub fn id() -> u32 {
    panic!("no pids on this platform")
}

/// rustc's `sys::exit::exit` for a target without one: the process aborts.
pub fn exit(code: i32) -> ! {
    crate::io::cleanup();
    let _ = code;
    unsafe { core::intrinsics_rt::abort() }
}

pub fn abort() -> ! {
    unsafe { core::intrinsics_rt::abort() }
}

// ---- Stdio

#[derive(Debug)]
enum StdioKind {
    Inherit,
    Null,
    MakePipe,
    ParentStdout,
    ParentStderr,
    #[allow(dead_code)] // exists only for the Debug impl
    InheritFile(File),
}

pub struct Stdio(StdioKind);

impl fmt::Debug for Stdio {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stdio").finish_non_exhaustive()
    }
}

impl Stdio {
    pub fn piped() -> Stdio {
        Stdio(StdioKind::MakePipe)
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
        Stdio(StdioKind::InheritFile(f))
    }
}
impl From<io::Stdout> for Stdio {
    fn from(_: io::Stdout) -> Stdio {
        Stdio(StdioKind::ParentStdout)
    }
}
impl From<io::Stderr> for Stdio {
    fn from(_: io::Stderr) -> Stdio {
        Stdio(StdioKind::ParentStderr)
    }
}
impl From<io::PipeReader> for Stdio {
    fn from(p: io::PipeReader) -> Stdio {
        p.diverge()
    }
}
impl From<io::PipeWriter> for Stdio {
    fn from(p: io::PipeWriter) -> Stdio {
        p.diverge()
    }
}

// ---- ExitStatus / Output

/// rustc's `ExitStatus()`: a dummy, `success` and code 0.
#[derive(PartialEq, Eq, Clone, Copy, Default)]
pub struct ExitStatus();

impl ExitStatus {
    pub fn success(&self) -> bool {
        true
    }
    pub fn code(&self) -> Option<i32> {
        Some(0)
    }
    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        Ok(())
    }
}

// std::process::ExitStatus(imp::ExitStatus) derives Debug over the unit-like inner struct
impl fmt::Debug for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("ExitStatus(ExitStatus)")
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<dummy exit status>")
    }
}

pub struct ExitStatusError(!);

impl Clone for ExitStatusError {
    fn clone(&self) -> ExitStatusError {
        self.0
    }
}
impl Copy for ExitStatusError {}
impl PartialEq for ExitStatusError {
    fn eq(&self, _other: &ExitStatusError) -> bool {
        self.0
    }
}
impl Eq for ExitStatusError {}
impl fmt::Debug for ExitStatusError {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

impl ExitStatusError {
    pub fn code(&self) -> Option<i32> {
        self.0
    }
    pub fn into_status(&self) -> ExitStatus {
        self.0
    }
}

impl fmt::Display for ExitStatusError {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        self.0
    }
}

impl core::error::Error for ExitStatusError {}

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct ExitCode(u8);

// std::process::ExitCode(imp::ExitCode) derives Debug over rustc's derived `ExitCode(u8)`
impl fmt::Debug for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        struct Platform(u8);
        impl fmt::Debug for Platform {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.debug_tuple("ExitCode").field(&self.0).finish()
            }
        }
        f.debug_tuple("ExitCode").field(&Platform(self.0)).finish()
    }
}

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

#[derive(Clone, Default)]
struct CommandEnv {
    clear: bool,
    vars: BTreeMap<OsString, Option<OsString>>,
}

impl fmt::Debug for CommandEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandEnv").field("clear", &self.clear).field("vars", &self.vars).finish()
    }
}

pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    env: CommandEnv,
    cwd: Option<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

impl Command {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Command {
        let program = program.as_ref().to_os_string();
        Command { args: alloc::vec![program.clone()], program, env: CommandEnv::default(), cwd: None, stdin: None, stdout: None, stderr: None }
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
        self.env.vars.insert(key.as_ref().to_os_string(), Some(val.as_ref().to_os_string()));
        self
    }
    pub fn envs<I: IntoIterator<Item = (K, V)>, K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, vars: I) -> &mut Command {
        for (k, v) in vars {
            self.env(k, v);
        }
        self
    }
    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Command {
        let key = key.as_ref().to_os_string();
        if self.env.clear {
            self.env.vars.remove(&key);
        } else {
            self.env.vars.insert(key, None);
        }
        self
    }
    pub fn env_clear(&mut self) -> &mut Command {
        self.env.clear = true;
        self.env.vars.clear();
        self
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
    /// The variables set or removed explicitly, in key order.
    pub fn get_envs(&self) -> CommandEnvs<'_> {
        CommandEnvs { iter: self.env.vars.iter() }
    }
    pub fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_ref().map(|d| Path::new(d))
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        Err(io::Error::UNSUPPORTED_PLATFORM)
    }

    pub fn output(&mut self) -> io::Result<Output> {
        Err(io::Error::UNSUPPORTED_PLATFORM)
    }

    pub fn status(&mut self) -> io::Result<ExitStatus> {
        Err(io::Error::UNSUPPORTED_PLATFORM)
    }
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
#[derive(Debug)]
pub struct CommandEnvs<'a> {
    iter: crate::collections::btree_map::Iter<'a, OsString, Option<OsString>>,
}

impl<'a> Iterator for CommandEnvs<'a> {
    type Item = (&'a OsStr, Option<&'a OsStr>);
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(key, value)| (key.as_os_str(), value.as_deref()))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for CommandEnvs<'a> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl fmt::Debug for Command {
    // rustc's sys/process/unsupported.rs: every attribute
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            let mut debug_command = f.debug_struct("Command");
            debug_command.field("program", &self.program).field("args", &self.args);
            if !(!self.env.clear && self.env.vars.is_empty()) {
                debug_command.field("env", &self.env);
            }
            if self.cwd.is_some() {
                debug_command.field("cwd", &self.cwd);
            }
            if self.stdin.is_some() {
                debug_command.field("stdin", &self.stdin.as_ref().map(|s| &s.0));
            }
            if self.stdout.is_some() {
                debug_command.field("stdout", &self.stdout.as_ref().map(|s| &s.0));
            }
            if self.stderr.is_some() {
                debug_command.field("stderr", &self.stderr.as_ref().map(|s| &s.0));
            }
            debug_command.finish()
        } else {
            if let Some(ref cwd) = self.cwd {
                write!(f, "cd {cwd:?} && ")?;
            }
            if self.env.clear {
                write!(f, "env -i ")?;
            } else {
                // removed env vars need the command to be wrapped in `env`
                let mut any_removed = false;
                for (key, value_opt) in self.get_envs() {
                    if value_opt.is_none() {
                        if !any_removed {
                            write!(f, "env ")?;
                            any_removed = true;
                        }
                        write!(f, "-u {} ", key.to_string_lossy())?;
                    }
                }
            }
            // altered env vars can just be added in front of the program
            for (key, value_opt) in self.get_envs() {
                if let Some(value) = value_opt {
                    write!(f, "{}={value:?} ", key.to_string_lossy())?;
                }
            }
            if self.program != self.args[0] {
                write!(f, "[{:?}] ", self.program)?;
            }
            write!(f, "{:?}", self.args[0])?;
            for arg in &self.args[1..] {
                write!(f, " {:?}", arg)?;
            }
            Ok(())
        }
    }
}

// ---- Child: none can exist

pub struct Child {
    status: !,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl Child {
    pub fn id(&self) -> u32 {
        self.status
    }
    pub fn kill(&mut self) -> io::Result<()> {
        self.status
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.status
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.status
    }
    pub fn wait_with_output(self) -> io::Result<Output> {
        self.status
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

pub struct ChildStdin(io::PipeWriter);
pub struct ChildStdout(io::PipeReader);
pub struct ChildStderr(io::PipeReader);

impl Write for ChildStdin {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl Read for ChildStdout {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}
impl Read for ChildStderr {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl From<ChildStdin> for Stdio {
    fn from(c: ChildStdin) -> Stdio {
        Stdio::from(c.0)
    }
}
impl From<ChildStdout> for Stdio {
    fn from(c: ChildStdout) -> Stdio {
        Stdio::from(c.0)
    }
}
impl From<ChildStderr> for Stdio {
    fn from(c: ChildStderr) -> Stdio {
        Stdio::from(c.0)
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
