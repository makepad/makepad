//! ConPTY ownership and one overlapped operation per pipe. No GUI dependency.
//! The session's private Job Object owns its entire process tree.
use std::{
    collections::BTreeMap,
    ffi::{c_void, OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::{Path, PathBuf},
    sync::mpsc::{self, SyncSender},
    thread,
};
type Raw = *mut c_void;
const CHUNK: usize = 16384;

struct Handle(usize);
impl Handle {
    fn new(raw: Raw) -> io::Result<Self> {
        if raw.is_null() || raw as isize == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw as usize))
        }
    }
    fn raw(&self) -> Raw {
        self.0 as Raw
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                ffi::CloseHandle(self.raw());
            }
        }
    }
}

#[repr(C)]
#[derive(Default)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: [u32; 2],
    event: usize,
}
struct AsyncPipe {
    file: File,
    operation: Box<Overlapped>,
    buffer: Box<[u8; CHUNK]>,
    pending: bool,
    begin: usize,
    end: usize,
}
impl AsyncPipe {
    fn new(file: File) -> Self {
        Self {
            file,
            operation: Box::default(),
            buffer: Box::new([0; CHUNK]),
            pending: false,
            begin: 0,
            end: 0,
        }
    }
    fn raw(&self) -> Raw {
        self.file.as_raw_handle()
    }
    fn operation_raw(&mut self) -> Raw {
        (self.operation.as_mut() as *mut Overlapped).cast()
    }
    fn completed(&mut self) -> io::Result<usize> {
        let mut count = 0;
        if unsafe { ffi::GetOverlappedResult(self.raw(), self.operation_raw(), &mut count, 0) } == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(996) {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.pending = false;
            return Err(error);
        }
        self.pending = false;
        Ok(count as usize)
    }
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.begin == self.end {
            if !self.pending {
                *self.operation = Overlapped::default();
                let mut count = 0;
                let result = unsafe {
                    ffi::ReadFile(
                        self.raw(),
                        self.buffer.as_mut_ptr().cast(),
                        CHUNK as u32,
                        &mut count,
                        self.operation.as_mut(),
                    )
                };
                if result == 0 {
                    let error = io::Error::last_os_error();
                    if matches!(error.raw_os_error(), Some(109 | 232)) {
                        return Ok(0);
                    }
                    if error.raw_os_error() != Some(997) {
                        return Err(error);
                    }
                }
                self.pending = true;
            }
            self.begin = 0;
            self.end = match self.completed() {
                Err(error) if matches!(error.raw_os_error(), Some(109 | 232)) => return Ok(0),
                result => result?,
            };
        }
        let count = output.len().min(self.end - self.begin);
        output[..count].copy_from_slice(&self.buffer[self.begin..self.begin + count]);
        self.begin += count;
        Ok(count)
    }
    fn advance_write(&mut self) -> io::Result<()> {
        if self.pending {
            let count = self.completed()?;
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            self.begin += count;
        }
        if self.begin < self.end {
            *self.operation = Overlapped::default();
            let mut count = 0;
            let result = unsafe {
                ffi::WriteFile(
                    self.raw(),
                    self.buffer[self.begin..self.end].as_ptr().cast(),
                    (self.end - self.begin) as u32,
                    &mut count,
                    self.operation.as_mut(),
                )
            };
            if result == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(997) {
                    return Err(error);
                }
            }
            self.pending = true;
            return Err(io::ErrorKind::WouldBlock.into());
        }
        Ok(())
    }
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.advance_write()?;
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = bytes.len().min(CHUNK);
        self.buffer[..count].copy_from_slice(&bytes[..count]);
        self.begin = 0;
        self.end = count;
        match self.advance_write() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => {
                self.begin = 0;
                self.end = 0;
                return Err(error);
            }
        }
        // The stable operation buffer now owns these bytes until completion.
        Ok(count)
    }
}
impl Drop for AsyncPipe {
    fn drop(&mut self) {
        if self.pending {
            unsafe {
                ffi::CancelIoEx(self.raw(), self.operation_raw());
                let mut count = 0;
                // Only the cleanup worker drops live pipe operations. Their
                // buffers cannot be freed before cancellation completes.
                ffi::GetOverlappedResult(self.raw(), self.operation_raw(), &mut count, 1);
            }
        }
    }
}

struct Resources {
    input: Option<AsyncPipe>,
    output: Option<AsyncPipe>,
    job: Handle,
    console: usize,
}
impl Drop for Resources {
    fn drop(&mut self) {
        unsafe {
            ffi::TerminateJobObject(self.job.raw(), 1);
        }
        self.input.take();
        self.output.take();
        // Closing the output pipe first prevents the documented pre-24H2
        // ClosePseudoConsole deadlock while a client still writes output.
        unsafe {
            ffi::ClosePseudoConsole(self.console as Raw);
        }
    }
}

pub struct Pty {
    resources: Option<Resources>,
    process: Handle,
    pid: u32,
    status: Option<i32>,
    cleanup: Option<SyncSender<Resources>>,
    stopping: bool,
    etx_pending: bool,
}
impl Pty {
    pub fn spawn_with_env(
        program: &OsStr,
        args: &[OsString],
        cwd: &Path,
        cols: u16,
        rows: u16,
        environment: &[(&OsStr, &OsStr)],
    ) -> io::Result<Self> {
        let size = dimensions(cols, rows)?;
        let program = executable(program, cwd)?;
        let application = wide(program.as_os_str())?;
        let directory = wide(cwd.as_os_str())?;
        let mut command = command_line(program.as_os_str(), args)?;
        let mut environment = environment_block(environment)?;
        let (output, child_output) = crate::windows_security::pipe_pair(true)?;
        let (input, child_input) = crate::windows_security::pipe_pair(false)?;
        let job =
            Handle::new(unsafe { ffi::CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut limits = JobLimits::default();
        limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        checked(unsafe {
            ffi::SetInformationJobObject(
                job.raw(),
                9,
                (&limits as *const JobLimits).cast(),
                std::mem::size_of::<JobLimits>() as u32,
            )
        })?;
        let mut console = std::ptr::null_mut();
        hresult(unsafe {
            ffi::CreatePseudoConsole(
                size,
                child_input.as_raw_handle(),
                child_output.as_raw_handle(),
                0,
                &mut console,
            )
        })?;
        let resources = Resources {
            input: Some(AsyncPipe::new(input)),
            output: Some(AsyncPipe::new(output)),
            job,
            console: console as usize,
        };
        drop(child_input);
        drop(child_output);
        let mut attributes = Attributes::new()?;
        checked(unsafe {
            ffi::UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                0x00020016,
                console,
                std::mem::size_of::<Raw>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        })?;
        let mut startup: StartupEx = unsafe { std::mem::zeroed() };
        startup.startup.size = std::mem::size_of::<StartupEx>() as u32;
        startup.attributes = attributes.pointer();
        let mut info: ProcessInfo = unsafe { std::mem::zeroed() };
        checked(unsafe {
            ffi::CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0x00080000 | 0x00000400 | 0x00000004, // extended startup, Unicode env, suspended
                environment.as_mut_ptr().cast(),
                directory.as_ptr(),
                &startup.startup,
                &mut info,
            )
        })?;
        let mut process = LaunchGuard {
            process: Handle::new(info.process)?,
            thread: Handle::new(info.thread)?,
            armed: true,
        };
        checked(unsafe {
            ffi::AssignProcessToJobObject(resources.job.raw(), process.process.raw())
        })?;
        // Start the one lifetime cleanup worker before allowing child code.
        let (cleanup_tx, cleanup_rx) = mpsc::sync_channel::<Resources>(1);
        thread::Builder::new()
            .name("screen-conpty-cleanup".into())
            .spawn(move || {
                if let Ok(resources) = cleanup_rx.recv() {
                    drop(resources);
                }
            })?;
        if unsafe { ffi::ResumeThread(process.thread.raw()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        process.armed = false;
        let handle = Handle(process.process.0);
        process.process.0 = 0;
        Ok(Self {
            resources: Some(resources),
            process: handle,
            pid: info.pid,
            status: None,
            cleanup: Some(cleanup_tx),
            stopping: false,
            etx_pending: false,
        })
    }
    pub fn child_pid(&self) -> u32 {
        self.pid
    }
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let resources = self
            .resources
            .as_ref()
            .ok_or_else(|| io::Error::other("ConPTY is closed"))?;
        hresult(unsafe {
            ffi::ResizePseudoConsole(resources.console as Raw, dimensions(cols, rows)?)
        })
    }
    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        if self.status.is_none() {
            match unsafe { ffi::WaitForSingleObject(self.process.raw(), 0) } {
                0 => {
                    let mut code = 0;
                    checked(unsafe { ffi::GetExitCodeProcess(self.process.raw(), &mut code) })?;
                    if let Some(resources) = &self.resources {
                        checked(unsafe { ffi::TerminateJobObject(resources.job.raw(), 1) })?;
                    }
                    self.status = Some(code as i32);
                }
                258 => {}
                _ => return Err(io::Error::last_os_error()),
            }
        }
        Ok(self.status)
    }
    pub fn terminate_group(&mut self, force: bool) -> io::Result<()> {
        let Some(resources) = &mut self.resources else {
            return Ok(());
        };
        self.stopping = true;
        if force {
            self.etx_pending = false;
            checked(unsafe { ffi::TerminateJobObject(resources.job.raw(), 1) })
        } else {
            // A control character reaches the foreground ConPTY application;
            // escalation targets the owned job, never the shared console.
            self.etx_pending = true;
            self.flush_control()
        }
    }
    fn flush_control(&mut self) -> io::Result<()> {
        if self.etx_pending {
            let Some(input) = self
                .resources
                .as_mut()
                .and_then(|resources| resources.input.as_mut())
            else {
                return Ok(());
            };
            match input.write(b"\x03") {
                Ok(_) => self.etx_pending = false,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) if matches!(error.raw_os_error(), Some(109 | 232)) => {
                    self.etx_pending = false
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    pub fn shutdown(&mut self) -> io::Result<Option<i32>> {
        let termination = self.terminate_group(true);
        if let Some(resources) = self.resources.take() {
            if let Some(sender) = self.cleanup.take() {
                if let Err(error) = sender.try_send(resources) {
                    // A disconnected cleanup worker is exceptional; retain
                    // RAII cleanup rather than orphaning a live Job Object.
                    match error {
                        mpsc::TrySendError::Full(resources)
                        | mpsc::TrySendError::Disconnected(resources) => drop(resources),
                    }
                }
            } else {
                drop(resources);
            }
        }
        termination?;
        self.try_wait()
    }
}
impl Read for Pty {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Some(input) = self
            .resources
            .as_mut()
            .and_then(|resources| resources.input.as_mut())
        {
            match input.advance_write() {
                Ok(()) => {}
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || matches!(error.raw_os_error(), Some(109 | 232)) => {}
                Err(error) => return Err(error),
            }
        }
        self.flush_control()?;
        self.resources
            .as_mut()
            .and_then(|resources| resources.output.as_mut())
            .ok_or_else(|| io::Error::other("ConPTY is closed"))?
            .read(bytes)
    }
}
impl Write for Pty {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.stopping {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.resources
            .as_mut()
            .and_then(|resources| resources.input.as_mut())
            .ok_or_else(|| io::Error::other("ConPTY is closed"))?
            .write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.resources
            .as_mut()
            .and_then(|resources| resources.input.as_mut())
            .ok_or_else(|| io::Error::other("ConPTY is closed"))?
            .advance_write()
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("makepad-screen: ConPTY cleanup: {error}");
        }
    }
}

struct LaunchGuard {
    process: Handle,
    thread: Handle,
    armed: bool,
}
impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if self.armed {
            unsafe {
                ffi::TerminateProcess(self.process.raw(), 1);
            }
        }
    }
}
struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut bytes = 0;
        unsafe {
            ffi::InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 || bytes > 65536 {
            return Err(io::Error::other("Invalid process attribute-list size"));
        }
        let mut result = Self {
            storage: vec![0; bytes.div_ceil(std::mem::size_of::<usize>())],
            initialized: false,
        };
        checked(unsafe {
            ffi::InitializeProcThreadAttributeList(result.pointer(), 1, 0, &mut bytes)
        })?;
        result.initialized = true;
        Ok(result)
    }
    fn pointer(&mut self) -> Raw {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                ffi::DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}

fn executable(program: &OsStr, cwd: &Path) -> io::Result<PathBuf> {
    let path = Path::new(program);
    if path.extension().is_some_and(|extension| {
        matches!(
            extension.to_string_lossy().to_ascii_lowercase().as_str(),
            "bat" | "cmd"
        )
    }) {
        return Err(io::Error::other("ConPTY requires a native executable; batch scripts need an explicit cmd.exe invocation"));
    }
    if path.is_absolute() || path.components().count() > 1 {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "ConPTY executable does not exist",
            ));
        }
        return Ok(path);
    }
    let name = wide(program)?;
    let extension = [b'.' as u16, b'e' as u16, b'x' as u16, b'e' as u16, 0];
    let mut output = vec![0u16; 32768];
    let count = unsafe {
        ffi::SearchPathW(
            std::ptr::null(),
            name.as_ptr(),
            extension.as_ptr(),
            output.len() as u32,
            output.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    if count == 0 {
        return Err(io::Error::last_os_error());
    }
    if count >= output.len() {
        return Err(io::Error::other("ConPTY executable path exceeds its bound"));
    }
    use std::os::windows::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_wide(&output[..count])))
}
fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::other("Windows process argument contains NUL"));
    }
    value.push(0);
    Ok(value)
}
fn command_line(program: &OsStr, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut result = Vec::new();
    for argument in std::iter::once(program).chain(args.iter().map(OsString::as_os_str)) {
        if !result.is_empty() {
            result.push(32);
        }
        result.push(34);
        let mut slashes = 0;
        for unit in argument.encode_wide() {
            if unit == 0 {
                return Err(io::Error::other("Windows process argument contains NUL"));
            }
            if unit == 92 {
                slashes += 1;
                continue;
            }
            result.extend(std::iter::repeat_n(
                92,
                slashes * if unit == 34 { 2 } else { 1 },
            ));
            if unit == 34 {
                result.push(92);
            }
            slashes = 0;
            result.push(unit);
        }
        result.extend(std::iter::repeat_n(92, slashes * 2));
        result.push(34);
        if result.len() >= 32767 {
            return Err(io::Error::other(
                "Windows process command exceeds 32767 UTF-16 units",
            ));
        }
    }
    result.push(0);
    Ok(result)
}
fn environment_block(overrides: &[(&OsStr, &OsStr)]) -> io::Result<Vec<u16>> {
    let mut entries = BTreeMap::new();
    for (key, value) in std::env::vars_os() {
        entries.insert(key.to_string_lossy().to_uppercase(), (key, value));
    }
    for &(key, value) in overrides {
        if key.is_empty() || key.encode_wide().any(|unit| unit == 0 || unit == 61) {
            return Err(io::Error::other("Invalid Windows environment key"));
        }
        entries.insert(
            key.to_string_lossy().to_uppercase(),
            (key.to_owned(), value.to_owned()),
        );
    }
    entries.remove("NO_COLOR");
    for (key, value) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")] {
        entries.insert(key.into(), (key.into(), value.into()));
    }
    let mut result = Vec::new();
    for (_, (key, value)) in entries {
        let key = wide(&key)?;
        let value = wide(&value)?;
        result.extend_from_slice(&key[..key.len() - 1]);
        result.push(61);
        result.extend(value);
        if result.len() > 128 * 1024 {
            return Err(io::Error::other("Windows environment exceeds 256KiB"));
        }
    }
    result.push(0);
    Ok(result)
}
fn checked(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn hresult(result: i32) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::other(format!(
            "ConPTY failed (HRESULT {result:#010x})"
        )))
    } else {
        Ok(())
    }
}
fn dimensions(cols: u16, rows: u16) -> io::Result<Coord> {
    if !(2..=400).contains(&cols) || !(2..=200).contains(&rows) {
        return Err(io::Error::other(
            "ConPTY dimensions must be 2..400 by 2..200",
        ));
    }
    Ok(Coord {
        x: cols as i16,
        y: rows as i16,
    })
}
#[repr(C)]
struct Coord {
    x: i16,
    y: i16,
}
#[repr(C)]
struct Startup {
    size: u32,
    reserved: Raw,
    desktop: Raw,
    title: Raw,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    chars_x: u32,
    chars_y: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved_size: u16,
    reserved_bytes: Raw,
    input: Raw,
    output: Raw,
    error: Raw,
}
#[repr(C)]
struct StartupEx {
    startup: Startup,
    attributes: Raw,
}
#[repr(C)]
struct ProcessInfo {
    process: Raw,
    thread: Raw,
    pid: u32,
    tid: u32,
}
#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    active_processes: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
#[derive(Default)]
struct JobLimits {
    basic: BasicLimits,
    io: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}
mod ffi {
    use super::{Coord, Overlapped, ProcessInfo, Raw, Startup};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CloseHandle(handle: Raw) -> i32;
        pub fn CreatePseudoConsole(
            size: Coord,
            input: Raw,
            output: Raw,
            flags: u32,
            console: *mut Raw,
        ) -> i32;
        pub fn ResizePseudoConsole(console: Raw, size: Coord) -> i32;
        pub fn ClosePseudoConsole(console: Raw);
        pub fn ReadFile(
            handle: Raw,
            buffer: Raw,
            size: u32,
            count: *mut u32,
            operation: *mut Overlapped,
        ) -> i32;
        pub fn WriteFile(
            handle: Raw,
            buffer: *const std::ffi::c_void,
            size: u32,
            count: *mut u32,
            operation: *mut Overlapped,
        ) -> i32;
        pub fn GetOverlappedResult(handle: Raw, operation: Raw, count: *mut u32, wait: i32) -> i32;
        pub fn CancelIoEx(handle: Raw, operation: Raw) -> i32;
        pub fn CreateJobObjectW(attributes: *const std::ffi::c_void, name: *const u16) -> Raw;
        pub fn SetInformationJobObject(
            job: Raw,
            kind: u32,
            info: *const std::ffi::c_void,
            length: u32,
        ) -> i32;
        pub fn AssignProcessToJobObject(job: Raw, process: Raw) -> i32;
        pub fn TerminateJobObject(job: Raw, exit_code: u32) -> i32;
        pub fn CreateProcessW(
            application: *const u16,
            command: *mut u16,
            process_security: *const std::ffi::c_void,
            thread_security: *const std::ffi::c_void,
            inherit: i32,
            flags: u32,
            environment: Raw,
            cwd: *const u16,
            startup: *const Startup,
            info: *mut ProcessInfo,
        ) -> i32;
        pub fn InitializeProcThreadAttributeList(
            list: Raw,
            count: u32,
            flags: u32,
            size: *mut usize,
        ) -> i32;
        pub fn UpdateProcThreadAttribute(
            list: Raw,
            flags: u32,
            attribute: usize,
            value: Raw,
            size: usize,
            previous: Raw,
            returned: *mut usize,
        ) -> i32;
        pub fn DeleteProcThreadAttributeList(list: Raw);
        pub fn ResumeThread(thread: Raw) -> u32;
        pub fn WaitForSingleObject(handle: Raw, milliseconds: u32) -> u32;
        pub fn GetExitCodeProcess(process: Raw, exit_code: *mut u32) -> i32;
        pub fn TerminateProcess(process: Raw, exit_code: u32) -> i32;
        pub fn SearchPathW(
            path: *const u16,
            file: *const u16,
            extension: *const u16,
            size: u32,
            output: *mut u16,
            file_part: *mut *mut u16,
        ) -> u32;
    }
}
