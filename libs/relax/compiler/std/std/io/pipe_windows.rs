//! Anonymous pipes on Windows (real std's io::pipe over sys/pipe/windows.rs: CreatePipe with no
//! security attributes, so neither end is inheritable). The handle conversions are below in
//! os/windows/io style, the Stdio ones in process_windows.rs.

use core::ffi::c_void;
use core::fmt;

use crate::io::{IoSlice, IoSliceMut, Read, Result, Write};
use crate::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};

#[link(name = "kernel32")]
extern "system" {
    fn CreatePipe(read: *mut RawHandle, write: *mut RawHandle, attrs: *mut c_void, size: u32) -> i32;
    fn ReadFile(h: RawHandle, buf: *mut u8, n: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn WriteFile(h: RawHandle, buf: *const u8, n: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
}

const INVALID_HANDLE_VALUE: RawHandle = -1isize as RawHandle;

/// Real std's Windows Pipe is a sys Handle(OwnedHandle); Debug prints the same nesting.
struct Handle(OwnedHandle);

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Handle").field(&self.0).finish()
    }
}

pub struct PipeReader(Handle);
pub struct PipeWriter(Handle);

impl fmt::Debug for PipeReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PipeReader").field(&self.0).finish()
    }
}
impl fmt::Debug for PipeWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PipeWriter").field(&self.0).finish()
    }
}

/// An anonymous pipe: (read end, write end), neither inheritable by child processes.
pub fn pipe() -> Result<(PipeReader, PipeWriter)> {
    let mut r: RawHandle = INVALID_HANDLE_VALUE;
    let mut w: RawHandle = INVALID_HANDLE_VALUE;
    if unsafe { CreatePipe(&mut r, &mut w, core::ptr::null_mut(), 0) } == 0 {
        return Err(crate::io::Error::last_os_error());
    }
    unsafe { Ok((PipeReader(Handle(OwnedHandle::from_raw_handle(r))), PipeWriter(Handle(OwnedHandle::from_raw_handle(w))))) }
}

/// ReadFile on a synchronous handle; a closed write end (ERROR_BROKEN_PIPE) reads as end of file
/// (real std's Handle::read).
fn read_handle(h: RawHandle, buf: &mut [u8]) -> Result<usize> {
    let len = if buf.len() > u32::MAX as usize { u32::MAX } else { buf.len() as u32 };
    let mut n: u32 = 0;
    if unsafe { ReadFile(h, buf.as_mut_ptr(), len, &mut n, core::ptr::null_mut()) } == 0 {
        let e = crate::io::Error::last_os_error();
        if e.kind() == crate::io::ErrorKind::BrokenPipe {
            return Ok(0);
        }
        return Err(e);
    }
    Ok(n as usize)
}

/// WriteFile on a synchronous handle, at most u32::MAX bytes at a time (real std's Handle::write).
fn write_handle(h: RawHandle, buf: &[u8]) -> Result<usize> {
    let len = if buf.len() > u32::MAX as usize { u32::MAX } else { buf.len() as u32 };
    let mut n: u32 = 0;
    if unsafe { WriteFile(h, buf.as_ptr(), len, &mut n, core::ptr::null_mut()) } == 0 {
        return Err(crate::io::Error::last_os_error());
    }
    Ok(n as usize)
}

impl PipeReader {
    pub fn try_clone(&self) -> Result<PipeReader> {
        Ok(PipeReader(Handle(self.0 .0.try_clone()?)))
    }
}
impl PipeWriter {
    pub fn try_clone(&self) -> Result<PipeWriter> {
        Ok(PipeWriter(Handle(self.0 .0.try_clone()?)))
    }
}

impl Read for &PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        read_handle(self.0 .0.as_raw_handle(), buf)
    }
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        for b in bufs.iter_mut() {
            if !b.is_empty() {
                return self.read(&mut **b);
            }
        }
        Ok(0)
    }
}
impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        (&*self).read(buf)
    }
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        (&*self).read_vectored(bufs)
    }
}
impl Write for &PipeWriter {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        write_handle(self.0 .0.as_raw_handle(), buf)
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        for b in bufs.iter() {
            if !b.is_empty() {
                return self.write(&**b);
            }
        }
        Ok(0)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        (&*self).write(buf)
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        (&*self).write_vectored(bufs)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl AsRawHandle for PipeReader {
    fn as_raw_handle(&self) -> RawHandle {
        self.0 .0.as_raw_handle()
    }
}
impl AsRawHandle for PipeWriter {
    fn as_raw_handle(&self) -> RawHandle {
        self.0 .0.as_raw_handle()
    }
}
impl FromRawHandle for PipeReader {
    unsafe fn from_raw_handle(h: RawHandle) -> PipeReader {
        PipeReader(Handle(OwnedHandle::from_raw_handle(h)))
    }
}
impl FromRawHandle for PipeWriter {
    unsafe fn from_raw_handle(h: RawHandle) -> PipeWriter {
        PipeWriter(Handle(OwnedHandle::from_raw_handle(h)))
    }
}
impl IntoRawHandle for PipeReader {
    fn into_raw_handle(self) -> RawHandle {
        self.0 .0.into_raw_handle()
    }
}
impl IntoRawHandle for PipeWriter {
    fn into_raw_handle(self) -> RawHandle {
        self.0 .0.into_raw_handle()
    }
}
impl AsHandle for PipeReader {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.0 .0.as_handle()
    }
}
impl AsHandle for PipeWriter {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.0 .0.as_handle()
    }
}
impl From<PipeReader> for OwnedHandle {
    fn from(p: PipeReader) -> OwnedHandle {
        p.0 .0
    }
}
impl From<PipeWriter> for OwnedHandle {
    fn from(p: PipeWriter) -> OwnedHandle {
        p.0 .0
    }
}
impl From<OwnedHandle> for PipeReader {
    fn from(h: OwnedHandle) -> PipeReader {
        PipeReader(Handle(h))
    }
}
impl From<OwnedHandle> for PipeWriter {
    fn from(h: OwnedHandle) -> PipeWriter {
        PipeWriter(Handle(h))
    }
}
