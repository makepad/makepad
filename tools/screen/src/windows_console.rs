//! Windows console boundary. Blocking standard-handle I/O stays on two
//! bounded, cancellable workers; the attachment loop never waits on a read.
use std::{
    ffi::c_void,
    fs::File,
    io::{self, Read, Write},
    os::windows::io::{AsRawHandle, FromRawHandle, RawHandle},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

type Handle = *mut c_void;
static INTERRUPTED: AtomicU32 = AtomicU32::new(0);
unsafe extern "system" fn control_handler(event: u32) -> i32 {
    if matches!(event, 0 | 1 | 2 | 5 | 6) {
        let _ = INTERRUPTED.compare_exchange(0, event + 1, Ordering::Relaxed, Ordering::Relaxed);
        1
    } else {
        0
    }
}

pub struct Console {
    input: Handle,
    output: Handle,
    input_mode: Option<u32>,
    output_mode: Option<u32>,
    input_cp: Option<u32>,
    output_cp: Option<u32>,
    title: Option<Vec<u16>>,
    handler: bool,
}

impl Console {
    pub fn new() -> io::Result<Self> {
        let input = unsafe { ffi::GetStdHandle(-10i32 as u32) };
        let output = unsafe { ffi::GetStdHandle(-11i32 as u32) };
        if invalid(input) || invalid(output) {
            return Err(io::Error::other(
                "Attach requires valid standard input and output handles",
            ));
        }
        let mut console = Self {
            input,
            output,
            input_mode: console_mode(input),
            output_mode: console_mode(output),
            input_cp: None,
            output_cp: None,
            title: None,
            handler: false,
        };
        if let Some(mode) = console.input_mode {
            // VT input supplies UTF-8 escape sequences, including Ctrl-C as
            // input. Quick-edit must not freeze a running attachment.
            let raw = (mode | 0x200 | 0x80) & !(0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x40);
            checked(unsafe { ffi::SetConsoleMode(input, raw) })?;
            let cp = unsafe { ffi::GetConsoleCP() };
            if cp == 0 {
                return Err(io::Error::last_os_error());
            }
            console.input_cp = Some(cp);
            checked(unsafe { ffi::SetConsoleCP(65001) })?;
        }
        if let Some(mode) = console.output_mode {
            checked(unsafe { ffi::SetConsoleMode(output, mode | 0x1 | 0x2 | 0x4 | 0x8) })?;
            let cp = unsafe { ffi::GetConsoleOutputCP() };
            if cp == 0 {
                return Err(io::Error::last_os_error());
            }
            console.output_cp = Some(cp);
            checked(unsafe { ffi::SetConsoleOutputCP(65001) })?;
            let mut title = vec![0u16; 32768];
            let count =
                unsafe { ffi::GetConsoleTitleW(title.as_mut_ptr(), title.len() as u32) } as usize;
            if count < title.len() {
                title.truncate(count + 1);
                title[count] = 0;
                console.title = Some(title);
            }
        }
        INTERRUPTED.store(0, Ordering::Relaxed);
        checked(unsafe { ffi::SetConsoleCtrlHandler(Some(control_handler), 1) })?;
        console.handler = true;
        Ok(console)
    }

    pub fn interrupted(&self) -> Option<u32> {
        INTERRUPTED.load(Ordering::Relaxed).checked_sub(1)
    }
    pub fn is_terminal(&self) -> bool {
        self.output_mode.is_some()
    }
    pub fn size(&self) -> io::Result<(u16, u16)> {
        let size = if self.is_terminal() {
            let mut info = ConsoleInfo::default();
            checked(unsafe { ffi::GetConsoleScreenBufferInfo(self.output, &mut info) })?;
            let width = i32::from(info.window.right) - i32::from(info.window.left) + 1;
            let height = i32::from(info.window.bottom) - i32::from(info.window.top) + 1;
            (
                u16::try_from(width).unwrap_or(0),
                u16::try_from(height).unwrap_or(0),
            )
        } else {
            let dimension = |name: &str, default| {
                std::env::var(name)
                    .ok()
                    .and_then(|value| value.parse::<u16>().ok())
                    .unwrap_or(default)
            };
            (dimension("COLUMNS", 80), dimension("LINES", 24))
        };
        if !(2..=400).contains(&size.0) || !(2..=200).contains(&size.1) {
            return Err(io::Error::other(format!("Terminal size {}×{} is outside 2..400 columns by 2..200 rows; resize before attaching", size.0, size.1)));
        }
        Ok(size)
    }

    pub fn io(&self) -> io::Result<ConsoleIo> {
        ConsoleIo::new(duplicate(self.input)?, duplicate(self.output)?)
    }

    pub fn restore(&mut self) -> io::Result<()> {
        let mut failure = None;
        let mut record = |result: io::Result<()>| {
            if failure.is_none() {
                failure = result.err();
            }
        };
        if self.handler {
            record(checked(unsafe {
                ffi::SetConsoleCtrlHandler(Some(control_handler), 0)
            }));
            self.handler = false;
        }
        if let Some(title) = self.title.take() {
            record(checked(unsafe { ffi::SetConsoleTitleW(title.as_ptr()) }));
        }
        if let Some(cp) = self.output_cp.take() {
            record(checked(unsafe { ffi::SetConsoleOutputCP(cp) }));
        }
        if let Some(cp) = self.input_cp.take() {
            record(checked(unsafe { ffi::SetConsoleCP(cp) }));
        }
        if let Some(mode) = self.output_mode.take() {
            record(checked(unsafe { ffi::SetConsoleMode(self.output, mode) }));
        }
        if let Some(mode) = self.input_mode.take() {
            record(checked(unsafe { ffi::SetConsoleMode(self.input, mode) }));
        }
        failure.map_or(Ok(()), Err)
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

enum Input {
    Bytes(Vec<u8>),
    End,
    Error(io::Error),
}
pub struct ConsoleIo {
    input: Option<Receiver<Input>>,
    output: Option<SyncSender<Vec<u8>>>,
    errors: Receiver<String>,
    written: Arc<AtomicUsize>,
    queued: usize,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}
impl ConsoleIo {
    fn new(mut input: File, mut output: File) -> io::Result<Self> {
        let (input_tx, input_rx) = mpsc::sync_channel(8);
        let (output_tx, output_rx) = mpsc::sync_channel::<Vec<u8>>(32);
        let (error_tx, errors) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let written = Arc::new(AtomicUsize::new(0));
        let mut workers = Self {
            input: Some(input_rx),
            output: Some(output_tx),
            errors,
            written: written.clone(),
            queued: 0,
            stop: stop.clone(),
            reader: None,
            writer: None,
        };
        let reading = stop.clone();
        workers.reader = Some(
            thread::Builder::new()
                .name("screen-console-input".into())
                .spawn(move || {
                    let mut bytes = [0u8; 4096];
                    while !reading.load(Ordering::Acquire) {
                        let event = match input.read(&mut bytes) {
                            Ok(0) => Input::End,
                            Ok(count) => Input::Bytes(bytes[..count].to_vec()),
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                            Err(error) => Input::Error(error),
                        };
                        let ended = !matches!(event, Input::Bytes(_));
                        if reading.load(Ordering::Acquire) || input_tx.send(event).is_err() || ended
                        {
                            break;
                        }
                    }
                })?,
        );
        workers.writer = Some(
            thread::Builder::new()
                .name("screen-console-output".into())
                .spawn(move || {
                    while let Ok(bytes) = output_rx.recv() {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        if let Err(error) = output.write_all(&bytes) {
                            let _ = error_tx.try_send(error.to_string());
                            break;
                        }
                        written.fetch_add(bytes.len(), Ordering::Release);
                    }
                })?,
        );
        Ok(workers)
    }

    pub fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        match self
            .input
            .as_ref()
            .ok_or_else(|| io::Error::other("Console input closed"))?
            .try_recv()
        {
            Ok(Input::Bytes(bytes)) => Ok(Some(bytes)),
            Ok(Input::End) | Err(TryRecvError::Disconnected) => Ok(Some(Vec::new())),
            Ok(Input::Error(error)) => Err(error),
            Err(TryRecvError::Empty) => Ok(None),
        }
    }
    pub fn error(&self) -> io::Result<()> {
        match self.errors.try_recv() {
            Ok(error) => Err(io::Error::other(error)),
            Err(_) => Ok(()),
        }
    }
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.error()?;
        let count = bytes.len().min(16384);
        let sender = self
            .output
            .as_ref()
            .ok_or_else(|| io::Error::other("Console output closed"))?;
        match sender.try_send(bytes[..count].to_vec()) {
            Ok(()) => {
                self.queued = self.queued.wrapping_add(count);
                Ok(count)
            }
            Err(TrySendError::Full(_)) => Err(io::ErrorKind::WouldBlock.into()),
            Err(TrySendError::Disconnected(_)) => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }
    pub fn drained(&self) -> bool {
        self.written.load(Ordering::Acquire) == self.queued
    }
    pub fn restore_vt(&mut self, bytes: &[u8]) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut offset = 0;
        while offset < bytes.len() || !self.drained() {
            self.error()?;
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Console restoration did not drain within 500ms",
                ));
            }
            if offset < bytes.len() {
                match self.write(&bytes[offset..]) {
                    Ok(count) => offset += count,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    pub fn shutdown(&mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Release);
        self.input.take();
        self.output.take();
        let deadline = Instant::now() + Duration::from_millis(500);
        while self
            .reader
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
            || self
                .writer
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
        {
            for thread in [&self.reader, &self.writer].into_iter().flatten() {
                if !thread.is_finished() {
                    unsafe {
                        ffi::CancelSynchronousIo(thread.as_raw_handle());
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Console I/O workers did not cancel within 500ms",
                ));
            }
            thread::sleep(Duration::from_millis(5));
        }
        for thread in [self.reader.take(), self.writer.take()]
            .into_iter()
            .flatten()
        {
            thread
                .join()
                .map_err(|_| io::Error::other("Console worker panicked"))?;
        }
        Ok(())
    }
}
impl Drop for ConsoleIo {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn checked(value: i32) -> io::Result<()> {
    if value != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
fn invalid(handle: Handle) -> bool {
    handle.is_null() || handle == -1isize as Handle
}
fn console_mode(handle: Handle) -> Option<u32> {
    let mut mode = 0;
    (unsafe { ffi::GetConsoleMode(handle, &mut mode) } != 0).then_some(mode)
}
fn duplicate(handle: Handle) -> io::Result<File> {
    let process = unsafe { ffi::GetCurrentProcess() };
    let mut copy = std::ptr::null_mut();
    checked(unsafe { ffi::DuplicateHandle(process, handle, process, &mut copy, 0, 0, 2) })?;
    Ok(unsafe { File::from_raw_handle(copy as RawHandle) })
}
#[repr(C)]
#[derive(Default)]
struct Coord {
    x: i16,
    y: i16,
}
#[repr(C)]
#[derive(Default)]
struct SmallRect {
    left: i16,
    top: i16,
    right: i16,
    bottom: i16,
}
#[repr(C)]
#[derive(Default)]
struct ConsoleInfo {
    size: Coord,
    cursor: Coord,
    attributes: u16,
    window: SmallRect,
    maximum: Coord,
}
mod ffi {
    use super::{ConsoleInfo, Handle};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetStdHandle(kind: u32) -> Handle;
        pub fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
        pub fn SetConsoleMode(handle: Handle, mode: u32) -> i32;
        pub fn GetConsoleCP() -> u32;
        pub fn SetConsoleCP(code_page: u32) -> i32;
        pub fn GetConsoleOutputCP() -> u32;
        pub fn SetConsoleOutputCP(code_page: u32) -> i32;
        pub fn GetConsoleTitleW(title: *mut u16, size: u32) -> u32;
        pub fn SetConsoleTitleW(title: *const u16) -> i32;
        pub fn GetConsoleScreenBufferInfo(handle: Handle, info: *mut ConsoleInfo) -> i32;
        pub fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
        pub fn GetCurrentProcess() -> Handle;
        pub fn DuplicateHandle(
            source: Handle,
            handle: Handle,
            target: Handle,
            copy: *mut Handle,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        pub fn CancelSynchronousIo(thread: Handle) -> i32;
    }
}
