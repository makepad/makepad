use std::io;

#[cfg(windows)]
use std::{
    fs::File,
    io::{Read, Write},
    os::windows::io::FromRawHandle,
    sync::{mpsc, Arc, Mutex},
};
#[cfg(windows)]
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT,
        },
        Security::SECURITY_ATTRIBUTES,
        System::{
            Console::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON},
            Pipes::CreatePipe,
            Threading::{
                CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
                TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
                EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTUPINFOEXW,
            },
        },
    },
};

/// PTY writer handle for optional cross-thread input forwarding.
#[derive(Clone)]
pub struct PtyWriter {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    master_fd: i32,
    #[cfg(windows)]
    stdin: Arc<Mutex<File>>,
}

impl PtyWriter {
    pub fn send(&self, data: Vec<u8>) -> io::Result<()> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            return write_all_fd(self.master_fd, &data);
        }
        #[cfg(windows)]
        {
            let mut stdin = self.stdin.lock().map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "terminal stdin lock poisoned")
            })?;
            stdin.write_all(&data)?;
            stdin.flush()?;
            return Ok(());
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = data;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "PTY not implemented for this platform",
            ));
        }
    }
}

/// PTY handle — spawns a shell in a pseudo-terminal.
///
/// Unix implementation uses `openpty` plus `std::process::Command::spawn`.
/// I/O is done directly on a nonblocking master fd (no background worker threads).
pub struct Pty {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    master_fd: i32,
    #[cfg(windows)]
    stdin: Arc<Mutex<File>>,
    #[cfg(windows)]
    read_rx: mpsc::Receiver<Vec<u8>>,
    #[cfg(windows)]
    child_pid: i32,
    #[cfg(windows)]
    process_handle: isize,
    #[cfg(windows)]
    pseudo_console: isize,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    child: Option<std::process::Child>,
}

impl Pty {
    pub fn spawn(
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        env: &[(&str, &str)],
        cwd: Option<&std::path::Path>,
    ) -> io::Result<Self> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            Self::spawn_unix(cols, rows, shell, env, cwd)
        }
        #[cfg(windows)]
        {
            Self::spawn_windows(cols, rows, shell, env, cwd)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = (cols, rows, shell, env, cwd);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "PTY not implemented for this platform",
            ))
        }
    }

    #[cfg(windows)]
    fn spawn_windows(
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        _env: &[(&str, &str)],
        cwd: Option<&std::path::Path>,
    ) -> io::Result<Self> {
        let shell = shell
            .map(str::to_owned)
            .unwrap_or_else(|| "cmd.exe".to_owned());
        let command_line = windows_shell_command(&shell);
        let mut command_line_wide = wide_null(&command_line);
        let mut startup_info = STARTUPINFOEXW::default();
        startup_info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        // STARTF_USESTDHANDLES
        startup_info.StartupInfo.dwFlags.0 |= 0x00000100;
        startup_info.StartupInfo.hStdInput = invalid_handle();
        startup_info.StartupInfo.hStdOutput = invalid_handle();
        startup_info.StartupInfo.hStdError = invalid_handle();

        let pipe_security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: true.into(),
        };

        let mut conpty_input_read = WinHandle::invalid();
        let mut host_input_write = WinHandle::invalid();
        let mut host_output_read = WinHandle::invalid();
        let mut conpty_output_write = WinHandle::invalid();

        unsafe {
            CreatePipe(
                &mut conpty_input_read.0,
                &mut host_input_write.0,
                Some(&pipe_security),
                0,
            )
            .map_err(windows_err)?;
            CreatePipe(
                &mut host_output_read.0,
                &mut conpty_output_write.0,
                Some(&pipe_security),
                0,
            )
            .map_err(windows_err)?;
            SetHandleInformation(
                host_input_write.raw(),
                HANDLE_FLAG_INHERIT.0,
                HANDLE_FLAGS::default(),
            )
            .map_err(windows_err)?;
            SetHandleInformation(
                host_output_read.raw(),
                HANDLE_FLAG_INHERIT.0,
                HANDLE_FLAGS::default(),
            )
            .map_err(windows_err)?;
        }

        let mut pseudo_console = WinPseudoConsole(unsafe {
            CreatePseudoConsole(
                to_conpty_coord(cols, rows),
                conpty_input_read.raw(),
                conpty_output_write.raw(),
                0,
            )
            .map_err(windows_err)?
        });

        // ConPTY owns these ends now.
        drop(conpty_input_read);
        drop(conpty_output_write);

        let attr_list = ProcThreadAttributeList::new(1)?;
        let pseudo_console_value = pseudo_console.raw();
        unsafe {
            UpdateProcThreadAttribute(
                attr_list.raw(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(pseudo_console_value.0 as *const std::ffi::c_void),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )
            .map_err(windows_err)?;
        }
        startup_info.lpAttributeList = attr_list.raw();

        let mut process_info = PROCESS_INFORMATION::default();
        let cwd_wide = cwd.map(|path| wide_null_os(path.as_os_str()));
        let cwd_ptr = cwd_wide
            .as_ref()
            .map(|value| PCWSTR(value.as_ptr()))
            .unwrap_or(PCWSTR::null());
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(command_line_wide.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                cwd_ptr,
                &startup_info.StartupInfo,
                &mut process_info,
            )
            .map_err(windows_err)?;
        }

        unsafe {
            let _ = CloseHandle(process_info.hThread);
        }
        drop(attr_list);

        let stdin_file = host_input_write.into_file();
        let stdout_file = host_output_read.into_file();
        let stdin = Arc::new(Mutex::new(stdin_file));

        let (read_tx, read_rx) = mpsc::channel::<Vec<u8>>();
        spawn_pipe_reader(stdout_file, read_tx);

        Ok(Self {
            stdin,
            read_rx,
            child_pid: process_info.dwProcessId as i32,
            process_handle: process_info.hProcess.0 as isize,
            pseudo_console: pseudo_console.take().0,
        })
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn spawn_unix(
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        env: &[(&str, &str)],
        cwd: Option<&std::path::Path>,
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
            set_cloexec(slave);
            set_nonblocking(master);
        }

        let mut cmd = Command::new(&shell);
        cmd.arg("-l");
        cmd.env("TERM", "xterm-256color");
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }

        // Make the spawned shell/session own the slave PTY as controlling terminal,
        // so kernel SIGWINCH delivery works for foreground jobs on resize.
        let master_for_child = master;
        let slave_for_child = slave;
        unsafe {
            cmd.pre_exec(move || {
                if libc_ffi::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc_ffi::ioctl(slave_for_child, libc_ffi::TIOCSCTTY, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                // Child no longer needs these raw PTY fds after stdio setup.
                libc_ffi::close(slave_for_child);
                libc_ffi::close(master_for_child);
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

    pub fn try_write(&self, data: &[u8]) -> io::Result<usize> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            return write_fd_once(self.master_fd, data);
        }
        #[cfg(windows)]
        {
            let mut stdin = self.stdin.lock().map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "terminal stdin lock poisoned")
            })?;
            return stdin.write(data);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = data;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "PTY not implemented for this platform",
            ));
        }
    }

    pub fn write(&self, data: &[u8]) -> io::Result<()> {
        let mut offset = 0usize;
        while offset < data.len() {
            match self.try_write(&data[offset..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "PTY write returned 0",
                    ));
                }
                Ok(n) => offset += n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    pub fn writer_clone(&self) -> PtyWriter {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            return PtyWriter {
                master_fd: self.master_fd,
            };
        }
        #[cfg(windows)]
        {
            return PtyWriter {
                stdin: self.stdin.clone(),
            };
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            unreachable!()
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
            #[cfg(windows)]
            {
                return self.read_rx.try_recv().ok();
            }
            #[cfg(not(windows))]
            {
                None
            }
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
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            #[cfg(windows)]
            {
                let pseudo_console = raw_hpcon(self.pseudo_console);
                unsafe {
                    ResizePseudoConsole(pseudo_console, to_conpty_coord(cols, rows))
                        .map_err(windows_err)?;
                }
            }
            #[cfg(not(windows))]
            {
                let _ = (cols, rows);
            }
        }
        Ok(())
    }

    pub fn child_pid(&self) -> i32 {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.child.as_ref().map(|c| c.id() as i32).unwrap_or(-1)
        }
        #[cfg(windows)]
        {
            self.child_pid
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            -1
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        unsafe {
            libc_ffi::close(self.master_fd);
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.try_wait();
        }
        #[cfg(windows)]
        unsafe {
            let process_handle = raw_handle(self.process_handle);
            if !process_handle.is_invalid() {
                let _ = TerminateProcess(process_handle, 1);
                let _ = WaitForSingleObject(process_handle, 50);
                let _ = CloseHandle(process_handle);
                self.process_handle = 0;
            }
            let pseudo_console = raw_hpcon(self.pseudo_console);
            if !pseudo_console.is_invalid() {
                ClosePseudoConsole(pseudo_console);
                self.pseudo_console = 0;
            }
        }
    }
}

#[cfg(windows)]
fn windows_err(err: windows::core::Error) -> io::Error {
    io::Error::other(err.to_string())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn wide_null_os(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn windows_shell_command(shell: &str) -> String {
    shell.to_string()
}

#[cfg(windows)]
fn to_conpty_coord(cols: u16, rows: u16) -> COORD {
    COORD {
        X: cols.clamp(1, i16::MAX as u16) as i16,
        Y: rows.clamp(1, i16::MAX as u16) as i16,
    }
}

#[cfg(windows)]
fn raw_handle(raw: isize) -> HANDLE {
    HANDLE(raw as *mut std::ffi::c_void)
}

#[cfg(windows)]
fn raw_hpcon(raw: isize) -> HPCON {
    HPCON(raw)
}

#[cfg(windows)]
fn invalid_handle() -> HANDLE {
    HANDLE((-1isize) as *mut std::ffi::c_void)
}

#[cfg(windows)]
struct WinPseudoConsole(HPCON);

#[cfg(windows)]
impl WinPseudoConsole {
    fn raw(&self) -> HPCON {
        self.0
    }

    fn take(&mut self) -> HPCON {
        let handle = self.0;
        self.0 = HPCON::default();
        handle
    }
}

#[cfg(windows)]
impl Drop for WinPseudoConsole {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_invalid() {
                ClosePseudoConsole(self.0);
            }
        }
    }
}

#[cfg(windows)]
struct WinHandle(HANDLE);

#[cfg(windows)]
impl WinHandle {
    fn invalid() -> Self {
        Self(HANDLE::default())
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    fn take(&mut self) -> HANDLE {
        let handle = self.0;
        self.0 = HANDLE::default();
        handle
    }

    fn into_file(mut self) -> File {
        let handle = self.take();
        unsafe { File::from_raw_handle(handle.0) }
    }
}

#[cfg(windows)]
impl Drop for WinHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_invalid() {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
struct ProcThreadAttributeList {
    _storage: Vec<u8>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}

#[cfg(windows)]
impl ProcThreadAttributeList {
    fn new(attribute_count: u32) -> io::Result<Self> {
        let mut size = 0usize;
        unsafe {
            let _ = InitializeProcThreadAttributeList(None, attribute_count, Some(0), &mut size);
        }
        if size == 0 {
            return Err(io::Error::other(
                "InitializeProcThreadAttributeList returned size 0",
            ));
        }

        let mut storage = vec![0u8; size];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        unsafe {
            InitializeProcThreadAttributeList(Some(list), attribute_count, Some(0), &mut size)
                .map_err(windows_err)?;
        }
        Ok(Self {
            _storage: storage,
            list,
        })
    }

    fn raw(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.list
    }
}

#[cfg(windows)]
impl Drop for ProcThreadAttributeList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.list);
        }
    }
}

#[cfg(windows)]
fn spawn_pipe_reader<R: Read + Send + 'static>(mut reader: R, tx: mpsc::Sender<Vec<u8>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_all_fd(fd: i32, data: &[u8]) -> io::Result<()> {
    let mut offset = 0usize;
    while offset < data.len() {
        let n = unsafe {
            libc_ffi::write(fd, data[offset..].as_ptr() as *const _, data.len() - offset)
        };
        if n > 0 {
            offset += n as usize;
            continue;
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "PTY write returned 0",
            ));
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_fd_once(fd: i32, data: &[u8]) -> io::Result<usize> {
    let n = unsafe { libc_ffi::write(fd, data.as_ptr() as *const _, data.len()) };
    if n > 0 {
        return Ok(n as usize);
    }
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "PTY write returned 0",
        ));
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc_ffi::EINTR => Err(io::Error::new(io::ErrorKind::Interrupted, err)),
        Some(code) if code == libc_ffi::EAGAIN || code == libc_ffi::EWOULDBLOCK => {
            Err(io::Error::new(io::ErrorKind::WouldBlock, err))
        }
        _ => Err(err),
    }
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

    #[repr(C)]
    pub struct winsize {
        pub ws_row: u16,
        pub ws_col: u16,
        pub ws_xpixel: u16,
        pub ws_ypixel: u16,
    }
}
