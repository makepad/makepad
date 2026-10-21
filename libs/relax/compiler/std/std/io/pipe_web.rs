//! Anonymous pipes, web half (rustc's sys/pipe/unsupported.rs: `Pipe(!)`): none can be made, so
//! the ends are uninhabited. Same public API as pipe_unix.rs / pipe_windows.rs.

use core::fmt;

use crate::io::{IoSlice, IoSliceMut, Read, Result, Write};

struct Pipe(!);

impl fmt::Debug for Pipe {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

pub struct PipeReader(Pipe);
pub struct PipeWriter(Pipe);

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

/// `Err(Unsupported)`.
pub fn pipe() -> Result<(PipeReader, PipeWriter)> {
    Err(crate::io::Error::UNSUPPORTED_PLATFORM)
}

impl PipeReader {
    pub(crate) fn diverge(&self) -> ! {
        self.0 .0
    }
    pub fn try_clone(&self) -> Result<PipeReader> {
        self.0 .0
    }
}
impl PipeWriter {
    pub(crate) fn diverge(&self) -> ! {
        self.0 .0
    }
    pub fn try_clone(&self) -> Result<PipeWriter> {
        self.0 .0
    }
}

impl Read for &PipeReader {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        self.0 .0
    }
    fn read_vectored(&mut self, _bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        self.0 .0
    }
}
impl Read for PipeReader {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        self.0 .0
    }
    fn read_vectored(&mut self, _bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        self.0 .0
    }
}
impl Write for &PipeWriter {
    fn write(&mut self, _buf: &[u8]) -> Result<usize> {
        self.0 .0
    }
    fn write_vectored(&mut self, _bufs: &[IoSlice<'_>]) -> Result<usize> {
        self.0 .0
    }
    fn flush(&mut self) -> Result<()> {
        self.0 .0
    }
}
impl Write for PipeWriter {
    fn write(&mut self, _buf: &[u8]) -> Result<usize> {
        self.0 .0
    }
    fn write_vectored(&mut self, _bufs: &[IoSlice<'_>]) -> Result<usize> {
        self.0 .0
    }
    fn flush(&mut self) -> Result<()> {
        self.0 .0
    }
}
