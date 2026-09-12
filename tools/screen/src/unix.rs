//! Small Unix PTY and terminal boundary. No platform GUI or libc crate needed.
#[cfg(target_os = "macos")]
use crate::pty_spawn::{self, Child};
use std::{
    ffi::{c_void, OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{ffi::OsStrExt, net::UnixStream, process::ExitStatusExt},
    },
    path::Path,
    process::Command,
    sync::atomic::{AtomicI32, Ordering},
    time::{Duration, Instant},
};
#[cfg(target_os = "linux")]
use std::{
    os::unix::process::CommandExt,
    process::{Child, Stdio},
};

pub struct Pty {
    master: File,
    child: Child,
    status: Option<i32>,
}

impl Pty {
    pub fn spawn(
        program: &OsStr,
        args: &[OsString],
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> io::Result<Self> {
        Self::spawn_with_env(program, args, cwd, cols, rows, &[])
    }

    pub fn spawn_with_env(
        program: &OsStr,
        args: &[OsString],
        cwd: &Path,
        cols: u16,
        rows: u16,
        environment: &[(&OsStr, &OsStr)],
    ) -> io::Result<Self> {
        let dimensions = dimensions(cols, rows)?;
        let (mut master, mut slave) = (-1, -1);
        if unsafe {
            ffi::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &dimensions,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        cloexec(master.as_raw_fd())?;
        cloexec(slave.as_raw_fd())?;
        nonblocking(master.as_raw_fd())?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env_remove("NO_COLOR");
        for (key, value) in environment {
            command.env(key, value);
        }
        #[cfg(target_os = "linux")]
        unsafe {
            command
                .stdin(Stdio::from(slave.try_clone()?))
                .stdout(Stdio::from(slave.try_clone()?))
                .stderr(Stdio::from(slave));
            command.pre_exec(|| {
                if ffi::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                // Command has already connected the slave to fd 0. A new
                // session owns its controlling terminal and kernel SIGWINCH.
                if ffi::ioctl(0, ffi::TIOCSCTTY, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        #[cfg(target_os = "linux")]
        let child = command.spawn()?;
        #[cfg(target_os = "macos")]
        let child = pty_spawn::spawn(&command, slave.as_raw_fd(), &pty_spawn::screen_helper()?)?;
        Ok(Self {
            master,
            child,
            status: None,
        })
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let size = dimensions(cols, rows)?;
        if unsafe { ffi::ioctl(self.master.as_raw_fd(), ffi::TIOCSWINSZ, &size) } == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub fn child_pid(&self) -> u32 {
        self.child.id()
    }
    // Observe exit without reaping: the zombie leader reserves its PID/session
    // identity until every owned job-control group has been told to exit.
    fn exited_unreaped(&self) -> io::Result<bool> {
        if self.status.is_some() {
            return Ok(true);
        }
        let mut info = [0usize; 16]; // aligned storage for either platform's siginfo_t
        #[cfg(target_os = "macos")]
        let options = 1 | 4 | 0x20;
        #[cfg(target_os = "linux")]
        let options = 1 | 4 | 0x01000000;
        if unsafe { ffi::waitid(1, self.child.id(), info.as_mut_ptr().cast(), options) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { *(info.as_ptr().cast::<i32>()) } != 0)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        if self.status.is_none() && self.exited_unreaped()? {
            self.terminate_group(false)?;
            // A root shell can exit before its jobs. Retain the leader until
            // TERM-ignoring descendants, including foreground jobs, are gone.
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline && !self.session_groups()?.is_empty() {
                std::thread::sleep(Duration::from_millis(10));
            }
            self.terminate_group(true)?;
            let status = self.child.wait()?;
            self.status = Some(
                status
                    .code()
                    .unwrap_or_else(|| -status.signal().unwrap_or(1)),
            );
        }
        Ok(self.status)
    }

    fn session_groups(&self) -> io::Result<Vec<(i32, i32)>> {
        if self.status.is_some() {
            return Ok(Vec::new());
        }
        let owner = self.child.id() as i32;
        // Darwin removes a zombie from session/group lookups before wait()
        // reaps it. WNOWAIT still proves this exact child reserves the PID;
        // use that evidence while cleaning up its surviving jobs.
        if (unsafe { ffi::getsid(owner) } != owner || unsafe { ffi::getpgid(owner) } != owner)
            && !self.exited_unreaped()?
        {
            return Err(io::Error::other(
                "Owned PTY leader no longer has its original session",
            ));
        }
        #[cfg(target_os = "macos")]
        let pids = {
            let mut pids = vec![0i32; 32768];
            let count =
                unsafe { ffi::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            if count as usize >= pids.len() {
                return Err(io::Error::other(
                    "Process inventory exceeds PTY cleanup bound",
                ));
            }
            pids.truncate(count as usize);
            pids
        };
        #[cfg(target_os = "linux")]
        let pids = {
            let mut pids = Vec::new();
            for entry in std::fs::read_dir("/proc")? {
                let entry = entry?;
                if let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<i32>().ok())
                {
                    if pids.len() >= 32768 {
                        return Err(io::Error::other(
                            "Process inventory exceeds PTY cleanup bound",
                        ));
                    }
                    pids.push(pid);
                }
            }
            pids
        };
        let mut groups = Vec::new();
        for pid in pids {
            if pid <= 1 || pid == owner || unsafe { ffi::getsid(pid) } != owner {
                continue;
            }
            let group = unsafe { ffi::getpgid(pid) };
            if group > 1 && !groups.iter().any(|(existing, _)| *existing == group) {
                groups.push((group, pid));
            }
        }
        Ok(groups)
    }

    /// The retained leader proves the session until cleanup has completed.
    pub fn terminate_group(&mut self, force: bool) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        let owner = self.child.id() as i32;
        let mut groups = self.session_groups()?;
        groups.push((owner, owner));
        for (group, witness) in groups {
            if unsafe { ffi::getsid(witness) } != owner || unsafe { ffi::getpgid(witness) } != group
            {
                continue;
            }
            if unsafe { ffi::kill(-group, if force { 9 } else { 15 }) } == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(3) {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> io::Result<Option<i32>> {
        if let Some(status) = self.try_wait()? {
            return Ok(Some(status));
        }
        self.terminate_group(false)?;
        let graceful = Instant::now() + Duration::from_millis(500);
        while Instant::now() < graceful {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.terminate_group(true)?;
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Owned PTY child did not exit after TERM/KILL",
        ))
    }
}
impl Read for Pty {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.master.read(buffer) {
            // Linux PTY masters report EIO once the last slave is closed.
            Err(error) if error.raw_os_error() == Some(5) => Ok(0),
            result => result,
        }
    }
}
impl Write for Pty {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.master.write(buffer)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl AsRawFd for Pty {
    fn as_raw_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("makepad-screen: PTY cleanup: {error}");
        }
    }
}

pub fn detach_session() -> io::Result<()> {
    if unsafe { ffi::setsid() } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
pub fn current_uid() -> u32 {
    unsafe { ffi::geteuid() }
}
pub fn connect(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    let path = path.as_os_str().as_bytes();
    let mut address = SocketAddress::empty();
    if path.len() >= address.path.len() || path.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid Unix socket path",
        ));
    }
    address.path[..path.len()].copy_from_slice(path);
    let length = (path.len() + 3) as u32;
    #[cfg(target_os = "macos")]
    {
        address.length = length as u8;
    }
    let fd = unsafe { ffi::socket(1, 1, 0) };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    let socket = unsafe { UnixStream::from_raw_fd(fd) };
    cloexec(fd)?;
    nonblocking(fd)?;
    if unsafe { ffi::connect(fd, &address as *const _ as *const c_void, length) } == 0 {
        return Ok(socket);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ffi::EINPROGRESS) {
        return Err(error);
    }
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Unix socket connection timed out",
            ));
        }
        let mut fds = [PollFd {
            fd,
            events: WRITABLE,
            revents: 0,
        }];
        match poll(&mut fds, remaining.as_millis().clamp(1, 100) as i32) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        }
        if fds[0].revents == 0 {
            continue;
        }
        let mut error = 0i32;
        let mut size = std::mem::size_of_val(&error) as u32;
        if unsafe {
            ffi::getsockopt(
                fd,
                ffi::SOL_SOCKET,
                ffi::SO_ERROR,
                &mut error as *mut _ as *mut c_void,
                &mut size,
            )
        } == -1
        {
            return Err(io::Error::last_os_error());
        }
        if size as usize != std::mem::size_of_val(&error) {
            return Err(io::Error::other("Unexpected socket error size"));
        }
        return if error == 0 {
            Ok(socket)
        } else {
            Err(io::Error::from_raw_os_error(error))
        };
    }
}
#[repr(C)]
struct SocketAddress {
    #[cfg(target_os = "macos")]
    length: u8,
    #[cfg(target_os = "macos")]
    family: u8,
    #[cfg(target_os = "linux")]
    family: u16,
    #[cfg(target_os = "macos")]
    path: [u8; 104],
    #[cfg(target_os = "linux")]
    path: [u8; 108],
}
impl SocketAddress {
    fn empty() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            length: 0,
            family: 1,
            #[cfg(target_os = "macos")]
            path: [0; 104],
            #[cfg(target_os = "linux")]
            path: [0; 108],
        }
    }
}
pub fn peer_uid(socket: &UnixStream) -> io::Result<u32> {
    #[cfg(target_os = "macos")]
    {
        let (mut uid, mut gid) = (0, 0);
        if unsafe { ffi::getpeereid(socket.as_raw_fd(), &mut uid, &mut gid) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
    #[cfg(target_os = "linux")]
    {
        #[repr(C)]
        struct Credentials {
            pid: i32,
            uid: u32,
            gid: u32,
        }
        let mut credentials = Credentials {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut size = std::mem::size_of::<Credentials>() as u32;
        if unsafe {
            ffi::getsockopt(
                socket.as_raw_fd(),
                1,
                17,
                &mut credentials as *mut _ as *mut c_void,
                &mut size,
            )
        } == -1
        {
            return Err(io::Error::last_os_error());
        }
        if size as usize != std::mem::size_of::<Credentials>() {
            return Err(io::Error::other("Unexpected peer credential size"));
        }
        Ok(credentials.uid)
    }
}

fn dimensions(cols: u16, rows: u16) -> io::Result<WinSize> {
    if cols == 0 || rows == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PTY dimensions must be nonzero",
        ));
    }
    Ok(WinSize {
        rows,
        cols,
        x_pixels: 0,
        y_pixels: 0,
    })
}
#[repr(C)]
struct WinSize {
    rows: u16,
    cols: u16,
    x_pixels: u16,
    y_pixels: u16,
}
pub fn window_size(fd: RawFd) -> io::Result<(u16, u16)> {
    let mut size = WinSize {
        rows: 0,
        cols: 0,
        x_pixels: 0,
        y_pixels: 0,
    };
    if unsafe { ffi::ioctl(fd, ffi::TIOCGWINSZ, &mut size) } == -1 {
        return Err(io::Error::last_os_error());
    }
    if size.cols == 0 || size.rows == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Terminal reported zero dimensions",
        ));
    }
    Ok((size.cols, size.rows))
}
pub fn is_tty(fd: RawFd) -> bool {
    unsafe { ffi::isatty(fd) == 1 }
}
fn cloexec(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { ffi::fcntl(fd, 1, 0) };
    if flags == -1 || unsafe { ffi::fcntl(fd, 2, flags | 1) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn flags(fd: RawFd) -> io::Result<i32> {
    let flags = unsafe { ffi::fcntl(fd, 3, 0) };
    if flags == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(flags)
    }
}
fn nonblocking(fd: RawFd) -> io::Result<()> {
    let original = flags(fd)?;
    if unsafe { ffi::fcntl(fd, 4, original | ffi::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub struct NonblockingGuard {
    fd: RawFd,
    original: Option<i32>,
}
impl NonblockingGuard {
    pub fn new(fd: RawFd) -> io::Result<Self> {
        let original = flags(fd)?;
        nonblocking(fd)?;
        Ok(Self {
            fd,
            original: Some(original),
        })
    }
    pub fn restore(&mut self) -> io::Result<()> {
        if let Some(original) = self.original {
            if unsafe { ffi::fcntl(self.fd, 4, original) } == -1 {
                return Err(io::Error::last_os_error());
            }
            self.original = None;
        }
        Ok(())
    }
}
impl Drop for NonblockingGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

// termios stays opaque: only the platform's libc accesses its fields. This
// aligned storage exceeds both Darwin's 72-byte and Linux's <=64-byte ABI.
#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct Termios([u8; 256]);
pub struct RawTerminal {
    fd: RawFd,
    original: Option<Termios>,
}
impl RawTerminal {
    pub fn new(fd: RawFd) -> io::Result<Self> {
        if !is_tty(fd) {
            return Ok(Self { fd, original: None });
        }
        let mut original = Termios([0; 256]);
        if unsafe { ffi::tcgetattr(fd, &mut original as *mut _ as *mut c_void) } == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        unsafe {
            ffi::cfmakeraw(&mut raw as *mut _ as *mut c_void);
        }
        if unsafe { ffi::tcsetattr(fd, 0, &raw as *const _ as *const c_void) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            original: Some(original),
        })
    }
    pub fn restore(&mut self) -> io::Result<()> {
        if let Some(original) = &self.original {
            if unsafe { ffi::tcsetattr(self.fd, 0, original as *const _ as *const c_void) } == -1 {
                return Err(io::Error::last_os_error());
            }
            self.original = None;
        }
        Ok(())
    }
}
impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub const READABLE: i16 = 1;
pub const WRITABLE: i16 = 4;
pub const POLL_ERROR: i16 = 8 | 16 | 32;
#[repr(C)]
pub struct PollFd {
    pub fd: RawFd,
    pub events: i16,
    pub revents: i16,
}
pub fn poll(fds: &mut [PollFd], milliseconds: i32) -> io::Result<()> {
    if unsafe { ffi::poll(fds.as_mut_ptr(), fds.len() as ffi::Nfds, milliseconds) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
pub fn read_fd(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    let count = unsafe { ffi::read(fd, buffer.as_mut_ptr() as *mut c_void, buffer.len()) };
    if count < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(count as usize)
    }
}
pub fn write_fd(fd: RawFd, buffer: &[u8]) -> io::Result<usize> {
    let count = unsafe { ffi::write(fd, buffer.as_ptr() as *const c_void, buffer.len()) };
    if count < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(count as usize)
    }
}

static INTERRUPTED: AtomicI32 = AtomicI32::new(0);
extern "C" fn interrupt(signal: i32) {
    let _ = INTERRUPTED.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}
pub struct SignalGuard {
    previous: Vec<(i32, usize)>,
}
impl SignalGuard {
    pub fn new() -> io::Result<Self> {
        INTERRUPTED.store(0, Ordering::Relaxed);
        let mut guard = Self { previous: vec![] };
        for signal in [1, 2, 3, 15] {
            let previous = unsafe { ffi::signal(signal, interrupt as *const () as usize) };
            if previous == usize::MAX {
                return Err(io::Error::last_os_error());
            }
            guard.previous.push((signal, previous));
        }
        Ok(guard)
    }
    pub fn interrupted(&self) -> Option<i32> {
        match INTERRUPTED.load(Ordering::Relaxed) {
            0 => None,
            signal => Some(signal),
        }
    }
}
impl Drop for SignalGuard {
    fn drop(&mut self) {
        for (signal, handler) in &self.previous {
            unsafe {
                ffi::signal(*signal, *handler);
            }
        }
    }
}

mod ffi {
    use super::{c_void, PollFd, WinSize};
    #[link(name = "util")]
    unsafe extern "C" {
        pub fn openpty(
            master: *mut i32,
            slave: *mut i32,
            name: *mut i8,
            termios: *const c_void,
            dimensions: *const WinSize,
        ) -> i32;
    }
    unsafe extern "C" {
        pub fn ioctl(fd: i32, request: usize, ...) -> i32;
        pub fn fcntl(fd: i32, command: i32, ...) -> i32;
        pub fn setsid() -> i32;
        pub fn getsid(pid: i32) -> i32;
        pub fn getpgid(pid: i32) -> i32;
        pub fn kill(pid: i32, signal: i32) -> i32;
        pub fn waitid(kind: i32, pid: u32, info: *mut c_void, options: i32) -> i32;
        #[cfg(target_os = "macos")]
        pub fn proc_listallpids(buffer: *mut c_void, size: i32) -> i32;
        pub fn geteuid() -> u32;
        pub fn isatty(fd: i32) -> i32;
        pub fn tcgetattr(fd: i32, termios: *mut c_void) -> i32;
        pub fn tcsetattr(fd: i32, action: i32, termios: *const c_void) -> i32;
        pub fn cfmakeraw(termios: *mut c_void);
        pub fn read(fd: i32, buffer: *mut c_void, count: usize) -> isize;
        pub fn write(fd: i32, buffer: *const c_void, count: usize) -> isize;
        pub fn poll(fds: *mut PollFd, count: Nfds, milliseconds: i32) -> i32;
        pub fn signal(signal: i32, handler: usize) -> usize;
        pub fn socket(domain: i32, socket_type: i32, protocol: i32) -> i32;
        pub fn connect(fd: i32, address: *const c_void, length: u32) -> i32;
        #[cfg(target_os = "macos")]
        pub fn getpeereid(fd: i32, uid: *mut u32, gid: *mut u32) -> i32;
        pub fn getsockopt(
            fd: i32,
            level: i32,
            option: i32,
            value: *mut c_void,
            size: *mut u32,
        ) -> i32;
    }
    #[cfg(target_os = "macos")]
    pub type Nfds = u32;
    #[cfg(target_os = "linux")]
    pub type Nfds = usize;
    #[cfg(target_os = "macos")]
    pub const O_NONBLOCK: i32 = 4;
    #[cfg(target_os = "linux")]
    pub const O_NONBLOCK: i32 = 0x800;
    #[cfg(target_os = "linux")]
    pub const TIOCSCTTY: usize = 0x540e;
    #[cfg(target_os = "macos")]
    pub const TIOCSWINSZ: usize = 0x80087467;
    #[cfg(target_os = "linux")]
    pub const TIOCSWINSZ: usize = 0x5414;
    #[cfg(target_os = "macos")]
    pub const TIOCGWINSZ: usize = 0x40087468;
    #[cfg(target_os = "linux")]
    pub const TIOCGWINSZ: usize = 0x5413;
    #[cfg(target_os = "macos")]
    pub const EINPROGRESS: i32 = 36;
    #[cfg(target_os = "linux")]
    pub const EINPROGRESS: i32 = 115;
    #[cfg(target_os = "macos")]
    pub const SOL_SOCKET: i32 = 0xffff;
    #[cfg(target_os = "linux")]
    pub const SOL_SOCKET: i32 = 1;
    #[cfg(target_os = "macos")]
    pub const SO_ERROR: i32 = 0x1007;
    #[cfg(target_os = "linux")]
    pub const SO_ERROR: i32 = 4;
}
