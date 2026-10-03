//! stdin/stdout/stderr. stdout is line-buffered (LineWriter) behind a reentrant lock, stderr
//! is unbuffered, stdin is a BufReader behind a Mutex: the same structure as real std, so
//! output interleaving matches.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

use super::{BufRead, BufReader, Error, LineWriter, Lines, Read, Result, Write};
use crate::sync::{Mutex, MutexGuard, OnceLock, ReentrantLock, ReentrantLockGuard};
use crate::sys;

/// fd writer; EBADF (closed stdio) counts as success, like real std.
struct Raw {
    fd: i32,
}

impl Write for Raw {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let len = if buf.len() > isize::MAX as usize { isize::MAX as usize } else { buf.len() };
        let r = unsafe { sys::write(self.fd, buf.as_ptr() as *const core::ffi::c_void, len) };
        if r < 0 {
            let e = sys::errno();
            if e == sys::os::EBADF {
                return Ok(buf.len());
            }
            return Err(Error::from_raw_os_error(e));
        }
        Ok(r as usize)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Read for Raw {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let len = if buf.len() > isize::MAX as usize { isize::MAX as usize } else { buf.len() };
        let r = unsafe { sys::read(self.fd, buf.as_mut_ptr() as *mut core::ffi::c_void, len) };
        if r < 0 {
            let e = sys::errno();
            if e == sys::os::EBADF {
                return Ok(0);
            }
            return Err(Error::from_raw_os_error(e));
        }
        Ok(r as usize)
    }
}

type StdoutCell = ReentrantLock<RefCell<LineWriter<Raw>>>;
type StderrCell = ReentrantLock<RefCell<Raw>>;

static STDOUT: OnceLock<StdoutCell> = OnceLock::new();
static STDERR: OnceLock<StderrCell> = OnceLock::new();
static STDIN: OnceLock<Mutex<BufReader<Raw>>> = OnceLock::new();

fn stdout_cell() -> &'static StdoutCell {
    STDOUT.get_or_init(|| ReentrantLock::new(RefCell::new(LineWriter::new(Raw { fd: 1 }))))
}

fn stderr_cell() -> &'static StderrCell {
    STDERR.get_or_init(|| ReentrantLock::new(RefCell::new(Raw { fd: 2 })))
}

fn stdin_cell() -> &'static Mutex<BufReader<Raw>> {
    STDIN.get_or_init(|| Mutex::new(BufReader::with_capacity(8 * 1024, Raw { fd: 0 })))
}

pub struct Stdout {
    inner: &'static StdoutCell,
}

pub struct StdoutLock<'a> {
    inner: ReentrantLockGuard<'a, RefCell<LineWriter<Raw>>>,
}

pub struct Stderr {
    inner: &'static StderrCell,
}

pub struct StderrLock<'a> {
    inner: ReentrantLockGuard<'a, RefCell<Raw>>,
}

pub struct Stdin {
    inner: &'static Mutex<BufReader<Raw>>,
}

pub struct StdinLock<'a> {
    inner: MutexGuard<'a, BufReader<Raw>>,
}

pub fn stdout() -> Stdout {
    Stdout { inner: stdout_cell() }
}

pub fn stderr() -> Stderr {
    Stderr { inner: stderr_cell() }
}

pub fn stdin() -> Stdin {
    Stdin { inner: stdin_cell() }
}

impl Stdout {
    pub fn lock(&self) -> StdoutLock<'static> {
        StdoutLock { inner: self.inner.lock() }
    }
}

impl Stderr {
    pub fn lock(&self) -> StderrLock<'static> {
        StderrLock { inner: self.inner.lock() }
    }
}

impl Stdin {
    pub fn lock(&self) -> StdinLock<'static> {
        StdinLock { inner: self.inner.lock().unwrap_or_else(|e| e.into_inner()) }
    }
    pub fn read_line(&self, buf: &mut String) -> Result<usize> {
        self.lock().read_line(buf)
    }
    pub fn lines(self) -> Lines<StdinLock<'static>> {
        self.lock().lines()
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        (&*self).write(buf)
    }
    fn flush(&mut self) -> Result<()> {
        (&*self).flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        (&*self).write_all(buf)
    }
    fn write_fmt(&mut self, args: fmt::Arguments) -> Result<()> {
        (&*self).write_fmt(args)
    }
}

impl Write for &Stdout {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.lock().write(buf)
    }
    fn flush(&mut self) -> Result<()> {
        self.lock().flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.lock().write_all(buf)
    }
    fn write_fmt(&mut self, args: fmt::Arguments) -> Result<()> {
        self.lock().write_fmt(args)
    }
}

impl<'a> Write for StdoutLock<'a> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.inner.borrow_mut().write(buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.inner.borrow_mut().write_all(buf)
    }
    fn flush(&mut self) -> Result<()> {
        self.inner.borrow_mut().flush()
    }
}

impl Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        (&*self).write(buf)
    }
    fn flush(&mut self) -> Result<()> {
        (&*self).flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        (&*self).write_all(buf)
    }
    fn write_fmt(&mut self, args: fmt::Arguments) -> Result<()> {
        (&*self).write_fmt(args)
    }
}

impl Write for &Stderr {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.lock().write(buf)
    }
    fn flush(&mut self) -> Result<()> {
        self.lock().flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.lock().write_all(buf)
    }
    fn write_fmt(&mut self, args: fmt::Arguments) -> Result<()> {
        self.lock().write_fmt(args)
    }
}

impl<'a> Write for StderrLock<'a> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.inner.borrow_mut().write(buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.inner.borrow_mut().write_all(buf)
    }
    fn flush(&mut self) -> Result<()> {
        self.inner.borrow_mut().flush()
    }
}

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.lock().read(buf)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        self.lock().read_to_end(buf)
    }
}

impl<'a> Read for StdinLock<'a> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        self.inner.read_to_end(buf)
    }
}

impl<'a> BufRead for StdinLock<'a> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        self.inner.fill_buf()
    }
    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt)
    }
}

impl fmt::Debug for Stdout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stdout").finish_non_exhaustive()
    }
}
impl fmt::Debug for Stderr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stderr").finish_non_exhaustive()
    }
}
impl fmt::Debug for Stdin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stdin").finish_non_exhaustive()
    }
}

/// print!/println! land here (frontend lowering, coord.md S2).
pub fn _print(args: fmt::Arguments) {
    if let Err(e) = stdout().write_fmt(args) {
        panic!("failed printing to stdout: {}", e);
    }
}

/// eprint!/eprintln! land here.
pub fn _eprint(args: fmt::Arguments) {
    if let Err(e) = stderr().write_fmt(args) {
        panic!("failed printing to stderr: {}", e);
    }
}

/// Flushes stdout; the runtime calls this at process exit (real std does it in rt::cleanup).
pub fn cleanup() {
    if let Some(cell) = STDOUT.get() {
        if let Some(guard) = cell.try_lock() {
            let _ = guard.borrow_mut().flush();
        }
    }
}

pub trait IsTerminal {
    fn is_terminal(&self) -> bool;
}

impl IsTerminal for Stdin {
    fn is_terminal(&self) -> bool {
        unsafe { sys::isatty(0) == 1 }
    }
}
impl IsTerminal for Stdout {
    fn is_terminal(&self) -> bool {
        unsafe { sys::isatty(1) == 1 }
    }
}
impl IsTerminal for Stderr {
    fn is_terminal(&self) -> bool {
        unsafe { sys::isatty(2) == 1 }
    }
}
