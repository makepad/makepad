//! Anonymous pipes (real std's io::pipe, PipeReader, PipeWriter; unix: pipe2 with O_CLOEXEC on
//! Linux, pipe + FD_CLOEXEC on macOS). The fd conversions are in os/fd.rs style below, the Stdio ones
//! in process_unix.rs.

use core::ffi::{c_int, c_void};
use core::fmt;

use crate::io::{IoSlice, IoSliceMut, Read, Result, Write};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::sys;

/// Real std's unix Pipe is a FileDesc(OwnedFd); Debug prints the same nesting.
struct FileDesc(OwnedFd);

impl fmt::Debug for FileDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FileDesc").field(&self.0).finish()
    }
}

pub struct PipeReader(FileDesc);
pub struct PipeWriter(FileDesc);

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

/// An anonymous pipe: (read end, write end), both close-on-exec.
pub fn pipe() -> Result<(PipeReader, PipeWriter)> {
    let mut fds = [0 as c_int; 2];
    unsafe {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            if sys::pipe2(fds.as_mut_ptr(), sys::os::O_CLOEXEC) < 0 {
                return Err(crate::io::Error::last_os_error());
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            if sys::pipe(fds.as_mut_ptr()) < 0 {
                return Err(crate::io::Error::last_os_error());
            }
        }
        let r = OwnedFd::from_raw_fd(fds[0]);
        let w = OwnedFd::from_raw_fd(fds[1]);
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            for fd in [fds[0], fds[1]] {
                if sys::fcntl(fd, sys::F_SETFD, sys::FD_CLOEXEC) < 0 {
                    return Err(crate::io::Error::last_os_error());
                }
            }
        }
        Ok((PipeReader(FileDesc(r)), PipeWriter(FileDesc(w))))
    }
}

/// real std caps one read/write at this many bytes (macOS rejects larger counts)
const READ_LIMIT: usize = crate::os::unix::fs::READ_LIMIT;

fn read_fd(fd: RawFd, buf: &mut [u8]) -> Result<usize> {
    let len = if buf.len() > READ_LIMIT { READ_LIMIT } else { buf.len() };
    let n = unsafe { sys::read(fd, buf.as_mut_ptr() as *mut c_void, len) };
    if n < 0 { Err(crate::io::Error::last_os_error()) } else { Ok(n as usize) }
}

fn write_fd(fd: RawFd, buf: &[u8]) -> Result<usize> {
    let len = if buf.len() > READ_LIMIT { READ_LIMIT } else { buf.len() };
    let n = unsafe { sys::write(fd, buf.as_ptr() as *const c_void, len) };
    if n < 0 { Err(crate::io::Error::last_os_error()) } else { Ok(n as usize) }
}

impl PipeReader {
    pub fn try_clone(&self) -> Result<PipeReader> {
        Ok(PipeReader(FileDesc(self.0 .0.try_clone()?)))
    }
}
impl PipeWriter {
    pub fn try_clone(&self) -> Result<PipeWriter> {
        Ok(PipeWriter(FileDesc(self.0 .0.try_clone()?)))
    }
}

impl Read for &PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        read_fd(self.0 .0.as_raw_fd(), buf)
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
        write_fd(self.0 .0.as_raw_fd(), buf)
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

impl AsRawFd for PipeReader {
    fn as_raw_fd(&self) -> RawFd {
        self.0 .0.as_raw_fd()
    }
}
impl AsRawFd for PipeWriter {
    fn as_raw_fd(&self) -> RawFd {
        self.0 .0.as_raw_fd()
    }
}
impl FromRawFd for PipeReader {
    unsafe fn from_raw_fd(fd: RawFd) -> PipeReader {
        PipeReader(FileDesc(OwnedFd::from_raw_fd(fd)))
    }
}
impl FromRawFd for PipeWriter {
    unsafe fn from_raw_fd(fd: RawFd) -> PipeWriter {
        PipeWriter(FileDesc(OwnedFd::from_raw_fd(fd)))
    }
}
impl IntoRawFd for PipeReader {
    fn into_raw_fd(self) -> RawFd {
        self.0 .0.into_raw_fd()
    }
}
impl IntoRawFd for PipeWriter {
    fn into_raw_fd(self) -> RawFd {
        self.0 .0.into_raw_fd()
    }
}
impl AsFd for PipeReader {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0 .0.as_fd()
    }
}
impl AsFd for PipeWriter {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0 .0.as_fd()
    }
}
impl From<PipeReader> for OwnedFd {
    fn from(p: PipeReader) -> OwnedFd {
        p.0 .0
    }
}
impl From<PipeWriter> for OwnedFd {
    fn from(p: PipeWriter) -> OwnedFd {
        p.0 .0
    }
}
impl From<OwnedFd> for PipeReader {
    fn from(fd: OwnedFd) -> PipeReader {
        PipeReader(FileDesc(fd))
    }
}
impl From<OwnedFd> for PipeWriter {
    fn from(fd: OwnedFd) -> PipeWriter {
        PipeWriter(FileDesc(fd))
    }
}
