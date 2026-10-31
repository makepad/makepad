//! wl_shm pools: read-only client memory maps with a SIGBUS guard. A client may truncate the
//! file behind a pool at any time; touching the lost pages raises SIGBUS. Reads run inside a
//! guard whose handler replaces the faulting page with zeroes and records the fault, so the
//! copy completes and the caller reports an error instead of the compositor dying.
use crate::sys::*;
use std::cell::Cell;
use std::ffi::c_void;
use std::fs::File;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Once;

#[derive(Debug, PartialEq, Eq)]
pub enum ShmError { BadFd, TooSmall, Map, BadBuffer, Truncated }

pub const PAGE: usize = 4096;

thread_local! {
    static GUARD_LO: Cell<usize> = const { Cell::new(0) };
    static GUARD_HI: Cell<usize> = const { Cell::new(0) };
    static FAULTED: Cell<bool> = const { Cell::new(false) };
}
static INSTALL: Once = Once::new();

extern "C" fn on_sigbus(sig: i32, info: *const u8, _ctx: *mut c_void) {
    // si_addr sits at offset 16 of siginfo_t on 64-bit Linux.
    let addr = unsafe { std::ptr::read_unaligned(info.add(16) as *const usize) };
    let (lo, hi) = (GUARD_LO.with(|c| c.get()), GUARD_HI.with(|c| c.get()));
    if addr >= lo && addr < hi {
        let page = addr & !(PAGE - 1);
        let r = unsafe { mmap(page as *mut c_void, PAGE, PROT_READ, MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS, -1, 0) };
        if r as usize == page {
            FAULTED.with(|c| c.set(true));
            return;
        }
    }
    // Not ours: restore the default action; the retried access terminates the process.
    let dfl = sigaction_t { handler: 0, mask: [0; 16], flags: 0, restorer: 0 };
    unsafe { sigaction(sig, &dfl, std::ptr::null_mut()) };
}
fn install_handler() {
    INSTALL.call_once(|| {
        let act = sigaction_t { handler: on_sigbus as extern "C" fn(i32, *const u8, *mut c_void) as usize, mask: [0; 16], flags: SA_SIGINFO | SA_NODEFER, restorer: 0 };
        unsafe { sigaction(SIGBUS, &act, std::ptr::null_mut()) };
    });
}

pub struct Pool {
    base: *mut u8,
    len: usize,
    fd: OwnedFd,
    /// Set once a read hit a truncated page; those pages are zeroes from then on.
    poisoned: Cell<bool>,
}
unsafe impl Send for Pool {}

fn file_len(fd: &OwnedFd) -> Result<u64, ShmError> {
    let f = File::from(fd.try_clone().map_err(|_| ShmError::BadFd)?);
    f.metadata().map(|m| m.len()).map_err(|_| ShmError::BadFd)
}
fn map(fd: &OwnedFd, len: usize) -> Result<*mut u8, ShmError> {
    if len == 0 { return Err(ShmError::TooSmall); }
    if file_len(fd)? < len as u64 { return Err(ShmError::TooSmall); }
    let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_SHARED, fd.as_raw_fd(), 0) };
    if p as isize == -1 { Err(ShmError::Map) } else { Ok(p as *mut u8) }
}

impl Pool {
    pub fn new(fd: OwnedFd, size: usize) -> Result<Pool, ShmError> {
        install_handler();
        let base = map(&fd, size)?;
        Ok(Pool { base, len: size, fd, poisoned: Cell::new(false) })
    }
    pub fn len(&self) -> usize { self.len }
    /// Pools only grow.
    pub fn resize(&mut self, size: usize) -> Result<(), ShmError> {
        if size < self.len { return Err(ShmError::TooSmall); }
        let base = map(&self.fd, size)?;
        unsafe { munmap(self.base as *mut c_void, self.len) };
        self.base = base;
        self.len = size;
        Ok(())
    }
    /// Copies `out.len()` bytes from `offset`. `Truncated` if the client shrank the file under us
    /// (the copy still completes, with zeroes for the lost pages).
    pub fn read(&self, offset: usize, out: &mut [u8]) -> Result<(), ShmError> {
        let end = offset.checked_add(out.len()).ok_or(ShmError::BadBuffer)?;
        if end > self.len { return Err(ShmError::BadBuffer); }
        if out.is_empty() { return Ok(()); }
        if self.poisoned.get() { return Err(ShmError::Truncated); }
        let src = self.base as usize + offset;
        // Volatile accesses and fences: the handler writes these from a signal, so the
        // compiler must not reorder or drop them around the copy.
        use std::sync::atomic::{compiler_fence, Ordering};
        GUARD_LO.with(|c| unsafe { std::ptr::write_volatile(c.as_ptr(), self.base as usize) });
        GUARD_HI.with(|c| unsafe { std::ptr::write_volatile(c.as_ptr(), self.base as usize + self.len) });
        FAULTED.with(|c| unsafe { std::ptr::write_volatile(c.as_ptr(), false) });
        compiler_fence(Ordering::SeqCst);
        unsafe { std::ptr::copy_nonoverlapping(src as *const u8, out.as_mut_ptr(), out.len()) };
        compiler_fence(Ordering::SeqCst);
        GUARD_LO.with(|c| unsafe { std::ptr::write_volatile(c.as_ptr(), 0) });
        GUARD_HI.with(|c| unsafe { std::ptr::write_volatile(c.as_ptr(), 0) });
        if FAULTED.with(|c| unsafe { std::ptr::read_volatile(c.as_ptr()) }) { self.poisoned.set(true); Err(ShmError::Truncated) } else { Ok(()) }
    }
}
impl Drop for Pool {
    fn drop(&mut self) { unsafe { munmap(self.base as *mut c_void, self.len) }; }
}

pub const ARGB8888: u32 = 0;
pub const XRGB8888: u32 = 1;
pub const ABGR8888: u32 = 0x3432_4241;
pub const XBGR8888: u32 = 0x3432_4258;

pub fn format_supported(f: u32) -> bool { matches!(f, ARGB8888 | XRGB8888 | ABGR8888 | XBGR8888) }
pub fn has_alpha(f: u32) -> bool { matches!(f, ARGB8888 | ABGR8888) }

/// A buffer inside a pool. Validated against the pool size on creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferSpec { pub offset: usize, pub width: usize, pub height: usize, pub stride: usize, pub format: u32 }
impl BufferSpec {
    pub fn new(pool_len: usize, offset: i32, width: i32, height: i32, stride: i32, format: u32) -> Result<Self, ShmError> {
        if offset < 0 || width <= 0 || height <= 0 || stride <= 0 || !format_supported(format) { return Err(ShmError::BadBuffer); }
        let (o, w, h, s) = (offset as usize, width as usize, height as usize, stride as usize);
        if s < w.checked_mul(4).ok_or(ShmError::BadBuffer)? { return Err(ShmError::BadBuffer); }
        let end = s.checked_mul(h - 1).and_then(|v| v.checked_add(o)).and_then(|v| v.checked_add(w * 4)).ok_or(ShmError::BadBuffer)?;
        if end > pool_len { return Err(ShmError::BadBuffer); }
        Ok(Self { offset: o, width: w, height: h, stride: s, format })
    }
}
/// Copies rows `y0..y1` of a buffer into `out` as tightly packed rows (`width*4` bytes each).
pub fn copy_rows(pool: &Pool, b: &BufferSpec, y0: usize, y1: usize, out: &mut Vec<u8>) -> Result<(), ShmError> {
    let y1 = y1.min(b.height);
    if y0 >= y1 { return Ok(()); }
    let row = b.width * 4;
    let start = out.len();
    out.resize(start + (y1 - y0) * row, 0);
    let mut res = Ok(());
    for (i, y) in (y0..y1).enumerate() {
        let r = pool.read(b.offset + y * b.stride, &mut out[start + i * row..start + (i + 1) * row]);
        if r.is_err() { res = r; }
    }
    res
}
