//! Darwin PTY spawning without running any code between fork and exec.
//!
//! Shared with makepad-screen by source path so its tiny host stays GUI-free.
//! Darwin's opaque spawn types/flags are defined by the SDK's spawn.h.
use std::{
    collections::BTreeMap,
    ffi::{c_char, c_void, CString, OsString},
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    ptr,
    time::{Duration, Instant},
};

pub struct Child {
    pid: i32,
    status: Option<ExitStatus>,
}

impl Child {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_with(1) // WNOHANG
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wait_with(0)?
            .ok_or_else(|| io::Error::other("waitpid returned no child"))
    }

    fn wait_with(&mut self, options: i32) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        loop {
            let mut status = 0;
            let result = unsafe { waitpid(self.pid, &mut status, options) };
            if result == self.pid {
                self.status = Some(ExitStatus::from_raw(status));
                return Ok(self.status);
            }
            if result == 0 {
                return Ok(None);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    pub fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        if unsafe { kill(self.pid, 9) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

fn checked(result: i32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}

fn cstring(value: &std::ffi::OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "spawn argument contains NUL"))
}

fn executable(
    command: &Command,
    environment: &BTreeMap<OsString, OsString>,
    cwd: &Path,
) -> io::Result<PathBuf> {
    let program = command.get_program();
    if program.as_bytes().contains(&b'/') {
        return Ok(cwd.join(program));
    }
    let path = environment
        .get(std::ffi::OsStr::new("PATH"))
        .map(OsString::as_os_str)
        .unwrap_or_else(|| std::ffi::OsStr::new("/usr/bin:/bin"));
    let mut denied = false;
    for directory in std::env::split_paths(path) {
        let candidate = cwd.join(directory).join(program);
        if let Ok(metadata) = candidate.metadata() {
            if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                return Ok(candidate);
            }
            denied = true;
        }
    }
    Err(io::Error::from_raw_os_error(if denied { 13 } else { 2 }))
}

/// Locate the repository helper next to the application. Cargo test binaries
/// live one level down in `deps`; they use the same profile's real helper.
pub fn screen_helper() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("executable has no directory"))?;
    let directory = if directory.file_name() == Some(std::ffi::OsStr::new("deps")) {
        directory.parent().unwrap_or(directory)
    } else {
        directory
    };
    Ok(directory.join("makepad-screen"))
}

/// Start a new session with `slave` as its controlling terminal and stdio.
/// The Command supplies only program, argv, cwd and environment; stdio is
/// deliberately supplied by file actions, never by a pre_exec callback.
/// Callers inherit the environment; env_clear/uid/gid/arg0/pre_exec options
/// are not part of this API. Explicit env/env_remove changes are supported.
pub fn spawn(command: &Command, slave: i32, helper: &Path) -> io::Result<Child> {
    let mut name = [0 as c_char; 1024];
    checked(unsafe { ttyname_r(slave, name.as_mut_ptr(), name.len()) })?;
    let mut environment: BTreeMap<_, _> = std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            environment.insert(key.to_owned(), value.to_owned());
        } else {
            environment.remove(key);
        }
    }
    let cwd = std::env::current_dir()?.join(command.get_current_dir().unwrap_or(Path::new(".")));
    let program = cstring(executable(command, &environment, &cwd)?.as_os_str())?;
    let cwd = cstring(cwd.as_os_str())?;
    let helper = cstring(std::env::current_dir()?.join(helper).as_os_str())?;
    let mut args = vec![helper.clone(), CString::new("--exec-pty").unwrap(), program];
    args.extend(
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(cstring)
            .collect::<io::Result<Vec<_>>>()?,
    );
    let (mut completion, completed) = completion_pipe()?;
    let env = environment
        .into_iter()
        .map(|(mut key, value)| {
            key.push("=");
            key.push(value);
            cstring(&key)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let argv: Vec<_> = args
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .chain(std::iter::once(ptr::null_mut()))
        .collect();
    let envp: Vec<_> = env
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .chain(std::iter::once(ptr::null_mut()))
        .collect();

    struct Actions(*mut c_void);
    impl Drop for Actions {
        fn drop(&mut self) {
            unsafe {
                posix_spawn_file_actions_destroy(&mut self.0);
            }
        }
    }
    struct Attributes(*mut c_void);
    impl Drop for Attributes {
        fn drop(&mut self) {
            unsafe {
                posix_spawnattr_destroy(&mut self.0);
            }
        }
    }
    let mut actions = ptr::null_mut();
    checked(unsafe { posix_spawn_file_actions_init(&mut actions) })?;
    let mut actions = Actions(actions);
    let mut attrs = ptr::null_mut();
    checked(unsafe { posix_spawnattr_init(&mut attrs) })?;
    let mut attrs = Attributes(attrs);
    let mut pid = 0;
    unsafe {
        // SETSID runs before file actions. Darwin still requires TIOCSCTTY:
        // the tiny exec helper performs it after exec, then execs the target
        // in this same PID/session. The GUI process never calls fork.
        checked(posix_spawnattr_setflags(
            &mut attrs.0,
            0x0400 | 0x4000 | 0x0004 | 0x0008,
        ))?;
        checked(posix_spawnattr_setsigdefault(&mut attrs.0, &0xffff_ffff))?;
        checked(posix_spawnattr_setsigmask(&mut attrs.0, &0))?;
        checked(posix_spawn_file_actions_addchdir_np(
            &mut actions.0,
            cwd.as_ptr(),
        ))?;
        checked(posix_spawn_file_actions_addopen(
            &mut actions.0,
            0,
            name.as_ptr(),
            2,
            0,
        ))?;
        checked(posix_spawn_file_actions_adddup2(&mut actions.0, 0, 1))?;
        checked(posix_spawn_file_actions_adddup2(&mut actions.0, 0, 2))?;
        checked(posix_spawn_file_actions_adddup2(
            &mut actions.0,
            completed.as_raw_fd(),
            3,
        ))?;
        checked(posix_spawn(
            &mut pid,
            helper.as_ptr(),
            &actions.0,
            &attrs.0,
            argv.as_ptr(),
            envp.as_ptr(),
        ))?;
    }
    drop(completed);
    let mut child = Child { pid, status: None };
    if let Err(error) = wait_for_exec(&mut completion) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}

fn completion_pipe() -> io::Result<(File, File)> {
    let mut descriptors = [-1; 2];
    if unsafe { pipe(descriptors.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let reader = unsafe { File::from_raw_fd(descriptors[0]) };
    let writer = unsafe { File::from_raw_fd(descriptors[1]) };
    for fd in descriptors {
        if unsafe { fcntl(fd, 2, 1) } == -1 {
            // F_SETFD, FD_CLOEXEC
            return Err(io::Error::last_os_error());
        }
    }
    Ok((reader, writer))
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

fn wait_for_exec(pipe: &mut File) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = [0; 8];
    let mut length = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "PTY exec helper did not respond",
            ));
        }
        let mut fd = PollFd {
            fd: pipe.as_raw_fd(),
            events: 1,
            revents: 0,
        };
        if unsafe {
            poll(
                &mut fd,
                1,
                remaining.as_millis().min(i32::MAX as u128) as i32,
            )
        } < 0
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fd.revents == 0 {
            continue;
        }
        match pipe.read(&mut bytes[length..]) {
            // A ready marker proves the helper actually ran. EOF then proves
            // its close-on-exec descriptor closed at the target exec.
            Ok(0) if length == 4 && bytes[..4] == [0; 4] => return Ok(()),
            Ok(0) => {
                return Err(io::Error::other(
                    "PTY exec helper closed before setup completed",
                ))
            }
            Ok(count) => {
                length += count;
                if length >= 4 {
                    let error = i32::from_ne_bytes(bytes[..4].try_into().unwrap());
                    if error != 0 {
                        return Err(io::Error::from_raw_os_error(error));
                    }
                }
                if length == 8 {
                    return Err(io::Error::from_raw_os_error(i32::from_ne_bytes(
                        bytes[4..].try_into().unwrap(),
                    )));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Called at makepad-screen entry, before any host, socket, worker or UI.
/// Returns for ordinary invocations; exec mode never returns.
pub fn exec_helper() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--exec-pty")) {
        return;
    }
    let result = (|| -> io::Result<()> {
        let program = cstring(
            &arguments
                .next()
                .ok_or_else(|| io::Error::other("missing PTY program"))?,
        )?;
        let args = arguments
            .map(|s| cstring(&s))
            .collect::<io::Result<Vec<_>>>()?;
        if args.is_empty() {
            return Err(io::Error::other("missing PTY argv"));
        }
        let argv: Vec<_> = args
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(ptr::null()))
            .collect();
        if unsafe { fcntl(3, 2, 1) } == -1 || unsafe { ioctl(0, 0x20007461, 0) } == -1 {
            return Err(io::Error::last_os_error());
        }
        // This marker distinguishes successful exec from a helper that exits
        // without recognizing its protocol (e.g. an old sibling binary).
        let mut completion = unsafe { File::from_raw_fd(3) };
        completion.write_all(&0i32.to_ne_bytes())?;
        unsafe {
            execve(program.as_ptr(), argv.as_ptr(), *_NSGetEnviron());
        }
        let error = io::Error::last_os_error();
        completion.write_all(&error.raw_os_error().unwrap_or(5).to_ne_bytes())?;
        Err(error)
    })();
    if let Err(error) = result {
        let code = error.raw_os_error().unwrap_or(5).to_ne_bytes();
        unsafe {
            write(3, code.as_ptr().cast(), code.len());
        }
        eprintln!("PTY exec failed: {error}");
    }
    std::process::exit(127);
}

extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn fcntl(fd: i32, command: i32, ...) -> i32;
    fn ioctl(fd: i32, request: usize, ...) -> i32;
    fn poll(fds: *mut PollFd, count: u32, timeout: i32) -> i32;
    fn write(fd: i32, bytes: *const c_void, count: usize) -> isize;
    fn execve(path: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> i32;
    fn _NSGetEnviron() -> *mut *const *const c_char;
    fn ttyname_r(fd: i32, name: *mut c_char, len: usize) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn posix_spawn_file_actions_init(actions: *mut *mut c_void) -> i32;
    fn posix_spawn_file_actions_destroy(actions: *mut *mut c_void) -> i32;
    fn posix_spawn_file_actions_addchdir_np(actions: *mut *mut c_void, path: *const c_char) -> i32;
    fn posix_spawn_file_actions_addopen(
        actions: *mut *mut c_void,
        fd: i32,
        path: *const c_char,
        flags: i32,
        mode: u16,
    ) -> i32;
    fn posix_spawn_file_actions_adddup2(actions: *mut *mut c_void, from: i32, to: i32) -> i32;
    fn posix_spawnattr_init(attrs: *mut *mut c_void) -> i32;
    fn posix_spawnattr_destroy(attrs: *mut *mut c_void) -> i32;
    fn posix_spawnattr_setflags(attrs: *mut *mut c_void, flags: i16) -> i32;
    fn posix_spawnattr_setsigdefault(attrs: *mut *mut c_void, signals: *const u32) -> i32;
    fn posix_spawnattr_setsigmask(attrs: *mut *mut c_void, signals: *const u32) -> i32;
    fn posix_spawn(
        pid: *mut i32,
        path: *const c_char,
        actions: *const *mut c_void,
        attrs: *const *mut c_void,
        argv: *const *mut c_char,
        envp: *const *mut c_char,
    ) -> i32;
}
