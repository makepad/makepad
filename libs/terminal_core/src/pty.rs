use std::io;

/// PTY writer handle for optional cross-thread input forwarding.
#[derive(Clone, Copy)]
pub struct PtyWriter {
    master_fd: i32,
}

impl PtyWriter {
    pub fn send(&self, data: Vec<u8>) -> io::Result<()> {
        write_all_fd(self.master_fd, &data)
    }
}

/// PTY handle — spawns a shell in a pseudo-terminal.
///
/// Unix implementation uses `openpty` plus `std::process::Command::spawn`.
/// I/O is done directly on a nonblocking master fd (no background worker threads).
pub struct Pty {
    master_fd: i32,
    child: Option<std::process::Child>,
}

impl Pty {
    pub fn spawn(
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        env: &[(&str, &str)],
    ) -> io::Result<Self> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            Self::spawn_unix(cols, rows, shell, env)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (cols, rows, shell, env);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "PTY not implemented for this platform",
            ))
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn spawn_unix(
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        env: &[(&str, &str)],
    ) -> io::Result<Self> {
        use std::os::fd::FromRawFd;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let shell = shell
            .map(str::to_owned)
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/zsh".to_owned());

        let (master, slave) = unsafe { open_pty(cols, rows)? };
        unsafe {
            set_cloexec(master);
            set_nonblocking(master);
        }

        let mut cmd = Command::new(&shell);
        cmd.arg("-l");
        cmd.env("TERM", "xterm-256color");
        for (k, v) in env {
            cmd.env(k, v);
        }

        // Make the spawned shell/session own the slave PTY as controlling terminal,
        // so kernel SIGWINCH delivery works for foreground jobs on resize.
        unsafe {
            cmd.pre_exec(|| {
                if libc_ffi::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc_ffi::ioctl(0, libc_ffi::TIOCSCTTY, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        // Attach slave side to child stdio. These handles are consumed by Command.
        let slave_file = unsafe { std::fs::File::from_raw_fd(slave) };
        let slave_out = slave_file.try_clone()?;
        let slave_err = slave_file.try_clone()?;
        cmd.stdin(Stdio::from(slave_file));
        cmd.stdout(Stdio::from(slave_out));
        cmd.stderr(Stdio::from(slave_err));

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                unsafe {
                    libc_ffi::close(master);
                }
                return Err(err);
            }
        };

        Ok(Self {
            master_fd: master,
            child: Some(child),
        })
    }

    pub fn write(&self, data: &[u8]) -> io::Result<()> {
        write_all_fd(self.master_fd, data)
    }

    pub fn writer_clone(&self) -> PtyWriter {
        PtyWriter {
            master_fd: self.master_fd,
        }
    }

    pub fn try_read(&self) -> Option<Vec<u8>> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        unsafe {
            let mut buf = [0u8; 4096];
            let n = libc_ffi::read(self.master_fd, buf.as_mut_ptr() as *mut _, buf.len());
            if n > 0 {
                return Some(buf[..n as usize].to_vec());
            }
            if n == 0 {
                return None;
            }

            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(code)
                    if code == libc_ffi::EAGAIN
                        || code == libc_ffi::EWOULDBLOCK
                        || code == libc_ffi::EINTR =>
                {
                    None
                }
                _ => None,
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            None
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        unsafe {
            let ws = libc_ffi::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            if libc_ffi::ioctl(self.master_fd, libc_ffi::TIOCSWINSZ, &ws) != 0 {
                return Err(io::Error::last_os_error());
            }
            if let Some(child) = &self.child {
                let _ = libc_ffi::kill(child.id() as i32, libc_ffi::SIGWINCH);
            }
        }
        Ok(())
    }

    pub fn child_pid(&self) -> i32 {
        self.child.as_ref().map(|c| c.id() as i32).unwrap_or(-1)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        unsafe {
            libc_ffi::close(self.master_fd);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.try_wait();
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_all_fd(fd: i32, data: &[u8]) -> io::Result<()> {
    let mut offset = 0usize;
    while offset < data.len() {
        let n = unsafe {
            libc_ffi::write(
                fd,
                data[offset..].as_ptr() as *const _,
                data.len() - offset,
            )
        };
        if n > 0 {
            offset += n as usize;
            continue;
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "PTY write returned 0"));
        }

        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(code) if code == libc_ffi::EINTR => continue,
            Some(code) if code == libc_ffi::EAGAIN || code == libc_ffi::EWOULDBLOCK => {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, err));
            }
            _ => return Err(err),
        }
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn write_all_fd(_fd: i32, _data: &[u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "PTY not implemented for this platform",
    ))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
unsafe fn set_cloexec(fd: i32) {
    let flags = libc_ffi::fcntl_int(fd, libc_ffi::F_GETFD, 0);
    if flags >= 0 {
        libc_ffi::fcntl_int(fd, libc_ffi::F_SETFD, flags | libc_ffi::FD_CLOEXEC);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
unsafe fn set_nonblocking(fd: i32) {
    let flags = libc_ffi::fcntl_int(fd, libc_ffi::F_GETFL, 0);
    if flags >= 0 {
        libc_ffi::fcntl_int(fd, libc_ffi::F_SETFL, flags | libc_ffi::O_NONBLOCK);
    }
}

/// Open a PTY master/slave pair. Returns `(master_fd, slave_fd)`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
unsafe fn open_pty(cols: u16, rows: u16) -> io::Result<(i32, i32)> {
    let mut master: i32 = -1;
    let mut slave: i32 = -1;
    let ws = libc_ffi::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let ret = libc_ffi::openpty(
        &mut master,
        &mut slave,
        std::ptr::null_mut(),
        std::ptr::null(),
        &ws,
    );
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok((master, slave))
}

/// Minimal FFI bindings — no libc crate dependency.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod libc_ffi {
    #[cfg_attr(any(target_os = "macos", target_os = "linux"), link(name = "util"))]
    extern "C" {
        pub fn openpty(
            amaster: *mut i32,
            aslave: *mut i32,
            name: *mut i8,
            termp: *const std::ffi::c_void,
            winp: *const winsize,
        ) -> i32;
    }

    extern "C" {
        pub fn close(fd: i32) -> i32;
        pub fn read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize;
        pub fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
        pub fn ioctl(fd: i32, request: u64, ...) -> i32;
        pub fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        pub fn setsid() -> i32;
        pub fn kill(pid: i32, sig: i32) -> i32;
    }

    pub unsafe fn fcntl_int(fd: i32, cmd: i32, arg: i32) -> i32 {
        fcntl(fd, cmd, arg)
    }

    pub const F_GETFD: i32 = 1;
    pub const F_SETFD: i32 = 2;
    pub const F_GETFL: i32 = 3;
    pub const F_SETFL: i32 = 4;
    pub const FD_CLOEXEC: i32 = 1;

    #[cfg(target_os = "macos")]
    pub const O_NONBLOCK: i32 = 0x0004;
    #[cfg(target_os = "linux")]
    pub const O_NONBLOCK: i32 = 0x0800;

    pub const EINTR: i32 = 4;
    #[cfg(target_os = "macos")]
    pub const EAGAIN: i32 = 35;
    #[cfg(target_os = "macos")]
    pub const EWOULDBLOCK: i32 = 35;
    #[cfg(target_os = "linux")]
    pub const EAGAIN: i32 = 11;
    #[cfg(target_os = "linux")]
    pub const EWOULDBLOCK: i32 = 11;

    #[cfg(target_os = "macos")]
    pub const TIOCSWINSZ: u64 = 0x80087467;
    #[cfg(target_os = "macos")]
    pub const TIOCSCTTY: u64 = 0x20007461;
    #[cfg(target_os = "linux")]
    pub const TIOCSWINSZ: u64 = 0x5414;
    #[cfg(target_os = "linux")]
    pub const TIOCSCTTY: u64 = 0x540E;

    pub const SIGWINCH: i32 = 28;

    #[repr(C)]
    pub struct winsize {
        pub ws_row: u16,
        pub ws_col: u16,
        pub ws_xpixel: u16,
        pub ws_ypixel: u16,
    }
}
