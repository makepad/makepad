//! Free-VT guard from the native direct shell, with VT_PROCESS release/acquire handshakes.
//! The signal handler only writes a wake byte. Display draining happens on the presenter.
use super::sys;
use std::{fs::{File, OpenOptions}, io::{self, Read}, os::{fd::AsRawFd, unix::net::UnixStream},
    sync::atomic::{AtomicI32, AtomicU32, Ordering}};

const KDSETMODE: u64 = 0x4b3a;
const KDGETMODE: u64 = 0x4b3b;
const KDGKBMODE: u64 = 0x4b44;
const KDSKBMODE: u64 = 0x4b45;
const VT_GETMODE: u64 = 0x5601;
const VT_SETMODE: u64 = 0x5602;
const VT_GETSTATE: u64 = 0x5603;
const VT_RELDISP: u64 = 0x5605;
const VT_ACTIVATE: u64 = 0x5606;
const TIOCGDEV: u64 = 0x80045432;
const RELEASE_SIGNAL: i32 = 10;
const ACQUIRE_SIGNAL: i32 = 12;
static SIGNAL_FD: AtomicI32 = AtomicI32::new(-1);
static SIGNALS: AtomicU32 = AtomicU32::new(0);

extern "C" fn vt_signal(signal: i32) {
    SIGNALS.fetch_or(if signal == RELEASE_SIGNAL { 1 } else { 2 }, Ordering::Release);
    let fd = SIGNAL_FD.load(Ordering::Relaxed);
    if fd >= 0 { unsafe { sys::write(fd, [1u8].as_ptr().cast(), 1); } }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct VtMode { mode: u8, waitv: u8, relsig: i16, acqsig: i16, frsig: i16 }
#[repr(C)]
#[derive(Default)]
struct VtState { active: u16, signal: u16, state: u16 }

pub(super) struct DirectTerminal {
    file: File,
    keyboard: i32,
    display: i32,
    mode: VtMode,
    handlers: [usize; 2],
    reader: UnixStream,
    _writer: UnixStream,
}

impl DirectTerminal {
    pub fn enter() -> Result<Option<Self>, String> {
        let file = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
            Ok(file) => file, Err(_) => return Ok(None),
        };
        let fd = file.as_raw_fd();
        let mut display = 0;
        if unsafe { sys::ioctl(fd, KDGETMODE as _, &mut display) } < 0 { return Ok(None); }
        if display != 0 { return Err("direct display requires a free text VT".into()); }
        let mut state = VtState::default();
        let mut device = 0u32;
        if unsafe { sys::ioctl(fd, VT_GETSTATE as _, &mut state) } < 0
            || unsafe { sys::ioctl(fd, TIOCGDEV as _, &mut device) } < 0 { return Err(io::Error::last_os_error().to_string()); }
        if device != ((4 << 8) | u32::from(state.active)) { return Err("launch direct display from the active text VT".into()); }
        let mut keyboard = 0;
        let mut mode = VtMode::default();
        if unsafe { sys::ioctl(fd, KDGKBMODE as _, &mut keyboard) } < 0
            || unsafe { sys::ioctl(fd, VT_GETMODE as _, &mut mode) } < 0 { return Err(io::Error::last_os_error().to_string()); }
        let (reader, writer) = UnixStream::pair().map_err(|e| e.to_string())?;
        reader.set_nonblocking(true).map_err(|e| e.to_string())?;
        writer.set_nonblocking(true).map_err(|e| e.to_string())?;
        SIGNALS.store(0, Ordering::Release);
        SIGNAL_FD.store(writer.as_raw_fd(), Ordering::Release);
        let handlers = unsafe { [sys::signal(RELEASE_SIGNAL, vt_signal as *const () as usize),
            sys::signal(ACQUIRE_SIGNAL, vt_signal as *const () as usize)] };
        let guard = Self { file, keyboard, display, mode, handlers, reader, _writer: writer };
        if handlers.contains(&usize::MAX) { return Err("install VT signal handlers failed".into()); }
        let process = VtMode { mode: 1, relsig: RELEASE_SIGNAL as i16, acqsig: ACQUIRE_SIGNAL as i16, ..VtMode::default() };
        if unsafe { sys::ioctl(fd, VT_SETMODE as _, &process) } < 0 { return Err(io::Error::last_os_error().to_string()); }
        guard.graphics()?;
        Ok(Some(guard))
    }

    fn graphics(&self) -> Result<(), String> {
        let fd = self.file.as_raw_fd();
        if unsafe { sys::ioctl(fd, KDSETMODE as _, 1i32) } < 0
            || unsafe { sys::ioctl(fd, KDSKBMODE as _, 4i32) } < 0 { return Err(io::Error::last_os_error().to_string()); }
        Ok(())
    }
    pub fn fd(&self) -> i32 { self.reader.as_raw_fd() }
    pub fn signals(&mut self) -> u32 {
        let mut bytes = [0; 64];
        while self.reader.read(&mut bytes).is_ok_and(|n| n > 0) {}
        SIGNALS.swap(0, Ordering::AcqRel)
    }
    /// Called only after the display worker has drained and released every display/card.
    pub fn release(&self) -> Result<(), String> {
        if unsafe { sys::ioctl(self.file.as_raw_fd(), VT_RELDISP as _, 1i32) } < 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    pub fn acquire(&self) -> Result<(), String> {
        if unsafe { sys::ioctl(self.file.as_raw_fd(), VT_RELDISP as _, 2i32) } < 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        self.graphics()
    }
    pub fn switch(&self, vt: u32) {
        if unsafe { sys::ioctl(self.file.as_raw_fd(), VT_ACTIVATE as _, vt) } < 0 {
            eprintln!("[hal-direct] switch VT: {}", io::Error::last_os_error());
        }
    }
}

impl Drop for DirectTerminal {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        for (request, value) in [(KDSKBMODE, self.keyboard), (KDSETMODE, self.display)] {
            if unsafe { sys::ioctl(fd, request as _, value) } < 0 { eprintln!("[hal-direct] restore console: {}", io::Error::last_os_error()); }
        }
        if unsafe { sys::ioctl(fd, VT_SETMODE as _, &self.mode) } < 0 { eprintln!("[hal-direct] restore VT mode: {}", io::Error::last_os_error()); }
        unsafe {
            if self.handlers[0] != usize::MAX { sys::signal(RELEASE_SIGNAL, self.handlers[0]); }
            if self.handlers[1] != usize::MAX { sys::signal(ACQUIRE_SIGNAL, self.handlers[1]); }
        }
        SIGNAL_FD.store(-1, Ordering::Release);
    }
}
